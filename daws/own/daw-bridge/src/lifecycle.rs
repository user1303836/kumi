//! Installing a DAW bridge's files: install, upgrade, rollback and uninstall, each recorded in a receipt, with every
//! installed version kept as a generation in Kumi's folder to go back to. The DAW's copy is replaced by renaming a
//! finished copy into place, never written in place: a running REAPER keeps the old file mapped (Windows locks it).
//! A copy in the DAW's folder that isn't what Kumi placed (changed since, or there before Kumi) is kept in Kumi's
//! folder before anything replaces or removes it, never lost.
use crate::auth::sha256_hex;
use crate::daw::Daw;
use crate::files::{ensure_secret, FileError, Reference};
use crate::owner;
use crate::paths::BridgePaths;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const RECEIPT_SCHEMA: &str = "kumi-daw-receipt/1";
/// Earlier generations kept for rollback.
pub const KEPT_GENERATIONS: usize = 3;
const HISTORY: usize = 20;
/// Files inside a bridge folder that aren't the bridge's own: written beside it by the installer or by the DAW.
pub const NOT_PAYLOAD: [&str; 2] = ["bridge-reference.json", "preferences.xml"];
/// What a file browser leaves in any folder it shows (Finder's, Explorer's): never a change, never copied.
pub const LITTER: [&str; 3] = [".DS_Store", "Thumbs.db", "desktop.ini"];

/// Where a DAW's bridge goes: one file (REAPER's extension) or one folder (Renoise's tool) in `dir`, and the
/// reference file beside or inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub daw: Daw,
    /// REAPER's `UserPlugins`, Renoise's `Scripts/Tools`.
    pub dir: PathBuf,
    /// `reaper_kumi.dylib`, `com.kumi.Bridge.xrnx`.
    pub name: String,
    pub folder: bool,
    pub reference: PathBuf,
}

impl Target {
    pub fn path(&self) -> PathBuf {
        self.dir.join(&self.name)
    }

    /// The same bridge at another place: a copy's path and its reference.
    fn at(&self, copy: &Copy) -> Target {
        let path = Path::new(&copy.target);
        Target {
            daw: self.daw,
            dir: path.parent().map(Path::to_path_buf).unwrap_or_default(),
            name: path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| self.name.clone()),
            folder: self.folder,
            reference: PathBuf::from(&copy.reference),
        }
    }

    fn copy(&self) -> Copy {
        Copy { target: self.path().to_string_lossy().into_owned(), reference: self.reference.to_string_lossy().into_owned() }
    }
}

/// Another copy of Kumi's bridge in a DAW's folder, besides the receipt's target: where the target was before (a Renoise
/// version folder the tool moved on from), or a copy a newer Renoise made of it in the version folder it made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Copy {
    pub target: String,
    pub reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRow {
    pub action: String,
    pub generation: Option<String>,
    pub ms: u64,
    /// Where the action put the bridge or took it from (each copy it updated or removed is in the receipt's `copies`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Where the copy that was in the DAW's folder went, when it wasn't what Kumi placed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<String>,
    /// [`tree_sha256`] of what the action placed at `target`: uninstall removes Kumi's copy from each folder it was ever
    /// in (a newer Renoise's version folder takes the receipt there).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// An old copy set aside in the DAW's folder that couldn't be removed yet: a DLL a running REAPER holds (Windows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub schema: String,
    pub daw: String,
    /// `installed` or `uninstalled`.
    pub status: String,
    /// The installed file or folder.
    pub target: String,
    pub reference: String,
    pub generation: Option<String>,
    pub version: Option<String>,
    /// [`tree_sha256`] of what was placed.
    pub sha256: Option<String>,
    /// Generations to roll back to, newest first.
    #[serde(default)]
    pub previous: Vec<String>,
    /// The other copies of Kumi's bridge, each brought to the same generation as `target` and removed with it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub copies: Vec<Copy>,
    pub updated_ms: u64,
    #[serde(default)]
    pub history: Vec<HistoryRow>,
}

/// An action cut short leaves this; the next action cleans up after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Journal {
    action: String,
    /// Paths to remove if they're still there.
    leftovers: Vec<String>,
    ms: u64,
}

#[derive(Debug)]
pub enum LifecycleError {
    Io(std::io::Error),
    File(FileError),
    /// Nothing to do that way: not installed, already installed, nothing to roll back to.
    State(String),
    Invalid(String),
}

impl std::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LifecycleError::Io(error) => error.fmt(f),
            LifecycleError::File(error) => error.fmt(f),
            LifecycleError::State(why) | LifecycleError::Invalid(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for LifecycleError {}

impl From<std::io::Error> for LifecycleError {
    fn from(error: std::io::Error) -> Self {
        LifecycleError::Io(error)
    }
}

impl From<FileError> for LifecycleError {
    fn from(error: FileError) -> Self {
        LifecycleError::File(error)
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A file's or folder's contents as one SHA-256: each file's path relative to the root (with `/`), size and SHA-256,
/// sorted by path. A folder's [`NOT_PAYLOAD`] files at its top are left out, and [`LITTER`] anywhere.
pub fn tree_sha256(path: &Path) -> std::io::Result<String> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_file() {
        return Ok(sha256_hex(&std::fs::read(path)?));
    }
    if !meta.is_dir() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "neither a file nor a folder"));
    }
    let mut rows = vec![];
    collect(path, "", &mut rows)?;
    rows.sort();
    Ok(sha256_hex(rows.concat().as_bytes()))
}

fn collect(dir: &Path, prefix: &str, rows: &mut Vec<String>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if (prefix.is_empty() && NOT_PAYLOAD.contains(&name.as_str())) || LITTER.contains(&name.as_str()) {
            continue;
        }
        let relative = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect(&entry.path(), &relative, rows)?;
        } else if kind.is_file() {
            let bytes = std::fs::read(entry.path())?;
            rows.push(format!("{relative}\0{}\0{}\n", bytes.len(), sha256_hex(&bytes)));
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{} is a link, and bridge files can't hold links", entry.path().display()),
            ));
        }
    }
    Ok(())
}

/// Copies a bridge's file or folder. A link inside one is refused, by name (`keeping`: a copy kept for the producer
/// writes down where it pointed instead, as `<name>.link`, so nothing of theirs blocks an upgrade or an uninstall).
fn copy_tree(from: &Path, to: &Path, top: bool, keeping: bool) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.is_file() {
        std::fs::copy(from, to)?;
        return Ok(());
    }
    std::fs::create_dir(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if (top && NOT_PAYLOAD.contains(&name.to_string_lossy().as_ref())) || LITTER.contains(&name.to_string_lossy().as_ref()) {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() || kind.is_file() {
            copy_tree(&entry.path(), &to.join(&name), false, keeping)?;
        } else if keeping {
            let pointed = std::fs::read_link(entry.path()).map(|to| to.display().to_string()).unwrap_or_default();
            std::fs::write(
                to.join(format!("{}.link", name.to_string_lossy())),
                format!("{} was a link to {pointed}\n", entry.path().display()),
            )?;
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{} is a link, and bridge files can't hold links: move it out of the folder and try again", entry.path().display()),
            ));
        }
    }
    Ok(())
}

