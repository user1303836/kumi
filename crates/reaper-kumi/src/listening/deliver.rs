//! Renders for delivery (#401, the render pipeline): the producer's own files, named by REAPER's wildcards, where they belong.
//!
//! `deliver_targets` sets the render settings asked for, reads the files REAPER would write (`RENDER_TARGETS`), and puts every
//! setting back, all in the one call. `deliver_start` plans the same render as a job; on a following tick it sets the settings,
//! checks REAPER would write exactly the files the dry run showed (and none of them is there yet, unless asked to write over
//! them), renders with REAPER's own render (42230), checks each file was written, and puts every setting back however it ends.
//! Files a failed or cancelled render left are deleted, never one that was there before.
//!
//! A delivery renders without the render dialog's extras (normalizing, a limiter, fades, trimming, dither, a second format)
//! unless asked to keep them, and says which it turned off. Picking regions (`regionRefs`) renders only those: for the render,
//! the region render matrix holds rows for them alone (the master's, from source master), every row put back after.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{json, Value};

use super::render::{self, Ended, Hooks, PutBack, RENDER_WITH_LAST};
use super::{bad, failed, gone, stale, Fault};
use crate::read::Reads;
use crate::reaper::{Project, Reaper, Track};
use crate::refs::{Ref, TrackId};

/// The most files one delivery writes.
pub const MAX_FILES: usize = 2000;
/// The longest tail asked for (ms).
pub const MAX_TAIL_MS: u64 = 30_000;

/// What's rendered: the master mix, each region's tracks in the region render matrix (summed through the master, a file per
/// region), or the tracks named as stems.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Master,
    Matrix,
    Stems,
}

/// `RENDER_SETTINGS` for "Region render matrix via master": the render matrix (8) through the master (128), a file per region
/// with its row's tracks summed, none for a region whose row is empty. (The lab batch checks the value.)
const MATRIX_VIA_MASTER: i32 = 8 | 128;
/// The producer's own bits of `RENDER_SETTINGS` a delivery keeps: the start delay, and embedding transients (256), metadata
/// (512: UCS, BWF) and take markers (1024) in the files.
const KEPT_SETTINGS: i32 = render::DELAY_START | 256 | 512 | 1024;

/// Over what: the project, the time selection, every region, the selected regions, or a stretch (seconds).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Bounds {
    Project,
    TimeSelection,
    Regions,
    SelectedRegions,
    Range(f64, f64),
}

impl Bounds {
    /// `RENDER_BOUNDSFLAG`, and the bit of `RENDER_TAILFLAG` that adds a tail to these bounds.
    fn flags(self) -> (f64, i64) {
        match self {
            Bounds::Range(..) => (0., 1),
            Bounds::Project => (1., 2),
            Bounds::TimeSelection => (2., 4),
            Bounds::Regions => (3., 8),
            Bounds::SelectedRegions => (5., 32),
        }
    }
}

/// A delivery as asked: the settings, the tracks of a stems render, whether files there may be written over, and the files the
/// dry run showed (a render refuses when REAPER would write others).
#[derive(Debug, Clone)]
pub struct Spec {
    pub source: Source,
    pub bounds: Bounds,
    pub tracks: Vec<String>,
    /// `RENDER_FORMAT` (base64 of the sink's settings), or None for the project's own format.
    pub format: Option<String>,
    pub rate: Option<u32>,
    pub channels: u32,
    pub tail_ms: u64,
    pub pattern: String,
    pub folder: PathBuf,
    pub overwrite: bool,
    pub expect: Option<Vec<String>>,
    /// The regions that render (GUIDs), when only some do.
    pub regions: Option<Vec<String>>,
    /// The render dialog's extras kept (normalizing, a limiter, fades, dither, a second format).
    pub dialog: bool,
}

/// A delivery planned as a job: run on a following tick.
#[derive(Debug, Clone)]
pub struct Plan {
    pub job: String,
    pub queued: Instant,
    pub proj: Project,
    pub epoch: String,
    pub spec: Spec,
}

/// WAV at a bit depth: REAPER's sink settings (`evaw`, the bits, 0, 1; 32 is float, measured).
fn wav_format(bits: u64) -> Option<&'static str> {
    match bits {
        16 => Some("ZXZhdxAAAQ=="),
        24 => Some("ZXZhdxgAAQ=="),
        32 => Some(render::FLOAT_WAV),
        _ => None,
    }
}

