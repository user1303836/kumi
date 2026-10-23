//! Where Kumi keeps each DAW bridge's files: `<Kumi's folder>/bridges/<daw>/`. Ableton's bridge keeps its own layout
//! (`<Kumi's folder>/bridge/`), untouched.
use crate::daw::Daw;
use std::path::{Path, PathBuf};

/// Kumi's folder: KUMI_HOME when set and not empty, else `.kumi` in the user's home.
/// The reference file beside REAPER's extension in UserPlugins: Kumi's lifecycle writes it and the extension reads it.
pub const REAPER_REFERENCE: &str = "kumi-bridge-reference.json";

pub fn kumi_home() -> Option<PathBuf> {
    kumi_home_from(&|name| std::env::var(name).ok(), home::home_dir())
}

pub fn kumi_home_from(env: &dyn Fn(&str) -> Option<String>, home: Option<PathBuf>) -> Option<PathBuf> {
    env("KUMI_HOME").filter(|value| !value.is_empty()).map(PathBuf::from).or_else(|| home.map(|home| home.join(".kumi")))
}

/// One DAW bridge's files in Kumi's folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgePaths {
    pub daw: Daw,
    /// `<Kumi's folder>/bridges/<daw>`, owner-only.
    pub root: PathBuf,
}

impl BridgePaths {
    pub fn new(kumi_home: &Path, daw: Daw) -> Self {
        Self { daw, root: kumi_home.join("bridges").join(daw.id()) }
    }

    /// The secret both ends prove they know.
    pub fn secret(&self) -> PathBuf {
        self.root.join("bridge.secret")
    }

    /// Where running bridges say where they listen.
    pub fn discovery(&self) -> PathBuf {
        self.root.join("discovery")
    }

    /// The bridge's own log (REAPER keeps none of its own).
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// What the host keeps of each project between sessions, to tell what changed since (catch-up).
    pub fn snapshots(&self) -> PathBuf {
        self.root.join("snapshots")
    }

    /// Each installed version of the bridge's files, kept for rollback.
    pub fn generations(&self) -> PathBuf {
        self.root.join("generations")
    }

    /// What the lifecycle installed where, and what it can go back to.
    pub fn receipt(&self) -> PathBuf {
        self.root.join("install-receipt.json")
    }

    /// Held (locked) for the length of an install, upgrade, rollback or uninstall: two at once would undo each other.
    pub fn lock(&self) -> PathBuf {
        self.root.join("lifecycle.lock")
    }

    /// An install, upgrade, rollback or uninstall in progress, so one cut short is finished or undone next time.
    pub fn journal(&self) -> PathBuf {
        self.root.join("lifecycle-journal.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kumi_home_honors_a_non_empty_override() {
        let home = Some(PathBuf::from("/h"));
        assert_eq!(kumi_home_from(&|_| None, home.clone()), Some(PathBuf::from("/h/.kumi")));
        assert_eq!(kumi_home_from(&|_| Some(String::new()), home.clone()), Some(PathBuf::from("/h/.kumi")));
        assert_eq!(kumi_home_from(&|_| Some("/k".into()), home), Some(PathBuf::from("/k")));
        let paths = BridgePaths::new(Path::new("/k"), Daw::Renoise);
        assert_eq!(paths.secret(), Path::new("/k/bridges/renoise/bridge.secret"));
        assert_eq!(paths.discovery(), Path::new("/k/bridges/renoise/discovery"));
    }
}