fn remove_tree(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// What [`status`] found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub receipt: Option<Receipt>,
    /// The installed file or folder is there.
    pub present: bool,
    /// It is what the receipt says was placed.
    pub matches: bool,
    pub reference: bool,
    pub secret: bool,
}

/// One DAW's bridge lifecycle, over Kumi's folder for it.
pub struct Lifecycle {
    pub paths: BridgePaths,
    pub target: Target,
    /// Other places a copy of Kumi's bridge may be (Renoise's other version folders): each one that holds a copy of
    /// Kumi's is updated and removed with the target, as the receipt's copies are.
    pub elsewhere: Vec<Target>,
}

impl Lifecycle {
    pub fn new(kumi_home: &Path, target: Target) -> Self {
        Self { paths: BridgePaths::new(kumi_home, target.daw), target, elsewhere: vec![] }
    }

    /// The same, with other places a copy of Kumi's bridge may be.
    pub fn with_elsewhere(mut self, elsewhere: Vec<Target>) -> Self {
        self.elsewhere = elsewhere;
        self
    }

    /// Where Kumi's bridge is now, by its receipt: the target while anything is there, else the first other place
    /// holding a copy of Kumi's (one a newer Renoise made). None when it isn't installed, or the producer took every
    /// copy out: then nothing should put it back unasked.
    pub fn found(&self) -> Result<Option<Target>, LifecycleError> {
        let Some(receipt) = self.receipt()?.filter(|receipt| receipt.status == "installed") else { return Ok(None) };
        if std::fs::symlink_metadata(self.target.path()).is_ok() {
            return Ok(Some(self.target.clone()));
        }
        let ours = self.ours(&receipt);
        Ok(self.others(Some(&receipt)).into_iter().find(|other| tree_sha256(&other.path()).is_ok_and(|sha| ours.contains(&sha))))
    }

    /// Every other place there may be a copy of Kumi's bridge, besides the target: the receipt's copies, its target when
    /// that was elsewhere, and the places given; each once.
    fn others(&self, receipt: Option<&Receipt>) -> Vec<Target> {
        let here = self.target.path();
        let mut others: Vec<Target> = vec![];
        let recorded = receipt.into_iter().flat_map(|receipt| {
            receipt
                .copies
                .iter()
                .cloned()
                .chain(std::iter::once(Copy { target: receipt.target.clone(), reference: receipt.reference.clone() }))
        });
        for target in recorded.map(|copy| self.target.at(&copy)).chain(self.elsewhere.iter().cloned()) {
            if target.path() != here && !others.iter().any(|other| other.path() == target.path()) {
                others.push(target);
            }
        }
        others
    }

    pub fn receipt(&self) -> Result<Option<Receipt>, LifecycleError> {
        let path = self.paths.receipt();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let receipt: Receipt =
            serde_json::from_str(&text).map_err(|error| LifecycleError::Invalid(format!("the receipt can't be read: {error}")))?;
        if receipt.schema != RECEIPT_SCHEMA || receipt.daw != self.target.daw.id() {
            return Err(LifecycleError::Invalid("the receipt is another kind".into()));
        }
        Ok(Some(receipt))
    }

    pub fn status(&self) -> Result<Status, LifecycleError> {
        let receipt = self.receipt()?.filter(|receipt| receipt.status == "installed");
        let target = receipt.as_ref().map(|receipt| PathBuf::from(&receipt.target)).unwrap_or_else(|| self.target.path());
        let present = target.exists();
        let matches = present && receipt.as_ref().and_then(|r| r.sha256.clone()) == tree_sha256(&target).ok();
        let reference = Reference::read(&self.target.reference).is_ok_and(|reference| reference.daw == self.target.daw.id());
        let secret = crate::files::read_secret(&self.paths.secret()).is_ok();
        Ok(Status { receipt, present, matches, reference, secret })
    }

    fn write_receipt(&self, receipt: &Receipt) -> Result<(), LifecycleError> {
        let mut text = serde_json::to_string_pretty(receipt).expect("receipts serialize");
        text.push('\n');
        owner::write_owner_only(&self.paths.receipt(), text.as_bytes())?;
        Ok(())
    }

    /// Waits for any other install, upgrade, rollback or uninstall of this DAW's bridge (in this process or another) to
    /// finish, then holds the lock until the returned file is dropped.
    fn exclusive(&self) -> Result<std::fs::File, LifecycleError> {
        self.make_root()?;
        // Opened in place, never replaced: a lock is on the file itself, and a replaced one is another file. The folder
        // is owner-only already.
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(self.paths.lock())?;
        file.lock()?;
        Ok(file)
    }

    /// Makes Kumi's folder for this DAW when it isn't there. The lock is in it, so two actions starting at once both get
    /// here before either waits: the folder is made owner-only beside its place (`.<name>.tmp-<random>`) and only then
    /// renamed into it, so neither ever works in it while the other is still setting its permissions (on Windows a
    /// PowerShell run of seconds; a folder's permissions, as they're set, spread to what's inside it, and could undo those
    /// the other was setting in there). When another's got there first, this one gives way. One there already is left as
    /// it is: it was locked as it was made.
    fn make_root(&self) -> Result<(), LifecycleError> {
        let root = &self.paths.root;
        if !root.is_dir() {
            let aside = owner::temporary_beside(root);
            if let Err(error) = owner::create_owner_only_dir(&aside).and_then(|()| std::fs::rename(&aside, root)) {
                let _ = std::fs::remove_dir(&aside);
                // (another's, or this one taken away by its clean-up: see recover)
                if !root.is_dir() {
                    return Err(error.into());
                }
            }
        }
        // (unix also sets a folder that's there back to 0700, as before)
        owner::create_owner_only_dir(root)?;
        Ok(())
    }

    fn begin(&self, action: &str, leftovers: &[&Path]) -> Result<(), LifecycleError> {
        let journal = Journal {
            action: action.into(),
            leftovers: leftovers.iter().map(|path| path.to_string_lossy().into_owned()).collect(),
            ms: now_ms(),
        };
        owner::write_owner_only(&self.paths.journal(), serde_json::to_string(&journal).expect("journals serialize").as_bytes())?;
        Ok(())
    }

    fn end(&self) {
        let _ = std::fs::remove_file(self.paths.journal());
    }

