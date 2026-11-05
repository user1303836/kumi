//! Build a checked Max for Live device in Live's User Library; loading it waits for its Browser.

use super::{
    amxd::{encode_amxd, DeviceType},
    existing,
    findings::Findings,
    gen::{audio_effect_patcher, instrument_patcher, GenSpec, MIX, OUTPUT},
    harness::{check_midi_device_isolated, IsolatedOptions},
    midi::midi_device_patcher,
    patch::{page, reference},
    patched::patched_device,
    spec::{check_spec, kept_unrun, Control, DeviceSpec},
};
use crate::ears::device::EARS_NAME;
use async_trait::async_trait;
use futures::future::LocalBoxFuture;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::{json::stringify, number::to_string},
    time::now_ms,
};
use kumi_types::contracts::{JsonObject, KernelTool, Reach, ToolResult};
use kumi_types::{errors::RuntimeError, system::windows_device_name};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    future::Future,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, LazyLock},
    time::Duration,
};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

pub const MAKE_DEVICE_TOOL: &str = "make_device";
const BROWSER_LOADS: &str = "load_device waits for Live's Browser to list this file; it can be called now.";
/// How many objects one call can ask about.
const MOST_OBJECTS: usize = 16;
/// How many replaced devices' files are kept, the newest.
const KEPT: usize = 50;
static DATA: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("tool-data.json")).expect("device tool data"));
pub type BrowserSees = Rc<dyn Fn(String, Signal) -> LocalBoxFuture<'static, Result<bool, RuntimeError>>>;
#[derive(Clone)]
pub struct DeviceToolOptions {
    pub user_library: String,
    /// Typed builds check the Browser; standalone make_device leaves that wait to load_device.
    pub browser_sees: BrowserSees,
    /// The typed build's Browser wait; standalone make_device ignores it.
    pub wait_ms: Option<u64>,
    /// The devices this conversation made: the tool is made anew each turn, and these are kept for the conversation.
    pub made: Rc<RefCell<Made>>,
}
struct DeviceTool {
    options: DeviceToolOptions,
}
pub fn device_tool(options: DeviceToolOptions) -> Rc<dyn KernelTool> {
    Rc::new(DeviceTool { options })
}

/// The devices made in this conversation, each by the name it was asked for and the file it was made as. Making one
/// again by either name replaces it while its file is as Kumi wrote it. Any other device in the folder keeps its file:
/// Live keeps a Max for Live device in a Set as its file, so a saved Set may use it.
#[derive(Debug, Default)]
pub struct Made {
    conversation: String,
    devices: Vec<MadeDevice>,
    /// Where a replaced device's file is kept, if anywhere.
    kept: Option<PathBuf>,
    /// Devices written without asking Live's Browser, by item id: loading one waits for the Browser first.
    unlisted: Vec<String>,
}

#[derive(Debug, Clone)]
struct MadeDevice {
    asked: String,
    file: String,
    kind: DeviceType,
    written: u64,
}

impl Made {
    /// Made devices whose files are kept in `folder` when they're replaced.
    pub fn keeping(folder: PathBuf) -> Made {
        Made { kept: Some(folder), ..Made::default() }
    }

    /// The conversation from here on: the devices another made aren't this one's to replace.
    pub fn conversation(&mut self, id: &str) {
        if self.conversation != id {
            self.conversation = id.to_string();
            self.devices.clear();
        }
    }

    /// The devices of `kind` this conversation made that a device made by the name `asked` would replace: the one asked
    /// for by that name, then the one made as it. Which is still as Kumi wrote it is [`still_ours`]'s to say. No name asked
    /// for is no device's, and one of another kind isn't this one's to replace: a saved Set expects the kind it has.
    fn candidates(&self, asked: &str, kind: DeviceType) -> Vec<MadeDevice> {
        if asked.is_empty() {
            return Vec::new();
        }
        let by_name = self.devices.iter().find(|device| device.kind == kind && device.asked == asked);
        let by_file = self.devices.iter().find(|device| device.kind == kind && device.file == asked);
        let by_file = by_file.filter(|file| !by_name.is_some_and(|name| std::ptr::eq(*file, name)));
        by_name.into_iter().chain(by_file).cloned().collect()
    }

    /// Whether Live's Browser may not list this item yet: a device written without waiting for it.
    pub fn unlisted(&self, item_id: &str) -> bool {
        self.unlisted.iter().any(|item| item == item_id)
    }

    /// Live's Browser lists the item (or its wait is over): loading it waits no more.
    pub fn listed(&mut self, item_id: &str) {
        self.unlisted.retain(|item| item != item_id);
    }

    /// A device written whose presence in Live's Browser hasn't been checked yet.
    pub fn written_unlisted(&mut self, item_id: &str) {
        if !self.unlisted(item_id) {
            self.unlisted.push(item_id.to_string());
        }
    }

    /// A device made: it takes the place of the record of its own kind that had its file or its asked name. Each kind has
    /// a folder of its own, so a device of another kind with the same file is another device; and no name asked for is
    /// no device's name, so two devices made without one keep their records.
    fn made(&mut self, device: MadeDevice) {
        self.devices.retain(|kept| {
            kept.kind != device.kind || (kept.file != device.file && (device.asked.is_empty() || kept.asked != device.asked))
        });
        self.devices.push(device);
    }
}

