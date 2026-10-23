//! `sample-load`: a sample (an audio file) made playable on a track of its own. Logic loads a sample into its Sampler
//! through the Track menu's "Convert Regions to New Sampler Track…", which takes the selected audio regions, so the
//! file comes in first as a producer brings one in: File > Import > Audio File… through the panel service (the go-to
//! sheet, Open), onto an audio track at the playhead, a region Logic selects alone and names "Insert" in its history.
//! Then the region is converted: the sheet's zones from Regions, its instrument Sampler, the instrument's name, OK.
//! Logic makes a software instrument track below the audio track, named after the instrument, with a Sampler holding
//! the sample as one zone and a MIDI region of its trigger note where the audio region was, mutes the audio region it
//! took, and names the step "Convert Regions to New Sampler Track" (measured on Logic Pro 12.3.1). Read back from the
//! new header and the new track's strip in the Inspector (Logic selects it): its name and its instrument slot's
//! plug-in. A restore is Logic's own undo of both steps while they are on top: the conversion, then the import right
//! under it, read back from the headers (the Sampler track gone) and the regions (the audio region the import made
//! gone too); a step that didn't land is `uncertain`, named.
//!
//! Logic copies the file into the project's package at once, saved or not: `Media/Audio Files` for the import and
//! `Media/Samples/<instrument>` for the Sampler's zone; its undo takes the tracks and regions back and leaves those
//! copies. So this was measured on a copy of Scratch, never on Scratch.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

use super::{
    changed, front, import, nudge, panel, patch, quoted, ref_text, structure, timeline, tracks, Applied, Ctx, Done, Epoch, Preview,
    Restore, Restored, Watch,
};
use crate::ax::{Node, Root};
use crate::refs::{bad, failed, Ref};
use crate::rows::{Position, Region};

/// Logic's names for the two steps a load makes, the later first.
const STEP: &str = "Convert Regions to New Sampler Track";
const IMPORT_STEP: &str = "Insert";
/// The Track menu's item and the sheet it opens.
const CONVERT_ITEM: &str = "Convert Regions to New Sampler Track…";
/// The files the import takes, by extension.
const AUDIO: [&str; 8] = ["wav", "wave", "aif", "aiff", "aifc", "caf", "mp3", "m4a"];
/// How long the sheet gets to come up and to go, Logic after the import, and after the conversion.
const SHEET_WAIT: Duration = Duration::from_secs(3);
const AFTER_IMPORT: Duration = Duration::from_millis(800);
const AFTER_CONVERT: Duration = Duration::from_millis(800);
/// How often and how far apart the Undo title is read after the conversion's undo, until it names the import.
const TITLE_POLLS: usize = 6;
const TITLE_POLL: Duration = Duration::from_millis(250);
/// How long the import panel gets to read the file before its Open button takes a press.
const OPEN_WAIT: Duration = Duration::from_secs(3);

/// What a request asks, checked.
struct Plan {
    file: String,
    /// The audio track the file comes in on (its place from 1), and its label.
    track: usize,
    label: String,
    /// Where the region goes; None: the playhead where it is.
    position: Option<f64>,
    /// The instrument's name (Logic's default: the file's name without its extension).
    name: String,
}