/// The folder the producer's renders go to: the project's render folder (relative to the project's own folder when it isn't
/// a whole path), else the project's folder; None for a project never saved with no render folder set.
pub fn render_folder(reaper: &Reaper, proj: Project, path: &str) -> Option<PathBuf> {
    let project_folder = Path::new(path).parent().filter(|_| !path.is_empty()).map(Path::to_path_buf);
    let set = reaper.project_text_long(proj, "RENDER_FILE").unwrap_or_default();
    let set = set.trim();
    if !set.is_empty() {
        let set = PathBuf::from(set);
        if set.is_absolute() {
            return Some(set);
        }
        return project_folder.map(|folder| folder.join(set));
    }
    project_folder
}

/// The asked delivery, checked.
pub fn spec(reaper: &Reaper, reads: &Reads, proj: Project, path: &str, params: &Value) -> Result<Spec, Fault> {
    let source = match params.get("source").and_then(Value::as_str).unwrap_or("master") {
        "master" => Source::Master,
        "matrix" => Source::Matrix,
        "stems" => Source::Stems,
        _ => return Err(bad("source is master (the mix), matrix (each region's tracks in the region render matrix) or stems")),
    };
    let bounds = match params.get("bounds") {
        None | Some(Value::Null) => Bounds::Regions,
        Some(Value::String(word)) => match word.as_str() {
            "project" => Bounds::Project,
            "time-selection" => Bounds::TimeSelection,
            "regions" => Bounds::Regions,
            "selected-regions" => Bounds::SelectedRegions,
            _ => return Err(bad("bounds is project, time-selection, regions, selected-regions or {start, end} in seconds")),
        },
        Some(range) => {
            let at = |key: &str| range.get(key).and_then(Value::as_f64).filter(|n| n.is_finite() && *n >= 0.);
            match (at("start"), at("end")) {
                (Some(start), Some(end)) if end > start => Bounds::Range(start, end),
                _ => return Err(bad("bounds {start, end}: seconds, the end after the start")),
            }
        }
    };
    if source == Source::Matrix && !matches!(bounds, Bounds::Regions | Bounds::SelectedRegions) {
        return Err(bad("the region render matrix renders regions: bounds regions or selected-regions"));
    }
    let mut tracks = Vec::new();
    if let Some(list) = params.get("trackRefs").and_then(Value::as_array) {
        for reference in list {
            let text = reference.as_str().unwrap_or_default();
            let (_, guid) = render::track_of(reaper, reads, proj, text)?;
            tracks.push(guid);
        }
    }
    if source == Source::Stems && tracks.is_empty() {
        return Err(bad("stems render the tracks named: trackRefs"));
    }
    if source != Source::Stems && !tracks.is_empty() {
        return Err(bad("trackRefs are a stems render's"));
    }
    let format = match params.get("format") {
        None | Some(Value::Null) => wav_format(24).map(str::to_owned),
        Some(Value::String(word)) if word == "project" => None,
        Some(Value::String(word)) if word == "wav" => {
            let bits = params.get("bits").and_then(Value::as_u64).unwrap_or(24);
            Some(wav_format(bits).ok_or_else(|| bad("bits is 16, 24 or 32 (float)"))?.to_owned())
        }
        Some(_) => return Err(bad("format is wav (with bits), or project: the format set in REAPER's render dialog")),
    };
    let rate = match params.get("sampleRate") {
        None | Some(Value::Null) => None,
        Some(Value::String(word)) if word == "project" => None,
        Some(rate) => Some(
            rate.as_u64()
                .filter(|rate| [22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000].contains(rate))
                .ok_or_else(|| bad("sampleRate is project or 22050 to 192000 (a rate REAPER renders at)"))? as u32,
        ),
    };
    let channels = match params.get("channels").and_then(Value::as_u64) {
        None => 2,
        Some(n @ (1 | 2)) => n as u32,
        Some(_) => return Err(bad("channels is 1 (mono) or 2 (stereo)")),
    };
    let tail_ms = match params.get("tailMs") {
        None | Some(Value::Null) => 0,
        Some(ms) => ms.as_u64().filter(|ms| *ms <= MAX_TAIL_MS).ok_or_else(|| bad(format!("tailMs: 0 to {MAX_TAIL_MS}")))?,
    };
    // (a render's tail runs on past each region's end, into the next region's sound)
    if tail_ms > 0 && matches!(bounds, Bounds::Regions | Bounds::SelectedRegions) {
        return Err(bad(
            "tailMs is for project, time-selection or {start, end}: a region's tail is the region's own length (make it longer)",
        ));
    }
    let regions = match params.get("regionRefs") {
        None | Some(Value::Null) => None,
        Some(Value::Array(list)) if !list.is_empty() && list.len() <= MAX_FILES => {
            if source == Source::Stems || bounds != Bounds::Regions {
                return Err(bad("regionRefs pick the regions a master or matrix render renders, with bounds regions"));
            }
            let mut guids = Vec::new();
            for reference in list {
                let Ok(Ref::Region(guid)) = Ref::parse(reference.as_str().unwrap_or_default()) else {
                    return Err(bad("regionRefs: region refs"));
                };
                if reaper.marker_by_guid(proj, &guid).is_none_or(|marker| !marker.is_region) {
                    return Err(gone("a region of regionRefs isn't in the project any more (deleted, or another tab): read again"));
                }
                guids.push(guid);
            }
            Some(guids)
        }
        Some(_) => return Err(bad(format!("regionRefs: 1 to {MAX_FILES} region refs"))),
    };
    let pattern = params.get("pattern").and_then(Value::as_str).unwrap_or_default().to_string();
    if pattern.is_empty() || pattern.chars().count() > 512 || pattern.chars().any(char::is_control) {
        return Err(bad("pattern: the file names, with REAPER's wildcards ($region, $track, $project…), one line"));
    }
    // (a pattern may make subfolders of the folder, never leave it)
    if Path::new(&pattern).is_absolute() || pattern.split(['/', '\\']).any(|part| part == "..") {
        return Err(bad("pattern names files inside the folder: no whole path, no .."));
    }
    let folder = match params.get("folder").and_then(Value::as_str) {
        Some(folder) => {
            let folder = PathBuf::from(folder);
            if !folder.is_absolute() {
                return Err(bad("folder: a whole path"));
            }
            folder
        }
        None => render_folder(reaper, proj, path)
            .ok_or_else(|| failed("the project isn't saved and has no render folder: save it, or give folder"))?,
    };
    let expect = match params.get("expect") {
        None | Some(Value::Null) => None,
        Some(Value::Array(list)) => Some(list.iter().filter_map(Value::as_str).map(str::to_owned).collect()),
        Some(_) => return Err(bad("expect: the files the dry run listed")),
    };
    Ok(Spec {
        source,
        bounds,
        tracks,
        format,
        rate,
        channels,
        tail_ms,
        pattern,
        folder,
        overwrite: params.get("overwrite") == Some(&Value::Bool(true)),
        expect,
        regions,
        dialog: params.get("dialog") == Some(&Value::Bool(true)),
    })
}