/// The first of `candidates` whose file in `folder` is still as Kumi wrote it: each is read and hashed whole, which takes
/// as long as it's big.
fn still_ours(folder: &Path, candidates: Vec<MadeDevice>) -> Option<MadeDevice> {
    candidates
        .into_iter()
        .find(|device| std::fs::read(folder.join(format!("{}.amxd", device.file))).is_ok_and(|bytes| hashed(&bytes) == device.written))
}

fn hashed(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// The device's file name: its own, numbered past a file the folder has ("Echo 2") and a name `reserved` keeps (in any
/// case). On Windows a name it takes for a device ("Aux", "Con.Dist") is taken as a file's would be, and its number goes
/// right after that part ("Aux 2", "Con 2.Dist").
pub(crate) fn free_name(folder: &Path, name: &str, windows: bool, reserved: &[&str]) -> String {
    let (stem, rest) = name.split_at(name.find('.').unwrap_or(name.len()));
    let numbered = |number: &str| {
        if windows && windows_device_name(stem) {
            format!("{stem} {number}{rest}")
        } else {
            format!("{name} {number}")
        }
    };
    for index in 1..1000 {
        let candidate = if index == 1 { name.into() } else { numbered(&index.to_string()) };
        let taken = (windows && windows_device_name(&candidate))
            || reserved.iter().any(|kept| kept.eq_ignore_ascii_case(&candidate))
            || folder.join(format!("{candidate}.amxd")).exists();
        if !taken {
            return candidate;
        }
    }
    numbered(&Uuid::new_v4().to_string()[..8])
}

/// A copy of a device's file before it's replaced, in `kept` (the newest [`KEPT`] stay): where it is, None when it
/// couldn't be copied.
async fn keep(file: &Path, name: &str, kept: &Path) -> Option<PathBuf> {
    tokio::fs::create_dir_all(kept).await.ok()?;
    let copy = kept.join(format!("{name} {}.amxd", now_ms()));
    tokio::fs::copy(file, &copy).await.ok()?;
    let mut copies: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(kept)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .filter(|(_, path)| path.extension().is_some_and(|extension| extension == "amxd"))
        .collect();
    copies.sort();
    for (_, old) in copies.iter().rev().skip(KEPT) {
        let _ = tokio::fs::remove_file(old).await;
    }
    Some(copy)
}

/// The folder in the User Library's Kumi folder that holds a device of `kind`, named for its kind as Live's own preset
/// folders are. Live's Browser item says nothing else of what a device is, and the bridge loads an item that might be an
/// instrument onto no track that has one (Live would replace it): an effect kept anywhere else never goes after an
/// instrument.
pub fn kind_folder(kind: DeviceType) -> &'static str {
    match kind {
        DeviceType::MidiEffect => "MIDI Effects",
        DeviceType::AudioEffect => "Audio Effects",
        DeviceType::Instrument => "Instruments",
    }
}

fn kind_name(kind: DeviceType) -> &'static str {
    match kind {
        DeviceType::MidiEffect => "MIDI effect",
        DeviceType::AudioEffect => "audio effect",
        DeviceType::Instrument => "instrument",
    }
}

fn describe_control(control: &Control) -> String {
    match control {
        Control::Choice(c) => format!("{} ({}; {})", c.name, c.options.join(" / "), c.default),
        Control::Switch(c) => format!("{} (on/off; {})", c.name, if c.default { "on" } else { "off" }),
        Control::Number(c) | Control::Integer(c) => format!(
            "{} ({}–{}{}; {})",
            c.name,
            to_string(c.min),
            to_string(c.max),
            if c.unit.as_str().is_empty() { String::new() } else { format!(" {}", c.unit) },
            to_string(c.default)
        ),
    }
}

/// Whether a call needs what Kumi learned of this machine's Max: objects to look up, or a patch to check.
fn needs_reference(input: &JsonObject, asked: &[String]) -> bool {
    let patch = match input.get("patch") {
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(Value::Array(lines)) => lines.iter().filter_map(Value::as_str).any(|line| !line.trim().is_empty()),
        _ => false,
    };
    !asked.is_empty() || patch
}

/// What this machine's Max says of the objects asked about, each as it's typed in a box.
fn pages(texts: &[String], reference: Option<&reference::Reference>) -> String {
    let mut pages = match (texts.is_empty(), reference) {
        (true, _) => "objects: give one or more of Max's objects, each as it's typed in a box (\"makenote 60 100 250\").".to_string(),
        (false, None) => page::no_max(),
        (false, Some(reference)) => page::describe_all(reference, &texts[..texts.len().min(MOST_OBJECTS)]),
    };
    if texts.len() > MOST_OBJECTS {
        pages.push_str(&format!(
            "\n\nOnly the first {MOST_OBJECTS} were looked up: ask for the other {} in another call.",
            texts.len() - MOST_OBJECTS
        ));
    }
    pages
}

