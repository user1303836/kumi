use std::collections::HashMap;

/// `process.platform` as Node names this platform.
pub fn current_platform() -> &'static str {
    if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "freebsd") {
        "freebsd"
    } else if cfg!(target_os = "openbsd") {
        "openbsd"
    } else if cfg!(target_os = "android") {
        "android"
    } else {
        "unknown"
    }
}

/// `platform` is a Node platform name; `None` is this process's platform.
pub fn npm_executable(platform: Option<&str>) -> &'static str {
    let platform = match platform {
        Some(platform) => platform,
        None => current_platform(),
    };
    if platform == "win32" {
        "npm.cmd"
    } else {
        "npm"
    }
}

/// Windows PowerShell by its full path. A client may start the bridge with a PATH that holds only
/// Node's folder (Kumi does), where a bare `powershell.exe` isn't found; a bare name is also looked
/// for in the working folder first, which is no place to take a security check's program from.
///
/// `env` is the environment to read; `None` is this process's.
pub fn windows_powershell(env: Option<&HashMap<String, String>>) -> String {
    let lookup = |name: &str| match env {
        Some(map) => map.get(name).cloned(),
        None => std::env::var(name).ok(),
    };
    let root = lookup("SystemRoot").or_else(|| lookup("SYSTEMROOT")).unwrap_or_else(|| "C:\\Windows".to_string());
    win32_join(&[&root, "System32", "WindowsPowerShell", "v1.0", "powershell.exe"])
}

/// `path.win32.join`: the parts joined with `\` and normalized (forward slashes, repeated separators,
/// `.` and `..` segments), whatever platform this runs on.
fn win32_join(parts: &[&str]) -> String {
    let joined: Vec<&str> = parts.iter().copied().filter(|part| !part.is_empty()).collect();
    if joined.is_empty() {
        return ".".to_string();
    }
    win32_normalize(&joined.join("\\"))
}

fn win32_normalize(path: &str) -> String {
    let path = path.replace('/', "\\");
    let bytes = path.as_bytes();
    // The root: a UNC share, a drive (absolute or drive-relative), a bare separator, or none.
    let (root, rest, absolute) = if path.starts_with("\\\\") {
        let after = &path[2..];
        let mut segments = after.splitn(3, '\\');
        let host = segments.next().unwrap_or("");
        let share = segments.next().unwrap_or("");
        let rest = segments.next().unwrap_or("");
        (format!("\\\\{host}\\{share}\\"), rest.to_string(), true)
    } else if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        let drive = &path[..2];
        if bytes.len() > 2 && bytes[2] == b'\\' {
            (format!("{drive}\\"), path[3..].to_string(), true)
        } else {
            (drive.to_string(), path[2..].to_string(), false)
        }
    } else if path.starts_with('\\') {
        ("\\".to_string(), path[1..].to_string(), true)
    } else {
        (String::new(), path.clone(), false)
    };
    let mut segments: Vec<&str> = Vec::new();
    for segment in rest.split('\\') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.last().is_some_and(|last| *last != "..") {
                    segments.pop();
                } else if !absolute {
                    segments.push("..");
                }
            }
            other => segments.push(other),
        }
    }
    let tail = segments.join("\\");
    if root.is_empty() && tail.is_empty() {
        return ".".to_string();
    }
    format!("{root}{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npm_executable_follows_the_platform() {
        assert_eq!(npm_executable(Some("win32")), "npm.cmd");
        assert_eq!(npm_executable(Some("darwin")), "npm");
        assert_eq!(npm_executable(Some("linux")), "npm");
    }

    #[test]
    fn powershell_is_named_by_its_full_path() {
        let mut env = HashMap::new();
        assert_eq!(windows_powershell(Some(&env)), "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
        env.insert("SYSTEMROOT".to_string(), "D:\\Win\\".to_string());
        assert_eq!(windows_powershell(Some(&env)), "D:\\Win\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
        env.insert("SystemRoot".to_string(), "E:/Windows".to_string());
        assert_eq!(windows_powershell(Some(&env)), "E:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    }

    #[test]
    fn win32_join_normalizes_like_node() {
        assert_eq!(win32_join(&["C:\\a\\", "\\b", ".\\c", "..", "d"]), "C:\\a\\b\\d");
        assert_eq!(win32_join(&["\\\\server\\share\\", "x//y"]), "\\\\server\\share\\x\\y");
        assert_eq!(win32_join(&["relative", "..", "..", "z"]), "..\\z");
        assert_eq!(win32_join(&["C:", "tools"]), "C:\\tools");
    }
}

/// Volume, file index and link count from the same open handle, as Node's stat uses on Windows.
#[cfg(windows)]
pub fn windows_file_identity(file: &std::fs::File) -> std::io::Result<(u64, u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Information {
        attributes: u32,
        creation: FileTime,
        access: FileTime,
        write: FileTime,
        volume: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(handle: *mut std::ffi::c_void, information: *mut Information) -> i32;
    }
    let mut information = Information::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok((
        u64::from(information.volume),
        (u64::from(information.index_high) << 32) | u64::from(information.index_low),
        u64::from(information.links),
    ))
}