/// The render dialog's extras on now, in words: what a delivery turns off unless asked to keep them.
fn extras(reaper: &Reaper, proj: Project) -> Vec<&'static str> {
    let number = |key: &str| reaper.project_number(proj, key).unwrap_or(0.0) as i64;
    let mut out = Vec::new();
    let post = number("RENDER_NORMALIZE");
    if post & (4 << 16) == 0 {
        if post & 1 != 0 {
            out.push("normalizing");
        }
        if post & 64 != 0 {
            out.push("a limiter");
        }
        if post & (512 | 1024) != 0 {
            out.push("fades");
        }
        // (trimming and padding are bits past those)
        if post & !0x3fff & !(4 << 16) != 0 {
            out.push("trimming or padding");
        }
    }
    if number("RENDER_DITHER") & 15 != 0 {
        out.push("dither");
    }
    if reaper.project_text_long(proj, "RENDER_FORMAT2").is_some_and(|format| !format.trim().is_empty()) {
        out.push("a second format");
    }
    out
}

/// What a delivery says of its render settings: the extras it turned off, or kept.
fn extras_note(spec: &Spec, extras: &[&str]) -> Option<String> {
    if extras.is_empty() {
        return None;
    }
    Some(if spec.dialog {
        format!("the render dialog's {} kept, as asked", extras.join(", "))
    } else {
        format!("the render dialog's {} off for this delivery (dialog: true keeps them)", extras.join(", "))
    })
}

