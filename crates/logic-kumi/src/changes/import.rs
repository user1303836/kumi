//! `midi-import`: notes in bulk, as one step. The Event List adds a few notes a second, so the bridge writes a
//! Standard MIDI File of them ([`super::smf`]) under a folder it can read and Logic imports it: the track selected,
//! the playhead at the position, File > Import > MIDI File…, the open panel taken to the file through the panel
//! service and its Import pressed, Logic's question about the file's tempo answered No, and the new region read back
//! from the Tracks area (with its notes counted in the Event List). Logic's history names the step "Import"; a restore
//! is Logic's own undo while that step is on top. The file goes to `/tmp/kumi-logic`, never under `~/Music`, which
//! macOS keeps from a process without the Files and Folders permission; its name is a hash of its bytes, so a replay
//! of a recorded scene writes the same path.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use super::{
    changed, front, panel, quoted, ref_text, structure, timeline, tracks, Applied, Ctx, Done, Epoch, Preview, Restore, Restored, Watch,
};
use crate::ax::Node;
use crate::refs::{bad, failed};
use crate::rows::Position;

/// Where the files go.
pub const FOLDER: &str = "/tmp/kumi-logic";
/// The most notes one import takes.
const MOST_NOTES: usize = 10000;
/// How long the panel may take to read the file and enable Open, when its first press didn't take.
const OPEN_WAIT: Duration = Duration::from_secs(3);
/// How often Logic's windows are looked at for a question after the import (it comes at once when it comes), and
/// the pause between looks: a fixed count, so a replay asks exactly what the recording did.
const QUESTION_POLLS: usize = 8;
const QUESTION_POLL: Duration = Duration::from_millis(250);
const AFTER_IMPORT: Duration = Duration::from_millis(1200);

/// What a request asks: the notes as the file takes them, and where.
struct Plan {
    position: f64,
    notes: Vec<super::smf::Note>,
}

fn plan(ctx: &mut Ctx, input: &Value, epoch: Epoch) -> Done<(Plan, tracks::Found)> {
    let found = tracks::find(ctx, input, "ref", epoch)?;
    let position = input
        .get("positionBeats")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
        .ok_or_else(|| bad("positionBeats must be a number from 0: beats from the project's start, where the region goes"))?;
    let list = input.get("notes").and_then(Value::as_array).ok_or_else(|| bad("notes must be a list"))?;
    if list.is_empty() || list.len() > MOST_NOTES {
        return Err(bad(format!("notes must hold from 1 to {MOST_NOTES} notes")));
    }
    let whole = |spec: &Value, key: &str, low: u64, high: u64, fallback: Option<u64>| -> Done<u8> {
        match spec.get(key) {
            None | Some(Value::Null) => fallback.map(|n| n as u8).ok_or_else(|| bad(format!("every note needs a {key} ({low} to {high})"))),
            Some(value) => value
                .as_u64()
                .filter(|n| (low..=high).contains(n))
                .map(|n| n as u8)
                .ok_or_else(|| bad(format!("{key} must be a whole number from {low} to {high}"))),
        }
    };
    let number = |spec: &Value, key: &str| -> Done<f64> {
        spec.get(key)
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
            .ok_or_else(|| bad(format!("every note needs a {key} (beats from the region's start)")))
    };
    let mut notes = Vec::with_capacity(list.len());
    for spec in list {
        let length = number(spec, "length")?;
        if length <= 0.0 {
            return Err(bad("every note needs a length above 0 (beats)"));
        }
        notes.push(super::smf::Note {
            pitch: whole(spec, "pitch", 0, 127, None)?,
            start: number(spec, "start")?,
            length,
            velocity: whole(spec, "velocity", 1, 127, Some(100))?,
            channel: whole(spec, "channel", 1, 16, Some(1))?,
        });
    }
    Ok((Plan { position, notes }, found))
}

