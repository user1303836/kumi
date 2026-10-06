//! Live's own conversions to MIDI (melody, harmony, drums) through its API, Live.Conversions, rather than its menus:
//! any audio clip, Session or Arrangement, with no Accessibility access, on Windows too. Live converts in the
//! background and lands a new MIDI track next to the clip's, so Kumi waits for that track, saying so, and brings Live
//! forward when it hangs back (a conversion asked of Live in the background waits for its window, 12.4.15b5).
use super::{args, delay, CommandError, CommandTools};
use crate::core::contracts::{ChangeRecord, JsonObject, ToolResult};
use crate::integrations::ableton::views::ViewHost;
use kumi_common::{abort::Signal, js::json::stringify};
use serde_json::{json, Value};
use std::time::Duration;
// Tokio's clock, which a test can pause and run ahead.
use tokio::time::Instant;

/// The conversions, by `live_command`'s command: Live.Conversions.AudioToMidiType's member, and what Live calls it.
const CONVERSIONS: [(&str, &str, &str); 3] = [
    ("convert_melody_to_midi", "melody_to_midi", "Melody to MIDI"),
    ("convert_harmony_to_midi", "harmony_to_midi", "Harmony to MIDI"),
    ("convert_drums_to_midi", "drums_to_midi", "Drums to MIDI"),
];
/// How long Live has before Kumi brings its window forward, and before Kumi stops waiting for the new track.
const FRONT_AFTER: Duration = Duration::from_secs(3);
const GIVE_UP_AFTER: Duration = Duration::from_secs(120);

/// The conversion a `live_command` command names, if it's one: its API type and its name.
pub(super) fn conversion(command: &str) -> Option<(&'static str, &'static str)> {
    CONVERSIONS.iter().find(|(name, _, _)| *name == command).map(|(_, kind, title)| (*kind, *title))
}

/// The Python Kumi runs in Live: the clip (`obj`, or the one selected in Live), checked, then converted.
fn script(kind: &str, given: bool) -> String {
    format!(
        r#"clip = obj if {given} else song.view.detail_clip
if clip is None:
    raise ValueError("that clip isn't in Live any more: discover it again" if {given} else "no clip is selected in Live")
if not getattr(clip, "is_audio_clip", False):
    raise ValueError("that's a MIDI clip: only an audio clip converts to MIDI")
conversions = getattr(Live, "Conversions", None)
if conversions is None or not hasattr(conversions, "audio_to_midi_clip"):
    raise LookupError("this Live has no conversions API")
if not conversions.is_convertible_to_midi(song, clip):
    raise ValueError("Live can't convert that clip to MIDI")
result = {{"name": clip.name}}
conversions.audio_to_midi_clip(song, clip, conversions.AudioToMidiType.{kind})
"#,
        given = if given { "True" } else { "False" }
    )
}

impl CommandTools {
    /// A conversion to MIDI through Live's API; None when this Live has no API for it (the menus do it then).
    pub(super) async fn convert(
        &self,
        input: &JsonObject,
        (kind, title): (&str, &str),
        signal: &Signal,
    ) -> Result<Option<ToolResult>, CommandError> {
        let Some(clip) = input.get("clip").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
            return Ok(Some(ToolResult::error(format!(
                "{title} works on an audio clip: give clip (its clipRef from this turn, Session or Arrangement, or \"selected\" for the one selected in Live)."
            ))));
        };
        let given = clip != "selected";
        let mut call = args(json!({"code":script(kind, given),"mode":"exec","timeoutMs":10000}));
        if given {
            call.insert("ref".into(), json!(self.lengthen(clip, "clipRef")));
        }
        let before = self.track_names(signal).await?;
        let ran = self.connection.call("live_run_python", call, signal.clone()).await.map_err(|e| CommandError::Other(e.to_string()))?;
        let body = (ran.is_error != Some(true)).then(|| super::payload(&ran).ok()).flatten();
        let Some(body) = body.clone().filter(|body| body.get("ok") == Some(&Value::Bool(true))) else {
            let why = body
                .as_ref()
                .and_then(|body| body.get("error"))
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| super::result_text(&ran));
            if why.contains("no conversions API") {
                return Ok(None);
            }
            return Ok(Some(ToolResult::error(format!("Kumi couldn't start {title}: {}", kumi_common::js::string::head(&why, 300)))));
        };
        let name = body.get("result").and_then(|r| r.get("name")).and_then(Value::as_str).unwrap_or("").to_owned();
        self.tell(format!("{title}: converting “{name}”"));
        let started = Instant::now();
        let mut fronted = false;
        let landed = loop {
            delay(300, signal).await?;
            // A track more: Live's new one, named for the conversion ("4-Melody to MIDI"). Names alone won't do, since
            // Live renumbers the auto-named tracks after it ("4-Audio" becomes "5-Audio", 12.4.15b5).
            let now = self.track_names(signal).await?;
            if now.len() > before.len() {
                let added: Vec<String> = now
                    .iter()
                    .filter(|track| now.iter().filter(|n| n == track).count() > before.iter().filter(|n| n == track).count())
                    .cloned()
                    .collect();
                break added.iter().find(|track| track.contains(title)).or(added.first()).cloned().unwrap_or_else(|| title.to_owned());
            }
            if !fronted && started.elapsed() >= FRONT_AFTER {
                // The producer asked Live for this, so Live coming forward to do it is expected.
                fronted = true;
                let forward = match &self.options.front_live {
                    Some(front) => front().await,
                    None => bring_live_forward().await,
                };
                self.tell(if forward {
                    format!("{title}: Live converts in front, so Kumi brought its window forward")
                } else {
                    format!("{title}: Live may be waiting to be in front: click its window")
                });
            }
            if started.elapsed() >= GIVE_UP_AFTER {
                return Ok(Some(ToolResult::error(format!(
                    "Live hasn't finished {title} on “{name}” after two minutes; its new track may still land. Look for it in Live, then discover again."
                ))));
            }
        };
        {
            // A track came in next to the clip's: the tracks after it moved along.
            let mut refs = self.connection.references.borrow_mut();
            refs.invalidate();
            refs.clear_names();
        }
        self.connection.lease.set(self.connection.lease.get() + 1);
        let done = format!("{title}: new track “{landed}”");
        self.tell(&done);
        let record: ChangeRecord = serde_json::from_value(json!({"id":format!("l{}",&uuid::Uuid::new_v4().to_string()[..8]),"family":"structure","title":done,"state":"kept","note":"Done with Live's own conversion: Live's undo (Cmd-Z) takes it back.","at":self.connection.now().timestamp_millis()})).unwrap();
        self.history.emit(&record);
        Ok(Some(ToolResult::text(stringify(&json!({
            "converted": title,
            "from": name,
            "newTracks": [landed],
            "note": "Tracks after the new one moved along: discover again before using earlier track references."
        })))))
    }
}

/// Bring Live's window forward: the Live that's open, by its app (macOS); false where Kumi can't.
async fn bring_live_forward() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    let Ok(listed) = tokio::process::Command::new("ps").args(["-axo", "comm="]).output().await else { return false };
    let listed = String::from_utf8_lossy(&listed.stdout);
    let Some(app) = listed.lines().find_map(|line| line.trim().strip_suffix("/Contents/MacOS/Live").filter(|app| app.ends_with(".app")))
    else {
        return false;
    };
    tokio::process::Command::new("open").args(["-a", app]).status().await.is_ok_and(|status| status.success())
}
