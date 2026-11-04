//! The files Live's bridge checks before it starts: the reference beside its Remote Script, the configuration that
//! names, and the secret the configuration names. The Remote Script refuses to start unless each is a regular file, not a
//! link, that this account owns and only it can read (on Windows: a protected owner-only ACL). `kumi bridge` with the
//! bridge already current puts back what Kumi can: their permissions.
use kumi_library::sources::join;
use serde_json::Value;
use std::{fs, path::Path};

/// What's wrong with one of the files, for the Remote Script's check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Wrong {
    /// It isn't there, or isn't a regular file.
    Missing,
    /// It's a link: the Remote Script reads no file through one.
    Link,
    /// Another account owns it.
    NotYours,
    /// Others can read or write it (macOS: its mode; Windows: its ACL). Kumi can make it yours alone again.
    Open,
}

/// The files the Remote Script in `scripts` (`<Remote Scripts>/AbletonMcpBridge`'s folder) checks, as far as each names
/// the next: its reference, the configuration the reference names, and the secret the configuration names.
pub(crate) fn checked(scripts: &str) -> Vec<String> {
    let reference = join(scripts, "AbletonMcpBridge/bridge-reference.json");
    let read = |file: &str| fs::read(file).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let mut files = vec![reference.clone()];
    let Some(config) = read(&reference).and_then(|r| r["config"].as_str().map(str::to_string)) else { return files };
    files.push(config.clone());
    if let Some(secret) = read(&config).and_then(|c| c["bridge"]["secretFile"].as_str().map(str::to_string)) {
        files.push(secret);
    }
    files
}

/// What the Remote Script's check would find wrong with `file`; None when it passes. `owner_only` is Windows' ACL check
/// (one protected rule, full control for you, who own it), asked only there.
pub(crate) fn wrong(file: &str, owner_only: &dyn Fn(&Path) -> bool) -> Option<Wrong> {
    let path = Path::new(file);
    let Ok(meta) = fs::symlink_metadata(path) else { return Some(Wrong::Missing) };
    if meta.file_type().is_symlink() {
        return Some(Wrong::Link);
    }
    if !meta.is_file() {
        return Some(Wrong::Missing);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // (the Remote Script compares with Live's own account, which is the one running Kumi)
        if meta.uid() != unsafe { libc::getuid() } {
            return Some(Wrong::NotYours);
        }
        if meta.permissions().mode() & 0o077 != 0 {
            return Some(Wrong::Open);
        }
    }
    if cfg!(windows) && !owner_only(path) {
        return Some(Wrong::Open);
    }
    None
}

/// Make `file` this account's alone again (mode 0600; on Windows `make_owner_only`, the owner-only ACL Kumi gives its
/// own credentials), for a file `wrong` found [`Wrong::Open`].
pub(crate) fn make_yours(file: &str, make_owner_only: &dyn Fn(&Path) -> Result<(), String>) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(file, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    }
    if cfg!(windows) {
        make_owner_only(Path::new(file))?;
    }
    Ok(())
}

/// What `kumi bridge` says of the files once it has looked (`files` with what was wrong, and whether it put that back):
/// None when all were fine.
pub(crate) fn said(found: &[(String, Wrong, Result<(), String>)]) -> Option<String> {
    if found.is_empty() {
        return None;
    }
    let name = |file: &str| Path::new(file).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| file.into());
    let mut said = vec![];
    let made: Vec<_> = found.iter().filter(|(_, wrong, done)| *wrong == Wrong::Open && done.is_ok()).map(|(file, ..)| name(file)).collect();
    if !made.is_empty() {
        said.push(format!(
            "Others could read {}, so Live's bridge wouldn't start: {} yours alone again. Restart Live to start it.",
            made.join(" and "),
            if made.len() == 1 { "it's" } else { "they're" }
        ));
    }
    for (file, wrong, done) in found {
        said.push(match (wrong, done) {
            (Wrong::Open, Ok(())) => continue,
            (Wrong::Open, Err(why)) => format!("Kumi couldn't make {file} yours alone ({why}), so Live's bridge won't start."),
            (Wrong::NotYours, _) => format!(
                "{file} belongs to another account, so Live's bridge won't start. Make it yours (in a terminal: sudo chown \"$(whoami)\" '{file}'), then run kumi bridge again."
            ),
            (Wrong::Link, _) => format!(
                "{file} is a link, which Live's bridge doesn't read through. Put the file itself there (or remove the link and run kumi bridge again)."
            ),
            // (a missing reference or configuration isn't a current bridge: kumi bridge installs it again; a missing
            // secret the bridge's own repair won't make anew)
            (Wrong::Missing, _) => format!(
                "{file} is missing or isn't a file, so Live's bridge won't start: kumi report collects what's needed for a bug report."
            ),
        });
    }
    Some(said.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never(_: &Path) -> bool {
        panic!("Windows' ACL check, asked off Windows")
    }

    #[cfg(unix)]
    #[test]
    fn a_file_others_can_read_is_made_yours_alone_and_links_and_missing_files_are_said() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let scripts = dir.path().join("Remote Scripts");
        fs::create_dir_all(scripts.join("AbletonMcpBridge")).unwrap();
        let (config, secret) = (dir.path().join("bridge-config.json"), dir.path().join("secret"));
        fs::write(&config, serde_json::json!({"bridge":{"secretFile":secret}}).to_string()).unwrap();
        fs::write(&secret, "s".repeat(40)).unwrap();
        let reference = scripts.join("AbletonMcpBridge/bridge-reference.json");
        fs::write(&reference, serde_json::json!({"config":config}).to_string()).unwrap();
        for file in [&reference, &config, &secret] {
            fs::set_permissions(file, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let files = checked(scripts.to_str().unwrap());
        assert_eq!(files, [&reference, &config, &secret].map(|f| f.display().to_string()));
        assert!(files.iter().all(|f| wrong(f, &never).is_none()), "as Kumi installs them");
        // The reference readable by others (the lab's failed start): made yours alone again.
        fs::set_permissions(&reference, fs::Permissions::from_mode(0o644)).unwrap();
        let file = reference.display().to_string();
        assert_eq!(wrong(&file, &never), Some(Wrong::Open));
        make_yours(&file, &|_| unreachable!()).unwrap();
        assert_eq!(fs::metadata(&reference).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(wrong(&file, &never), None);
        assert_eq!(
            said(&[(file.clone(), Wrong::Open, Ok(()))]).unwrap(),
            "Others could read bridge-reference.json, so Live's bridge wouldn't start: it's yours alone again. Restart Live to start it."
        );
        // A link isn't followed (its target stays as it is), and a missing secret is said.
        let target = dir.path().join("elsewhere.json");
        fs::write(&target, "{}").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        fs::remove_file(&config).unwrap();
        std::os::unix::fs::symlink(&target, &config).unwrap();
        assert_eq!(wrong(&config.display().to_string(), &never), Some(Wrong::Link));
        assert_eq!(fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o644);
        fs::remove_file(&secret).unwrap();
        assert_eq!(wrong(&secret.display().to_string(), &never), Some(Wrong::Missing));
        let both =
            said(&[(config.display().to_string(), Wrong::Link, Ok(())), (secret.display().to_string(), Wrong::Missing, Ok(()))]).unwrap();
        assert!(both.contains("is a link, which Live's bridge doesn't read through"), "{both}");
        assert!(both.contains("secret is missing or isn't a file, so Live's bridge won't start: kumi report collects"), "{both}");
        assert_eq!(said(&[]), None);
    }
}