/// Every region's row of the region render matrix (its number, GUID and tracks), noted before a delivery that picks regions
/// and put back however it ends (a panic between included).
struct Rows<'a, 'r> {
    reaper: &'a Reaper<'r>,
    proj: Project,
    rows: Vec<(i32, String, Vec<Track>)>,
    done: bool,
}

impl<'a, 'r> Rows<'a, 'r> {
    fn note(reaper: &'a Reaper<'r>, proj: Project) -> Self {
        let regions = (0..reaper.marker_count(proj)).filter_map(|index| reaper.marker(proj, index)).filter(|marker| marker.is_region);
        let rows = regions.map(|marker| (marker.number, marker.guid, reaper.region_render_tracks(proj, marker.number))).collect();
        Rows { reaper, proj, rows, done: false }
    }

    /// The rows for a render of these regions alone: theirs as they are (the master's, for a master render), the others empty.
    fn pick(&self, regions: &[String], source: Source) {
        let master: Vec<Track> = self.reaper.master_track(self.proj).into_iter().collect();
        for (number, guid, now) in &self.rows {
            let wanted: &[Track] = match (regions.contains(guid), source) {
                (true, Source::Master) => &master,
                (true, _) => continue,
                (false, _) => &[],
            };
            set_row(self.reaper, self.proj, *number, now, wanted);
        }
    }

    /// Every row as noted: whether each reads back so.
    fn put_back(&mut self) -> bool {
        self.done = true;
        let (reaper, proj) = (self.reaper, self.proj);
        let mut ok = true;
        for (number, _, was) in &self.rows {
            let now = reaper.region_render_tracks(proj, *number);
            set_row(reaper, proj, *number, &now, was);
            let back = reaper.region_render_tracks(proj, *number);
            ok &= back.len() == was.len() && was.iter().all(|track| back.contains(track));
        }
        ok
    }

    /// The regions asked for whose row is empty: they render nothing.
    fn empty(&self, regions: &[String]) -> Vec<String> {
        let (reaper, proj) = (self.reaper, self.proj);
        self.rows
            .iter()
            .filter(|(number, guid, _)| regions.contains(guid) && reaper.region_render_tracks(proj, *number).is_empty())
            .filter_map(|(_, guid, _)| reaper.marker_by_guid(proj, guid).map(|marker| marker.name))
            .collect()
    }
}

impl Drop for Rows<'_, '_> {
    fn drop(&mut self) {
        if !self.done {
            let back = self.put_back();
            crate::log::line(&format!(
                "a delivery was cut short; the region render matrix {}",
                if back { "was put back" } else { "wasn't all put back" }
            ));
        }
    }
}

/// What a delivery's files won't hear, in words: soloed tracks (every file holds only what solo lets through), and muted
/// tracks a rendering region's row has.
fn hearing(reaper: &Reaper, proj: Project, spec: &Spec) -> Vec<String> {
    let named = |tracks: &[Track], one: &str, many: &str| {
        let names: Vec<String> = tracks.iter().take(3).map(|track| format!("\u{201c}{}\u{201d}", reaper.track_name(*track))).collect();
        let more = if tracks.len() > 3 { format!(" and {} more", tracks.len() - 3) } else { String::new() };
        format!("{}{more} {}", names.join(", "), if tracks.len() == 1 { one } else { many })
    };
    let mut notes = Vec::new();
    let tracks: Vec<Track> = (0..reaper.track_count(proj)).filter_map(|index| reaper.track_at(proj, index)).collect();
    let soloed: Vec<Track> = tracks.iter().copied().filter(|track| reaper.track_value(*track, "I_SOLO") != 0.0).collect();
    if !soloed.is_empty() {
        notes.push(format!("{}: every file holds only what solo lets through", named(&soloed, "is soloed", "are soloed")));
    }
    if spec.source == Source::Matrix {
        let mut muted: Vec<Track> = Vec::new();
        let regions = (0..reaper.marker_count(proj)).filter_map(|index| reaper.marker(proj, index)).filter(|marker| marker.is_region);
        for region in regions.filter(|region| spec.regions.as_ref().is_none_or(|picked| picked.contains(&region.guid))) {
            for track in reaper.region_render_tracks(proj, region.number) {
                if reaper.track_value(track, "B_MUTE") != 0.0 && !muted.contains(&track) {
                    muted.push(track);
                }
            }
        }
        if !muted.is_empty() {
            notes.push(format!(
                "{}: the files of the regions that render {} won't have {}",
                named(&muted, "is muted", "are muted"),
                if muted.len() == 1 { "it" } else { "them" },
                if muted.len() == 1 { "it" } else { "them" }
            ));
        }
    }
    notes
}

