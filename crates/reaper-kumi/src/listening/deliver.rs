//! Renders for delivery (#401, the render pipeline): the producer's own files, named by REAPER's wildcards, where they belong.
//!
//! `deliver_targets` sets the render settings asked for, reads the files REAPER would write (`RENDER_TARGETS`), and puts every
//! setting back, all in the one call. `deliver_start` plans the same render as a job; on a following tick it sets the settings,
//! checks REAPER would write exactly the files the dry run showed (and none of them is there yet, unless asked to write over
//! them), renders with REAPER's own render (42230), checks each file was written, and puts every setting back however it ends.
//! Files a failed or cancelled render left are deleted, never one that was there before.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{json, Value};

use super::render::{self, Ended, Hooks, PutBack, RENDER_WITH_LAST};
use super::{bad, failed, stale, Fault};
use crate::read::Reads;
use crate::reaper::{Project, Reaper, Track};
use crate::refs::TrackId;

/// The most files one delivery writes.
pub const MAX_FILES: usize = 2000;
/// The longest tail asked for (ms).
pub const MAX_TAIL_MS: u64 = 30_000;

/// What's rendered: the master mix, each region's tracks in the region render matrix, or the tracks named as stems.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Master,
    Matrix,
    Stems,
}

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
    })
}

/// The settings a delivery renders with, set (the put-back noted them first): the tracks of a stems render are the selection.
fn set(reaper: &Reaper, reads: &Reads, proj: Project, spec: &Spec) -> Result<(), Fault> {
    let number = |key: &str, value: f64| reaper.set_project_number(proj, key, value);
    let settings = match spec.source {
        Source::Master => 0.,
        Source::Matrix => 8.,
        Source::Stems => 3.,
    };
    number("RENDER_SETTINGS", settings);
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
    let spec = spec(reaper, reads, info.ptr, &info.path, params)?;
    let mut put_back = PutBack::note(reaper, info.ptr);
    let listed = set(reaper, reads, info.ptr, &spec).map(|()| targets_now(reaper, info.ptr));
    let missed = put_back.now();
    let listed = listed?;
    let there: Vec<&String> = listed.iter().filter(|target| Path::new(target).exists()).collect();
    let mut answer = json!({"targets": listed, "folder": spec.folder.to_string_lossy(), "count": listed.len()});
    if !there.is_empty() {
        answer["there"] = json!(there);
    }
    if !missed.is_empty() {
        answer["notes"] = json!([format!("Kumi couldn't put back {} as it was", missed.join(", "))]);
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
    let mut put_back = PutBack::note(reaper, proj);
    let mut outcome: Result<Vec<String>, (String, String)> = set(reaper, reads, proj, spec).map(|()| Vec::new());
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
    let missed = put_back.now();
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
