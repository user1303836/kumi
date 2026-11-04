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
    /// What a bridge program beside its DAW (SoundCraft's) keeps of the copy of the DAW it started last
    /// ([`crate::files::Launched`]).
    pub const LAUNCH_RECORD: &str = "launched.json";
    /// Held from a look for the copy the program started through a start's record of it.
    pub const LAUNCH_LOCK: &str = "launch.lock";
    /// The output of the program's last starts of its DAW, a folder each.
    pub const RUNS: &str = "runs";

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

    /// The program's record of the copy of the DAW it started last ([`Self::LAUNCH_RECORD`]).
    pub fn launch_record(&self) -> PathBuf {
        self.root.join(Self::LAUNCH_RECORD)
    }

    /// Held (locked) from a look for the copy the program started through a start's record of it, so a second look waits
    /// and finds the first one's copy instead of starting another.
    pub fn launch_lock(&self) -> PathBuf {
        self.root.join(Self::LAUNCH_LOCK)
    }

    /// The output of the program's last starts of its DAW, a folder each.
    pub fn runs(&self) -> PathBuf {
        self.root.join(Self::RUNS)
    }

    /// Held (locked) by the one bridge program serving the copy of the DAW whose process is `pid`.
    pub fn serving_lock(&self, pid: u32) -> PathBuf {
        self.root.join(format!("serving-{pid}.lock"))
    }

    /// The pid a file in the bridge's folder is the serving lock of ([`Self::serving_lock`]), by its name.
    pub fn serving_lock_pid(name: &str) -> Option<u32> {
        name.strip_prefix("serving-")?.strip_suffix(".lock")?.parse().ok()
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
        let soundcraft = BridgePaths::new(Path::new("/k"), Daw::SoundCraft);
        assert_eq!(soundcraft.launch_record(), Path::new("/k/bridges/soundcraft/launched.json"));
        assert_eq!(soundcraft.serving_lock(42), Path::new("/k/bridges/soundcraft/serving-42.lock"));
        assert_eq!(BridgePaths::serving_lock_pid("serving-42.lock"), Some(42));
        assert_eq!(BridgePaths::serving_lock_pid("serving-x.lock"), None);
    }
}
