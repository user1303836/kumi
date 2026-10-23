//! `bounce` and `export_tracks`: Logic's mix and its tracks written to audio files, as actions (no change, no row, no
//! step of Logic's undo), methods of the bridge as `project_save` is. Both go to a folder Kumi can read and never over
//! a file: the bounce's file must not be there yet, and the export's folder must hold none of the files Logic will
//! write. What Logic wrote is found on disk and read back from its header ([`crate::disk`]).
//!
//! The bounce goes through File > Bounce > Project or Section… (Logic's own Bounce dialog: its destination list with
//! Uncompressed alone checked, the file type and bit depth pop-ups, Normalize off, Add to Project and Add to Apple
//! Music Library off, the Start and End positions' segments stepped), then the save panel through its service as
//! `project_save`'s save as goes, and the file is waited for on disk and its header read. With Add to Project off a
//! bounce leaves Logic's undo as it found it. The export goes through File > Export > All Tracks as Audio Files…, an
//! open panel that takes a folder (its options left as the producer set them): the folder's new files are the answer.

use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::nudge;
use super::panel;
use super::{front, timeline, Ctx, Done};
use crate::ax::{Node, Root};
use crate::queue::Answer;
use crate::refs::{bad, failed};
use crate::rows::Position;

/// The longest bounce Kumi asks for, in beats (an hour at 240 BPM).
pub const MOST_BEATS: f64 = 14400.0;

/// A file format the bounce writes: Logic's PCM destination, its file format and resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub name: &'static str,
    /// The Bounce dialog's File Type pop-up's item.
    pub file_type: &'static str,
    /// Its Bit Depth pop-up's item.
    pub bit_depth: &'static str,
    pub extensions: &'static [&'static str],
    pub bits: u16,
    pub float: bool,
}

pub const FORMATS: [Format; 4] = [
    Format { name: "wav24", file_type: "WAVE", bit_depth: "24-bit", extensions: &["wav", "wave"], bits: 24, float: false },
    Format { name: "wav16", file_type: "WAVE", bit_depth: "16-bit", extensions: &["wav", "wave"], bits: 16, float: false },
    Format { name: "wav32f", file_type: "WAVE", bit_depth: "32-bit float", extensions: &["wav", "wave"], bits: 32, float: true },
    Format { name: "aiff24", file_type: "AIFF", bit_depth: "24-bit", extensions: &["aif", "aiff"], bits: 24, float: false },
];

/// A bounce asked for: the range in beats, the file and its format.
#[derive(Debug, Clone, PartialEq)]
pub struct Bounce {
    pub start: f64,
    pub end: f64,
    pub path: PathBuf,
    pub format: Format,
}

/// A whole path with no `..`, a name Logic's panels take (no colon, slash or control character), whose folder Kumi
/// can see.
fn plain(text: &str, what: &str) -> Done<PathBuf> {
    let path = PathBuf::from(text);
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(bad(format!("{what} must be a whole path, with no \"..\" in it")));
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if name.is_empty() || name.contains(':') || name.chars().any(char::is_control) || name.chars().count() > 200 {
        return Err(bad(format!("{what} must end in a name Logic Pro can write: no colon in it")));
    }
    Ok(path)
}

fn beats(params: &Value, key: &str) -> Done<f64> {
    params
        .get(key)
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
        .ok_or_else(|| bad(format!("{key} must be a number from 0: beats from the project's start")))
}

/// `bounce {startBeats, endBeats, path, format?}`, checked: the file isn't there yet, its folder is.
pub fn bounce_params(params: &Value) -> Done<Bounce> {
    let (start, end) = (beats(params, "startBeats")?, beats(params, "endBeats")?);
    if end <= start {
        return Err(bad("endBeats must be after startBeats"));
    }
    if end - start > MOST_BEATS {
        return Err(bad(format!("Kumi bounces at most {MOST_BEATS} beats at once")));
    }
    let format = match params.get("format") {
        None | Some(Value::Null) => FORMATS[0],
        Some(Value::String(name)) => FORMATS
            .iter()
            .copied()
            .find(|f| f.name == name)
            .ok_or_else(|| bad(format!("format must be one of {}", FORMATS.iter().map(|f| f.name).collect::<Vec<_>>().join(", "))))?,
        Some(_) => return Err(bad("format must be a string, such as \"wav24\"")),
    };
    let text = params
        .get("path")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| bad("path: a whole path to the audio file to write (a .wav that isn't there yet, in a folder that is)"))?;
    let path = plain(text, "path")?;
    let extension = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !format.extensions.contains(&extension.as_str()) {
        return Err(bad(format!("path must end in .{} for {}", format.extensions[0], format.name)));
    }
    if crate::disk::exists(&path) {
        return Err(bad(format!("{} is there already: Kumi never writes over a file", path.display())));
    }
    let folder = path.parent().unwrap_or(Path::new("/"));
    if !crate::disk::exists(folder) {
        return Err(bad(format!("the folder {} isn't there (or Kumi can't see it); Kumi makes no folders", folder.display())));
    }
    Ok(Bounce { start, end, path, format })
}

