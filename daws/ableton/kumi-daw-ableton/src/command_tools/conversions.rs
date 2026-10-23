//! Live's own conversions to MIDI (melody, harmony, drums) through its API, Live.Conversions, rather than its menus:
//! any audio clip, Session or Arrangement, with no Accessibility access, on Windows too. Live converts in the
//! background and lands a new MIDI track right after the clip's, so Kumi waits for that track, saying so, and brings
//! Live forward when it hangs back (a fallback: 12.4.15b5 converted in the background every time it was probed).
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
/// How long a new MIDI track that isn't named as Live names a conversion's stays before it's taken for it: Live's in
/// another language, or a producer's.
const UNNAMED_AFTER: Duration = Duration::from_secs(5);
/// When the reads of the tracks slow down, from every 300 ms to every second.
const SLOWER_AFTER: Duration = Duration::from_secs(10);
/// What the script prints just before it asks Live to convert: past it, Live may be converting whatever the answer says.
const ASKED: &str = "kumi: asked Live to convert";

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
print("{ASKED}")
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
        // A bridge without Python: the menus do it.
        if !self.connection.has("live_run_python") {
            return Ok(None);
        }
        let given = clip != "selected";
        let mut call = args(json!({"code":script(kind, given),"mode":"exec","timeoutMs":10000}));
        if given {
            call.insert("ref".into(), json!(self.lengthen(clip, "clipRef")));
        }
        let before = self.track_identities(signal).await?;
        let ran = match self.connection.call("live_run_python", call, signal.clone()).await {
            Ok(ran) => ran,
            Err(error) => {
                // Live may have taken the call and started converting before its answer was lost.
                let unsure = self.unsure(title, &error.to_string());
                if signal.is_cancelled() {
                    return Err(CommandError::Other("Operation cancelled".into()));
                }
                return Ok(Some(unsure));
            }
        };
        let reply = if ran.is_error == Some(true) { None } else { super::payload(&ran).ok() };
        let Some(body) = reply.as_ref().filter(|body| body.get("ok") == Some(&Value::Bool(true))) else {
            let error = reply.as_ref().and_then(|body| body.get("error"));
            let why =
                error.and_then(|error| error.get("message")).and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| reason(&ran));
            let asked = reply.as_ref().and_then(|body| body.get("stdout")).and_then(Value::as_str).is_some_and(|out| out.contains(ASKED));
            // No API, or no Python in Live to call it with: the menus do it (never once Live was asked).
            let unavailable = ["no conversions API", "Python execution is unavailable", "Python module is unavailable"];
            if !asked && unavailable.iter().any(|said| why.contains(said)) {
                return Ok(None);
            }
            // The bridge's own words for a ref it no longer holds (a KeyError before the script runs).
            let gone = ["stale", "reference is invalid", "unknown reference"].iter().any(|said| why.contains(said))
                || error.and_then(|error| error.get("type")) == Some(&json!("KeyError"));
            if gone && !asked {
                return Ok(Some(ToolResult::error("That clip isn't in Live any more: discover it again, then ask again.")));
            }
            let why = kumi_common::js::string::head(&why, 300);
            // The script stopped before asking Live (its checks, or Live refusing the clip): nothing started.
            if reply.is_some() && !asked {
                return Ok(Some(ToolResult::error(format!("Live didn't start {title}: {why}"))));
            }
            // Past asking Live, or the bridge failing on the way: Live may be converting all the same.
            return Ok(Some(self.unsure(title, &why)));
        };
        let name = body.get("result").and_then(|r| r.get("name")).and_then(Value::as_str).unwrap_or("").to_owned();
        let source = body.get("result").and_then(|r| r.get("track")).and_then(Value::as_str).map(str::to_owned);
        self.tell(format!("{title}: converting “{name}”"));
        let landed = self.new_track(&before, source.as_deref(), title, signal).await;
        // Once Live is converting, a track may land next to the clip's whatever happens here.
        self.retire_references();
        let Some(landed) = landed? else {
            return Ok(Some(may_still_land(title, Some(&name), format!("Kumi hasn't seen {title} on “{name}” land after two minutes"))));
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
    /// Live's answer lost, or an error once the script had asked Live to convert: Live may be converting all the same,
    /// so the turn's references retire and it isn't said as a failure.
    fn unsure(&self, title: &str, why: &str) -> ToolResult {
        self.retire_references();
        may_still_land(title, None, format!("Kumi can't tell whether Live started {title} ({why})"))
    }
    /// A new track moves the ones after it along: the turn's references retire.
    fn retire_references(&self) {
        let mut refs = self.connection.references.borrow_mut();
        refs.invalidate();
        refs.clear_names();
        drop(refs);
        self.connection.lease.set(self.connection.lease.get() + 1);
    }
    /// The Set's tracks, each by its identity, with its name and whether it's a MIDI track.
    async fn track_identities(&self, signal: &Signal) -> Result<Vec<(String, String, bool)>, CommandError> {
        let rows = self.connection.rows("track", args(json!({"fields":["name","objectIdentity","mediaKind"]})), signal.clone()).await?;
        Ok(rows
            .iter()
            .filter_map(|row| {
                Some((
                    row.get("objectIdentity")?.as_str()?.to_owned(),
                    row.get("name").and_then(Value::as_str).unwrap_or("").to_owned(),
                    row.get("mediaKind").and_then(Value::as_str) == Some("midi"),
                ))
            })
            .collect())
    }
    /// The track Live's conversion lands, told by identity among the tracks that weren't there before:
    /// - one named for the conversion, as Live names it ("4-Melody to MIDI"), at once;
    /// - else, knowing the clip's track (`source`), the MIDI track right after it, where Live puts it, once it's been
    ///   there a few seconds: a Live in another language names its track otherwise, and a producer's new track lands
    ///   there too (Cmd-Shift-T with the clip's track selected);
    /// - else, not knowing it, a MIDI track that's been there a few seconds.
    ///
    /// A read that fails is tried again, never taken as the conversion failing (Live is converting all the same).
    /// None after two minutes; Live is brought forward on the way, in case it waits for its window. Reads come every
    /// 300 ms, then every second after the first ten.
    async fn new_track(
        &self,
        before: &[(String, String, bool)],
        source: Option<&str>,
        title: &str,
        signal: &Signal,
    ) -> Result<Option<String>, CommandError> {
        let started = Instant::now();
        let mut fronted = false;
        let mut unnamed_since = None;
        loop {
            delay(if started.elapsed() < SLOWER_AFTER { 300 } else { 1000 }, signal).await?;
            if let Ok(now) = self.track_identities(signal).await {
                let is_new = |identity: &str| !before.iter().any(|(known, _, _)| known == identity);
                if let Some((_, name, _)) = now.iter().find(|(identity, name, _)| is_new(identity) && name.contains(title)) {
                    return Ok(Some(name.clone()));
                }
                let unnamed = match source {
                    Some(source) => now
                        .iter()
                        .position(|(identity, _, _)| identity == source)
                        .and_then(|at| now.get(at + 1))
                        .filter(|(identity, _, midi)| *midi && is_new(identity)),
                    None => now.iter().find(|(identity, _, midi)| *midi && is_new(identity)),
                };
                // The same unnamed track for a few seconds, by its identity.
                match unnamed {
                    Some((identity, name, _)) => {
                        if unnamed_since.as_ref().is_none_or(|(seen, _)| seen != identity) {
                            unnamed_since = Some((identity.clone(), Instant::now()));
                        }
                        if unnamed_since.as_ref().is_some_and(|(_, since)| since.elapsed() >= UNNAMED_AFTER) {
                            return Ok(Some(name.clone()));
                        }
                    }
                    None => unnamed_since = None,
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

/// The reason in a bridge error ({"reason", "remediation"}), or its whole text.
fn reason(result: &crate::mcp::types::CallToolResult) -> String {
    let text = super::result_text(result);
    serde_json::from_str::<Value>(&text).ok().and_then(|error| Some(error.get("reason")?.as_str()?.to_owned())).unwrap_or(text)
}

/// What Kumi says when it can't tell whether a conversion's new track will land: Live may still be converting, so it
/// isn't a failure, and asking again could make a second track.
fn may_still_land(title: &str, from: Option<&str>, what: String) -> ToolResult {
    let mut said = json!({
        "converting": title,
        "landed": false,
        "note": format!("{what}, and Live may still be converting. Don't ask again (that could make a second track): look in Live for a new MIDI track next to the clip's in a moment, then discover again.")
    });
    if let Some(from) = from {
        said["from"] = json!(from);
    }
    ToolResult::text(stringify(&said))
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