fn set_row(reaper: &Reaper, proj: Project, number: i32, now: &[Track], wanted: &[Track]) {
    for track in now.iter().filter(|track| !wanted.contains(track)) {
        reaper.remove_from_region_render(proj, number, *track);
    }
    for track in wanted.iter().filter(|track| !now.contains(track)) {
        reaper.add_to_region_render(proj, number, *track);
    }
}

/// The settings a delivery renders with, set (the put-back noted them first): the tracks of a stems render are the selection,
/// the rows of the regions picked the matrix's (`rows` noted them first).
fn set(reaper: &Reaper, reads: &Reads, proj: Project, spec: &Spec, rows: Option<&Rows>) -> Result<(), Fault> {
    let number = |key: &str, value: f64| reaper.set_project_number(proj, key, value);
    let source = match spec.source {
        Source::Master if spec.regions.is_none() => 0,
        Source::Master | Source::Matrix => MATRIX_VIA_MASTER,
        Source::Stems => 3,
    };
    let kept = reaper.project_number(proj, "RENDER_SETTINGS").unwrap_or(0.0) as i32 & KEPT_SETTINGS;
    number("RENDER_SETTINGS", (source | kept) as f64);
    let (bounds, tail_bit) = spec.bounds.flags();
    number("RENDER_BOUNDSFLAG", bounds);
    if let Bounds::Range(start, end) = spec.bounds {
        number("RENDER_STARTPOS", start);
        number("RENDER_ENDPOS", end);
    }
    number("RENDER_CHANNELS", spec.channels as f64);
    number("RENDER_SRATE", spec.rate.unwrap_or(0) as f64);
    number("RENDER_TAILFLAG", if spec.tail_ms > 0 { tail_bit as f64 } else { 0. });
    if spec.tail_ms > 0 {
        number("RENDER_TAILMS", spec.tail_ms as f64);
    }
    number("RENDER_ADDTOPROJ", 0.);
    reaper.set_project_text(proj, "RENDER_FILE", &spec.folder.to_string_lossy());
    reaper.set_project_text(proj, "RENDER_PATTERN", &spec.pattern);
    if let Some(format) = &spec.format {
        reaper.set_project_text(proj, "RENDER_FORMAT", format);
    }
    if !spec.dialog {
        number("RENDER_DITHER", 0.);
        number("RENDER_NORMALIZE", render::NO_POSTPROCESSING);
        reaper.set_project_text(proj, "RENDER_FORMAT2", "");
    }
    if let (Some(regions), Some(rows)) = (&spec.regions, rows) {
        rows.pick(regions, spec.source);
    }
    if spec.source == Source::Stems {
        let wanted: Vec<Track> = spec
            .tracks
            .iter()
            .map(|guid| reads.track_by_id(reaper, proj, &TrackId::Guid(guid.clone())).map_err(|_| failed("a stem's track went")))
            .collect::<Result<_, _>>()?;
        for index in 0..reaper.track_count(proj) {
            if let Some(track) = reaper.track_at(proj, index) {
                let on = if wanted.contains(&track) { 1.0 } else { 0.0 };
                if reaper.track_value(track, "I_SELECTED") != on {
                    reaper.set_track_value(track, "I_SELECTED", on);
                }
            }
        }
    }
    Ok(())
}