/// `export_tracks {path}`, checked: the folder the tracks' files go into, which must be there.
pub fn export_params(params: &Value) -> Done<PathBuf> {
    let text = params
        .get("path")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| bad("path: a whole path to the folder the tracks' audio files go into (it must be there)"))?;
    let folder = plain(text, "path")?;
    if !crate::disk::exists(&folder) || crate::disk::list(&folder).is_none() {
        return Err(bad(format!("the folder {} isn't there (or Kumi can't read it); Kumi makes no folders", folder.display())));
    }
    Ok(folder)
}

/// How long Logic gets to show the Bounce dialog, and a bounce or an export to finish writing.
const DIALOG_WAIT: Duration = Duration::from_secs(4);
const WRITE_WAIT: Duration = Duration::from_secs(50);
const POLL: Duration = Duration::from_millis(250);
const SETTLE_MS: u64 = 300;
/// How long a pop-up's menu gets to come up before it is looked for again.
const MENU_WAIT: Duration = Duration::from_millis(400);
/// The File Type, Bit Depth, Normalize and Mode pop-ups' items, by which each pop-up is told from the others.
const FILE_TYPES: [&str; 3] = ["WAVE", "AIFF", "CAF"];
const BIT_DEPTHS: [&str; 3] = ["16-bit", "24-bit", "32-bit float"];
const NORMALIZE: [&str; 3] = ["Off", "Overload Protection Only", "On"];
/// The destination Kumi bounces to, and the ones it turns off.
const UNCOMPRESSED: &str = "Uncompressed";

/// The modal window Logic shows whose title starts with `prefix` (the Bounce dialog), when it is up.
fn modal(ctx: &mut Ctx, prefix: &str) -> Done<Option<String>> {
    let reply = nudge::read(ctx.ax.windows())?;
    let (windows, _) = crate::logic::parse_windows(&reply);
    Ok(windows.into_iter().find(|w| w.dialog() && w.title.starts_with(prefix)).map(|w| w.title))
}

/// Whether Logic shows any modal window or sheet (a dialog, a panel, a bounce's progress), or is too busy writing to
/// answer at all (its main thread held while an export writes: the helper's read times out).
fn busy_now(ctx: &mut Ctx) -> bool {
    match ctx.ax.windows() {
        Ok(reply) => {
            let (windows, _) = crate::logic::parse_windows(&reply);
            windows.iter().any(|w| w.modal || w.sheet() || w.subrole == "AXDialog" && w.main)
        }
        Err(_) => true,
    }
}

/// The dialog's tree.
fn dialog_tree(ctx: &mut Ctx, root: &Root) -> Done<Node> {
    nudge::read(ctx.ax.tree(root, "", 6, 80, Some(&["role", "subrole", "title", "desc", "value", "valueDesc", "enabled", "selected"])))
}

/// The menu a press of a pop-up opened, read through the pop-up: a menu that isn't up yet (one pop-up's menu just
/// after another's closed) is looked for again after a moment, then pressed for once more.
fn open_menu(ctx: &mut Ctx, root: &Root, path: &str) -> Done<Node> {
    let mut menu = Node::default();
    for press in 0..2 {
        nudge::wrote(ctx.ax.action(root, path, "AXPress", SETTLE_MS))?;
        for look in 0..2 {
            if look > 0 {
                super::pause(MENU_WAIT);
            }
            menu = nudge::wrote(ctx.ax.tree_in_popup(root, path, 3, 60, Some(&["role", "title"])))?;
            if menu.find(&|n| n.role == "AXMenu").is_some() {
                return Ok(menu);
            }
        }
        if press == 0 {
            super::pause(MENU_WAIT);
        }
    }
    Ok(menu)
}