    /// Cleans up after an action that was cut short, and after old copies a running DAW held.
    fn recover(&self) -> Result<(), LifecycleError> {
        if let Ok(text) = std::fs::read_to_string(self.paths.journal()) {
            if let Ok(journal) = serde_json::from_str::<Journal>(&text) {
                for leftover in journal.leftovers {
                    let _ = remove_tree(Path::new(&leftover));
                }
            }
            self.end();
        }
        let old = format!("{}.kumi-old-", self.target.name);
        if let Ok(entries) = std::fs::read_dir(&self.target.dir) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with(&old) {
                    let _ = remove_tree(&entry.path());
                }
            }
        }
        // Kumi's folder made aside by an action stopped before it was renamed into place (make_root, named as
        // owner::temporary_beside names it): always empty. One another action is still making gives way to this.
        if let (Some(parent), Some(name)) = (self.paths.root.parent(), self.paths.root.file_name()) {
            let aside = format!(".{}.tmp-", name.to_string_lossy());
            if let Ok(entries) = std::fs::read_dir(parent) {
                for entry in entries.flatten() {
                    if entry.file_name().to_string_lossy().starts_with(&aside) {
                        let _ = std::fs::remove_dir(entry.path());
                    }
                }
            }
        }
        Ok(())
    }

    /// Copies `source` into a new generation; returns its name and hash.
    fn stage(&self, source: &Path, version: &str) -> Result<(String, String), LifecycleError> {
        if version.is_empty() || !version.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')) {
            return Err(LifecycleError::Invalid(format!("not a version: {version}")));
        }
        owner::create_owner_only_dir(&self.paths.generations())?;
        // Named by version and time; two in the same millisecond take the next free one.
        let mut ms = now_ms();
        let (generation, dir) = loop {
            let generation = format!("{version}-{ms}");
            let dir = self.paths.generations().join(&generation);
            match std::fs::create_dir(&dir) {
                Ok(()) => break (generation, dir),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => ms += 1,
                Err(error) => return Err(error.into()),
            }
        };
        let staged = dir.join(&self.target.name);
        if let Err(error) = copy_tree(source, &staged, true, false) {
            let _ = remove_tree(&dir);
            return Err(error.into());
        }
        let sha = tree_sha256(&staged)?;
        if sha != tree_sha256(source)? {
            let _ = remove_tree(&dir);
            return Err(LifecycleError::Invalid("the copy differs from its source".into()));
        }
        Ok((generation, sha))
    }

    /// Copies what's at `path` (a file, or a folder with everything in it) into Kumi's `changed` folder; where it went.
    /// A link isn't followed: where it pointed is written down instead.
    fn keep(&self, path: &Path) -> Result<PathBuf, LifecycleError> {
        let kept = self.paths.root.join("changed");
        std::fs::create_dir_all(&kept)?;
        let link = std::fs::symlink_metadata(path)?.file_type().is_symlink();
        let mut ms = now_ms();
        let copy = loop {
            let copy = kept.join(format!("{}-{ms}{}", self.target.name, if link { ".link" } else { "" }));
            if std::fs::symlink_metadata(&copy).is_err() {
                break copy;
            }
            ms += 1;
        };
        let copied = match link {
            true => std::fs::read_link(path)
                .and_then(|to| std::fs::write(&copy, format!("{} was a link to {}\n", path.display(), to.display()))),
            false => copy_tree(path, &copy, false, true),
        };
        if let Err(error) = copied {
            let _ = remove_tree(&copy);
            return Err(error.into());
        }
        Ok(copy)
    }

    /// Puts a generation's copy in the DAW's folder: a finished copy beside the target, then renamed over it. A
    /// folder, or a file the system won't replace while it's in use, first has the old one renamed aside. What's there
    /// first is kept unless it's a copy Kumi placed (one of `ours`: hashes of what Kumi installed): its SHA-256 and where
    /// the kept copy went.
    fn place(&self, generation: &str, ours: &[String]) -> Result<(String, Option<PathBuf>), LifecycleError> {
        self.place_at(&self.target, generation, ours)
    }

    /// [`place`](Self::place), at `at`: the target or one of the other copies.
    fn place_at(&self, at: &Target, generation: &str, ours: &[String]) -> Result<(String, Option<PathBuf>), LifecycleError> {
        let source = self.paths.generations().join(generation).join(&self.target.name);
        let sha = tree_sha256(&source)?;
        std::fs::create_dir_all(&at.dir)?;
        let target = at.path();
        let changed = std::fs::symlink_metadata(&target).is_ok() && !tree_sha256(&target).is_ok_and(|sha| ours.contains(&sha));
        let kept = if changed { Some(self.keep(&target)?) } else { None };
        let token = &crate::auth::random_hex16()[..12];
        let new = at.dir.join(format!(".{}.kumi-new-{token}", at.name));
        let old = at.dir.join(format!("{}.kumi-old-{token}", at.name));
        self.begin("place", &[&new])?;
        // Renoise writes its tool preferences inside the folder: they stay with the tool.
        let kept_preferences = at.folder.then(|| std::fs::read(target.join("preferences.xml")).ok()).flatten();
        copy_tree(&source, &new, true, false)?;
        if let Some(preferences) = kept_preferences {
            std::fs::write(new.join("preferences.xml"), preferences)?;
        }
        if tree_sha256(&new)? != sha {
            let _ = remove_tree(&new);
            return Err(LifecycleError::Invalid("the copy beside the bridge differs from its generation".into()));
        }
        // A copy keeps macOS's downloaded mark of what it came from (a bundle a browser fetched): the DAW wouldn't load it.
        if let Err(error) = crate::detect::unquarantine(&new) {
            let _ = remove_tree(&new);
            return Err(error.into());
        }
        let replaced = if !at.folder { std::fs::rename(&new, &target).is_ok() } else { false };
        if !replaced {
            if target.exists() {
                std::fs::rename(&target, &old)?;
            }
            if let Err(error) = std::fs::rename(&new, &target) {
                if old.exists() {
                    let _ = std::fs::rename(&old, &target);
                }
                let _ = remove_tree(&new);
                self.end();
                return Err(error.into());
            }
            // A DLL Windows holds open can't be removed yet: the next action removes it.
            let _ = remove_tree(&old);
        }
        self.end();
        Ok((sha, kept))
    }

    /// The hashes of what Kumi installed, by the receipt: the copy in place, each generation kept, and each copy placed.
    fn ours(&self, receipt: &Receipt) -> Vec<String> {
        let kept = receipt.generation.iter().chain(receipt.previous.iter());
        let kept = kept.filter_map(|generation| tree_sha256(&self.paths.generations().join(generation).join(&self.target.name)).ok());
        let placed = receipt.history.iter().filter_map(|row| row.sha256.clone());
        receipt.sha256.iter().cloned().chain(kept).chain(placed).collect()
    }

    fn write_reference(&self) -> Result<(), LifecycleError> {
        self.write_reference_at(&self.target)
    }

    fn write_reference_at(&self, at: &Target) -> Result<(), LifecycleError> {
        let reference = Reference::new(self.target.daw, &self.paths);
        if let Some(parent) = at.reference.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = owner::temporary_beside(&at.reference);
        std::fs::write(&temporary, reference.to_json())?;
        std::fs::rename(&temporary, &at.reference)?;
        Ok(())
    }

    /// Brings each other copy that's still Kumi's (one of `ours`) to `generation` (whose hash is `sha`): the copies kept,
    /// each now holding it. One gone, or one the producer changed since, is left as it is and no longer counted.
    fn follow_copies(&self, others: &[Target], generation: &str, sha: &str, ours: &[String]) -> Result<Vec<Copy>, LifecycleError> {
        let mut copies = vec![];
        for other in others {
            let Ok(found) = tree_sha256(&other.path()) else { continue };
            if found != sha {
                if !ours.contains(&found) {
                    continue;
                }
                self.place_at(other, generation, ours)?;
            }
            self.write_reference_at(other)?;
            copies.push(other.copy());
        }
        Ok(copies)
    }

    fn prune(&self, receipt: &Receipt) {
        let keep: Vec<&String> = receipt.generation.iter().chain(receipt.previous.iter()).collect();
        if let Ok(entries) = std::fs::read_dir(self.paths.generations()) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !keep.iter().any(|kept| **kept == name) {
                    let _ = remove_tree(&entry.path());
                }
            }
        }
    }

    fn record(&self, mut receipt: Receipt, action: &str, kept: Option<PathBuf>, left: Option<PathBuf>) -> Result<Receipt, LifecycleError> {
        receipt.updated_ms = now_ms();
        let (kept, left) = (kept.map(|path| path.to_string_lossy().into_owned()), left.map(|path| path.to_string_lossy().into_owned()));
        let row = HistoryRow {
            action: action.into(),
            generation: receipt.generation.clone(),
            ms: receipt.updated_ms,
            kept,
            left,
            target: Some(receipt.target.clone()),
            sha256: receipt.sha256.clone(),
        };
        receipt.history.insert(0, row);
        receipt.history.truncate(HISTORY);
        receipt.previous.truncate(KEPT_GENERATIONS);
        self.write_receipt(&receipt)?;
        self.prune(&receipt);
        Ok(receipt)
    }

    /// Installs `source` (the bridge's file or folder) as `version`, or upgrades to it when installed already: the
    /// secret is made if missing, the reference written, and the version before kept to roll back to. A copy Kumi keeps
    /// already (the one installed, or one to roll back to) is put back rather than copied again, and when it's the one
    /// in place, nothing changes: an update that brings the same bridge never pushes out the one to roll back to.
    pub fn install(&self, source: &Path, version: &str) -> Result<Receipt, LifecycleError> {
        self.put(source, version, false)
    }

    /// Goes to `source` (the copy an earlier Kumi carries), recorded as a rollback: the generation kept with the same
    /// files, else a new one; nothing changes when it's the one in place.
    pub fn rollback_to(&self, source: &Path, version: &str) -> Result<Receipt, LifecycleError> {
        self.put(source, version, true)
    }

    fn put(&self, source: &Path, version: &str, rollback: bool) -> Result<Receipt, LifecycleError> {
        let _lock = self.exclusive()?;
        // The bridge writes into these but may not be able to make them (Renoise's Lua can't make folders).
        owner::create_owner_only_dir(&self.paths.discovery())?;
        owner::create_owner_only_dir(&self.paths.logs())?;
        self.recover()?;
        if version.is_empty() || !version.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')) {
            return Err(LifecycleError::Invalid(format!("not a version: {version}")));
        }
        let before = self.receipt()?.filter(|receipt| receipt.status == "installed");
        let sha = tree_sha256(source)?;
        ensure_secret(&self.paths.secret())?;
        let here = self.target.path().to_string_lossy().into_owned();
        // Kumi's own copies, wherever they turn up: a newer Renoise copies the tool into the version folder it makes.
        let ours = before.as_ref().map(|before| self.ours(before)).unwrap_or_default();
        let others = self.others(before.as_ref());
        let behind = |other: &Target| tree_sha256(&other.path()).is_ok_and(|found| found != sha && ours.contains(&found));
        let in_place = before.as_ref().is_some_and(|before| before.target == here && before.sha256.as_deref() == Some(sha.as_str()))
            && tree_sha256(&self.target.path()).ok().as_deref() == Some(sha.as_str());
        if let Some(before) = before.as_ref().filter(|_| in_place && !others.iter().any(behind)) {
            self.write_reference()?;
            return Ok(before.clone());
        }
        let kept = before.iter().flat_map(|before| before.generation.iter().chain(before.previous.iter())).find(|generation| {
            tree_sha256(&self.paths.generations().join(generation).join(&self.target.name)).ok().as_deref() == Some(sha.as_str())
        });
        let (generation, staged) = match kept {
            Some(generation) => (generation.clone(), false),
            None => (self.stage(source, version)?.0, true),
        };
        let placed = match in_place {
            true => Ok((sha.clone(), None)),
            false => self.place(&generation, &ours),
        };
        let (sha, kept) = match placed {
            Ok(placed) => placed,
            Err(error) => {
                if staged {
                    let _ = remove_tree(&self.paths.generations().join(&generation));
                }
                return Err(error);
            }
        };
        self.write_reference()?;
        let copies = self.follow_copies(&others, &generation, &sha, &ours)?;
        let current = before.as_ref().and_then(|before| before.generation.clone());
        let action = match (&before, rollback) {
            (None, false) => "install",
            (_, true) => "rollback",
            (Some(_), false) if current.as_deref() == Some(generation.as_str()) => "reinstall",
            (Some(_), false) => "upgrade",
        };
        // The one this replaces goes first among those to roll back to; the one put back leaves them.
        let previous = match &before {
            Some(before) => before.generation.iter().chain(before.previous.iter()).filter(|kept| **kept != generation).cloned().collect(),
            None => vec![],
        };
        let receipt = Receipt {
            schema: RECEIPT_SCHEMA.into(),
            daw: self.target.daw.id().into(),
            status: "installed".into(),
            target: here,
            reference: self.target.reference.to_string_lossy().into_owned(),
            generation: Some(generation),
            version: Some(version.into()),
            sha256: Some(sha),
            previous,
            copies,
            updated_ms: 0,
            history: before.map(|before| before.history).unwrap_or_default(),
        };
        self.record(receipt, action, kept, None)
    }

    /// Goes back to the generation installed before this one.
    pub fn rollback(&self) -> Result<Receipt, LifecycleError> {
        let _lock = self.exclusive()?;
        self.recover()?;
        let Some(mut receipt) = self.receipt()?.filter(|receipt| receipt.status == "installed") else {
            return Err(LifecycleError::State(format!("Kumi's {} bridge isn't installed", self.target.daw)));
        };
        let Some(earlier) = receipt.previous.first().cloned() else {
            return Err(LifecycleError::State("there's no earlier version to go back to".into()));
        };
        if !self.paths.generations().join(&earlier).join(&self.target.name).exists() {
            return Err(LifecycleError::State(format!("the earlier version {earlier} is no longer kept")));
        }
        let ours = self.ours(&receipt);
        let others = self.others(Some(&receipt));
        let (sha, kept) = self.place(&earlier, &ours)?;
        self.write_reference()?;
        receipt.copies = self.follow_copies(&others, &earlier, &sha, &ours)?;
        receipt.previous.remove(0);
        receipt.version = earlier.rsplit_once('-').map(|(version, _)| version.to_string());
        receipt.generation = Some(earlier);
        receipt.sha256 = Some(sha);
        receipt.target = self.target.path().to_string_lossy().into_owned();
        self.record(receipt, "rollback", kept, None)
    }

    /// Removes the bridge from the DAW's folder: the file or folder Kumi placed (one changed since is moved into
    /// Kumi's folder instead, never deleted), its reference, and the kept generations. Kumi's copies in the other
    /// folders it was in, and in the DAW's folder now (a newer Renoise copies the tool into its own), go too; one that
    /// isn't Kumi's any more stays. The secret stays unless `purge_secret`.
    pub fn uninstall(&self, purge_secret: bool) -> Result<Receipt, LifecycleError> {
        let _lock = self.exclusive()?;
        self.recover()?;
        let Some(mut receipt) = self.receipt()? else {
            return Err(LifecycleError::State(format!("Kumi's {} bridge isn't installed", self.target.daw)));
        };
        let ours = self.ours(&receipt);
        let mut elsewhere: Vec<PathBuf> = vec![];
        let candidates = receipt.history.iter().filter_map(|row| row.target.as_ref().map(PathBuf::from)).chain([self.target.path()]);
        for path in candidates {
            if path != Path::new(&receipt.target) && !elsewhere.contains(&path) {
                elsewhere.push(path);
            }
        }
        for path in elsewhere {
            if tree_sha256(&path).is_ok_and(|sha| ours.contains(&sha)) {
                let token = &crate::auth::random_hex16()[..12];
                let aside = path.with_file_name(format!("{}.kumi-old-{token}", self.target.name));
                if std::fs::rename(&path, &aside).is_ok() {
                    let _ = remove_tree(&aside);
                }
            }
        }
        let target = PathBuf::from(&receipt.target);
        let (mut kept, mut left) = (None, None);
        if std::fs::symlink_metadata(&target).is_ok() {
            // One changed since Kumi placed it is copied into Kumi's folder first (a copy works across disks).
            if tree_sha256(&target).ok() != receipt.sha256 {
                kept = Some(self.keep(&target)?);
            }
            let token = &crate::auth::random_hex16()[..12];
            let aside = target.with_file_name(format!("{}.kumi-old-{token}", self.target.name));
            std::fs::rename(&target, &aside)?;
            // A DLL a running REAPER holds can't be removed (Windows): the receipt names it, as no later action of
            // Kumi's will be there to clean it up.
            if remove_tree(&aside).is_err() {
                left = Some(aside);
            }
        }
        let reference = PathBuf::from(&receipt.reference);
        if Reference::read(&reference).is_ok_and(|reference| reference.daw == self.target.daw.id()) {
            let _ = std::fs::remove_file(&reference);
        }
        // The other copies: each still Kumi's goes too (a recorded one the producer changed since is kept first, as the
        // target is); one of theirs found elsewhere stays.
        let ours = self.ours(&receipt);
        let recorded: Vec<PathBuf> = receipt.copies.iter().map(|copy| PathBuf::from(&copy.target)).collect();
        for other in self.others(Some(&receipt)).iter().filter(|other| other.path() != target) {
            let path = other.path();
            let Ok(found) = tree_sha256(&path).or_else(|_| std::fs::symlink_metadata(&path).map(|_| String::new())) else { continue };
            if !ours.contains(&found) {
                if !recorded.contains(&path) {
                    continue;
                }
                kept = kept.or(Some(self.keep(&path)?));
            }
            let token = &crate::auth::random_hex16()[..12];
            let aside = path.with_file_name(format!("{}.kumi-old-{token}", other.name));
            std::fs::rename(&path, &aside)?;
            if remove_tree(&aside).is_err() {
                left = left.or(Some(aside));
            }
            if Reference::read(&other.reference).is_ok_and(|reference| reference.daw == self.target.daw.id()) {
                let _ = std::fs::remove_file(&other.reference);
            }
        }
        if purge_secret {
            let _ = std::fs::remove_file(self.paths.secret());
        }
        receipt.status = "uninstalled".into();
        receipt.generation = None;
        receipt.version = None;
        receipt.sha256 = None;
        receipt.previous.clear();
        receipt.copies.clear();
        self.record(receipt, "uninstall", kept, left)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge_folder(root: &Path, version: &str) -> PathBuf {
        let dir = root.join(format!("source-{version}")).join("com.kumi.Bridge.xrnx");
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(dir.join("main.lua"), format!("-- {version}\n")).unwrap();
        std::fs::write(dir.join("lib").join("json.lua"), "return {}\n").unwrap();
        dir
    }

    fn renoise(root: &Path) -> Lifecycle {
        let tools = root.join("prefs").join("V3.5.4").join("Scripts").join("Tools");
        let target = Target {
            daw: Daw::Renoise,
            dir: tools.clone(),
            name: "com.kumi.Bridge.xrnx".into(),
            folder: true,
            reference: tools.join("com.kumi.Bridge.xrnx").join("bridge-reference.json"),
        };
        Lifecycle::new(&root.join("kumi"), target)
    }

    #[test]
    fn install_upgrade_two_rollbacks_and_uninstall_of_a_folder() {
        let root = tempfile::tempdir().unwrap();
        let life = renoise(root.path());
        let installed = life.install(&bridge_folder(root.path(), "1.0"), "1.0").unwrap();
        assert_eq!((installed.status.as_str(), installed.version.as_deref()), ("installed", Some("1.0")));
        let status = life.status().unwrap();
        assert!(status.present && status.matches && status.reference && status.secret, "{status:?}");
        assert!(life.paths.discovery().is_dir() && life.paths.logs().is_dir());
        let secret = std::fs::read(life.paths.secret()).unwrap();

        // The tool's own preferences survive upgrades and aren't part of its hash.
        std::fs::write(life.target.path().join("preferences.xml"), "<prefs/>").unwrap();
        assert!(life.status().unwrap().matches);
        life.install(&bridge_folder(root.path(), "2.0"), "2.0").unwrap();
        let third = life.install(&bridge_folder(root.path(), "3.0"), "3.0").unwrap();
        assert_eq!(third.previous.len(), 2);
        assert_eq!(std::fs::read_to_string(life.target.path().join("main.lua")).unwrap(), "-- 3.0\n");
        assert_eq!(std::fs::read_to_string(life.target.path().join("preferences.xml")).unwrap(), "<prefs/>");
        assert_eq!(std::fs::read(life.paths.secret()).unwrap(), secret);

        assert_eq!(life.rollback().unwrap().version.as_deref(), Some("2.0"));
        assert_eq!(std::fs::read_to_string(life.target.path().join("main.lua")).unwrap(), "-- 2.0\n");
        assert_eq!(life.rollback().unwrap().version.as_deref(), Some("1.0"));
        assert_eq!(std::fs::read_to_string(life.target.path().join("main.lua")).unwrap(), "-- 1.0\n");
        assert!(matches!(life.rollback(), Err(LifecycleError::State(_))));
        assert!(life.status().unwrap().matches);
        assert_eq!(std::fs::read_dir(life.paths.generations()).unwrap().count(), 1);

        let removed = life.uninstall(false).unwrap();
        assert_eq!(removed.status, "uninstalled");
        assert!(!life.target.path().exists() && !life.target.reference.exists());
        assert!(life.paths.secret().exists());
        assert_eq!(std::fs::read_dir(&life.target.dir).unwrap().count(), 0);
        assert_eq!(
            removed.history.iter().map(|row| row.action.as_str()).collect::<Vec<_>>(),
            ["uninstall", "rollback", "rollback", "upgrade", "upgrade", "install"]
        );
        // Installing again after an uninstall is an install.
        assert_eq!(life.install(&bridge_folder(root.path(), "4.0"), "4.0").unwrap().history[0].action, "install");
    }

    fn read(path: PathBuf) -> String {
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    fn actions(receipt: &Receipt) -> Vec<&str> {
        receipt.history.iter().map(|row| row.action.as_str()).collect()
    }

    #[test]
    fn the_same_copy_again_changes_nothing_and_a_kept_one_is_put_back_not_copied_again() {
        let root = tempfile::tempdir().unwrap();
        let life = renoise(root.path());
        let (one, two) = (bridge_folder(root.path(), "1.0"), bridge_folder(root.path(), "2.0"));
        let installed = life.install(&one, "1.0").unwrap();
        assert_eq!(life.install(&one, "1.0").unwrap(), installed);
        // An update that brings the bridge that's in, however often: nothing changes, so the one to roll back to stays.
        let upgraded = life.install(&two, "2.0").unwrap();
        for _ in 0..KEPT_GENERATIONS + 2 {
            assert_eq!(life.install(&two, "2.0").unwrap(), upgraded);
        }
        assert_eq!(upgraded.previous, [installed.generation.clone().unwrap()]);
        // Kumi rolled back to one that carries 1.0, then forward again: each time the copy kept, never a new one.
        let back = life.rollback_to(&one, "1.0").unwrap();
        assert_eq!((back.generation.as_ref(), back.version.as_deref()), (installed.generation.as_ref(), Some("1.0")));
        assert_eq!(back.previous, [upgraded.generation.clone().unwrap()]);
        assert_eq!(read(life.target.path().join("main.lua")), "-- 1.0\n");
        let forward = life.rollback_to(&two, "2.0").unwrap();
        assert_eq!(
            (forward.generation.as_ref(), forward.previous.clone()),
            (upgraded.generation.as_ref(), vec![installed.generation.clone().unwrap()])
        );
        assert_eq!(read(life.target.path().join("main.lua")), "-- 2.0\n");
        assert_eq!(std::fs::read_dir(life.paths.generations()).unwrap().count(), 2);
        assert_eq!(actions(&forward), ["rollback", "rollback", "upgrade", "install"]);
        assert!(life.status().unwrap().matches);
        // A reference deleted meanwhile comes back, though nothing else changes.
        std::fs::remove_file(&life.target.reference).unwrap();
        assert_eq!(life.install(&two, "2.0").unwrap(), forward);
        assert!(life.status().unwrap().reference);
    }

    #[test]
    fn a_changed_copy_is_kept_when_an_upgrade_a_rollback_or_a_repair_replaces_it() {
        let root = tempfile::tempdir().unwrap();
        let life = renoise(root.path());
        let tool = life.target.path();
        life.install(&bridge_folder(root.path(), "1.0"), "1.0").unwrap();
        // The producer's notes in the tool's folder and an edit to it: the upgrade keeps them in Kumi's folder, with
        // Renoise's preferences, which stay with the tool too.
        std::fs::write(tool.join("my-notes.txt"), "mine").unwrap();
        std::fs::write(tool.join("main.lua"), "-- edited\n").unwrap();
        std::fs::write(tool.join("preferences.xml"), "<prefs/>").unwrap();
        let upgraded = life.install(&bridge_folder(root.path(), "2.0"), "2.0").unwrap();
        let kept = PathBuf::from(upgraded.history[0].kept.clone().expect("the changed copy is kept"));
        assert!(kept.starts_with(life.paths.root.join("changed")), "{}", kept.display());
        assert_eq!((read(kept.join("my-notes.txt")), read(kept.join("main.lua"))), ("mine".into(), "-- edited\n".into()));
        assert_eq!(read(kept.join("preferences.xml")), "<prefs/>");
        assert_eq!((read(tool.join("main.lua")), read(tool.join("preferences.xml"))), ("-- 2.0\n".into(), "<prefs/>".into()));
        assert!(!tool.join("my-notes.txt").exists() && life.status().unwrap().matches);
        // What Kumi placed, as it placed it, is replaced without a copy.
        let back = life.rollback_to(&bridge_folder(root.path(), "1.0"), "1.0").unwrap();
        assert_eq!((back.history[0].action.as_str(), back.history[0].kept.as_deref()), ("rollback", None));
        // The same copy again over an edited one is a repair, and the edit is kept first.
        std::fs::write(tool.join("main.lua"), "-- edited again\n").unwrap();
        let repaired = life.install(&bridge_folder(root.path(), "1.0"), "1.0").unwrap();
        assert_eq!(repaired.history[0].action, "reinstall");
        assert_eq!(read(PathBuf::from(repaired.history[0].kept.clone().unwrap()).join("main.lua")), "-- edited again\n");
        assert_eq!(read(tool.join("main.lua")), "-- 1.0\n");
        // The earlier copy's own rollback keeps an edit too.
        life.install(&bridge_folder(root.path(), "2.0"), "2.0").unwrap();
        std::fs::write(tool.join("main.lua"), "-- edited a third time\n").unwrap();
        let rolled = life.rollback().unwrap();
        assert_eq!(read(PathBuf::from(rolled.history[0].kept.clone().unwrap()).join("main.lua")), "-- edited a third time\n");
    }

    #[test]
    fn a_file_browsers_litter_is_no_change_and_a_copy_there_before_kumi_is_kept() {
        let root = tempfile::tempdir().unwrap();
        let life = renoise(root.path());
        // A tool folder by Kumi's name that Kumi didn't put there (one copied in by hand): kept.
        std::fs::create_dir_all(life.target.path()).unwrap();
        std::fs::write(life.target.path().join("main.lua"), "-- by hand\n").unwrap();
        let source = bridge_folder(root.path(), "1.0");
        std::fs::write(source.join(".DS_Store"), "finder").unwrap();
        let installed = life.install(&source, "1.0").unwrap();
        assert_eq!(read(PathBuf::from(installed.history[0].kept.clone().unwrap()).join("main.lua")), "-- by hand\n");
        // Finder's .DS_Store and Explorer's Thumbs.db: never copied, and never a change.
        assert!(!life.target.path().join(".DS_Store").exists());
        std::fs::write(life.target.path().join(".DS_Store"), "finder").unwrap();
        std::fs::write(life.target.path().join("lib").join("Thumbs.db"), "explorer").unwrap();
        assert!(life.status().unwrap().matches);
        assert_eq!(life.install(&bridge_folder(root.path(), "2.0"), "2.0").unwrap().history[0].kept, None);
    }

    #[test]
    fn kumis_own_copy_turning_up_in_a_newer_renoises_folder_is_replaced_without_a_copy() {
        let root = tempfile::tempdir().unwrap();
        let old = renoise(root.path());
        old.install(&bridge_folder(root.path(), "1.0"), "1.0").unwrap();
        // Renoise 3.5.5's first run copies 3.5.4's folder, Kumi's tool in it, into its own.
        let tools = root.path().join("prefs").join("V3.5.5").join("Scripts").join("Tools");
        std::fs::create_dir_all(&tools).unwrap();
        copy_tree(&old.target.path(), &tools.join("com.kumi.Bridge.xrnx"), false, false).unwrap();
        let target = Target {
            dir: tools.clone(),
            reference: tools.join("com.kumi.Bridge.xrnx").join("bridge-reference.json"),
            ..old.target.clone()
        };
        let new = Lifecycle::new(&root.path().join("kumi"), target);
        let upgraded = new.install(&bridge_folder(root.path(), "2.0"), "2.0").unwrap();
        assert_eq!((upgraded.history[0].kept.as_deref(), upgraded.target.as_str()), (None, new.target.path().to_str().unwrap()));
        assert!(new.status().unwrap().matches);
        // One the producer changed there is kept, as anywhere.
        std::fs::write(new.target.path().join("main.lua"), "-- edited\n").unwrap();
        assert!(new.install(&bridge_folder(root.path(), "1.0"), "1.0").unwrap().history[0].kept.is_some());
    }

    // Review H13: Kumi's copy goes from each Renoise folder it was in, and from the newest one's (Renoise copied it there,
    // Kumi wasn't updated since); one the producer changed stays.
    #[test]
    fn uninstall_removes_kumis_copy_from_each_renoise_folder_and_leaves_a_changed_one() {
        let root = tempfile::tempdir().unwrap();
        let version = |name: &str| {
            let tools = root.path().join("prefs").join(name).join("Scripts").join("Tools");
            std::fs::create_dir_all(&tools).unwrap();
            let target = Target {
                dir: tools.clone(),
                reference: tools.join("com.kumi.Bridge.xrnx").join("bridge-reference.json"),
                ..renoise(root.path()).target
            };
            Lifecycle::new(&root.path().join("kumi"), target)
        };
        let first = version("V3.5.3");
        first.install(&bridge_folder(root.path(), "1.0"), "1.0").unwrap();
        let second = version("V3.5.4");
        copy_tree(&first.target.path(), &second.target.path(), false, false).unwrap();
        second.install(&bridge_folder(root.path(), "2.0"), "2.0").unwrap();
        let third = version("V3.5.5");
        copy_tree(&second.target.path(), &third.target.path(), false, false).unwrap();
        let changed = version("V3.5.2");
        copy_tree(&first.target.path(), &changed.target.path(), false, false).unwrap();
        std::fs::write(changed.target.path().join("main.lua"), "-- the producer's\n").unwrap();

        third.uninstall(false).unwrap();
        for gone in [&first, &second, &third] {
            assert!(!gone.target.path().exists(), "{}", gone.target.path().display());
            assert_eq!(std::fs::read_dir(&gone.target.dir).unwrap().count(), 0, "nothing aside left");
        }
        assert_eq!(read(changed.target.path().join("main.lua")), "-- the producer's\n");
    }

    // Review H13: a link in the tool folder once stopped an upgrade that would keep it.
    #[cfg(unix)]
    #[test]
    fn a_link_in_the_tool_folder_doesnt_stop_an_upgrade() {
        // (review H13: a link once stopped every upgrade; the copy kept for the producer now notes where it pointed, as
        // the keeping test below checks, and the upgrade goes on)
        let root = tempfile::tempdir().unwrap();
        let life = renoise(root.path());
        life.install(&bridge_folder(root.path(), "1.0"), "1.0").unwrap();
        let link = life.target.path().join("lib").join("elsewhere.lua");
        std::os::unix::fs::symlink(root.path().join("nowhere.lua"), &link).unwrap();
        life.install(&bridge_folder(root.path(), "2.0"), "2.0").unwrap();
    }

    #[test]
    fn two_installs_at_once_wait_for_each_other_instead_of_undoing_each_other() {
        let root = tempfile::tempdir().unwrap();
        let folders: Vec<PathBuf> = (0..2).map(|i| bridge_folder(root.path(), &format!("{i}.0"))).collect();
        // (Each install on Windows runs PowerShell to lock its folders down: seconds each.)
        let rounds = if cfg!(windows) { 3 } else { 25 };
        let workers: Vec<_> = (0..2)
            .map(|i| {
                let life = renoise(root.path());
                let folder = folders[i].clone();
                std::thread::spawn(move || (0..rounds).map(|_| life.install(&folder, &format!("{i}.0")).map(|_| ())).collect::<Vec<_>>())
            })
            .collect();
        for worker in workers {
            for result in worker.join().unwrap() {
                result.unwrap();
            }
        }
        let status = renoise(root.path()).status().unwrap();
        assert!(status.present && status.matches, "{status:?}");
    }

    #[test]
    fn kumis_folder_is_owner_only_from_the_moment_another_action_can_see_it() {
        // Two actions starting at once both make Kumi's folder (the lock is in it): the one that gets the lock first must
        // never work in it while the other is still setting its permissions (#543). Taken the moment it appears, it's
        // owner-only already.
        let root = tempfile::tempdir().unwrap();
        let life = renoise(root.path());
        let folder = life.paths.root.clone();
        let taken = root.path().join("taken");
        let source = bridge_folder(root.path(), "1.0");
        let first = std::thread::spawn(move || life.install(&source, "1.0"));
        while std::fs::rename(&folder, &taken).is_err() {
            assert!(!first.is_finished() || folder.exists(), "the install made no folder");
            std::thread::yield_now();
        }
        // (the install, its folder taken, may fail: what it would have done doesn't matter here)
        let _ = first.join().unwrap();
        assert!(owner::is_owner_only(&taken));
    }

    #[test]
    fn a_file_target_is_replaced_by_rename_and_a_changed_one_is_kept_on_uninstall() {
        let root = tempfile::tempdir().unwrap();
        let plugins = root.path().join("REAPER").join("UserPlugins");
        let target = Target {
            daw: Daw::Reaper,
            dir: plugins.clone(),
            name: "reaper_kumi.dylib".into(),
            folder: false,
            reference: plugins.join("kumi-bridge-reference.json"),
        };
        let life = Lifecycle::new(&root.path().join("kumi"), target);
        let source = root.path().join("reaper_kumi.dylib");
        std::fs::write(&source, b"one").unwrap();
        life.install(&source, "0.1.0").unwrap();
        // Something the user dropped beside it is never touched.
        std::fs::write(plugins.join("reaper_other.dylib"), b"theirs").unwrap();
        std::fs::write(&source, b"two").unwrap();
        life.install(&source, "0.2.0").unwrap();
        assert_eq!(std::fs::read(plugins.join("reaper_kumi.dylib")).unwrap(), b"two");
        let reference = Reference::read(&plugins.join("kumi-bridge-reference.json")).unwrap();
        assert_eq!(reference.daw, "reaper");
        assert_eq!(reference.port, Some(9770));

        // A build the producer put in its place is kept when the next one goes in.
        std::fs::write(plugins.join("reaper_kumi.dylib"), b"their build").unwrap();
        std::fs::write(&source, b"three").unwrap();
        let upgraded = life.install(&source, "0.3.0").unwrap();
        assert_eq!(std::fs::read(upgraded.history[0].kept.clone().unwrap()).unwrap(), b"their build");
        assert_eq!(std::fs::read(plugins.join("reaper_kumi.dylib")).unwrap(), b"three");

        std::fs::write(plugins.join("reaper_kumi.dylib"), b"edited").unwrap();
        assert!(!life.status().unwrap().matches);
        let removed = life.uninstall(true).unwrap();
        assert_eq!(std::fs::read(removed.history[0].kept.clone().unwrap()).unwrap(), b"edited");
        assert!(!plugins.join("reaper_kumi.dylib").exists());
        assert_eq!(std::fs::read(plugins.join("reaper_other.dylib")).unwrap(), b"theirs");
        assert_eq!(std::fs::read_dir(life.paths.root.join("changed")).unwrap().count(), 2);
        assert!(!life.paths.secret().exists());
    }

    #[test]
    fn leftovers_of_an_interrupted_action_are_cleaned_up() {
        let root = tempfile::tempdir().unwrap();
        let life = renoise(root.path());
        life.install(&bridge_folder(root.path(), "1.0"), "1.0").unwrap();
        let stray = life.target.dir.join(".com.kumi.Bridge.xrnx.kumi-new-abc");
        let held = life.target.dir.join("com.kumi.Bridge.xrnx.kumi-old-def");
        // (Kumi's folder made aside by an action stopped before it was renamed into place)
        let aside = owner::temporary_beside(&life.paths.root);
        std::fs::create_dir_all(&stray).unwrap();
        std::fs::create_dir_all(&held).unwrap();
        std::fs::create_dir(&aside).unwrap();
        life.begin("place", &[&stray]).unwrap();
        life.install(&bridge_folder(root.path(), "2.0"), "2.0").unwrap();
        assert!(!stray.exists() && !held.exists() && !aside.exists() && !life.paths.journal().exists());
        assert!(life.status().unwrap().matches);
    }

    #[test]
    fn tree_hashes_see_names_and_contents_but_not_the_installers_files() {
        let root = tempfile::tempdir().unwrap();
        let folder = bridge_folder(root.path(), "1.0");
        let first = tree_sha256(&folder).unwrap();
        std::fs::write(folder.join("bridge-reference.json"), "{}").unwrap();
        assert_eq!(tree_sha256(&folder).unwrap(), first);
        std::fs::write(folder.join("lib").join("bridge-reference.json"), "{}").unwrap();
        assert_ne!(tree_sha256(&folder).unwrap(), first);
        assert!(life_version_rejected(root.path()));
    }

    /// Renoise's tool in another version folder, as a newer Renoise's first run copies it there.
    fn renoise_copy(life: &Lifecycle, root: &Path, version: &str) -> Target {
        let tools = root.join("prefs").join(version).join("Scripts").join("Tools");
        std::fs::create_dir_all(&tools).unwrap();
        copy_tree(&life.target.path(), &tools.join("com.kumi.Bridge.xrnx"), false, false).unwrap();
        Target { dir: tools.clone(), reference: tools.join("com.kumi.Bridge.xrnx").join("bridge-reference.json"), ..life.target.clone() }
    }

    #[test]
    fn every_copy_of_kumis_tool_is_brought_up_to_date_and_taken_out_with_it() {
        let root = tempfile::tempdir().unwrap();
        let life = renoise(root.path());
        life.install(&bridge_folder(root.path(), "1.0"), "1.0").unwrap();
        // Renoise 3.5.5 made its own copy; a 3.6 beta made one the producer then edited.
        let newer = renoise_copy(&life, root.path(), "V3.5.5");
        let beta = renoise_copy(&life, root.path(), "V3.6.0");
        std::fs::write(beta.path().join("main.lua"), "-- the producer's\n").unwrap();
        let life = renoise(root.path()).with_elsewhere(vec![newer.clone(), beta.clone()]);
        let upgraded = life.install(&bridge_folder(root.path(), "2.0"), "2.0").unwrap();
        assert_eq!(read(newer.path().join("main.lua")), "-- 2.0\n", "Kumi's copy follows the tool");
        assert!(newer.reference.exists());
        assert_eq!(read(beta.path().join("main.lua")), "-- the producer's\n", "one the producer changed is theirs");
        assert_eq!(upgraded.copies, vec![newer.copy()]);
        assert_eq!(upgraded.history[0].target.as_deref(), life.target.path().to_str());
        // Up to date everywhere: nothing changes. Behind somewhere: that copy is brought along.
        assert_eq!(life.install(&bridge_folder(root.path(), "2.0"), "2.0").unwrap().history.len(), upgraded.history.len());
        std::fs::write(newer.path().join("main.lua"), "-- 1.0\n").unwrap();
        let followed = life.install(&bridge_folder(root.path(), "2.0"), "2.0").unwrap();
        assert_eq!(followed.history.len(), upgraded.history.len() + 1);
        assert_eq!(read(newer.path().join("main.lua")), "-- 2.0\n", "a copy holding the 1.0 Kumi keeps is Kumi's: brought along");
        // The uninstall takes out every copy of Kumi's (the receipt's), and leaves the producer's.
        let removed = renoise(root.path()).uninstall(false).unwrap();
        assert!(!life.target.path().exists() && !newer.path().exists(), "{removed:?}");
        assert!(beta.path().join("main.lua").exists());
        assert!(removed.copies.is_empty());
    }

    #[test]
    fn a_tool_that_moves_to_another_version_folder_leaves_no_copy_behind() {
        let root = tempfile::tempdir().unwrap();
        let old = renoise(root.path());
        old.install(&bridge_folder(root.path(), "1.0"), "1.0").unwrap();
        // Set up again into Renoise 3.5.5's folder (nothing of Kumi's in it yet): the 3.5.4 copy is followed, then removed.
        let tools = root.path().join("prefs").join("V3.5.5").join("Scripts").join("Tools");
        let target = Target {
            dir: tools.clone(),
            reference: tools.join("com.kumi.Bridge.xrnx").join("bridge-reference.json"),
            ..old.target.clone()
        };
        let new = Lifecycle::new(&root.path().join("kumi"), target);
        let moved = new.install(&bridge_folder(root.path(), "2.0"), "2.0").unwrap();
        assert_eq!(moved.copies, vec![old.target.copy()]);
        assert_eq!(read(old.target.path().join("main.lua")), "-- 2.0\n");
        let removed = Lifecycle::new(&root.path().join("kumi"), new.target.clone()).uninstall(false).unwrap();
        assert!(!new.target.path().exists() && !old.target.path().exists(), "{removed:?}");
        assert!(!old.target.reference.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_the_tool_is_named_or_written_down_never_a_block() {
        let root = tempfile::tempdir().unwrap();
        let life = renoise(root.path());
        life.install(&bridge_folder(root.path(), "1.0"), "1.0").unwrap();
        // The producer's link inside Kumi's tool: kept as a note of where it pointed, and the upgrade and uninstall go on.
        std::os::unix::fs::symlink("/etc/hosts", life.target.path().join("lib").join("hosts")).unwrap();
        let upgraded = life.install(&bridge_folder(root.path(), "2.0"), "2.0").unwrap();
        let kept = PathBuf::from(upgraded.history[0].kept.as_ref().unwrap());
        assert!(read(kept.join("lib").join("hosts.link")).contains("was a link to /etc/hosts"));
        std::os::unix::fs::symlink("/etc/hosts", life.target.path().join("main2.lua")).unwrap();
        assert!(life.uninstall(false).unwrap().history[0].kept.is_some());
        assert!(!life.target.path().exists());
        // A source holding one is refused, by the link's name.
        let source = bridge_folder(root.path(), "3.0");
        std::os::unix::fs::symlink("/etc/hosts", source.join("lib").join("hosts")).unwrap();
        let refused = life.install(&source, "3.0").unwrap_err().to_string();
        assert!(refused.contains(&source.join("lib").join("hosts").display().to_string()) && refused.contains("is a link"), "{refused}");
    }

    fn life_version_rejected(root: &Path) -> bool {
        let life = renoise(root);
        matches!(life.install(&bridge_folder(root, "x"), "../x"), Err(LifecycleError::Invalid(_)))
    }
}