/// The files REAPER would write with the settings as they are now.
fn targets_now(reaper: &Reaper, proj: Project) -> Vec<String> {
    reaper
        .project_text_long(proj, "RENDER_TARGETS")
        .unwrap_or_default()
        .split(';')
        .map(str::trim)
        .filter(|target| !target.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `deliver_targets`: the files a delivery would write, every setting put back before the answer.
pub fn targets(reaper: &Reaper, reads: &Reads, params: &Value) -> Result<Value, Fault> {
    let info = reaper.current_project().ok_or_else(|| failed("REAPER has no project open"))?;
    let proj = info.ptr;
    let spec = spec(reaper, reads, proj, &info.path, params)?;
    let mut notes: Vec<String> = extras_note(&spec, &extras(reaper, proj)).into_iter().collect();
    let mut put_back = PutBack::note(reaper, proj);
    let mut rows = spec.regions.as_ref().map(|_| Rows::note(reaper, proj));
    let listed = set(reaper, reads, proj, &spec, rows.as_ref()).map(|()| targets_now(reaper, proj));
    if listed.is_ok() {
        notes.extend(hearing(reaper, proj, &spec));
    }
    if let (Some(regions), Some(rows), Source::Matrix) = (&spec.regions, &rows, spec.source) {
        let empty = rows.empty(regions);
        if !empty.is_empty() {
            notes.push(format!(
                "{} of the regions render nothing: no tracks in their row of the region render matrix ({})",
                empty.len(),
                empty.iter().take(5).map(|name| format!("\u{201c}{name}\u{201d}")).collect::<Vec<_>>().join(", ")
            ));
        }
    }
    let rows_back = rows.as_mut().is_none_or(|rows| rows.put_back());
    let mut missed = put_back.now();
    if !rows_back {
        missed.push("the region render matrix".into());
    }
    let listed = listed?;
    let there: Vec<&String> = listed.iter().filter(|target| Path::new(target).exists()).collect();
    let mut answer = json!({"targets": listed, "folder": spec.folder.to_string_lossy(), "count": listed.len()});
    if !there.is_empty() {
        answer["there"] = json!(there);
    }
    if !missed.is_empty() {
        notes.push(format!("Kumi couldn't put back {} as it was", missed.join(", ")));
    }
    if !notes.is_empty() {
        answer["notes"] = json!(notes);
    }
    Ok(answer)
}

/// `deliver_start`: checked and planned now, rendered on a following tick.
pub fn plan(reaper: &Reaper, reads: &Reads, params: &Value) -> Result<Plan, Fault> {
    let job = render::job_id(params)?;
    let info = reaper.current_project().ok_or_else(|| failed("REAPER has no project open"))?;
    let epoch = params.get("epoch").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| reads.epoch());
    if epoch != reads.epoch() && !reads.epoch_is_of(&epoch, info.ptr) {
        return Err(stale("those refs are from an earlier project: read again"));
    }
    let spec = spec(reaper, reads, info.ptr, &info.path, params)?;
    Ok(Plan { job, queued: Instant::now(), proj: info.ptr, epoch, spec })
}

/// Runs a planned delivery: the settings set, the files checked, REAPER's render, the files checked again, everything put back.
pub fn run(reaper: &Reaper, reads: &Reads, plan: &Plan, hooks: &dyn Hooks) -> Ended {
    let mut notes = Vec::new();
    let fail = |code: &str, error: String, notes: Vec<String>| Ended::Failed { code: code.into(), error, notes };
    let Some(info) = reaper.current_project() else { return fail("stale", "REAPER has no project open".into(), notes) };
    if info.ptr != plan.proj || !(reads.epoch() == plan.epoch || reads.epoch_is_of(&plan.epoch, plan.proj)) {
        return fail("stale", "the project in the tab changed before the render began: read again".into(), notes);
    }
    let proj = plan.proj;
    let state = reaper.play_state(proj);
    if state.recording {
        return fail("failed", "REAPER is recording: a render would stop the recording, so Kumi renders nothing".into(), notes);
    }
    if state.playing {
        notes.push("REAPER stopped playback to render".to_string());
    }
    let spec = &plan.spec;
    notes.extend(extras_note(spec, &extras(reaper, proj)));
    let mut put_back = PutBack::note(reaper, proj);
    let mut rows = spec.regions.as_ref().map(|_| Rows::note(reaper, proj));
    let mut outcome: Result<Vec<String>, (String, String)> = set(reaper, reads, proj, spec, rows.as_ref()).map(|()| Vec::new());
    let mut made: Vec<PathBuf> = Vec::new();
    let mut cancelled = false;
    if outcome.is_ok() {
        let listed = targets_now(reaper, proj);
        let there: Vec<&String> = listed.iter().filter(|target| Path::new(target).exists()).collect();
        outcome = if listed.is_empty() {
            Err((
                "failed".into(),
                "REAPER would write no files with these settings (are there regions to render?): nothing was rendered".into(),
            ))
        } else if listed.len() > MAX_FILES {
            Err(("failed".into(), format!("REAPER would write {} files, more than Kumi renders at once ({MAX_FILES})", listed.len())))
        } else if spec.expect.as_ref().is_some_and(|expect| sorted(expect) != sorted(&listed)) {
            let expect = spec.expect.clone().unwrap_or_default();
            let extra: Vec<&String> = listed.iter().filter(|t| !expect.contains(t)).take(5).collect();
            let missing: Vec<&String> = expect.iter().filter(|t| !listed.contains(t)).take(5).collect();
            Err((
                "changed".into(),
                format!(
                    "REAPER would write other files than the dry run listed ({} now, {} then; new {:?}, gone {:?}): nothing was rendered, check them again",
                    listed.len(),
                    expect.len(),
                    extra,
                    missing
                ),
            ))
        } else if !there.is_empty() && !spec.overwrite {
            Err((
                "failed".into(),
                format!(
                    "{} of the files are there already ({}…): nothing was rendered; overwrite: true writes over them",
                    there.len(),
                    there[0]
                ),
            ))
        } else {
            Ok(listed)
        };
    }
    if let Ok(listed) = &outcome {
        // (the files that weren't there are this render's: those a failed or cancelled render leaves are deleted)
        made = listed.iter().map(PathBuf::from).filter(|file| !file.exists()).collect();
        let folders: Vec<PathBuf> = listed.iter().filter_map(|target| Path::new(target).parent().map(Path::to_path_buf)).collect();
        if let Some(folder) = folders.iter().find(|folder| std::fs::create_dir_all(folder).is_err()) {
            outcome = Err(("failed".into(), format!("Kumi couldn't make the folder {}", folder.display())));
        }
    }
    if let Ok(listed) = outcome.clone() {
        let started = Instant::now();
        hooks.render(&mut || {
            reaper.run_action(RENDER_WITH_LAST);
        });
        crate::log::line(&format!("delivery {} took {:.3} s", plan.job, started.elapsed().as_secs_f64()));
        if hooks.cancelled() {
            cancelled = true;
        } else if let Some(missing) = listed.iter().find(|file| std::fs::metadata(file).map_or(true, |meta| meta.len() <= 44)) {
            outcome = Err(("failed".into(), format!("REAPER's render didn't write {missing}")));
        }
    }
    let rows_back = rows.as_mut().is_none_or(|rows| rows.put_back());
    let mut missed = put_back.now();
    if !rows_back {
        missed.push("the region render matrix".into());
    }
    if !missed.is_empty() {
        notes.push(format!("Kumi couldn't put back {} as it was", missed.join(", ")));
    }
    let clean = |made: &[PathBuf]| {
        for file in made {
            let _ = std::fs::remove_file(file);
        }
    };
    if cancelled {
        clean(&made);
        return Ended::Cancelled { notes };
    }
    match outcome {
        Err((code, error)) => {
            clean(&made);
            fail(&code, error, notes)
        }
        Ok(listed) => {
            let files: Vec<Value> = listed
                .iter()
                .map(|file| {
                    let bytes = std::fs::metadata(file).map(|meta| meta.len()).unwrap_or(0);
                    let mut row = json!({"file": file, "bytes": bytes});
                    if let Ok(info) = super::wav::read_info(Path::new(file)) {
                        row["seconds"] = json!((info.frames as f64 / info.rate.max(1) as f64 * 1000.).round() / 1000.);
                    }
                    row
                })
                .collect();
            Ended::Done { files: Value::Array(files), notes }
        }
    }
}

fn sorted(list: &[String]) -> Vec<&String> {
    let mut out: Vec<&String> = list.iter().collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reaper::fake;

    #[test]
    fn the_matrix_rows_come_back_when_a_delivery_is_cut_short() {
        fake::lab_project();
        fake::with(|s| {
            s.render_matrix.insert(1, vec!["{BBBB}".into()]);
        });
        let api = fake::api();
        let reaper = Reaper::new(&api);
        let proj = reaper.current_project().unwrap().ptr;
        {
            let rows = Rows::note(&reaper, proj);
            rows.pick(&["{ANOTHER}".to_string()], Source::Matrix);
            assert!(reaper.region_render_tracks(proj, 1).is_empty(), "emptied for the render");
            // (dropped without put_back: a panic's way out)
        }
        assert_eq!(reaper.region_render_tracks(proj, 1).len(), 1, "the row is back");
    }
}