/// The file's name without its extension, as Logic names the region and the instrument.
fn stem(file: &str) -> String {
    Path::new(file).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn plan(ctx: &mut Ctx, input: &Value, epoch: Epoch) -> Done<Plan> {
    match input.get("kind").and_then(Value::as_str) {
        Some("sampler") => {}
        Some("quick-sampler") => return Err(bad(QUICK_SAMPLER)),
        _ => return Err(bad("kind: sampler (Logic Pro's Sampler, on a new track) or quick-sampler")),
    }
    let file = input
        .get("file")
        .and_then(Value::as_str)
        .filter(|f| Path::new(f).is_absolute() && !f.chars().any(char::is_control))
        .ok_or_else(|| bad("file: the whole path of an audio file"))?
        .to_string();
    let extension = Path::new(&file).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !AUDIO.contains(&extension.as_str()) {
        return Err(bad(format!("file must be an audio file Logic Pro imports ({})", AUDIO.join(", "))));
    }
    if !crate::disk::exists(Path::new(&file)) {
        return Err(bad(format!(
            "{file} isn't there, or Kumi can't see it (macOS keeps ~/Music from a process without the Files and Folders permission)"
        )));
    }
    let position = match input.get("positionBeats") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_f64()
                .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
                .ok_or_else(|| bad("positionBeats must be a number from 0: beats from the project's start"))?,
        ),
    };
    let name = match input.get("name") {
        None | Some(Value::Null) => stem(&file),
        Some(Value::String(name)) if !name.trim().is_empty() && name.chars().count() <= 64 && !name.chars().any(char::is_control) => {
            name.clone()
        }
        Some(_) => return Err(bad("name must be one line of at most 64 characters")),
    };
    // The audio track the file comes in on: the one named, else the selected track when it is an audio track, else
    // the first audio track. Their kinds are the Mixer's strips'.
    let all = nudge::read(tracks::headers(ctx))?;
    let headers: Vec<crate::rows::Header> = all.iter().map(|f| f.header.clone()).collect();
    let strips = nudge::read(ctx.project.strips(ctx.ax, &headers))?;
    let audio = |place: usize| strips.get(place - 1).is_some_and(|strip| strip.kind == "audio");
    let track = match input.get("ref").filter(|r| !r.is_null()) {
        Some(_) => {
            let found = tracks::find(ctx, input, "ref", epoch)?;
            if !audio(found.place) {
                return Err(bad(format!(
                    "{} isn't an audio track: Logic Pro converts an audio region into a Sampler, so the sample comes in on an audio track",
                    quoted(&found.label())
                )));
            }
            found.place
        }
        None => all
            .iter()
            .find(|f| f.header.selected && audio(f.place))
            .or_else(|| all.iter().find(|f| audio(f.place)))
            .map(|f| f.place)
            .ok_or_else(|| failed("the project has no audio track for the sample to come in on before Logic Pro converts it"))?,
    };
    let label = all[track - 1].label();
    Ok(Plan { file, track, label, position, name })
}

/// Why Quick Sampler isn't one of the channels.
const QUICK_SAMPLER: &str = "Logic Pro's bridge can't load a sample into Quick Sampler: Logic converts audio regions into a Sampler, Alchemy or Drum Machine Designer track (whose pad holds the sample in a Quick Sampler, inside a Track Stack), and Quick Sampler takes a sample by a drag onto its window; ask for kind sampler";

fn fence_of(ctx: &mut Ctx) -> Done<Value> {
    let headers = nudge::read(tracks::headers(ctx))?;
    let regions = nudge::read(ctx.project.read_regions(ctx.ax))?;
    Ok(json!({"tracks": headers.len(), "names": tracks::names(&headers), "regions": regions.len()}))
}

fn title(name: &str) -> String {
    format!("Loaded {} into a new Sampler track", quoted(name))
}

pub fn preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let plan = plan(ctx, input, Epoch::Current)?;
    let fence = fence_of(ctx)?;
    Ok(Preview {
        target: json!({"ref": input.get("ref").cloned().unwrap_or(Value::Null), "kind": "track"}),
        prior: json!({"tracks": fence["tracks"], "regions": fence["regions"]}),
        fence,
        display: json!({
            "title": title(&plan.name),
            "track": plan.label,
            "note": format!("Logic Pro brings the file in on {} and converts it, leaving that audio region muted; it copies the file into the project's folder. Its own undo takes the load back while it is its latest step.", quoted(&plan.label)),
        }),
    })
}

/// The conversion's sheet, once it is up: the focused window, a sheet whose texts name the conversion.
fn sheet(ctx: &mut Ctx) -> Done<Option<Node>> {
    let node = nudge::read(ctx.ax.tree(&Root::Focused, "", 2, 40, Some(&["role", "subrole", "title", "desc", "value"])))?;
    let strings = &ctx.project.strings;
    let ours = node.find(&|n| n.role == "AXStaticText" && strings.is(n.value_text().trim(), STEP)).is_some();
    Ok((node.role == "AXSheet" && ours).then_some(node))
}

