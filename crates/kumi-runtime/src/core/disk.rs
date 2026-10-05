//! Free space before Kumi writes something big (a recording, a video, a program) or something that
//! must not be cut short (a device file): with the disk nearly full, Live's recordings and saves and
//! Kumi's own files fail partway, often without saying why.

use std::future::Future;
use std::path::{Path, PathBuf};

use kumi_common::js::number::{round, to_fixed, to_string};

/// Free space on the disk holding `path` (its nearest folder that exists), in bytes; None when the system won't say.
pub async fn free_bytes(path: &Path) -> Option<f64> {
    let mut at = path.to_path_buf();
    while !at.exists() {
        let up = match at.parent() {
            Some(parent) if parent.as_os_str().is_empty() => PathBuf::from("."),
            Some(parent) => parent.to_path_buf(),
            None => return None,
        };
        if up == at {
            return None;
        }
        at = up;
    }
    free_on(&at)
}

#[cfg(unix)]
fn free_on(path: &Path) -> Option<f64> {
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `name` is a valid C string and `stats` is a writable statvfs struct.
    if unsafe { libc::statvfs(name.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    Some(stats.f_bavail as f64 * stats.f_frsize as f64)
}

#[cfg(windows)]
fn free_on(path: &Path) -> Option<f64> {
    // TS: Node's statfs; here Windows' own GetDiskFreeSpaceExW, the bytes free to this user.
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(directory: *const u16, free_to_caller: *mut u64, total: *mut u64, total_free: *mut u64) -> i32;
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let (mut free, mut total, mut total_free) = (0u64, 0u64, 0u64);
    // SAFETY: `wide` is NUL-terminated and the three out-pointers are valid for writes.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut total_free) };
    if ok == 0 {
        None
    } else {
        Some(free as f64)
    }
}

pub const MB: f64 = 1_000_000.0;

fn size(bytes: f64) -> String {
    if bytes >= 1_000.0 * MB {
        format!("{} GB", to_fixed(bytes / (1_000.0 * MB), 1))
    } else {
        format!("{} MB", to_string(round(bytes / MB).max(0.0)))
    }
}

/// Why not to go ahead, in plain words, when the disk holding `path` has less than `needed` free
/// (what `what` would need); None when there's room, or the system won't say.
pub async fn low_disk(path: &str, needed: f64, what: &str) -> Option<String> {
    low_disk_with(path, needed, what, |at| async move { free_bytes(Path::new(&at)).await }).await
}

/// `lowDisk` with its own reading of free space (tests pass one).
pub async fn low_disk_with<F, Fut>(path: &str, needed: f64, what: &str, free: F) -> Option<String>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Option<f64>>,
{
    let left = free(path.to_string()).await?;
    if left >= needed {
        return None;
    }
    Some(format!(
        "Only {} is free on the disk {what}, so it would likely fail partway. Free some space (empty the Trash or Recycle Bin, or move old bounces and videos off that disk), then try again.",
        size(left)
    ))
}