fn fence_of(ctx: &mut Ctx) -> Done<Value> {
    let headers = super::nudge::read(tracks::headers(ctx))?;
    let regions = super::nudge::read(ctx.project.read_regions(ctx.ax))?;
    Ok(json!({"tracks": headers.len(), "names": tracks::names(&headers), "regions": regions.len()}))
}

fn title(count: usize, track: &str, position: &Position) -> String {
    format!("Imported {count} note{} on {} at {}", if count == 1 { "" } else { "s" }, quoted(track), timeline::bar_text(position))
}

/// The file's path: its bytes' hash as its name, in the folder.
fn file_for(bytes: &[u8]) -> PathBuf {
    let hash = daw_bridge::auth::sha256_hex(bytes);
    Path::new(FOLDER).join(format!("kumi-{}.mid", &hash[..12]))
}

/// Writes the file, unless it is there already with these bytes.
fn write_file(path: &Path, bytes: &[u8]) -> Done<()> {
    if std::fs::read(path).is_ok_and(|there| there == bytes) {
        return Ok(());
    }
    std::fs::create_dir_all(FOLDER).map_err(|error| failed(format!("Kumi couldn't make {FOLDER}: {error}")))?;
    std::fs::write(path, bytes).map_err(|error| failed(format!("Kumi couldn't write {}: {error}", path.display())))
}

pub fn preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let (plan, found) = plan(ctx, input, Epoch::Current)?;
    let beats_per_bar = timeline::control_bar(ctx)?.1.beats_per_bar();
    let position = Position::from_beats(plan.position, beats_per_bar);
    let fence = fence_of(ctx)?;
    let mut display = json!({"title": title(plan.notes.len(), &found.label(), &position), "notes": plan.notes.len(), "note": "Logic Pro's own undo takes an import back (\"Import\"), while it is its latest step."});
    if position.division > 1 || position.tick > 1 {
        display["note"] = "Logic Pro's playhead display places the region on a beat: the division and tick are dropped.".into();
    }
    Ok(Preview {
        target: json!({"ref": ref_text(ctx, "track", &[found.place]), "kind": "track"}),
        prior: json!({"tracks": fence["tracks"], "regions": fence["regions"]}),
        fence,
        display,
    })
}

/// A modal dialog of Logic's (not a panel), if one is up: its node.
fn question(ctx: &mut Ctx) -> Done<Option<Node>> {
    let reply = super::nudge::read(ctx.ax.windows())?;
    let (windows, _) = crate::logic::parse_windows(&reply);
    if !windows.iter().any(|window| window.dialog() || window.sheet()) {
        return Ok(None);
    }
    let node = super::nudge::read(ctx.ax.tree(&crate::ax::Root::Focused, "", 2, 40, Some(&["role", "subrole", "title", "desc", "value"])))?;
    Ok((node.role == "AXSheet" || node.subrole == "AXDialog" || node.role == "AXDialog").then_some(node))
}

/// Answers Logic's question after the import: No to a tempo (the file carries none), else the dialog is cancelled
/// and the import refused with its words. What was pressed, if anything.
pub(super) fn answer_questions(ctx: &mut Ctx) -> Done<Vec<String>> {
    let mut pressed = Vec::new();
    for _ in 0..QUESTION_POLLS {
        super::pause(QUESTION_POLL);
        if let Some(dialog) = question(ctx)? {
            let texts: Vec<String> = dialog.find_all_texts();
            let strings = &ctx.project.strings;
            let no = dialog.find(&|n| {
                n.role == "AXButton"
                    && (strings.is(&n.title, "No") || strings.is(&n.title, "Don’t Import") || strings.is(&n.title, "Don't Import"))
            });
            if let Some(button) = no {
                let label = button.title.clone();
                super::nudge::wrote(ctx.ax.action(&crate::ax::Root::Focused, &button.path, "AXPress", 400))?;
                pressed.push(label);
                continue;
            }
            if let Some(cancel) = dialog.find(&|n| n.role == "AXButton" && strings.is(&n.title, "Cancel")) {
                let _ = ctx.ax.action(&crate::ax::Root::Focused, &cancel.path, "AXPress", 400);
            }
            return Err(super::uncertain(format!("Logic Pro asked something Kumi didn't expect after the import ({})", texts.join(" "))));
        }
        if !pressed.is_empty() {
            break;
        }
    }
    Ok(pressed)
}