/// Why a device's file couldn't be written.
fn unwritable(folder: &Path, error: &std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::StorageFull {
        format!(
            "Kumi couldn't write the device: the disk Live's User Library is on ({}) is full. Free some space, then call make_device again.",
            folder.to_string_lossy()
        )
    } else {
        format!(
            "Kumi couldn't write the device to Live's User Library ({}): {error}. Make the folder writable, then call make_device again.",
            folder.to_string_lossy()
        )
    }
}

/// The name a device is saved under.
struct Naming {
    /// The name asked for: a device this conversation made by it is replaced.
    asked: String,
    /// The name it's made as, fitted, unless another device's file has it.
    wanted: String,
    /// Names it doesn't take, beside the folder's files: the device it's made from keeps its own.
    reserved: Vec<String>,
    /// The file it's never written over: the device it's made from.
    never: Option<PathBuf>,
}

/// A device written to Live's User Library.
struct Saved {
    name: String,
    file: PathBuf,
    item_id: String,
    /// The device of this conversation it replaced.
    ours: Option<MadeDevice>,
    /// Where the replaced device's file is kept.
    kept: Option<PathBuf>,
}

/// A new file put in place from `temporary` without replacing a file at `file`: a hard link, which fails when one's there
/// (a device another Kumi wrote since the name was chosen); where the disk has none, a rename unless one's there.
async fn put_new(temporary: &Path, file: &Path) -> std::io::Result<()> {
    match tokio::fs::hard_link(temporary, file).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(error),
        Err(_) => match tokio::fs::try_exists(file).await? {
            true => Err(std::io::ErrorKind::AlreadyExists.into()),
            false => tokio::fs::rename(temporary, file).await,
        },
    }
}

/// Whether two paths are one file.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Work as long as the device it reads is big, done beside the app's thread: a stop doesn't wait for it, and what it found
/// is dropped. None when the work itself failed.
pub(crate) async fn beside<T: Send + 'static>(
    signal: &Signal,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<Option<T>, RuntimeError> {
    let task = tokio::task::spawn_blocking(work);
    tokio::select! {
        biased;
        () = signal.cancelled() => Err(RuntimeError::Aborted),
        done = task => Ok(done.ok()),
    }
}

/// Writes `bytes` as `file` through a temporary file in `folder`: over the file there (`replace`), or only where there's
/// none ([`put_new`], AlreadyExists when one's there).
pub(crate) async fn write_device(folder: &Path, file: &Path, bytes: &[u8], replace: bool) -> std::io::Result<()> {
    let temporary = folder.join(format!(".{}.amxd", Uuid::new_v4()));
    let written: std::io::Result<()> = async {
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        options.mode(0o644);
        let mut out = options.open(&temporary).await?;
        out.write_all(bytes).await?;
        out.flush().await?;
        drop(out);
        if replace {
            tokio::fs::rename(&temporary, file).await
        } else {
            put_new(&temporary, file).await
        }
    }
    .await;
    // The temporary file goes as best it can: after a hard link it's still there, and a scanner holding it open (on
    // Windows) doesn't undo a device that's written.
    let _ = tokio::fs::remove_file(&temporary).await;
    written
}

