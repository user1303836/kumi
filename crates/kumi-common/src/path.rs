//! Paths a model or a file names, judged before anything opens them.

/// Whether `path` names another machine or a device the way Windows reads it: two separators in any mix at its start
/// (`\\host\share`, `//host/share`, `/\host\share`, `\/host\share`, and so `\\?\UNC\…` and `\\.\…` too), or the NT
/// object prefix `\??\` (which reaches `\??\UNC\host\share`). Opening one makes Windows connect to that host, offering
/// the user's NTLM credentials, and a host that doesn't answer holds the caller for the network's timeout. No local
/// path starts this way on any platform, so callers refuse these everywhere.
pub fn network_or_device(path: &str) -> bool {
    let separator = |c: Option<char>| matches!(c, Some('/' | '\\'));
    let mut chars = path.chars();
    let first = chars.next();
    if !separator(first) {
        return false;
    }
    let rest = chars.as_str();
    separator(rest.chars().next()) || (rest.starts_with("??") && separator(rest[2..].chars().next()))
}

#[cfg(test)]
mod tests {
    use super::network_or_device;
    #[test]
    fn every_spelling_of_a_share_or_device_is_caught_and_local_paths_are_not() {
        for path in [
            r"\\host\share\kick.wav",
            "//host/share/kick.wav",
            r"/\host\share\kick.wav",
            r"\/host\share\kick.wav",
            r"\\?\UNC\host\share\kick.wav",
            r"\\?\C:\Samples\kick.wav",
            r"\\.\pipe\name",
            r"\??\UNC\host\share\kick.wav",
            r"\??\C:\Samples\kick.wav",
            "/??/UNC/host/share",
        ] {
            assert!(network_or_device(path), "{path}");
        }
        for path in [
            "/Users/me/Music/kick.wav",
            r"C:\Users\me\Music\kick.wav",
            "C:/Users/me/Music/kick.wav",
            r"Z:\Samples\kick.wav",
            "/?/kick.wav",
            "/??kick.wav",
            "~/Music/kick.wav",
            "kick.wav",
            "",
            "/",
            "é/",
        ] {
            assert!(!network_or_device(path), "{path}");
        }
    }
}