impl Node {
    /// Every static text's value in the tree, for a dialog's words.
    fn find_all_texts(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut found = Vec::new();
        self.find_all(&|n| n.role == "AXStaticText" && !n.value_text().is_empty(), &mut found);
        for node in found {
            out.push(node.value_text());
        }
        out
    }
}

pub fn apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let (plan, found) = plan(ctx, input, Epoch::Current).map_err(|f| if f.0 == "failed" { changed(f.1) } else { f })?;
    if fence_of(ctx)? != *fence {
        return Err(changed("the tracks or regions changed since the preview"));
    }
    let beats_per_bar = timeline::control_bar(ctx)?.1.beats_per_bar();
    let position = Position::from_beats(plan.position, beats_per_bar);
    let bytes = super::smf::format0(&plan.notes);
    let file = file_for(&bytes);
    write_file(&file, &bytes)?;
    let before_regions = super::nudge::read(ctx.project.read_regions(ctx.ax))?;
    let before_tracks = super::nudge::read(tracks::headers(ctx))?.len();
    let (_, transport) = timeline::control_bar(ctx)?;
    let was = transport.position();
    structure::select(ctx, found.place)?;
    timeline::set_playhead(ctx, Position { bar: position.bar, beat: position.beat, ..Position::START })?;
    front::ensure(ctx.ax)?;
    structure::file_menu(ctx, &["Import", "MIDI File…"])?;
    let the_panel = panel::find_or_close(ctx.ax)?;
    let driven = (|| {
        panel::go_to(ctx.ax, &the_panel, &file.to_string_lossy())?;
        panel::press(ctx.ax, &the_panel, "OKButton")?;
        Ok(())
    })();
    if let Err(fault) = driven {
        panel::cancel(ctx.ax, &the_panel);
        return Err(fault);
    }
    // The panel reads the file it was taken to before its Open button takes a press, and pressed while it was still
    // reading, the panel stays up (the M6 gate's second run, once in several): the press is made again once Open is
    // enabled. A run where the first press took (every recorded scene) asks nothing more.
    let took = panel::gone(ctx.ax, panel::PANEL_WAIT)?
        || (panel::press_when_enabled(ctx.ax, &the_panel, "OKButton", OPEN_WAIT).is_ok() && panel::gone(ctx.ax, panel::PANEL_WAIT)?);
    if !took {
        panel::cancel(ctx.ax, &the_panel);
        return Err(failed(format!("Logic Pro's import panel stayed up: it didn't take {}", file.display())));
    }
    let answered = answer_questions(ctx)?;
    super::pause(AFTER_IMPORT);
    let _ = timeline::set_playhead(ctx, was);
    timeline::look(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let headers = super::nudge::wrote(tracks::headers(ctx))?;
    let regions = super::nudge::wrote(ctx.project.read_regions(ctx.ax))?;
    // The new region: the one the Tracks area didn't have, by track and bounds.
    let mut left: Vec<(usize, Position, Position)> = before_regions.iter().map(|(t, r)| (*t, r.start, r.end)).collect();
    let new_place = regions.iter().enumerate().find_map(|(i, (track, region))| {
        match left.iter().position(|(t, s, e)| *t == *track && *s == region.start && *e == region.end) {
            Some(at) => {
                left.swap_remove(at);
                None
            }
            None => Some(i + 1),
        }
    });
    let Some(place) = new_place else {
        return Err(super::uncertain(format!("Logic Pro shows {} regions after the import, none new", regions.len())));
    };
    let (track, region) = &regions[place - 1];
    // The Event List lists the rows on its screen: a long file reads as its first rows, and the region's length
    // (the file's span, as Logic rounds it up to the bar) says the rest came in.
    let rows = ctx.project.notes_rows(ctx.ax, place, beats_per_bar).map(|rows| rows.len()).unwrap_or(0);
    let (start, end) = (region.start.beats(beats_per_bar), region.end.beats(beats_per_bar));
    let span = plan.notes.iter().map(|n| n.start + n.length).fold(0.0, f64::max);
    if end - start + 1e-6 < span {
        return Err(super::uncertain(format!(
            "Logic Pro's new region runs {} beats, shorter than the file's {span} beats: the import didn't all come in",
            end - start
        )));
    }
    let track_label = headers.get(track - 1).map(tracks::Found::label).unwrap_or_else(|| format!("Track {track}"));
    let mut created = json!({"ref": ref_text(ctx, "region", &[place]), "kind": "region", "track": ref_text(ctx, "track", &[*track])});
    if headers.len() > before_tracks {
        created["trackAdded"] = true.into();
    }
    let mut display = json!({"title": title(plan.notes.len(), &track_label, &region.start), "notes": plan.notes.len()});
    if rows < plan.notes.len() {
        display["note"] =
            format!("Logic Pro's Event List lists its first {rows} rows; the region's {} beats hold the file's {span}.", end - start)
                .into();
    }
    let mut applied = json!({"notes": plan.notes.len(), "startBeats": start, "endBeats": end, "track": track, "trackName": track_label, "file": file.to_string_lossy(), "answered": answered});
    if rows < plan.notes.len() {
        applied["rows"] = rows.into();
    }
    Ok(Applied { applied, created: Some(created), display, step: Some("Import"), ..Applied::default() })
}