/// Waits for the sheet to come (`up`) or go.
fn wait_sheet(ctx: &mut Ctx, up: bool) -> Done<Option<Node>> {
    let until = std::time::Instant::now() + SHEET_WAIT;
    loop {
        let found = sheet(ctx)?;
        if found.is_some() == up {
            return Ok(found);
        }
        if std::time::Instant::now() >= until {
            return Ok(found);
        }
        super::pause(Duration::from_millis(150));
    }
}

/// How many selected regions the sheet says it converts ("… from 1 selected region…"), when it says.
fn selected_in(sheet: &Node) -> Option<u32> {
    let mut texts = vec![];
    sheet.find_all(&|n| n.role == "AXStaticText", &mut texts);
    texts.iter().find_map(|text| {
        let value = text.value_text();
        let after = value.split(" from ").nth(1)?;
        after.split_whitespace().next()?.parse().ok()
    })
}

/// Fills the sheet (zones from the region, the Sampler, the name) and presses OK; Cancel when anything is off.
fn convert(ctx: &mut Ctx, name: &str) -> Done<()> {
    let Some(up) = wait_sheet(ctx, true)? else {
        return Err(super::uncertain("Logic Pro didn't show its Convert Regions to New Sampler Track sheet"));
    };
    let strings = ctx.project.strings.clone();
    let press = |ctx: &mut Ctx, node: &Node| nudge::wrote(ctx.ax.action(&Root::Focused, &node.path, "AXPress", 250));
    let cancel = |ctx: &mut Ctx, sheet: &Node| {
        if let Some(button) = sheet.find(&|n| n.role == "AXButton" && strings.is(&n.title, "Cancel")) {
            let _ = ctx.ax.action(&Root::Focused, &button.path, "AXPress", 300);
        }
    };
    let filled = (|| {
        if let Some(count) = selected_in(&up) {
            if count != 1 {
                return Err(failed(format!(
                    "Logic Pro would convert {count} selected regions, not the one Kumi imported; nothing was converted"
                )));
            }
        }
        for label in ["Regions", "Sampler"] {
            let radio = up
                .find(&|n| n.role == "AXRadioButton" && strings.is(&n.title, label))
                .ok_or_else(|| failed(format!("Logic Pro's conversion sheet shows no {label} choice")))?;
            if !radio.is_on() {
                press(ctx, radio)?;
            }
        }
        let field = up.find(&|n| n.role == "AXTextField").ok_or_else(|| failed("Logic Pro's conversion sheet shows no name field"))?;
        if field.value_text() != name {
            let set = nudge::wrote(ctx.ax.set(&Root::Focused, &field.path, "AXValue", json!(name), 150))?;
            if set["after"].as_str() != Some(name) {
                return Err(failed(format!("Logic Pro's conversion sheet took the name as {}", set["after"])));
            }
        }
        Ok(())
    })();
    if let Err(fault) = filled {
        cancel(ctx, &up);
        return Err(fault);
    }
    let ok = up
        .find(&|n| n.role == "AXButton" && strings.is(&n.title, "OK"))
        .ok_or_else(|| failed("Logic Pro's conversion sheet shows no OK button"))?
        .clone();
    press(ctx, &ok)?;
    if wait_sheet(ctx, false)?.is_some() {
        return Err(super::uncertain("Logic Pro's conversion sheet stayed up after OK"));
    }
    Ok(())
}

/// The region a look found that wasn't there before, by track and bounds: its place through the project.
fn new_region(before: &[(usize, crate::rows::Region)], after: &[(usize, crate::rows::Region)], midi: bool) -> Option<usize> {
    let mut left: Vec<(usize, Position, Position)> = before.iter().map(|(t, r)| (*t, r.start, r.end)).collect();
    after.iter().enumerate().find_map(|(i, (track, region))| {
        match left.iter().position(|(t, s, e)| *t == *track && *s == region.start && *e == region.end) {
            Some(at) => {
                left.swap_remove(at);
                None
            }
            None => (region.midi == midi).then_some(i + 1),
        }
    })
}