/// Chooses an item of a standard pop-up (its menu read through the pop-up), and reads its value back.
fn choose(ctx: &mut Ctx, root: &Root, path: &str, item: &str) -> Done<()> {
    let menu = open_menu(ctx, root, path)?;
    let Some(found) = menu.find(&|n| n.role == "AXMenuItem" && n.title == item).map(|n| n.path.clone()) else {
        if let Some(open) = menu.find(&|n| n.role == "AXMenu").map(|n| n.path.clone()) {
            let _ = ctx.ax.action_in_popup(root, &open, "AXCancel", 100);
        }
        return Err(failed(format!("the Bounce dialog's pop-up offers no {item:?}")));
    };
    nudge::wrote(ctx.ax.action_in_popup(root, &found, "AXPress", SETTLE_MS))?;
    let attrs = nudge::wrote(ctx.ax.get(root, path, &["AXValue"]))?;
    if attrs.get("AXValue").and_then(Value::as_str) != Some(item) {
        return Err(super::uncertain(format!("the Bounce dialog's pop-up shows {:?} after Kumi chose {item:?}", attrs.get("AXValue"))));
    }
    Ok(())
}

/// A pop-up of the dialog told by its value being one of `items`.
fn popup_among<'a>(dialog: &'a Node, items: &[&str]) -> Option<&'a Node> {
    dialog.find(&|n| n.role == "AXPopUpButton" && items.contains(&n.value_text().as_str()))
}

/// A position group's segments (bar, beat, division, tick), from its description ("\t1\t1\t1\t1").
fn position_of(group: &Node) -> Option<[u32; 4]> {
    let numbers = crate::rows::numbers(&group.desc);
    (numbers.len() >= 4).then(|| [numbers[0] as u32, numbers[1] as u32, numbers[2] as u32, numbers[3] as u32])
}

/// Steps one segment of a position group to a value: ten at a time by increments, then one at a time by sets (a set
/// moves a segment one step toward the value asked, whatever it is), reading the group's description after each.
fn step_segment(ctx: &mut Ctx, root: &Root, group_path: &str, segment: usize, target: u32) -> Done<u32> {
    let read = |ctx: &mut Ctx| -> Done<(Node, [u32; 4])> {
        let group = nudge::read(ctx.ax.tree(root, group_path, 1, 8, Some(&["role", "desc", "value"])))?;
        let at = position_of(&group).ok_or_else(|| failed("the Bounce dialog's position shows no segments"))?;
        Ok((group, at))
    };
    let (mut group, mut at) = read(ctx)?;
    for _ in 0..nudge::MOST_STEPS {
        let now = at[segment];
        if now == target {
            break;
        }
        let slider =
            group.children.iter().filter(|c| c.role == "AXSlider").nth(segment).ok_or_else(|| failed("a position segment is missing"))?;
        let path = slider.path.clone();
        let away = i64::from(target) - i64::from(now);
        if away.abs() >= nudge::BIG_STEP {
            let action = if away > 0 { "AXIncrement" } else { "AXDecrement" };
            nudge::wrote(ctx.ax.action(root, &path, action, 60))?;
        } else {
            let raw = slider.number().unwrap_or(0.0);
            nudge::wrote(ctx.ax.set(root, &path, "AXValue", json!(if away > 0 { raw + 1.0 } else { (raw - 1.0).max(0.0) }), 60))?;
        }
        let (next_group, next) = read(ctx)?;
        if next[segment] == now {
            break;
        }
        group = next_group;
        at = next;
    }
    Ok(at[segment])
}

/// Sets a position group (Start or End) to a position, the bar first.
fn set_position(ctx: &mut Ctx, root: &Root, group_path: &str, to: Position) -> Done<[u32; 4]> {
    let wanted = [to.bar, to.beat, to.division, to.tick];
    let mut got = [0u32; 4];
    for (segment, target) in wanted.iter().enumerate() {
        got[segment] = step_segment(ctx, root, group_path, segment, *target)?;
    }
    let group = nudge::read(ctx.ax.tree(root, group_path, 1, 8, Some(&["role", "desc"])))?;
    let at = position_of(&group).ok_or_else(|| failed("the Bounce dialog's position shows no segments"))?;
    if at != wanted {
        return Err(super::uncertain(format!("the Bounce dialog shows the position {at:?} after Kumi asked for {wanted:?}")));
    }
    Ok(at)
}

