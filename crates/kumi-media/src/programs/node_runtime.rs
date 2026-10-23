//! Reuse the JavaScript runtime retained by an existing Kumi installation for yt-dlp.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub(crate) fn find_node(env: &HashMap<String, String>, home: &Path, platform: &str) -> Option<String> {
    let variable = |name: &str| {
        env.get(name).or_else(|| {
            (platform == "win32").then(|| env.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value)).flatten()
        })
    };
    let kumi = variable("KUMI_HOME").filter(|value| !value.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".kumi"));
    let executable = |path: &Path| {
        let Ok(metadata) = std::fs::metadata(path) else { return false };
        if !metadata.is_file() {
            return false;
        }
        #[cfg(unix)]
        if platform != "win32" {
            use std::os::unix::fs::PermissionsExt;
            return metadata.permissions().mode() & 0o111 != 0;
        }
        true
    };
    let managed = kumi.join(if platform == "win32" { "node/node.exe" } else { "node/bin/node" });
    // The old installed application ran on this exact runtime, irrespective of PATH.
    if executable(&managed) {
        return Some(managed.to_string_lossy().into_owned());
    }
    let name = if platform == "win32" { "node.exe" } else { "node" };
    let delimiter = if platform == "win32" { ';' } else { ':' };
    variable("PATH")
        .into_iter()
        .flat_map(|value| value.split(delimiter))
        .filter(|folder| !folder.is_empty())
        .map(|folder| Path::new(folder).join(name))
        .find(|path| executable(path))
        .map(|path| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("kumi-retained-node-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn binary(&self, relative: &str) -> String {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"fixture runtime").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            path.to_string_lossy().into_owned()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[cfg(unix)]
    #[test]
    fn existing_managed_runtime_wins_over_an_unrelated_path_runtime() {
        let f = Fixture::new();
        let managed = f.binary("custom home/node/bin/node");
        f.binary("tools/node");
        let env = HashMap::from([
            ("KUMI_HOME".into(), f.0.join("custom home").to_string_lossy().into_owned()),
            ("PATH".into(), f.0.join("tools").to_string_lossy().into_owned()),
        ]);
        assert_eq!(find_node(&env, &f.0, "darwin"), Some(managed));
    }
    #[cfg(unix)]
    #[test]
    fn default_home_and_empty_override_find_the_retained_runtime() {
        let f = Fixture::new();
        let managed = f.binary(".kumi/node/bin/node");
        assert_eq!(find_node(&HashMap::new(), &f.0, "linux"), Some(managed.clone()));
        assert_eq!(find_node(&HashMap::from([("KUMI_HOME".into(), String::new())]), &f.0, "linux"), Some(managed));
    }
    #[test]
    fn windows_uses_its_installer_layout_and_case_insensitive_environment() {
        let f = Fixture::new();
        let managed = f.binary("custom home/node/node.exe");
        let env = HashMap::from([("Kumi_Home".into(), f.0.join("custom home").to_string_lossy().into_owned())]);
        assert_eq!(find_node(&env, &f.0, "win32").map(PathBuf::from), Some(PathBuf::from(managed)));
        std::fs::remove_file(f.0.join("custom home/node/node.exe")).unwrap();
        let path = f.binary("tools/node.exe");
        let env = HashMap::from([("Path".into(), format!("{};{}", f.0.join("missing").display(), f.0.join("tools").display()))]);
        assert_eq!(find_node(&env, &f.0, "win32").map(PathBuf::from), Some(PathBuf::from(path)));
    }
    #[cfg(unix)]
    #[test]
    fn absent_or_invalid_managed_node_falls_back_to_path() {
        let f = Fixture::new();
        let node = f.binary("tools/node");
        let env = HashMap::from([("PATH".into(), f.0.join("tools").to_string_lossy().into_owned())]);
        assert_eq!(find_node(&env, &f.0, "linux"), Some(node.clone()));
        std::fs::create_dir_all(f.0.join(".kumi/node/bin/node")).unwrap();
        assert_eq!(find_node(&env, &f.0, "linux"), Some(node));
        assert_eq!(find_node(&HashMap::new(), &f.0, "linux"), None);
    }
    #[cfg(unix)]
    #[test]
    fn nonexecutable_managed_node_does_not_hide_the_path_runtime() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        let managed = f.binary(".kumi/node/bin/node");
        std::fs::set_permissions(managed, std::fs::Permissions::from_mode(0o600)).unwrap();
        let node = f.binary("tools/node");
        let env = HashMap::from([("PATH".into(), f.0.join("tools").to_string_lossy().into_owned())]);
        assert_eq!(find_node(&env, &f.0, "darwin"), Some(node));
    }
}