/// Waits for Live's Browser to list a device just written, asking every 400 ms until `wait_ms` (20 s) is over, and marks
/// it listed, or written but unlisted, so the first load of it waits for the Browser: whether it was listed.
pub(crate) async fn wait_listed(options: &DeviceToolOptions, item_id: &str, signal: &Signal) -> Result<bool, RuntimeError> {
    let deadline = now_ms().saturating_add(options.wait_ms.unwrap_or(20_000) as i64);
    let mut seen = false;
    while !seen && now_ms() < deadline {
        seen = (options.browser_sees)(item_id.to_string(), signal.clone()).await.unwrap_or(false);
        if !seen {
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        signal.check()?;
    }
    if seen {
        options.made.borrow_mut().listed(item_id);
    } else {
        options.made.borrow_mut().written_unlisted(item_id);
    }
    Ok(seen)
}

impl DeviceTool {
    /// Writes a device of `kind` to Kumi's folder in the User Library under its name (`make` makes its file for the name
    /// it's given, and says what it found, or why it can't), and records it for this conversation. Loading it waits for
    /// Live's Browser to list it. Err is the answer when it can't be made or written: nothing is written then. A device that
    /// isn't this conversation's to replace is put in place without replacing any file there.
    async fn save<Making: Future<Output = Result<(Vec<u8>, Findings), String>>>(
        &self,
        kind: DeviceType,
        naming: Naming,
        make: impl FnOnce(&str) -> Making,
        found: &mut Findings,
        signal: &Signal,
    ) -> Result<Result<Saved, ToolResult>, RuntimeError> {
        let folder = Path::new(&self.options.user_library).join("Kumi").join(kind_folder(kind));
        let mut builder = tokio::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o755);
        if let Err(error) = builder.create(&folder).await {
            return Ok(Err(ToolResult::error(unwritable(&folder, &error))));
        }
        let candidates = self.options.made.borrow().candidates(&naming.asked, kind);
        let ours = if candidates.is_empty() {
            None
        } else {
            // Reading the file of a device made here, and hashing it, take as long as it's big: beside the app's thread.
            let place = folder.clone();
            beside(signal, move || still_ours(&place, candidates)).await?.flatten()
        };
        let windows = kumi_types::system::platform() == "win32";
        // Kumi Ears is kept beside the audio effects, and Kumi writes it again whenever it differs: its name is its own.
        let mut reserved: Vec<&str> = naming.reserved.iter().map(String::as_str).collect();
        if kind == DeviceType::AudioEffect {
            reserved.push(EARS_NAME);
        }
        let wanted = naming.wanted.as_str();
        let name = match &ours {
            Some(device) => device.file.clone(),
            None => free_name(&folder, wanted, windows, &reserved),
        };
        if ours.is_none() && name != wanted {
            let stem = wanted.split('.').next().unwrap_or_default();
            let why = if windows && (windows_device_name(stem) || windows_device_name(wanted)) {
                format!("Windows keeps the name {wanted} for a device")
            } else if naming.reserved.iter().any(|kept| kept.eq_ignore_ascii_case(wanted)) {
                format!("it's made from {wanted}, which keeps its name and its file")
            } else if reserved.iter().any(|kept| kept.eq_ignore_ascii_case(wanted)) {
                format!("{wanted} is the name of Kumi's own listening device")
            } else {
                format!("{wanted} is another device in Live's User Library, and a saved Set may use it")
            };
            found.changed.push(format!("name: made as \"{name}\": {why}."));
        }
        let file = folder.join(format!("{name}.amxd"));
        // The device it's made from is never written over (a saved Set uses it by its file), unless it's one this
        // conversation made, which is replaced as any of those is.
        if let Some(never) = naming.never.as_deref().filter(|never| ours.is_none() && same_file(&file, never)) {
            return Ok(Err(ToolResult::error(format!(
                "Kumi didn't write the device: it would have been written over {}, the device it's made from. Give it another name.",
                never.to_string_lossy()
            ))));
        }
        let made = make(&name).await;
        signal.check()?;
        let (bytes, more) = match made {
            Ok(made) => made,
            Err(why) => return Ok(Err(ToolResult::error(why))),
        };
        found.append(more);
        let kept_in = self.options.made.borrow().kept.clone();
        let kept = match (&ours, kept_in) {
            (Some(_), Some(kept_in)) => keep(&file, &name, &kept_in).await,
            _ => None,
        };
        let written = write_device(&folder, &file, &bytes, ours.is_some()).await;
        if let Err(error) = written {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                return Ok(Err(ToolResult::error(format!(
                    "Kumi didn't write the device: a device named {name} appeared in {} while it was being written, and Kumi never writes over one. Make it again.",
                    folder.to_string_lossy()
                ))));
            }
            return Ok(Err(ToolResult::error(unwritable(&folder, &error))));
        }
        // Hashing what was written takes as long as it's big: beside the app's thread, and waited for even after a stop, as
        // the device is written.
        let written = tokio::task::spawn_blocking(move || hashed(&bytes)).await.unwrap_or_default();
        self.options.made.borrow_mut().made(MadeDevice { asked: naming.asked, file: name.clone(), kind, written });
        let item_id = format!("user_library/Kumi/{}/{name}", kind_folder(kind));
        self.options.made.borrow_mut().written_unlisted(&item_id);
        Ok(Ok(Saved { name, file, item_id, ours, kept }))
    }

    /// A device that's already there (`from`): opened and said in the notation when nothing's asked of it; else a new
    /// device made from it with only what's asked changed, saved as a device of its own.
    async fn from_existing(
        &self,
        from: &str,
        input: &JsonObject,
        given: &dyn Fn(&str) -> bool,
        reference: Option<&'static reference::Reference>,
        signal: Signal,
    ) -> Result<ToolResult, RuntimeError> {
        // Looking through the User Library and reading the file (up to 256 MB) take as long as they take: beside the app's
        // thread.
        let (asked_from, library) = (from.to_string(), PathBuf::from(&self.options.user_library));
        let opened =
            beside(&signal, move || existing::find(&asked_from, &library).and_then(|path| existing::open(&path, &library))).await?;
        let original = match opened {
            Some(Ok(original)) => Arc::new(original),
            Some(Err(why)) => return Ok(ToolResult::error(why)),
            None => return Ok(ToolResult::error("Kumi couldn't open the device.".to_string())),
        };
        // A new device is made from it only with a change: a name or an about alone opens it.
        if !["patch", "controls", "remove"].iter().any(|key| given(key)) {
            // Saying what's in it takes as long as it's big: beside the app's thread.
            let opened = Arc::clone(&original);
            let Some(mut said) = beside(&signal, move || existing::describe(&opened)).await? else {
                return Ok(ToolResult::error("Kumi couldn't read the device.".to_string()));
            };
            let mut aside = Vec::new();
            let other: Vec<&str> = ["code", "tests", "objects", "guide"].into_iter().filter(|key| given(key)).collect();
            if !other.is_empty() {
                aside.push(format!(
                    "{}: opening a device answers with the device alone; ask for these in a call of their own.",
                    other.join(", ")
                ));
            }
            let naming: Vec<&str> = ["name", "about"].into_iter().filter(|key| given(key)).collect();
            if !naming.is_empty() {
                aside.push(format!(
                    "{}: a device is made from another only with a change (patch, controls or remove), so this one was opened and nothing was made: give {} with the change.",
                    naming.join(", "),
                    if naming.len() == 1 { "it" } else { "them" }
                ));
            }
            if !aside.is_empty() {
                said["aside"] = json!(aside.join(" "));
            }
            return Ok(ToolResult::text(stringify(&said)));
        }
        let mut found = Findings::default();
        let kind = original.kind;
        if let Some(asked) = input.get("type").and_then(Value::as_str).filter(|asked| *asked != kind.as_str()) {
            found.changed.push(format!("type {asked}: it's made as a {}, as the device it's made from is one.", kind_name(kind)));
        }
        for key in ["code", "tests", "voices", "runs_free", "map"].into_iter().filter(|key| given(key)) {
            found.changed.push(format!(
                "{key}: a device made from another changes through patch, controls and remove, so {key} was left aside (a code box changes when it's given again by its name, with its new code in a {{ … }} block)."
            ));
        }
        // Its name and controls, checked as a new device's are.
        let asked_name = input.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let mut probe = JsonObject::new();
        probe.insert("type".into(), json!(kind.as_str()));
        probe.insert("name".into(), json!(if asked_name.is_empty() { original.name.clone() } else { asked_name.clone() }));
        probe.insert("about".into(), json!("-"));
        probe.insert("patch".into(), json!("in -> out"));
        if let Some(controls) = input.get("controls") {
            probe.insert("controls".into(), controls.clone());
        }
        let (spec, checked) = check_spec(&probe);
        found.changed.extend(checked.changed.into_iter().filter(|said| said.starts_with("name") || said.starts_with("controls")));
        found.wrong.extend(checked.wrong.into_iter().filter(|said| said.starts_with("controls")));
        let remove: Vec<String> = match input.get("remove") {
            Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(str::to_string).collect(),
            Some(Value::String(one)) if !one.trim().is_empty() => vec![one.clone()],
            _ => Vec::new(),
        };
        let patch = match input.get("patch") {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Array(lines)) => lines.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"),
            _ => String::new(),
        };
        let about = input.get("about").and_then(Value::as_str).map(str::trim).filter(|about| !about.is_empty()).map(|about| {
            if about.chars().count() > 400 {
                found.changed.push("about: cut to its first 400 characters.".into());
                about.chars().take(400).collect()
            } else {
                about.to_string()
            }
        });
        let asked = existing::Asked { patch, remove, controls: spec.controls().to_vec(), name: spec.name().to_string(), about };
        signal.check()?;
        let naming = Naming {
            asked: asked_name.clone(),
            wanted: spec.name().to_string(),
            // Unnamed, it's the original's name numbered: never the original's own, wherever that is.
            reserved: if asked_name.is_empty() { vec![original.name.clone()] } else { Vec::new() },
            never: Some(original.path.clone()),
        };
        let made: RefCell<Option<existing::Changed>> = RefCell::new(None);
        let make = |name: &str| {
            let (original, asked) = (Arc::clone(&original), existing::Asked { name: name.to_string(), ..asked.clone() });
            let (made, signal) = (&made, signal.clone());
            async move {
                // Changing the device and making its file again take as long as it's big: beside the app's thread.
                let done = beside(&signal, move || {
                    let changed = existing::change(&original, &asked, reference);
                    existing::device_bytes(&original, &changed.document).map(|bytes| (changed, bytes))
                })
                .await;
                match done {
                    Ok(Some(Ok((changed, bytes)))) => {
                        let said = changed.found.clone();
                        *made.borrow_mut() = Some(changed);
                        Ok((bytes, said))
                    }
                    Ok(Some(Err(why))) => Err(why),
                    // Stopped, which save says, or the work failed.
                    Ok(None) | Err(_) => Err("Kumi couldn't make the device.".to_string()),
                }
            }
        };
        let Saved { name, file, item_id, ours, kept } = match self.save(kind, naming, make, &mut found, &signal).await? {
            Ok(saved) => saved,
            Err(refused) => return Ok(refused),
        };
        let Some(made) = made.into_inner() else { return Ok(ToolResult::error("Kumi couldn't make the device.".to_string())) };
        let mut result = json!({
            "made": name, "type": kind_name(kind), "itemId": item_id, "file": file.to_string_lossy(),
            "from": original.path.to_string_lossy(), "controls": made.controls,
        });
        let fields = result.as_object_mut().unwrap();
        if let Some(device) = &ours {
            let kept = kept.map_or(String::new(), |kept| format!("; the old one is kept at {}", kept.to_string_lossy()));
            fields.insert(
                "replaced".into(),
                json!(format!("the {} this conversation made as {}{kept}", kind_name(device.kind), device.file)),
            );
        }
        // Made again by its own name from this conversation's own draft, the draft is what it replaced.
        let draft = ours.is_some() && same_file(&file, &original.path);
        fields.insert(
            "checks".into(),
            json!(format!(
                "Kumi checked what the change brought (its objects, cords and their order, its controls, its timing) and kept everything else as it was; {}",
                if draft {
                    "the original was this conversation's own draft, which this replaced (see replaced)"
                } else {
                    "the original's file is as it was"
                }
            )),
        );
        for (key, said) in [("wrong", &found.wrong), ("changed", &found.changed), ("notes", &found.notes)] {
            if !said.is_empty() {
                fields.insert(key.into(), json!(said));
            }
        }
        fields.insert("browser".into(), json!(BROWSER_LOADS));
        let load = match kind {
            DeviceType::MidiEffect => "load_device with this itemId on the MIDI track",
            DeviceType::AudioEffect => "load_device with this itemId on the track, then hear it with audition",
            DeviceType::Instrument => "load_device with this itemId on a MIDI track, then hear it with audition",
        };
        let shared = if made.shared.is_empty() {
            String::new()
        } else {
            format!(
                " (the original has these too: {})",
                made.shared.iter().map(|name| format!("\"{name}\"")).collect::<Vec<_>>().join(", ")
            )
        };
        let again =
            format!("call make_device again with from \"{}\" and the name \"{name}\", which replaces this one", file.to_string_lossy());
        let swap = format!(
            "Only when the producer asks for it in place of the original: load it on the original's track, move_device it beside the original, set_device_parameters its controls to the original's values{shared}, then switch_device the original off. Its automation and mappings don't carry over, and delete_device would lose them: say so, and delete the original only if the producer says to."
        );
        let next = if found.wrong.is_empty() {
            format!("{load}. To change it more, {again}. {swap}")
        } else {
            format!("It's made, and it loads as it is, but what's under wrong needs fixing: fix it and {again}. Or {load}. {swap}")
        };
        fields.insert("next".into(), json!(next));
        Ok(ToolResult::text(stringify(&result)))
    }
}