/// What the Bounce dialog would do besides writing the file, which a bounce of Kumi's leaves out: Add to Project puts
/// the file in the project's audio, a step of Logic's undo ("Bounce Project") on top of any Kumi's restore would take
/// back with Logic's own undo; Add to Apple Music Library puts it in the Music app.
const LEFT_OUT: [&str; 2] = ["Add to Project", "Add to Apple Music Library"];

/// Unchecks the dialog's Add to Project and Add to Apple Music Library, read back.
fn leave_out(ctx: &mut Ctx, root: &Root) -> Done<()> {
    let checked = |ctx: &mut Ctx| -> Done<Vec<(String, String)>> {
        let tree = dialog_tree(ctx, root)?;
        let strings = &ctx.project.strings;
        let mut on = vec![];
        for label in LEFT_OUT {
            if let Some(check) = tree.find(&|n| n.role == "AXCheckBox" && (strings.is(&n.title, label) || strings.is(&n.desc, label))) {
                if check.is_on() {
                    on.push((label.to_string(), check.path.clone()));
                }
            }
        }
        Ok(on)
    };
    for (_, path) in checked(ctx)? {
        nudge::wrote(ctx.ax.action(root, &path, "AXPress", 150))?;
    }
    if let Some((label, _)) = checked(ctx)?.first() {
        return Err(super::uncertain(format!("the Bounce dialog keeps {label} checked after Kumi unchecked it")));
    }
    Ok(())
}

/// Fills the Bounce dialog: Uncompressed alone, its file type and bit depth, Normalize off, Add to Project and Add to
/// Apple Music Library off, the range.
fn fill(ctx: &mut Ctx, root: &Root, ask: &Bounce, beats_per_bar: u32) -> Done<()> {
    let tree = dialog_tree(ctx, root)?;
    // The destinations: Uncompressed checked and its row selected (the pop-ups show the selected row's format), the
    // others unchecked.
    let rows: Vec<Node> =
        tree.find(&|n| n.role == "AXTable").map(|t| t.children.iter().filter(|c| c.role == "AXRow").cloned().collect()).unwrap_or_default();
    let mut uncompressed_row = None;
    for row in &rows {
        let Some(check) = row.find(&|n| n.role == "AXCheckBox") else { continue };
        let wanted = check.desc == UNCOMPRESSED;
        if wanted {
            uncompressed_row = Some(row.path.clone());
        }
        if check.is_on() != wanted {
            nudge::wrote(ctx.ax.action(root, &check.path, "AXPress", 150))?;
        }
    }
    let row = uncompressed_row.ok_or_else(|| failed("the Bounce dialog lists no Uncompressed destination"))?;
    let _ = ctx.ax.set(root, &row, "AXSelected", json!(true), 200);
    let tree = dialog_tree(ctx, root)?;
    let file_type =
        popup_among(&tree, &FILE_TYPES).map(|n| n.path.clone()).ok_or_else(|| failed("the Bounce dialog shows no File Type"))?;
    choose(ctx, root, &file_type, ask.format.file_type)?;
    let tree = dialog_tree(ctx, root)?;
    let depth = popup_among(&tree, &BIT_DEPTHS).map(|n| n.path.clone()).ok_or_else(|| failed("the Bounce dialog shows no Bit Depth"))?;
    choose(ctx, root, &depth, ask.format.bit_depth)?;
    let tree = dialog_tree(ctx, root)?;
    let normalize = popup_among(&tree, &NORMALIZE).map(|n| n.path.clone()).ok_or_else(|| failed("the Bounce dialog shows no Normalize"))?;
    choose(ctx, root, &normalize, "Off")?;
    leave_out(ctx, root)?;
    // The range: Start before End when it moves earlier, End first when it moves later.
    let tree = dialog_tree(ctx, root)?;
    let groups: Vec<String> =
        tree.children.iter().filter(|c| c.role == "AXGroup" && position_of(c).is_some()).map(|c| c.path.clone()).collect();
    let [start_group, end_group] = groups.as_slice() else { return Err(failed("the Bounce dialog shows no Start and End")) };
    let (start, end) = (Position::from_beats(ask.start, beats_per_bar), Position::from_beats(ask.end, beats_per_bar));
    set_position(ctx, root, end_group, end)?;
    set_position(ctx, root, start_group, start)?;
    let tree = dialog_tree(ctx, root)?;
    let end_now = tree.at(end_group).and_then(position_of);
    if end_now != Some([end.bar, end.beat, end.division, end.tick]) {
        set_position(ctx, root, end_group, end)?;
    }
    Ok(())
}

