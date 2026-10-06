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
/// How long Live has before Kumi brings its window forward (a fallback: it converts in the background, 12.4.15b5), and
/// before Kumi stops waiting for the new track.
const FRONT_AFTER: Duration = Duration::from_secs(12);
const GIVE_UP_AFTER: Duration = Duration::from_secs(120);
/// How long a new track that isn't where or as Live names a conversion's is waited past before it's taken for it.
const UNNAMED_AFTER: Duration = Duration::from_secs(5);

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
owner = getattr(clip, "canonical_parent", None)
track = owner if hasattr(owner, "is_frozen") else getattr(owner, "canonical_parent", None)
if getattr(track, "is_frozen", False):
    raise ValueError("its track is frozen: unfreeze it, then ask again")
conversions = getattr(Live, "Conversions", None)
if conversions is None or not hasattr(conversions, "audio_to_midi_clip"):
    raise LookupError("this Live has no conversions API")
if not conversions.is_convertible_to_midi(song, clip):
    raise ValueError("Live says it can't convert that clip to MIDI")
result = {{"name": clip.name, "track": bridge._capture_object_identity(track) if track is not None else None}}
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
        let before = self.track_identities(signal).await?;
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
            // No API, or no Python in Live to call it with: the menus do it.
            if ["no conversions API", "Python module is unavailable", "python.run is unavailable"].iter().any(|said| why.contains(said)) {
                return Ok(None);
            }
            if ["stale", "reference is invalid", "unknown reference"].iter().any(|said| why.contains(said)) {
                return Ok(Some(ToolResult::error("That clip isn't in Live any more: discover it again, then ask again.")));
            }
            return Ok(Some(ToolResult::error(format!("Live didn't start {title}: {}", kumi_common::js::string::head(&why, 300)))));
        };
        let name = body.get("result").and_then(|r| r.get("name")).and_then(Value::as_str).unwrap_or("").to_owned();
        let source = body.get("result").and_then(|r| r.get("track")).and_then(Value::as_str).map(str::to_owned);
        self.tell(format!("{title}: converting “{name}”"));
        let landed = self.new_track(&before, source.as_deref(), title, signal).await;
        {
            // Once Live is converting, a track may land next to the clip's whatever happens here: the tracks after it
            // move along, so references retire on every way out.
            let mut refs = self.connection.references.borrow_mut();
            refs.invalidate();
            refs.clear_names();
        }
        self.connection.lease.set(self.connection.lease.get() + 1);
        let Some(landed) = landed? else {
            return Ok(Some(ToolResult::error(format!(
                "Live hasn't finished {title} on “{name}” after two minutes; its new track may still land. Look for it in Live, then discover again."
            ))));
        };
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
    /// The Set's tracks, each by its identity, with its name.
    async fn track_identities(&self, signal: &Signal) -> Result<Vec<(String, String)>, CommandError> {
        let rows = self.connection.rows("track", args(json!({"fields":["name","objectIdentity"]})), signal.clone()).await?;
        Ok(rows
            .iter()
            .filter_map(|row| {
                Some((row.get("objectIdentity")?.as_str()?.to_owned(), row.get("name").and_then(Value::as_str).unwrap_or("").to_owned()))
            })
            .collect())
    }
    /// The track Live's conversion lands, told by identity: a new one right after the clip's track (`source`), where
    /// Live puts it; else one named for the conversion; else, after a few seconds, any new one (a Live in another
    /// language names it otherwise). A track the producer adds meanwhile isn't taken for it. A read that fails is
    /// tried again, never taken as the conversion failing (Live is converting all the same). None after two minutes;
    /// Live is brought forward on the way, in case it waits for its window.
    async fn new_track(
        &self,
        before: &[(String, String)],
        source: Option<&str>,
        title: &str,
        signal: &Signal,
    ) -> Result<Option<String>, CommandError> {
        let started = Instant::now();
        let mut fronted = false;
        let mut unnamed_since = None;
        loop {
            delay(300, signal).await?;
            if let Ok(now) = self.track_identities(signal).await {
                let is_new = |identity: &str| !before.iter().any(|(known, _)| known == identity);
                let after_source = source
                    .and_then(|source| now.iter().position(|(identity, _)| identity == source))
                    .and_then(|at| now.get(at + 1))
                    .filter(|(identity, _)| is_new(identity));
                if let Some((_, name)) = after_source {
                    return Ok(Some(name.clone()));
                }
                let new: Vec<&String> = now.iter().filter(|(identity, _)| is_new(identity)).map(|(_, name)| name).collect();
                if let Some(name) = new.iter().find(|name| name.contains(title)) {
                    return Ok(Some((*name).clone()));
                }
                if let Some(name) = new.first() {
                    let since = *unnamed_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= UNNAMED_AFTER {
                        return Ok(Some((*name).clone()));
                    }
                }
            }
            if !fronted && started.elapsed() >= FRONT_AFTER {
                fronted = true;
                let forward = match &self.options.front_live {
                    Some(front) => front().await,
                    None => bring_live_forward().await,
                };
                self.tell(if forward {
                    format!("{title}: Live hasn't finished yet, so Kumi brought its window forward in case it's waiting for it")
                } else {
                    format!("{title}: Live hasn't finished yet; if it's waiting, clicking its window may help")
                });
            }
            if started.elapsed() >= GIVE_UP_AFTER {
                return Ok(None);
            }
        }
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