pub fn restore(ctx: &mut Ctx, watch: &mut Watch, restore: &Restore) -> Done<Restored> {
    let created = restore.created.ok_or_else(|| bad("created: the region the import made"))?;
    let beats_per_bar = timeline::control_bar(ctx)?.1.beats_per_bar();
    let place = super::region_place(ctx, created, "ref", Epoch::Any);
    let regions = super::nudge::read(ctx.project.read_regions(ctx.ax))?;
    let there = place.as_ref().ok().and_then(|place| regions.get(place - 1)).map(|(track, region)| {
        json!({"track": track, "startBeats": region.start.beats(beats_per_bar), "endBeats": region.end.beats(beats_per_bar)})
    });
    let Some(there) = there else {
        return Ok(Restored::by_value(json!({"deleted": created["ref"]}), json!({"title": "The imported region is gone already"})));
    };
    if !restore.discard
        && (there["startBeats"] != restore.applied["startBeats"]
            || there["endBeats"] != restore.applied["endBeats"]
            || there["track"] != restore.applied["track"])
    {
        return Err(changed("the imported region has been moved in Logic Pro since Kumi imported it"));
    }
    let pressed = structure::undo_in_logic(ctx, watch, &restore.key, "Import", None)?;
    timeline::look(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let after = super::nudge::wrote(ctx.project.read_regions(ctx.ax))?;
    if after.len() >= regions.len() {
        return Err(super::uncertain(format!("Logic Pro's {pressed:?} was pressed, but the imported region is still there")));
    }
    Ok(Restored {
        restored: json!({"deleted": created["ref"], "pressed": pressed}),
        display: json!({"title": "Import taken back", "note": "Put back with Logic Pro's own undo."}),
        undone_in_logic: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_is_named_by_its_bytes_in_the_folder_kumi_can_read() {
        let bytes =
            super::super::smf::format0(&[super::super::smf::Note { pitch: 60, start: 0.0, length: 1.0, velocity: 100, channel: 1 }]);
        let path = file_for(&bytes);
        assert!(path.starts_with(FOLDER));
        assert!(path.to_string_lossy().ends_with(".mid"));
        assert_eq!(file_for(&bytes), path, "the same bytes, the same name");
        assert_ne!(file_for(b"other"), path);
        assert_eq!(title(64, "Keys", &Position { bar: 5, ..Position::START }), "Imported 64 notes on “Keys” at bar 5");
    }
}