/// `bounce {epoch, startBeats, endBeats, path, format?}`: the mix of the range written to `path`.
pub fn bounce(ctx: &mut Ctx, params: &Value) -> Answer {
    let ask = bounce_params(params)?;
    let epoch = ctx.project.epoch.to_string();
    let (_, transport) = timeline::control_bar(ctx)?;
    let (beats_per_bar, tempo) = (transport.beats_per_bar(), transport.tempo);
    let folder = ask.path.parent().unwrap_or(Path::new("/")).to_path_buf();
    let name = ask.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    front::ensure(ctx.ax)?;
    super::structure::file_menu(ctx, &["Bounce", "Project or Section…"])?;
    let until = Instant::now() + DIALOG_WAIT;
    let title = loop {
        if let Some(title) = modal(ctx, "Bounce")? {
            break title;
        }
        if Instant::now() >= until {
            return Err(failed("Logic Pro showed no Bounce dialog"));
        }
        super::pause(POLL);
    };
    let root = Root::Title(title.clone());
    let filled = fill(ctx, &root, &ask, beats_per_bar);
    let ok = filled.and_then(|_| {
        let tree = dialog_tree(ctx, &root)?;
        let ok = tree
            .children
            .iter()
            .find(|c| c.role == "AXButton" && matches!(c.title.as_str(), "OK" | "Bounce"))
            .map(|c| c.path.clone())
            .ok_or_else(|| failed("the Bounce dialog shows no OK button"))?;
        nudge::wrote(ctx.ax.action(&root, &ok, "AXPress", SETTLE_MS))?;
        Ok(())
    });
    if let Err(fault) = ok {
        cancel_dialog(ctx, &root);
        return Err(fault);
    }
    let the_panel = panel::find(ctx.ax)?;
    let driven = (|| {
        panel::go_to(ctx.ax, &the_panel, &folder.to_string_lossy())?;
        let wanted = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let landed = panel::location(ctx.ax, &the_panel)?;
        if landed != wanted {
            return Err(bad(format!(
                "the folder {} isn't there (Logic Pro's panel stands in {landed:?}); Kumi makes no folders",
                folder.display()
            )));
        }
        panel::set_name(ctx.ax, &the_panel, &name)?;
        panel::press(ctx.ax, &the_panel, "OKButton")
    })();
    if let Err(fault) = driven {
        panel::cancel(ctx.ax, &the_panel);
        return Err(fault);
    }
    let sound = wait_written(ctx, &ask.path)?;
    let expected = (ask.end - ask.start) * 60.0 / tempo.max(1.0);
    let seconds = sound.seconds();
    if (seconds - expected).abs() > 0.5 + expected * 0.05 {
        return Err(super::uncertain(format!(
            "Logic Pro wrote {} but it lasts {seconds} s, where the range is {:.3} s at {tempo} BPM",
            ask.path.display(),
            expected
        )));
    }
    Ok(json!({
        "epoch": epoch,
        "bounced": ask.path.to_string_lossy(),
        "format": ask.format.name,
        "bytes": sound.bytes,
        "seconds": seconds,
        "rate": sound.rate,
        "channels": sound.channels,
        "bits": sound.bits,
        "startBeats": ask.start,
        "endBeats": ask.end,
    }))
}

/// Cancels the Bounce dialog left up after something went wrong, so Logic takes input again.
fn cancel_dialog(ctx: &mut Ctx, root: &Root) {
    if let Ok(tree) = dialog_tree(ctx, root) {
        if let Some(cancel) = tree.children.iter().find(|c| c.role == "AXButton" && c.title == "Cancel") {
            let _ = ctx.ax.action(root, &cancel.path, "AXPress", SETTLE_MS);
        }
    }
}

/// Waits for Logic to finish writing a file: its modal windows gone (the panel, the progress) and the file there with
/// its header read, the same size twice in a row.
fn wait_written(ctx: &mut Ctx, path: &Path) -> Done<crate::disk::Sound> {
    let until = Instant::now() + WRITE_WAIT;
    let mut last: Option<u64> = None;
    loop {
        super::pause(POLL);
        let busy = busy_now(ctx);
        if let (false, Some(sound)) = (busy, crate::disk::sound(path)) {
            if last == Some(sound.bytes) {
                return Ok(sound);
            }
            last = Some(sound.bytes);
        }
        if Instant::now() >= until {
            return Err(super::uncertain(format!("Logic Pro didn't finish writing {} in {} s", path.display(), WRITE_WAIT.as_secs())));
        }
    }
}