pub fn apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let plan = plan(ctx, input, Epoch::Current).map_err(|f| if f.0 == "failed" { changed(f.1) } else { f })?;
    if fence_of(ctx)? != *fence {
        return Err(changed("the tracks or regions changed since the preview"));
    }
    let beats_per_bar = timeline::control_bar(ctx)?.1.beats_per_bar();
    let was = timeline::control_bar(ctx)?.1.position();
    let before_names = tracks::names(&nudge::read(tracks::headers(ctx))?);
    let before_regions = nudge::read(ctx.project.read_regions(ctx.ax))?;
    structure::select(ctx, plan.track)?;
    if let Some(position) = plan.position {
        let at = Position::from_beats(position, beats_per_bar);
        timeline::set_playhead(ctx, Position { bar: at.bar, beat: at.beat, ..Position::START })?;
    }
    let loaded = load(ctx, &plan, beats_per_bar, before_names, &before_regions);
    // The playhead back where it was, whatever happened after it moved.
    if plan.position.is_some() {
        let _ = timeline::set_playhead(ctx, was);
    }
    loaded
}

/// The import and the conversion, once the track is selected and the playhead placed; the apply's answer.
fn load(
    ctx: &mut Ctx,
    plan: &Plan,
    beats_per_bar: u32,
    before_names: Vec<String>,
    before_regions: &[(usize, crate::rows::Region)],
) -> Done<Applied> {
    // The file comes in through the open panel, onto the selected audio track at the playhead.
    front::ensure(ctx.ax)?;
    structure::file_menu(ctx, &["Import", "Audio File…"])?;
    let the_panel = panel::find_or_close(ctx.ax)?;
    let driven = (|| {
        panel::go_to(ctx.ax, &the_panel, &plan.file)?;
        panel::press_when_enabled(ctx.ax, &the_panel, "OKButton", OPEN_WAIT)
    })();
    if let Err(fault) = driven {
        panel::cancel(ctx.ax, &the_panel);
        return Err(fault);
    }
    if !panel::gone(ctx.ax, panel::PANEL_WAIT)? {
        panel::cancel(ctx.ax, &the_panel);
        return Err(failed(format!("Logic Pro's import panel stayed up: it didn't take {}", plan.file)));
    }
    let answered = import::answer_questions(ctx)?;
    super::pause(AFTER_IMPORT);
    timeline::look(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let imported = nudge::wrote(ctx.project.read_regions(ctx.ax))?;
    let Some(source) = new_region(before_regions, &imported, false) else {
        return Err(super::uncertain(format!("Logic Pro shows no new audio region after the import of {}", plan.file)));
    };
    let source_track = imported[source - 1].0;
    // The region, which Logic selected alone, converted.
    front::ensure(ctx.ax)?;
    structure::menu_item(ctx, "Track", &[CONVERT_ITEM])?;
    convert(ctx, &plan.name)?;
    super::pause(AFTER_CONVERT);
    timeline::look(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let headers = nudge::wrote(tracks::headers(ctx))?;
    let names = tracks::names(&headers);
    if headers.len() != before_names.len() + 1 {
        return Err(super::uncertain(format!(
            "Logic Pro shows {} tracks after converting the sample, not one more than {}",
            headers.len(),
            before_names.len()
        )));
    }
    // The new track: below the audio track, the one name that wasn't there.
    let place = (1..=headers.len()).find(|p| names.get(p - 1) != before_names.get(p - 1)).unwrap_or(source_track + 1);
    let track_name = headers[place - 1].header.name.clone();
    let (_, plugins) = patch::strip_now(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let instrument = plugins.first().cloned().unwrap_or_default();
    if !ctx.project.strings.is(&instrument, "Sampler") {
        return Err(super::uncertain(format!(
            "Logic Pro's new track {} shows {} as its instrument, not the Sampler",
            quoted(&track_name),
            if instrument.is_empty() { "nothing".to_string() } else { quoted(&instrument) }
        )));
    }
    let regions = nudge::wrote(ctx.project.read_regions(ctx.ax))?;
    let trigger = regions.iter().position(|(track, region)| *track == place && region.midi).map(|i| i + 1);
    let source_now =
        regions.iter().position(|(track, region)| *track == source_track && !region.midi && region.name.starts_with(&stem(&plan.file)));
    let (start, end) =
        trigger.map(|i| (regions[i - 1].1.start.beats(beats_per_bar), regions[i - 1].1.end.beats(beats_per_bar))).unwrap_or((0.0, 0.0));
    let mut created = json!({"ref": ref_text(ctx, "track", &[place]), "kind": "track"});
    if let Some(trigger) = trigger {
        created["region"] = ref_text(ctx, "region", &[trigger]).into();
    }
    if let Some(source) = source_now {
        created["source"] = ref_text(ctx, "region", &[source + 1]).into();
    }
    Ok(Applied {
        applied: json!({
            "file": plan.file, "kind": "sampler", "track": place, "name": track_name, "instrument": instrument,
            "startBeats": start, "endBeats": end, "sourceTrack": source_track, "answered": answered,
        }),
        created: Some(created),
        display: json!({
            "title": title(&track_name),
            "note": format!("Logic Pro left the audio region it converted muted on {} and copied the file into the project's folder.", quoted(&plan.label)),
        }),
        step: Some(STEP),
        probe: Some(super::undo::Probe::Headers { prior: before_names, applied: names }),
        ..Applied::default()
    })
}

pub fn restore(ctx: &mut Ctx, watch: &mut Watch, restore: &Restore) -> Done<Restored> {
    let created = restore.created.ok_or_else(|| bad("created: the track the load made"))?;
    let name = restore.applied["name"].as_str().unwrap_or_default().to_string();
    let place = super::track_place(ctx, created, "ref", Epoch::Any);
    let all = nudge::read(tracks::headers(ctx))?;
    let there = place.as_ref().ok().and_then(|place| all.get(place - 1)).filter(|found| found.header.name == name);
    let Some(found) = there else {
        return Ok(Restored::by_value(json!({"deleted": created["ref"]}), json!({"title": format!("{} is gone already", quoted(&name))})));
    };
    let regions = nudge::read(ctx.project.read_regions(ctx.ax))?;
    if !restore.discard {
        let held = regions.iter().filter(|(track, _)| *track == found.place).count();
        if held > 1 {
            return Err(changed(format!("the Sampler track Kumi made ({}) holds {held} regions now", quoted(&name))));
        }
    }
    // The audio region the import made (muted by the conversion), to read back that the import's undo took it too.
    let imported = imported_region(ctx, restore, &regions);
    let epoch = ctx.project.epoch;
    let before = all.len();
    let mut pressed = vec![structure::undo_in_logic(ctx, watch, &restore.key, STEP, None)?];
    // The import right under the conversion: Kumi's too, since nothing came between them. Logic moves its Undo title
    // a moment after the undo, so the title is read again until it stops naming the conversion.
    let (mut titles, mut verified) = watch.fresh_titles(ctx)?;
    for _ in 1..TITLE_POLLS {
        if !verified || titles.undo_action(&ctx.project.strings).as_deref() != Some(STEP) {
            break;
        }
        super::pause(TITLE_POLL);
        (titles, verified) = watch.fresh_titles(ctx)?;
    }
    let mut note = "Put back with Logic Pro's own undo; the copies of the file stay in the project's folder.".to_string();
    let import_on_top = verified && titles.undo_action(&ctx.project.strings).as_deref() == Some(IMPORT_STEP);
    if import_on_top {
        let mut menus = vec!["Edit".to_string()];
        menus.extend(ctx.project.strings.candidates("Edit"));
        if let Some(shown) = menus.iter().find_map(|menu| ctx.ax.menu(&[menu.as_str(), titles.undo.as_str()]).ok()) {
            pressed.push(shown);
            super::pause(Duration::from_millis(500));
        }
    } else {
        note = format!("{note} The audio region the sample came in on stays: Logic Pro's Edit menu says {:?}.", titles.undo);
    }
    let regions_after = look_regions(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let after = nudge::wrote(tracks::headers(ctx))?;
    if after.len() >= before {
        return Err(super::uncertain(format!("Logic Pro's {} was pressed, but the Sampler track is still there", quoted(&pressed[0]))));
    }
    // The import's undo named on top: the audio region it made is gone too, or its step didn't land.
    if let Some((track, start, end)) = imported.filter(|_| import_on_top) {
        let track_now = ctx.project.carry_track(epoch, track).unwrap_or(track);
        let at = |regions: &[(usize, Region)], track: usize| {
            regions.iter().filter(|(t, region)| *t == track && !region.midi && region.start == start && region.end == end).count()
        };
        if at(&regions_after, track_now) >= at(&regions, track) {
            let label = track_now
                .checked_sub(1)
                .and_then(|at| after.get(at))
                .map(tracks::Found::label)
                .unwrap_or_else(|| format!("track {track_now}"));
            let file = stem(restore.applied["file"].as_str().unwrap_or_default());
            return Err(super::uncertain(format!(
                "Logic Pro took the Sampler track back, but its {} didn't land: the audio region of {} is still on {}",
                quoted(&titles.undo),
                quoted(&file),
                quoted(&label)
            )));
        }
    }
    Ok(Restored {
        restored: json!({"deleted": created["ref"], "pressed": pressed}),
        display: json!({"title": format!("{} taken back", quoted(&name)), "note": note}),
        undone_in_logic: true,
    })
}

/// One focus refresh, as `timeline::look` makes it, with the regions it read.
fn look_regions(ctx: &mut Ctx) -> Done<Vec<(usize, Region)>> {
    nudge::read(tracks::ensure_layout(ctx))?;
    let (_, _, regions) = nudge::read(ctx.project.look(ctx.ax, None))?;
    Ok(regions)
}

/// The audio region a load's import made, as the regions read now show it: its track's place and its bounds. The
/// apply's `created.source` carried to now; else the audio region on the load's audio track that starts where the
/// Sampler's region does, named after the file.
fn imported_region(ctx: &Ctx, restore: &Restore, regions: &[(usize, Region)]) -> Option<(usize, Position, Position)> {
    let created = restore.created?;
    let audio = |place: usize| regions.get(place.checked_sub(1)?).filter(|(_, region)| !region.midi);
    let by_ref = created
        .get("source")
        .and_then(Value::as_str)
        .and_then(Ref::parse)
        .and_then(|source| ctx.project.carry_region(source.epoch, source.first()))
        .and_then(audio);
    let found = by_ref.or_else(|| {
        let epoch = Ref::parse(created.get("ref")?.as_str()?)?.epoch;
        let track = ctx.project.carry_track(epoch, restore.applied["sourceTrack"].as_u64()? as usize)?;
        let start = restore.applied["startBeats"].as_f64()?;
        let file = stem(restore.applied["file"].as_str()?);
        let per_bar = ctx.project.transport.beats_per_bar();
        regions.iter().find(|(at, region)| {
            *at == track && !region.midi && (region.start.beats(per_bar) - start).abs() < 1e-6 && region.name.starts_with(&file)
        })
    })?;
    Some((found.0, found.1.start, found.1.end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sample_is_an_audio_file_and_the_sheet_says_how_many_regions_it_converts() {
        assert_eq!(stem("/Users/me/Samples/Kick 01.wav"), "Kick 01");
        assert_eq!(title("Kick 01"), "Loaded “Kick 01” into a new Sampler track");
        let sheet = Node::from_json(&json!({"role": "AXSheet", "c": [
            {"i": "12", "role": "AXStaticText", "value": "OK creates an instrument with 1 zones (of a maximum of 128) from 1 selected region."},
            {"i": "13", "role": "AXStaticText", "value": "Convert Regions to New Sampler Track"}
        ]}));
        assert_eq!(selected_in(&sheet), Some(1));
        assert_eq!(selected_in(&Node::from_json(&json!({"role": "AXSheet"}))), None);
    }
}
