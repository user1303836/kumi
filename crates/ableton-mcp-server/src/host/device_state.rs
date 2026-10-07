//! Owner-scoped device-state files and exact recall/morph host transactions.
use super::*;
use crate::transactions::device_state::{
    build_device_state_file, plan_device_state_recall, validate_device_state_file, DeviceStateError, DEVICE_STATE_SCHEMA,
};
use kumi_common::{abort::Signal, js::json as js_json};
use rand::RngCore;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub fn device_state_directory(directory: &Value) -> Result<PathBuf, LiveError> {
    let path = directory
        .as_str()
        .filter(|s| is_non_empty_string(directory, 4096) && Path::new(s).is_absolute() && !s.contains('\0'))
        .ok_or_else(|| LiveError::error("an explicit absolute directory is required"))?;
    let path = crate::command::resolve(path)?;
    let stats = fs::symlink_metadata(&path).map_err(|e| crate::delivery::io_error(&e, "lstat", &[&path]))?;
    if !stats.is_dir() || stats.file_type().is_symlink() {
        return Err(LiveError::error("the device-state directory must be a real directory"));
    }
    let real = fs::canonicalize(&path).map_err(|e| crate::delivery::io_error(&e, "realpath", &[&path]))?;
    let text = real.to_string_lossy();
    Ok(PathBuf::from(
        text.strip_prefix(r"\\?\UNC\")
            .map(|s| format!(r"\\{s}"))
            .or_else(|| text.strip_prefix(r"\\?\").map(str::to_owned))
            .unwrap_or_else(|| text.into_owned()),
    ))
}
pub fn read_device_state_file(file: &Value) -> Result<Value, LiveError> {
    let path = file
        .as_str()
        .filter(|s| is_non_empty_string(file, 4096) && Path::new(s).is_absolute() && !s.contains('\0'))
        .ok_or_else(|| LiveError::error("an explicit absolute snapshot file path is required"))?;
    let path = crate::command::resolve(path)?;
    let stats = fs::symlink_metadata(&path).map_err(|e| crate::delivery::io_error(&e, "lstat", &[&path]))?;
    if !stats.is_file() || stats.file_type().is_symlink() {
        return Err(LiveError::error("the device-state snapshot must be a real regular file"));
    }
    if stats.len() > 256 * 1024 {
        return Err(LiveError::error("the device-state snapshot exceeds its 256 KiB bound"));
    }
    let parsed = (|| {
        let bytes = fs::read(&path).map_err(|e| crate::delivery::io_error(&e, "open", &[&path]))?;
        super::json_diagnostics::parse_json(&String::from_utf8_lossy(&bytes))
    })()
    .map_err(|e: LiveError| LiveError::error(format!("the device-state snapshot is not readable JSON ({})", e.message())))?;
    validate_device_state_file(&parsed)
}
struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
pub fn write_device_state_file_atomically(target: &Path, file: &Value, overwrite: bool) -> Result<bool, LiveError> {
    let target_exists = match fs::symlink_metadata(target) {
        Ok(stats) => {
            if !stats.is_file() || stats.file_type().is_symlink() {
                return Err(LiveError::error("device-state snapshot target must be a real regular file, not a symbolic link"));
            }
            if !overwrite {
                return Err(LiveError::error("device-state snapshot already exists; pass overwrite=true to replace it"));
            }
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(crate::delivery::io_error(&error, "lstat", &[target])),
    };
    let mut random = [0u8; 12];
    rand::rng().fill_bytes(&mut random);
    let temporary = PathBuf::from(format!("{}.tmp-{}-{}", target.display(), std::process::id(), hex::encode(random)));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut descriptor = options.open(&temporary).map_err(|e| crate::delivery::io_error(&e, "open", &[&temporary]))?;
    let _cleanup = Temporary(temporary.clone());
    descriptor.write_all(js_json::file_text(file).as_bytes()).map_err(|e| crate::delivery::io_error(&e, "write", &[]))?;
    descriptor.sync_all().map_err(|e| crate::delivery::io_error(&e, "fsync", &[]))?;
    drop(descriptor);
    let staged = read_device_state_file(&json!(temporary))?;
    if staged["digest"] != file["digest"] {
        return Err(LiveError::error("device-state staged snapshot failed read-back verification"));
    }
    if target_exists {
        fs::rename(&temporary, target).map_err(|e| crate::delivery::io_error(&e, "rename", &[&temporary, target]))?;
    } else {
        publish_new(&temporary, target, |from, to| fs::hard_link(from, to))?;
    }
    Ok(target_exists)
}
/// A first save goes in only if nothing is at the target: a hard link, which fails if something is. A volume without
/// hard links (FAT32, exFAT, many SMB shares) gets a rename that won't replace a file either, where the platform and
/// volume have one; otherwise a plain rename, once the target is seen to be still absent.
fn publish_new(temporary: &Path, target: &Path, link: impl FnOnce(&Path, &Path) -> std::io::Result<()>) -> Result<(), LiveError> {
    let changed = || LiveError::error("device-state snapshot target changed during save; retry");
    match link(temporary, target) {
        Ok(()) => fs::remove_file(temporary).map_err(|e| crate::delivery::io_error(&e, "unlink", &[temporary])),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(changed()),
        Err(e) => match rename_no_replace(temporary, target) {
            Ok(()) => Ok(()),
            Err(exists) if exists.kind() == std::io::ErrorKind::AlreadyExists => Err(changed()),
            Err(_) => match fs::symlink_metadata(target) {
                Err(absent) if absent.kind() == std::io::ErrorKind::NotFound => {
                    fs::rename(temporary, target).map_err(|e| crate::delivery::io_error(&e, "rename", &[temporary, target]))
                }
                Ok(_) => Err(changed()),
                Err(_) => Err(crate::delivery::io_error(&e, "link", &[temporary, target])),
            },
        },
    }
}
/// `rename`, refusing to replace a file already at `to` (AlreadyExists): Linux's renameat2 with RENAME_NOREPLACE,
/// macOS's renamex_np with RENAME_EXCL, Windows's MoveFileExW without MOVEFILE_REPLACE_EXISTING. Unsupported where
/// the platform or the volume has no such rename.
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    let unsupported = || std::io::Error::from(std::io::ErrorKind::Unsupported);
    #[cfg(any(target_os = "macos", all(target_os = "linux", target_env = "gnu")))]
    {
        use std::os::unix::ffi::OsStrExt;
        let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
        let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
        // SAFETY: two NUL-terminated paths that outlive the call.
        #[cfg(target_os = "macos")]
        let done = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
        // SAFETY: as above, relative to the working directory.
        #[cfg(target_os = "linux")]
        let done = unsafe { libc::renameat2(libc::AT_FDCWD, from.as_ptr(), libc::AT_FDCWD, to.as_ptr(), libc::RENAME_NOREPLACE) };
        if done == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        // The volume (or the kernel) can't do it.
        return Err(if matches!(error.raw_os_error(), Some(libc::EINVAL | libc::ENOSYS | libc::ENOTSUP)) { unsupported() } else { error });
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
        }
        let wide = |path: &Path| path.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>();
        let (from, to) = (wide(from), wide(to));
        // SAFETY: two NUL-terminated wide paths that outlive the call; no flags, so an existing target is refused.
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } != 0 {
            return Ok(());
        }
        let _ = unsupported;
        return Err(std::io::Error::last_os_error());
    }
    #[allow(unreachable_code)]
    {
        let _ = (from, to);
        Err(unsupported())
    }
}
impl McpHost {
    pub async fn dispatch_device_state_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        if !call.asynchronous {
            return None;
        }
        let args = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_device_state_save" => Some(self.live_device_state_save_async(&call.id, args).await),
            "live_device_state_recall_preview" => Some(self.live_device_state_recall_preview_async(&call.id, args).await),
            "live_device_state_recall_apply" => self.live_device_state_recall_apply_async(&call.id, args, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_device_state_save_async(&self, id: &Value, params: &Value) -> Value {
        let name_ok = params["name"].as_str().is_some_and(|s| {
            s.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
                && s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
        });
        if !has_only(params, &["deviceRef", "name", "directory", "overwrite"])
            || !is_non_empty_string(&params["deviceRef"], 256)
            || !is_non_empty_string(&params["name"], 64)
            || !name_ok
            || params.get("overwrite").is_some_and(|v| !v.is_boolean())
        {
            return error(id, -32602, "deviceRef, a filesystem-safe name, directory, and optional overwrite are required", None);
        }
        let result=async{
            let status=self.require_connected(Some("devices"))?;
            if !status.capabilities.iter().any(|c|c.as_str()=="parameters"){return Err(LiveError::error("parameter capability is unavailable"));}
            if !status.has_operation("snapshot"){return Err(LiveError::error("snapshot operation is unavailable"));}
            let snapshot=self.views.view_for(None,&[params["deviceRef"].clone()],None,&[]).await?;
            let file=build_device_state_file(&serde_json::to_value(snapshot).unwrap(),params["deviceRef"].as_str().unwrap(),params["name"].as_str().unwrap())?;
            let directory=device_state_directory(&params["directory"])?;
            let target=directory.join(format!("{}.ableton-device-state.json",params["name"].as_str().unwrap()));
            let overwritten=write_device_state_file_atomically(&target,&file,params["overwrite"]==true)?;
            Ok(success_text(id,&json!({"saved":true,"file":target,"overwritten":overwritten,"schema":DEVICE_STATE_SCHEMA,"name":params["name"],"device":file["device"],"privacy":file["privacy"],"digest":file["digest"]})))
        }.await;
        result.unwrap_or_else(|e| {
            adapter_tool_error(id, &e, "Device-state save is read-only toward Live and writes exactly one bounded owner-scoped file.")
        })
    }
    pub async fn live_device_state_recall_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["file", "targetDeviceRef", "morphFromFile", "morphFromLive", "amount", "allowPartialLayout"])
            || !is_non_empty_string(&params["targetDeviceRef"], 256)
            || params.get("morphFromLive").is_some_and(|v| !v.is_boolean())
            || params.get("allowPartialLayout").is_some_and(|v| !v.is_boolean())
            || params.get("amount").is_some_and(|v| !v.as_f64().is_some_and(|v| v.is_finite() && (0.0..=1.0).contains(&v)))
        {
            return error(id, -32602, "file, targetDeviceRef, and optional morph fields are invalid", None);
        }
        let morph_kinds = params.get("morphFromFile").is_some() as usize + (params["morphFromLive"] == true) as usize;
        if morph_kinds > 1 || (morph_kinds == 1 && params.get("amount").is_none()) || (morph_kinds == 0 && params.get("amount").is_some()) {
            return error(
                id,
                -32602,
                "morph requires exactly one source (morphFromFile or morphFromLive=true) plus an explicit amount from 0 to 1",
                None,
            );
        }
        let result:Result<Value,DeviceStateError>=async{
            let file=read_device_state_file(&params["file"])?;
            let morph_file=params.get("morphFromFile").map(read_device_state_file).transpose()?;
            let status=self.require_connected(Some("device.parameter.write"))?;
            if !status.capabilities.iter().any(|c|c.as_str()=="parameters"){return Err(LiveError::error("parameter capability is unavailable").into());}
            if !status.has_operation("device.parameter.set"){return Err(LiveError::error("device parameter writes are unavailable").into());}
            let snapshot=self.views.view_for(None,&[params["targetDeviceRef"].clone()],None,&[]).await?;
            let mode=if morph_kinds==1{"morph"}else{"recall"};
            let mut options=json!({"allowPartialLayout":params["allowPartialLayout"]==true});
            if let Some(morph_file)=&morph_file {options["morphFrom"]=json!({"kind":"file","file":morph_file});}else if params["morphFromLive"]==true{options["morphFrom"]=json!({"kind":"live"});}
            if let Some(amount)=params.get("amount"){options["amount"]=amount.clone();}
            let plan=plan_device_state_recall(&serde_json::to_value(snapshot).unwrap(),&file,params["targetDeviceRef"].as_str().unwrap(),&options)?;
            let record=self.device_state_transactions.preview_async(&plan,mode,params["amount"].as_f64()).await?;
            let mut result=json!({"transactionId":record["transactionId"],"epoch":record["epoch"],"mode":mode});
            if let Some(amount)=params.get("amount"){result["amount"]=amount.clone();}
            result["target"]=json!({"deviceRef":plan["deviceRef"],"identity":plan["identity"],"layoutFingerprint":plan["layoutFingerprint"]});
            result["source"]=json!({"name":file["name"],"identity":file["device"]["identity"],"layoutFingerprint":file["device"]["layoutFingerprint"],"digest":file["digest"]});
            if let Some(morph_file)=&morph_file {result["morphSource"]=json!({"name":morph_file["name"],"digest":morph_file["digest"]});}else if params["morphFromLive"]==true{result["morphSource"]=json!("live");}
            for key in ["applicable","skipped","dispositions"]{result[key]=plan[key].clone();}
            result["impact"]=json!(if mode=="morph"{"interpolates-device-parameter-state"}else{"restores-device-parameter-state"});result["confirmation"]=json!("apply");result["expiresAt"]=record["expiresAt"].clone();
            Ok(success_text(id,&result))
        }.await;
        match result {
            Ok(value) => value,
            Err(DeviceStateError { device_state_report: Some(report), .. }) => {
                response(id, json!({"content":[{"type":"text","text":js_json::stringify(&report)}],"isError":true}))
            }
            Err(e) => adapter_tool_error(
                id,
                &e.error,
                "Device-state recall preview failed without mutation; verify the snapshot file and target device identity.",
            ),
        }
    }
    pub async fn live_device_state_recall_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(params, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        let result = self
            .device_state_transactions
            .apply_async(
                params["transactionId"].as_str().unwrap(),
                &params["confirmation"],
                params["idempotencyKey"].as_str().unwrap(),
                Some(&self.transaction_context(params, signal, reads::AUDITION_DEADLINE_MS)),
            )
            .await;
        Some(match result {
            Ok(value) => success_text(id, &value),
            Err(e) => adapter_tool_error(
                id,
                &e,
                "Device-state recall may be uncertain; reconcile with the exact original idempotency key and do not retry blindly.",
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_first_save_goes_in_where_the_volume_has_no_hard_links() {
        let folder = tempfile::tempdir().unwrap();
        let (temporary, target) = (folder.path().join("save.tmp"), folder.path().join("Bass.json"));
        let unsupported = |_: &Path, _: &Path| Err(std::io::Error::from(std::io::ErrorKind::Unsupported));
        fs::write(&temporary, b"{}").unwrap();
        publish_new(&temporary, &target, unsupported).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"{}");
        assert!(!temporary.exists());
        // Something there by then still isn't replaced.
        fs::write(&temporary, b"{\"new\":true}").unwrap();
        let error = publish_new(&temporary, &target, unsupported).unwrap_err();
        assert!(error.message().contains("changed during save"), "{}", error.message());
        assert_eq!(fs::read(&target).unwrap(), b"{}");
    }
    #[test]
    fn the_no_replace_rename_never_replaces_a_file() {
        let folder = tempfile::tempdir().unwrap();
        let (from, to) = (folder.path().join("save.tmp"), folder.path().join("Bass.json"));
        fs::write(&from, b"new").unwrap();
        fs::write(&to, b"kept").unwrap();
        match rename_no_replace(&from, &to) {
            // A platform without one (none of the ones Kumi ships for): publish_new falls back to its check.
            Err(error) if error.kind() == std::io::ErrorKind::Unsupported => return,
            Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists, "{error}"),
            Ok(()) => panic!("replaced the file"),
        }
        assert_eq!(fs::read(&to).unwrap(), b"kept");
        fs::remove_file(&to).unwrap();
        rename_no_replace(&from, &to).unwrap();
        assert_eq!(fs::read(&to).unwrap(), b"new");
        assert!(!from.exists());
    }
}
