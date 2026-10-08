//! Devices probed: what a knob does to a measure across its range, heard once and saved, so the next time that knob
//! is turned toward that measure Kumi starts where the response says the target lies instead of feeling its way.
//! Saved per device kind (Live's own devices by class, plug-ins by name), knob, measure and what was heard (a track,
//! or the mix): a knob does different things to different sounds. A response unheard for a month isn't used.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One knob's response: what a measure read at each setting, the knob in its perceptual units (dB, octaves,
/// log-time), sorted by the knob.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub knob: String,
    /// The checklist item's id it was read for ("decay time", "loudness", "balance low mids"…).
    pub measure: String,
    pub points: Vec<(f64, f64)>,
    /// When it was last heard, Unix seconds.
    pub at: u64,
    /// What it was heard on: a track's name, or "mix".
    #[serde(default)]
    pub scope: String,
}

/// How long a response is used: a month, in seconds.
const KEPT_FOR: u64 = 30 * 24 * 3600;

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|since| since.as_secs()).unwrap_or(0)
}

impl Response {
    /// Where the knob should go to reach `aim`, given what it reads now (`now`: the knob and its reading): the saved
    /// curve moved by however far the sound now sits from it, read between its neighbouring points. Beyond what the
    /// knob ever reached, the end that comes closest. None with fewer than two points.
    pub fn predict(&self, now: (f64, f64), aim: f64) -> Option<f64> {
        let points = &self.points;
        if points.len() < 2 {
            return None;
        }
        let shift = now.1 - self.at_knob(now.0)?;
        let wanted = aim - shift;
        // Every crossing of the wanted value, the one nearest where the knob is.
        let crossing = points
            .windows(2)
            .filter_map(|pair| {
                let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
                let (low, high) = (y0.min(y1), y0.max(y1));
                if !(low..=high).contains(&wanted) {
                    return None;
                }
                Some(if (y1 - y0).abs() < 1e-12 { (x0 + x1) / 2. } else { x0 + (wanted - y0) * (x1 - x0) / (y1 - y0) })
            })
            .min_by(|a, b| (a - now.0).abs().total_cmp(&(b - now.0).abs()));
        crossing.or_else(|| points.iter().min_by(|a, b| (a.1 - wanted).abs().total_cmp(&(b.1 - wanted).abs())).map(|point| point.0))
    }

    /// The reading at a knob setting, along the saved curve (its ends held beyond them).
    pub fn at_knob(&self, knob: f64) -> Option<f64> {
        let points = &self.points;
        let first = points.first()?;
        let last = points.last()?;
        if knob <= first.0 {
            return Some(first.1);
        }
        if knob >= last.0 {
            return Some(last.1);
        }
        points.windows(2).find(|pair| knob >= pair[0].0 && knob <= pair[1].0).map(|pair| {
            let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
            if (x1 - x0).abs() < 1e-12 {
                y0
            } else {
                y0 + (knob - x0) * (y1 - y0) / (x1 - x0)
            }
        })
    }
}

/// The saved responses, a file per device kind.
#[derive(Debug, Clone)]
pub struct Probes {
    dir: PathBuf,
}

impl Probes {
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Kumi's own: under KUMI_HOME (or ~/.kumi), in probes.
    pub fn home() -> Self {
        let home = std::env::var("KUMI_HOME")
            .ok()
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home::home_dir().unwrap_or_default().join(".kumi"));
        Self::at(home.join("probes"))
    }

    fn file(&self, device: &str) -> PathBuf {
        let name: String =
            device.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '_' }).collect();
        self.dir.join(format!("{}.json", name.trim_matches('_')))
    }

    /// A device kind's saved responses: none when there's no file yet, and an error when the file is there but can't
    /// be read (a newer Kumi's, say), so it's never written over.
    fn all(&self, device: &str) -> std::io::Result<Vec<Response>> {
        match std::fs::read(self.file(device)) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
            Err(error) => Err(error),
        }
    }

    /// A knob's saved response for a measure, heard on `scope` (a track's name, or "mix") within the month.
    pub fn load(&self, device: &str, knob: &str, measure: &str, scope: &str) -> Option<Response> {
        self.all(device).ok()?.into_iter().find(|response| {
            response.knob.eq_ignore_ascii_case(knob)
                && response.measure == measure
                && response.scope == scope
                && response.at + KEPT_FOR >= now()
        })
    }

    /// Adds what was heard (knob, reading) on `scope` to a knob's response for a measure: a setting heard again (within
    /// `close`, a just-noticeable step of the knob) takes the newer reading. With `close` None (a probe, the knob
    /// heard across its range), what was heard replaces the response.
    pub fn add(
        &self,
        device: &str,
        knob: &str,
        measure: &str,
        scope: &str,
        heard: &[(f64, f64)],
        close: Option<f64>,
    ) -> std::io::Result<()> {
        let heard: Vec<(f64, f64)> = heard.iter().copied().filter(|(x, y)| x.is_finite() && y.is_finite()).collect();
        if heard.is_empty() {
            return Ok(());
        }
        let mut all = self.all(device)?;
        let index = match all
            .iter()
            .position(|response| response.knob.eq_ignore_ascii_case(knob) && response.measure == measure && response.scope == scope)
        {
            Some(index) => index,
            None => {
                all.push(Response { knob: knob.into(), measure: measure.into(), points: vec![], at: 0, scope: scope.into() });
                all.len() - 1
            }
        };
        let response = &mut all[index];
        // An old response is heard afresh rather than added to.
        match close {
            Some(close) if response.at + KEPT_FOR >= now() => {
                response.points.retain(|(x, _)| heard.iter().all(|(new, _)| (new - x).abs() > close.abs()))
            }
            _ => response.points.clear(),
        }
        response.points.extend(heard);
        response.points.sort_by(|a, b| a.0.total_cmp(&b.0));
        response.at = now();
        write(&self.file(device), &all)
    }
}

/// Written whole through a temporary file of this write's own, so a reader never sees half of it.
fn write(file: &Path, all: &[Response]) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let partial = file.with_extension(format!("json.partial-{}-{}", std::process::id(), &tag[..8]));
    std::fs::write(&partial, serde_json::to_vec_pretty(all).map_err(std::io::Error::other)?)?;
    std::fs::rename(&partial, file).inspect_err(|_| {
        let _ = std::fs::remove_file(&partial);
    })
}
