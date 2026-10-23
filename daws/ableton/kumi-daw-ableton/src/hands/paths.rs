use std::path::{Path, PathBuf};

pub(crate) fn carried_helper(executable: &Path, name: &str) -> Option<PathBuf> {
    let app = executable.parent()?;
    [app.join("packages/runtime/hands").join(name), app.join("hands").join(name)].into_iter().find(|path| path.exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Folder(PathBuf);
    impl Folder {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path =
                std::env::temp_dir().join(format!("kumi-hands-path-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn helper(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative).join("kumi-hands-16045380d382");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"fixture helper").unwrap();
            path
        }
    }
    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn legacy_carried_path_wins_over_previous_native_layout() {
        let folder = Folder::new();
        folder.helper("hands");
        let legacy = folder.helper("packages/runtime/hands");
        assert_eq!(carried_helper(&folder.0.join("kumi"), "kumi-hands-16045380d382"), Some(legacy));
    }

    #[test]
    fn previous_native_layout_remains_available() {
        let folder = Folder::new();
        let native = folder.helper("hands");
        assert_eq!(carried_helper(&folder.0.join("kumi"), "kumi-hands-16045380d382"), Some(native));
    }

    #[test]
    fn absent_or_other_source_helper_falls_through_to_existing_cache() {
        let folder = Folder::new();
        assert_eq!(carried_helper(&folder.0.join("kumi"), "kumi-hands-16045380d382"), None);
        folder.helper("packages/runtime/hands");
        assert_eq!(carried_helper(&folder.0.join("kumi"), "kumi-hands-000000000000"), None);
    }
}