#[async_trait(?Send)]
impl KernelTool for DeviceTool {
    fn reach(&self) -> Reach {
        // Making and opening a device use files on disk; load_device owns every read of Live's Browser.
        Reach::AwayFromDaw
    }
    fn name(&self) -> &str {
        MAKE_DEVICE_TOOL
    }
    fn description(&self) -> &str {
        DATA["description"].as_str().unwrap()
    }
    fn input_schema(&self) -> JsonObject {
        DATA["schema"].as_object().unwrap().clone()
    }
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let guide = (input.get("guide").and_then(Value::as_bool) == Some(true)).then(|| {
            let kind = input.get("type").and_then(Value::as_str).unwrap_or("");
            if matches!(kind, "midi_effect" | "audio_effect" | "instrument") {
                DATA[kind].as_str().unwrap().to_string()
            } else {
                ["midi_effect", "audio_effect", "instrument"].map(|k| DATA[k].as_str().unwrap()).join("\n\n")
            }
        });
        // What's given, as a model leaves a field it doesn't use: null, false, "" or [] is nothing.
        let given = |key: &str| match input.get(key) {
            None | Some(Value::Null | Value::Bool(false)) => false,
            Some(Value::String(text)) => !text.trim().is_empty(),
            Some(Value::Array(items)) => !items.is_empty(),
            Some(_) => true,
        };
        let objects = input.get("objects").filter(|objects| !matches!(objects, Value::Null | Value::Bool(false)));
        let asked: Vec<String> = match objects {
            Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
            Some(Value::String(text)) => vec![text.as_str()],
            _ => Vec::new(),
        }
        .into_iter()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .collect();
        // What Kumi learned of this machine's Max, when the call needs it: learned (once for each Max, a second or more)
        // off the thread the app runs on.
        let reference =
            if needs_reference(&input, &asked) { tokio::task::spawn_blocking(reference::installed).await.ok().flatten() } else { None };
        // A device that's already there: opened, or made into a new one.
        if let Some(from) = input.get("from").and_then(Value::as_str).map(str::trim).filter(|from| !from.is_empty()) {
            return self.from_existing(from, &input, &given, reference, signal).await;
        }
        // Max's objects as this machine's Max describes them, or a guide, with no device given to make beside them.
        if !given("code") && !given("patch") && (objects.is_some() || guide.is_some()) {
            let mut said: Vec<String> = Vec::new();
            if objects.is_some() {
                said.push(pages(&asked, reference));
            }
            said.extend(guide);
            if ["name", "controls", "about", "tests"].iter().any(|key| given(key)) {
                said.push("To make the device as well, give its code or its patch beside these.".to_string());
            }
            return Ok(ToolResult::text(said.join("\n\n")));
        }
        // The device is always made: what doesn't fit is made the nearest way it can be, and said beside it.
        let (mut spec, mut found) = check_spec(&input);
        if reference.is_none() && needs_reference(&input, &asked) {
            found.notes.extend(page::wrong_kumi_max());
        }
        // Code Kumi's check found never stops would hold Max (and Live) up: it's kept in the device without running.
        let mut runaway = false;
        let checked = match &spec {
            DeviceSpec::Patch(_) if reference.is_some() => {
                "Kumi checked every object against this machine's Max, every cord against the ports it meets, and the order its messages go in, and laid it out top to bottom".to_string()
            }
            DeviceSpec::Patch(_) => "Kumi checked the patch (Max wasn't found here, so its objects weren't checked against it) and laid it out top to bottom".to_string(),
            DeviceSpec::MidiEffect(midi) => {
                let verified = check_midi_device_isolated(midi, IsolatedOptions::default()).await;
                found.wrong.extend(verified.problems);
                runaway = verified.runaway;
                let tests = if verified.of > 0 {
                    format!("{} of {} of its tests passed", verified.passed, verified.of)
                } else if given("tests") {
                    "None of its tests ran (changed says why)".into()
                } else {
                    "It has no tests".into()
                };
                // What the spec's checks found counts too: the summary never says nothing's wrong beside a list that does.
                if runaway {
                    "Kumi's check found the code never stops, so the device keeps it without running it; what Kumi found is under wrong"
                        .to_string()
                } else if found.wrong.is_empty() {
                    let ran = if midi.runs_free { "no errors; it runs free" } else { "no errors, no hanging notes" };
                    format!("{tests}, and Kumi's checks found nothing wrong ({ran})")
                } else {
                    format!("{tests}; what Kumi found is under wrong")
                }
            }
            _ => "Kumi checked its outputs, inputs and controls; Max compiles the code when Live loads it".into(),
        };
        if let (true, DeviceSpec::MidiEffect(midi)) = (runaway, &mut spec) {
            found.wrong.push(
                "Kumi kept the code in the device without running it, so the device passes MIDI on as it comes: code that never stops would hold Max, and Live with it, up."
                    .to_string(),
            );
            midi.code = kept_unrun(&midi.code, "Kumi's check found it never stops, which would hold Max up");
            midi.tests.clear();
        }
        signal.check()?;
        // A device this conversation made is replaced; any other keeps its file, and this one is numbered past it.
        let naming = Naming {
            asked: input.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string(),
            wanted: spec.name().to_string(),
            reserved: Vec::new(),
            never: None,
        };
        let make = |name: &str| {
            std::future::ready(match &spec {
                DeviceSpec::MidiEffect(s) => {
                    let mut s = s.clone();
                    s.name = name.to_string();
                    Ok((encode_amxd(DeviceType::MidiEffect, &midi_device_patcher(&s)), Findings::default()))
                }
                DeviceSpec::AudioEffect(s) => {
                    let mut s = GenSpec::from(s);
                    s.name = name.to_string();
                    Ok((encode_amxd(DeviceType::AudioEffect, &audio_effect_patcher(&s)), Findings::default()))
                }
                DeviceSpec::Instrument(s) => {
                    let mut s = GenSpec::from(s);
                    s.name = name.to_string();
                    Ok((encode_amxd(DeviceType::Instrument, &instrument_patcher(&s)), Findings::default()))
                }
                DeviceSpec::Patch(s) => {
                    let mut s = s.clone();
                    s.name = name.to_string();
                    let (patcher, patched) = patched_device(&s, reference);
                    Ok((encode_amxd(s.kind, &patcher), patched))
                }
            })
        };
        let Saved { name, file, item_id, ours, kept } = match self.save(spec.kind(), naming, make, &mut found, &signal).await? {
            Ok(saved) => saved,
            Err(refused) => return Ok(refused),
        };
        // A silent render means the code didn't compile, or a patch passes no sound.
        let silent =
            if matches!(spec, DeviceSpec::Patch(_)) { "the patch passes no sound: fix it" } else { "the code didn't compile: fix it" };
        let (next,extra)=match spec.kind() {
            DeviceType::MidiEffect => ("load_device with this itemId on the MIDI track; Live puts a MIDI effect before the instrument.".to_string(),vec![]),
            DeviceType::AudioEffect => (format!("load_device with this itemId on the track, then hear it with audition while audio plays through that track (a silent render means {silent} and make it again)."),vec![&*MIX,&*OUTPUT]),
            DeviceType::Instrument => (format!("load_device with this itemId on a MIDI track, write a short clip, and hear it with audition (a silent render means {silent} and make it again)."),vec![&*OUTPUT]),
        };
        let controls: Vec<_> = spec.controls().iter().chain(extra).map(describe_control).collect();
        let mut result =
            json!({"made":name,"type":kind_name(spec.kind()),"itemId":item_id,"file":file.to_string_lossy(),"controls":controls});
        let fields = result.as_object_mut().unwrap();
        if let Some(device) = &ours {
            let kept = kept.map_or(String::new(), |kept| format!("; the old one is kept at {}", kept.to_string_lossy()));
            fields.insert(
                "replaced".into(),
                json!(format!("the {} this conversation made as {}{kept}", kind_name(device.kind), device.file)),
            );
        }
        if let DeviceSpec::Instrument(s) = &spec {
            fields.insert("voices".into(), json!(s.voices));
        }
        fields.insert("checks".into(), json!(checked));
        for (key, said) in [("wrong", &found.wrong), ("changed", &found.changed), ("notes", &found.notes)] {
            if !said.is_empty() {
                fields.insert(key.into(), json!(said));
            }
        }
        fields.insert("browser".into(), json!(BROWSER_LOADS));
        let again = format!("call make_device again in this conversation with the name \"{name}\", which replaces this device");
        let next = if found.wrong.is_empty() {
            format!("{next} To change it, {again}.")
        } else {
            format!("It's made, and it loads as it is, but what's under wrong needs fixing: fix it and {again}. Or {next}")
        };
        fields.insert("next".into(), json!(next));
        // What was asked beside the device: Max's objects, and the guide.
        if !asked.is_empty() {
            fields.insert("objects".into(), json!(pages(&asked, reference)));
        }
        if let Some(guide) = guide {
            fields.insert("guide".into(), json!(guide));
        }
        Ok(ToolResult::text(stringify(&result)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_another_device_has_is_numbered_and_a_name_windows_takes_for_a_device_too() {
        let folder = tempfile::tempdir().unwrap();
        assert_eq!(free_name(folder.path(), "Bass", false, &[]), "Bass");
        std::fs::write(folder.path().join("Bass.amxd"), "").unwrap();
        assert_eq!(free_name(folder.path(), "Bass", false, &[]), "Bass 2", "a file's name is taken everywhere");
        assert_eq!(
            (free_name(folder.path(), "Aux", false, &[]), free_name(folder.path(), "Aux", true, &[])),
            ("Aux".into(), "Aux 2".into())
        );
        assert_eq!(free_name(folder.path(), "Con.Dist", true, &[]), "Con 2.Dist", "the part before the dot is what Windows reads");
        assert_eq!(free_name(folder.path(), "LPT1 ", true, &[]), "LPT1  2");
        assert_eq!(free_name(folder.path(), "Console", true, &[]), "Console");
        assert_eq!(
            (free_name(folder.path(), "Kumi Ears", false, &["Kumi Ears"]), free_name(folder.path(), "kumi ears", false, &["Kumi Ears"])),
            ("Kumi Ears 2".into(), "kumi ears 2".into()),
            "a reserved name is taken, in any case, before its file is there"
        );
    }

    #[test]
    fn only_a_device_this_conversation_made_and_kumi_wrote_is_ours_to_replace() {
        let folder = tempfile::tempdir().unwrap();
        let mut made = Made::default();
        let ours = |made: &Made, asked: &str, kind| still_ours(folder.path(), made.candidates(asked, kind)).map(|device| device.file);
        made.conversation("first");
        std::fs::write(folder.path().join("Echo 2.amxd"), b"written").unwrap();
        made.made(MadeDevice { asked: "Echo".into(), file: "Echo 2".into(), kind: DeviceType::MidiEffect, written: hashed(b"written") });
        assert_eq!(ours(&made, "Echo", DeviceType::MidiEffect), Some("Echo 2".into()), "by the name asked for");
        assert_eq!(ours(&made, "Echo 2", DeviceType::MidiEffect), Some("Echo 2".into()), "by the name it was made as");
        assert!(ours(&made, "Delay", DeviceType::MidiEffect).is_none());
        assert!(ours(&made, "Echo", DeviceType::AudioEffect).is_none(), "an audio effect doesn't replace a MIDI effect");
        made.made(MadeDevice { asked: "".into(), file: "Echo 3".into(), kind: DeviceType::MidiEffect, written: hashed(b"written") });
        std::fs::write(folder.path().join("Echo 3.amxd"), b"written").unwrap();
        assert!(ours(&made, "", DeviceType::MidiEffect).is_none(), "two devices made without a name are two");
        std::fs::write(folder.path().join("Echo 2.amxd"), b"saved in Max").unwrap();
        assert!(ours(&made, "Echo", DeviceType::MidiEffect).is_none(), "a file changed since is someone's own");
        std::fs::write(folder.path().join("Echo 2.amxd"), b"written").unwrap();
        made.conversation("second");
        assert!(ours(&made, "Echo", DeviceType::MidiEffect).is_none(), "another conversation's");
    }

    #[test]
    fn only_a_lookup_or_a_patch_needs_what_kumi_learned_of_max() {
        let input = |value: serde_json::Value| value.as_object().unwrap().clone();
        assert!(!needs_reference(&input(json!({ "objects": [] })), &[]), "none to look up: nothing is learned");
        assert!(pages(&[], None).starts_with("objects: give one or more of Max's objects"));
        assert!(needs_reference(&input(json!({ "objects": ["metro"] })), &["metro".to_string()]));
        assert!(needs_reference(&input(json!({ "patch": "in -> out" })), &[]));
        assert!(!needs_reference(&input(json!({ "code": "function midi(event) { pass(event); }", "patch": " " })), &[]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_new_device_never_replaces_a_file_already_there() {
        let folder = tempfile::tempdir().unwrap();
        let (temporary, file) = (folder.path().join(".new.amxd"), folder.path().join("Echo.amxd"));
        std::fs::write(&temporary, b"new").unwrap();
        std::fs::write(&file, b"someone's own").unwrap();
        assert_eq!(put_new(&temporary, &file).await.unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&file).unwrap(), b"someone's own");
        let free = folder.path().join("Echo 2.amxd");
        put_new(&temporary, &free).await.unwrap();
        assert_eq!(std::fs::read(&free).unwrap(), b"new");
    }

    #[test]
    fn a_full_disk_and_an_unwritable_folder_say_what_to_do() {
        let folder = Path::new("/Music/User Library/Kumi");
        let full = unwritable(folder, &std::io::Error::from(std::io::ErrorKind::StorageFull));
        assert!(full.contains("is full. Free some space, then call make_device again."), "{full}");
        let denied = unwritable(folder, &std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert!(denied.contains("Make the folder writable, then call make_device again."), "{denied}");
    }
}