/// `export_tracks {epoch, path}`: every track written as an audio file into the folder `path`.
pub fn export_tracks(ctx: &mut Ctx, params: &Value) -> Answer {
    let folder = export_params(params)?;
    let epoch = ctx.project.epoch.to_string();
    let before = crate::disk::list(&folder).unwrap_or_default();
    front::ensure(ctx.ax)?;
    super::structure::file_menu(ctx, &["Export", "All Tracks as Audio Files…"])?;
    let the_panel = panel::find(ctx.ax)?;
    let driven = (|| {
        panel::go_to(ctx.ax, &the_panel, &folder.to_string_lossy())?;
        let wanted = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let landed = panel::location(ctx.ax, &the_panel)?;
        if landed != wanted {
            return Err(bad(format!(
                "the folder {} isn't there (Logic Pro's panel stands in {landed:?}); Kumi makes no folders",
                folder.display()
            )));
        }
        panel::press(ctx.ax, &the_panel, "OKButton")
    })();
    if let Err(fault) = driven {
        panel::cancel(ctx.ax, &the_panel);
        return Err(fault);
    }
    // The export writes one file per track: done when Logic shows no modal and the folder holds the same files twice.
    let until = Instant::now() + WRITE_WAIT;
    let mut last: Option<Vec<(String, u64)>> = None;
    let files = loop {
        super::pause(POLL);
        let busy = busy_now(ctx);
        let now = crate::disk::list(&folder).unwrap_or_default();
        let new: Vec<(String, u64)> = now.into_iter().filter(|file| !before.iter().any(|(name, _)| *name == file.0)).collect();
        if !busy && !new.is_empty() {
            if last.as_ref() == Some(&new) {
                break new;
            }
            last = Some(new);
        }
        if Instant::now() >= until {
            return Err(super::uncertain(format!(
                "Logic Pro didn't finish exporting into {} in {} s",
                folder.display(),
                WRITE_WAIT.as_secs()
            )));
        }
    };
    let listed: Vec<Value> = files
        .iter()
        .map(|(name, bytes)| {
            let sound = crate::disk::sound(&folder.join(name));
            json!({"name": name, "bytes": bytes, "seconds": sound.as_ref().map(crate::disk::Sound::seconds)})
        })
        .collect();
    Ok(json!({"epoch": epoch, "folder": folder.to_string_lossy(), "files": listed}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_bounce_asks_for_a_range_and_a_file_that_isnt_there_yet_in_a_folder_that_is() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Mix.wav");
        let ask = |extra: Value| {
            let mut params = json!({"startBeats": 0, "endBeats": 16, "path": path.to_string_lossy()});
            for (key, value) in extra.as_object().unwrap() {
                params[key] = value.clone();
            }
            bounce_params(&params)
        };
        let bounce = ask(json!({})).unwrap();
        assert_eq!((bounce.start, bounce.end, bounce.format.name, bounce.format.bit_depth), (0.0, 16.0, "wav24", "24-bit"));
        assert_eq!(ask(json!({"format": "wav16"})).unwrap().format.bits, 16);
        assert!(ask(json!({"format": "mp3"})).unwrap_err().1.contains("wav24"));
        assert!(ask(json!({"endBeats": 0})).is_err());
        assert!(ask(json!({"startBeats": -1})).is_err());
        assert!(ask(json!({"path": "Mix.wav"})).unwrap_err().1.contains("whole path"));
        assert!(ask(json!({"path": dir.path().join("Mix.aif").to_string_lossy()})).unwrap_err().1.contains(".wav"));
        assert!(ask(json!({"format": "aiff24", "path": dir.path().join("Mix.aif").to_string_lossy()})).is_ok());
        assert!(ask(json!({"path": dir.path().join("none/Mix.wav").to_string_lossy()})).unwrap_err().1.contains("isn't there"));
        std::fs::write(&path, b"x").unwrap();
        assert!(ask(json!({})).unwrap_err().1.contains("never writes over"));
        assert_eq!(export_params(&json!({"path": dir.path().to_string_lossy()})).unwrap(), dir.path());
        assert!(export_params(&json!({"path": dir.path().join("none").to_string_lossy()})).is_err());
        assert!(export_params(&json!({})).is_err());
    }
}
