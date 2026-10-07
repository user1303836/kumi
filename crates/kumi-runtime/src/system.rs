use std::collections::HashMap;

/// `process.env`, as the functions that took an `env` parameter read it (tests pass their own).
pub type Env = HashMap<String, String>;

/// `process.env` of this process.
pub fn process_env() -> Env {
    std::env::vars().collect()
}

/// `process.platform`: "win32", "darwin", "linux", or the system's own name elsewhere.
pub fn platform() -> &'static str {
    if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        std::env::consts::OS
    }
}

/// Whether Windows takes a file name for one of its devices: CON, PRN, AUX, NUL, COM0–9 or LPT0–9 (and COM¹–³ and
/// LPT¹–³) in any case, before the name's first dot and its trailing spaces. It does with an extension too ("aux.json",
/// "Con .amxd"), so a file by such a name can't be made there.
pub fn windows_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default().trim_end_matches(' ').to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    let (Some(port), Some(number)) = (stem.get(..3), stem.get(3..)) else {
        return false;
    };
    matches!(port, "COM" | "LPT") && (matches!(number.as_bytes(), [b'0'..=b'9']) || matches!(number, "\u{b9}" | "\u{b2}" | "\u{b3}"))
}

/// A program Windows comes with that Kumi runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SystemProgram {
    Tar,
    Tasklist,
    Powershell,
}

impl SystemProgram {
    fn name(self) -> &'static str {
        match self {
            Self::Tar => "tar",
            Self::Tasklist => "tasklist",
            Self::Powershell => "powershell",
        }
    }

    /// Where Windows keeps the programs of its own that Kumi runs, under its folder (SystemRoot).
    fn on_windows(self) -> &'static [&'static str] {
        match self {
            Self::Tar => &["System32", "tar.exe"],
            Self::Tasklist => &["System32", "tasklist.exe"],
            Self::Powershell => &["System32", "WindowsPowerShell", "v1.0", "powershell.exe"],
        }
    }
}

/// A program Windows comes with, by its full path when Kumi runs on Windows (its name elsewhere). A PATH
/// can put another of the same name first: a PowerShell started from Git Bash finds Git's GNU tar, which
/// reads "C:\…" as a remote host and opens no zip. And Windows looks for a bare name in the working folder
/// before the PATH.
pub fn system_program(name: SystemProgram, env: &Env, platform: &str) -> String {
    if platform != "win32" {
        return name.name().to_string();
    }
    let root = env.get("SystemRoot").or_else(|| env.get("SYSTEMROOT")).map(String::as_str).unwrap_or("C:\\Windows");
    win32_join(root, name.on_windows())
}

/// `systemProgram(name)` with this process's environment and platform.
pub fn system_program_default(name: SystemProgram) -> String {
    system_program(name, &process_env(), platform())
}

/// `path.win32.join(root, ...parts)`: backslashes, one between each part.
fn win32_join(root: &str, parts: &[&str]) -> String {
    let mut joined = root.replace('/', "\\");
    for part in parts {
        if !joined.ends_with('\\') {
            joined.push('\\');
        }
        joined.push_str(&part.replace('/', "\\"));
    }
    joined
}
