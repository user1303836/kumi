//! One approved Arrangement window through preinstalled Ears. No setup, recording-track
//! fallback, preroll, second attempt, shared-cache sweep or implicit audition.
use super::ears::{object_identity, RawFile};
use super::rig::{Rig, RigEars, Source, Transport, Window};
use super::*;
use crate::context::payload;
use crate::ears::{
    capture::{cover, frame_at, read_capture, runs, write_capture_wav, Anchors},
    device::{EARS_ITEM, EARS_VERSION},
    link::{open_ears_link, EarsOptions, Tap},
};
use kumi_types::errors::HearFailure;
use std::path::Path;
use tokio::time::Instant;

const MAX_AUDIO_SECONDS: f64 = 65.0;
const MAX_WAV_BYTES: u64 = 50 * 1024 * 1024;
const CALL_DEADLINE_SECONDS: u64 = 300;

#[derive(Debug, PartialEq)]
struct CaptureState {
    identity: String,
    epoch: f64,
    tempo: f64,
    transport: Value,
    song: Value,
    tracks: Vec<Value>,
    selection: Value,
    devices: Vec<String>,
}

enum CaptureTransport {
    Seek { position: f64, looped: bool },
    Continue,
    Stop,
}

struct Ready {
    state: CaptureState,
    target: String,
    label: String,
    window: Window,
    seconds: f64,
}

fn refused(code: &'static str, reason: impl Into<String>) -> RuntimeError {
    HearFailure::Refused { code, reason: reason.into() }.into()
}

fn unsure() -> RuntimeError {
    HearFailure::Unsure { reason: "Live's capture did not verify restoration. Observe again and check the transport and Kumi Ears device before another action.".into() }.into()
}

fn unreadable() -> RuntimeError {
    refused("unsupported", "This Live bridge cannot read every state needed to verify a bounded capture. Update its bridge and restart Live.")
}

fn number(row: &JsonObject, key: &str) -> Result<f64, RuntimeError> {
    row.get(key).and_then(Value::as_f64).filter(|n| n.is_finite()).ok_or_else(unreadable)
}

fn flag(row: &JsonObject, key: &str) -> Result<bool, RuntimeError> {
    row.get(key).and_then(Value::as_bool).ok_or_else(unreadable)
}

fn only(mut rows: Vec<JsonObject>) -> Result<JsonObject, RuntimeError> {
    if rows.len() != 1 { return Err(unreadable()); }
    Ok(rows.pop().expect("one row"))
}

fn window(request: &HearRequest) -> Result<Window, RuntimeError> {
    let from = request.from_beat.filter(|n| n.is_finite() && *n >= 0.0);
    let beats = request.beats.filter(|n| n.is_finite() && *n > 0.0 && *n <= 256.0);
    let source_ok = if request.mix == Some(true) { request.tracks.is_empty() } else { request.tracks.len() == 1 };
    match (from, beats) {
        (Some(from), Some(beats)) if source_ok && (from + beats).is_finite() && from + beats > from && request.seconds.is_none() && request.whole != Some(true) => {
            Ok(Window { from, beats })
        }
        _ => Err(refused("invalid_window", "Give one fresh track ref or the mix, from_beat and beats (at most 256); there is no implicit range.")),
    }
}

impl LiveRenderer {
    /// The same pure preflight is used by the approval preview and again under the write lease.
    pub(super) async fn preview_restricted_hear(&self, request: &HearRequest, signal: Signal) -> Result<HearPreview, RuntimeError> {
        let ready = self.restricted_preflight(request, signal).await?;
        Ok(HearPreview {
            label: ready.label, seconds: ready.seconds, method: HearMethod::Ears, deadline_seconds: CALL_DEADLINE_SECONDS,
            disclosure: "Live captures Arrangement audio. The Set may look modified, and Live's own Undo history includes the temporary device steps (Undo in Live can bring the Ears device back).",
        })
    }

    async fn restricted_preflight(&self, request: &HearRequest, signal: Signal) -> Result<Ready, RuntimeError> {
        let window = window(request)?;
        if !self.available() { return Err(refused("not_connected", "Connect to Live and observe the Set first.")); }
        if self.rendering.is_on() { return Err(refused("busy", "Kumi is already listening; wait for that call to settle.")); }
        if !self.supported(crate::bridge_version::EARS_BRIDGE)
            || !self.connection().has("live_song_state")
            || !self.connection().has("live_browser_load_preview")
        {
            return Err(unreadable());
        }
        if request.mix != Some(true) {
            let args = object(self.connection().references.borrow().lengthen(&json!({"trackRef":request.tracks[0]})));
            self.connection().references.borrow().require_fresh_references(&args)
                .map_err(|_| refused("stale", "That track ref is stale; observe again."))?;
        }
        let (state, target, label) = self.capture_state(request, signal.clone()).await?;
        if state.transport["playing"] != false || state.transport["arrangementRecord"] != false || state.transport["sessionRecord"] != false {
            return Err(refused("transport_running", "Stop playback and recording before listening to an Arrangement window."));
        }
        if state.song["backToArranger"] != false {
            return Err(refused("back_to_arrangement", "Live is not following the Arrangement. Resolve Back to Arrangement yourself, then observe again."));
        }
        let seconds = window.beats * 60.0 / state.tempo;
        if !seconds.is_finite() || seconds <= 0.0 || seconds > 60.0 {
            return Err(refused("window_too_long", "The approved beat window must be at most 60 seconds at the current tempo."));
        }
        if self.ears_disabled || std::env::var("KUMI_EARS").ok().as_deref() == Some("0") {
            return Err(refused("ears_missing", "Kumi Ears is unavailable. It requires Max for Live; listening never installs it."));
        }
        // A caller-provided link is a trusted integration/test dependency, not a model argument.
        if self.open_ears.is_none() {
            if !self.connection().has("live_browser_inspect") { return Err(unreadable()); }
            let found = self.connection().call("live_browser_inspect", object(json!({"itemId":EARS_ITEM})), signal).await?;
            if found.is_error == Some(true) {
                return Err(refused("ears_missing", "The preinstalled Kumi Ears device is missing. It requires Max for Live; listening never installs it."));
            }
        }
        Ok(Ready { state, target, label, window, seconds })
    }

    /// Validate the exact observed Set before any rows from this response enter the ref book.
    async fn capture_set_at(&self, epoch: f64, identity: &str, signal: Signal) -> Result<JsonObject, RuntimeError> {
        let args = crate::context::discovery_args(&object(json!({"kind":"set","fields":["objectIdentity","tempo"]})))?;
        let answer = self.connection().call("live_discover", args, signal).await?;
        let mut page = crate::context::discovery_payload(&answer, "set", epoch)
            .map_err(|_| refused("stale", "Live did not confirm the observed Set epoch; observe again before listening."))?;
        if page["truncated"] != false || page.contains_key("nextCursor") { return Err(unreadable()); }
        let Some(Value::Array(mut rows)) = page.remove("items") else { return Err(unreadable()) };
        if rows.len() != 1 { return Err(unreadable()); }
        let Some(Value::Object(set)) = rows.pop() else { return Err(unreadable()) };
        if !set.get("objectIdentity").and_then(Value::as_str).is_some_and(|id| !id.is_empty()) { return Err(unreadable()); }
        if crate::context::set_identity(&set)? != identity {
            return Err(refused("stale", "The observed Set identity changed; observe again before listening."));
        }
        Ok(set)
    }

    /// Only these transport steps belong to a capture. Native transactions fence the preview
    /// through apply; never accept a preview from a newer Set or fall back to an emergency stop.
    async fn capture_transport(&self, state: &CaptureState, step: CaptureTransport, signal: Signal) -> Result<(), RuntimeError> {
        let needs_stopped = !matches!(step, CaptureTransport::Stop);
        let (preview_name, apply_name, input) = match step {
            CaptureTransport::Seek { position, looped } => ("live_transport_preview", "live_transport_apply", json!({"position":position,"loopEnabled":looped})),
            CaptureTransport::Continue => ("live_transport_action_preview", "live_transport_action_apply", json!({"action":"continue"})),
            CaptureTransport::Stop => ("live_transport_action_preview", "live_transport_action_apply", json!({"action":"stop"})),
        };
        signal.check()?;
        let prepared = self.connection().call(preview_name, object(input), signal.clone()).await?;
        if prepared.is_error == Some(true) { return Err(unsure()); }
        let preview = payload(&prepared)?;
        if preview.get("epoch").and_then(Value::as_f64) != Some(state.epoch) {
            return Err(refused("stale", "The transport preview belongs to another Set epoch; no transport action was applied."));
        }
        let transaction = preview.get("transactionId").and_then(Value::as_str).filter(|id| !id.is_empty()).ok_or_else(unreadable)?;
        if preview.get("confirmation").and_then(Value::as_str) != Some("apply") { return Err(unreadable()); }
        self.capture_set_at(state.epoch, &state.identity, signal.clone()).await?;
        // Read AFTER preview: the native playback revision advances when recording/playing
        // state changes, and apply fences that revision again on Live's main thread.
        let (playback, song) = futures::try_join!(
            self.rows("session-playback", json!({}), signal.clone()),
            async { payload(&self.connection().call("live_song_state", JsonObject::new(), signal.clone()).await?).map_err(RuntimeError::from) },
        )?;
        let playback = only(playback)?;
        let transport = playback.get("transport").and_then(Value::as_object).ok_or_else(unreadable)?;
        if flag(transport, "arrangementRecord")? || flag(transport, "sessionRecord")?
            || (needs_stopped && flag(transport, "playing")?) || flag(&song, "backToArranger")?
        {
            return Err(refused("transport_changed", "Live's playback or recording state changed; the capture transport step was not applied."));
        }
        signal.check()?;
        let applied = self.connection().call(apply_name,
            object(json!({"transactionId":transaction,"confirmation":"apply","idempotencyKey":uuid::Uuid::new_v4().to_string()})),
            self.history.change_signal(),
        ).await?;
        if applied.is_error == Some(true) || payload(&applied)?.get("state").and_then(Value::as_str) != Some("applied") {
            return Err(unsure());
        }
        Ok(())
    }

    async fn capture_state(&self, request: &HearRequest, signal: Signal) -> Result<(CaptureState, String, String), RuntimeError> {
        let epoch = request.expected_native_epoch.as_deref().and_then(|value| value.parse::<f64>().ok())
            .filter(|epoch| epoch.is_finite() && self.connection().epoch.get() == Some(*epoch))
            .ok_or_else(|| refused("stale", "The current Live connection is not the request's observed Set epoch; observe again."))?;
        let identity = self.connection().set.borrow().clone().ok_or_else(|| refused("stale", "Observe the current Set before listening."))?;
        let set = self.capture_set_at(epoch, &identity, signal.clone()).await?;
        let light = json!({"fields":["objectIdentity","name","armed","monitoringState"]});
        let (playback, regular, returns, main, selection, song) = futures::try_join!(
            self.rows("session-playback", json!({}), signal.clone()),
            self.rows("track", light.clone(), signal.clone()),
            self.rows("return-track", light.clone(), signal.clone()),
            self.rows("main-track", light, signal.clone()),
            self.rows("selection", json!({}), signal.clone()),
            async { payload(&self.connection().call("live_song_state", JsonObject::new(), signal.clone()).await?).map_err(RuntimeError::from) },
        )?;
        let tempo = number(&set, "tempo")?;
        if tempo <= 0.0 { return Err(unreadable()); }
        let playback = only(playback)?;
        let transport = playback.get("transport").and_then(Value::as_object).ok_or_else(unreadable)?;
        for key in ["playing", "arrangementRecord", "sessionRecord", "punchIn", "punchOut"] { flag(transport, key)?; }
        if number(transport, "position")? < 0.0 { return Err(unreadable()); }
        let looped = transport.get("loop").and_then(Value::as_object).ok_or_else(unreadable)?;
        flag(looped, "enabled")?;
        number(looped, "start")?;
        if number(looped, "length")? <= 0.0 { return Err(unreadable()); }
        if !playback.get("firedTargets").is_some_and(Value::is_array) || !playback.get("playingTargets").is_some_and(Value::is_array) {
            return Err(unreadable());
        }
        let transport = json!({"playing":transport["playing"],"arrangementRecord":transport["arrangementRecord"],
            "sessionRecord":transport["sessionRecord"],"position":transport["position"],"loop":looped,
            "punchIn":transport["punchIn"],"punchOut":transport["punchOut"],
            "firedTargets":playback["firedTargets"],"playingTargets":playback["playingTargets"]});
        flag(&song, "backToArranger")?;
        let song = json!({"backToArranger":song["backToArranger"],"sessionRecord":song.get("sessionRecord"),
            "sessionAutomationRecord":song.get("sessionAutomationRecord"),"overdub":song.get("overdub"),
            "arrangementOverdub":song.get("arrangementOverdub"),"reEnableAutomationEnabled":song.get("reEnableAutomationEnabled")});
        let main = only(main)?;
        let all: Vec<_> = regular.into_iter().chain(returns).chain(std::iter::once(main)).collect();
        let target = if request.mix == Some(true) {
            all.last().ok_or_else(unreadable)?
        } else {
            all.iter().find(|row| row.get("ref").and_then(Value::as_str) == request.tracks.first().map(String::as_str))
                .ok_or_else(|| refused("stale", "The requested track is not in the current Set; observe again."))?
        };
        let reference = target.get("ref").and_then(Value::as_str).ok_or_else(unreadable)?.to_owned();
        let label = if request.mix == Some(true) { "The whole mix".to_owned() } else {
            target.get("name").and_then(Value::as_str).ok_or_else(unreadable)?.to_owned()
        };
        let native = self.connection().references.borrow().lengthen(&json!({"trackRef":reference}));
        let reference = native["trackRef"].as_str().ok_or_else(unreadable)?.to_owned();
        let mut tracks = Vec::with_capacity(all.len());
        for row in &all {
            let identity = object_identity(row);
            if identity.is_empty() || !row.contains_key("armed") || !row.contains_key("monitoringState") { return Err(unreadable()); }
            tracks.push(json!({"identity":identity,"armed":row["armed"],"monitoringState":row["monitoringState"]}));
        }
        let selection = only(selection)?;
        for key in ["selectedTrackRef", "selectedDeviceRef"] {
            if !selection.contains_key(key) { return Err(unreadable()); }
        }
        let selected = object(self.connection().references.borrow().lengthen(&Value::Object(selection)));
        let selection = json!({"track":selected.get("selectedTrackRef"),"device":selected.get("selectedDeviceRef")});
        let devices = self.rows("device", json!({"parent":reference,"fields":["objectIdentity"]}), signal.clone()).await?;
        let devices: Vec<_> = devices.iter().map(object_identity).collect();
        if devices.iter().any(String::is_empty) { return Err(unreadable()); }
        self.capture_set_at(epoch, &identity, signal).await?;
        Ok((CaptureState { identity, epoch, tempo, transport, song, tracks, selection, devices }, reference, label))
    }

    async fn capture_set_matches(&self, state: &CaptureState, signal: Signal) -> bool {
        self.capture_set_at(state.epoch, &state.identity, signal).await.is_ok()
    }

    pub(super) async fn hear_restricted(
        self: &Rc<Self>, request: &HearRequest, original: Signal,
    ) -> Result<Result<Vec<HeardTake>, String>, RuntimeError> {
        let directory = self.listening_policy.scratch_dir(request).await?;
        let signal = abort::any([original.clone(), self.connection().lifetime.clone(), abort::timeout(120_000)]);
        let ready = self.restricted_preflight(request, signal.clone()).await?;
        // Unlike standalone setup, opening this link does not install, create a cache or spawn
        // a detached setup task. Every exit below closes the per-call socket.
        let link = if let Some(open) = &self.open_ears {
            open().await?
        } else {
            open_ears_link(EarsOptions::default()).await.map_err(|_| refused("ears_unavailable", "Kumi could not open its local Ears link."))?
        };
        let mut rig = Rig {
            tag: uuid::Uuid::new_v4().to_string(), tempo: Some(ready.state.tempo),
            sources: vec![Source { track: ready.target.clone(), name: ready.label.clone(), scratch: String::new(),
                label: ready.label.clone(), clip: None, scene: None, mix: false, quiet: false }],
            clips: false, hold: None, window: Some(ready.window), from: ready.window.from, beats: ready.window.beats,
            steps: vec![], transport: Transport::default(), notes: vec![],
            ears: Some(RigEars { link: link.clone(), taps: IndexMap::new(), before_main: None }), recorded: vec![],
            before_main: false, before_main_missed: None, noted: false,
        };
        let rendering = self.rendering_now();
        let mut steps = Vec::new();
        let mut started = false;
        let mut transport_touched = false;
        let result = self.history.quietly(Some(&mut steps), async {
            // The tap is the only device mutation, never a recording-track fallback.
            let tap = self.place_tap(link.clone(), &ready.target, signal.clone()).await.map_err(|e| e.error)?;
            rig.ears.as_mut().expect("per-call Ears").taps.insert(ready.label.clone(), tap.clone());
            if !tap.version.is_finite() || tap.version < EARS_VERSION as f64 {
                return Err(refused("ears_missing", "The installed Kumi Ears is too old to verify this window. Max for Live is required."));
            }
            self.capture_window(&ready, &link, &tap, &directory, &mut started, &mut transport_touched, signal.clone()).await
        }).await;
        rig.steps.extend(steps);
        // Never restore a different Set after the producer changed documents.
        let cleanup = abort::any([self.connection().lifetime.clone(), abort::timeout(90_000)]);
        let same_set = self.capture_set_matches(&ready.state, cleanup.clone()).await;
        if same_set {
            for tap in rig.ears.as_ref().expect("per-call Ears").taps.values() { link.stop(tap); }
            if started {
                let _ = self.capture_transport(&ready.state, CaptureTransport::Stop, cleanup.clone()).await;
            }
            self.close_rig(&mut rig).await;
            if transport_touched {
                let _ = self.capture_transport(&ready.state, CaptureTransport::Seek {
                    position: ready.state.transport["position"].as_f64().expect("validated position"),
                    looped: ready.state.transport["loop"]["enabled"].as_bool().expect("validated loop state"),
                }, cleanup.clone()).await;
            }
        }
        link.close().await;
        let restored = same_set && self.capture_state(request, cleanup).await.is_ok_and(|(after, _, _)| after == ready.state);
        drop(rendering);
        if !restored { return Err(unsure()); }
        if original.is_cancelled() { return Err(RuntimeError::Aborted); }
        let (file, seconds) = result.map_err(|error| match error {
            RuntimeError::Hear(HearFailure::Refused { .. }) => error,
            _ => refused("capture_incomplete", "The approved window was not captured completely; temporary state was restored. No audio was published."),
        })?;
        Ok(Ok(vec![HeardTake { label: ready.label, file: file.to_string_lossy().into_owned(), start: 0.0,
            seconds: Some(seconds), live: true, note: None, marked_modified: false }]))
    }

    async fn capture_window(
        &self, ready: &Ready, link: &Rc<dyn EarsLink>, tap: &Tap, directory: &Path,
        started: &mut bool, transport_touched: &mut bool, signal: Signal,
    ) -> Result<(PathBuf, f64), RuntimeError> {
        // Re-read immediately before playback; a successful load did not authorize stopping
        // someone else's playback or changing to Session clips.
        let playback = only(self.rows("session-playback", json!({}), signal.clone()).await?)?;
        let transport = playback.get("transport").and_then(Value::as_object).ok_or_else(unreadable)?;
        if flag(transport, "playing")? || flag(transport, "arrangementRecord")? || flag(transport, "sessionRecord")? {
            return Err(refused("transport_running", "Playback or recording started while Kumi prepared its capture."));
        }
        let song = payload(&self.connection().call("live_song_state", JsonObject::new(), signal.clone()).await?)?;
        if flag(&song, "backToArranger")? { return Err(refused("back_to_arrangement", "Live stopped following the Arrangement.")); }
        let cap = (ready.seconds + 5.0).min(MAX_AUDIO_SECONDS);
        if !tap.sample_rate.is_finite() || tap.sample_rate <= 0.0
            || ready.seconds * tap.sample_rate * 8.0 + 44.0 > MAX_WAV_BYTES as f64
            || cap * tap.sample_rate * 16.0 > (150 * 1024 * 1024) as f64
        {
            return Err(refused("capture_too_large", "This window at Live's sample rate would exceed the private audio size limit."));
        }
        *transport_touched = true;
        self.capture_transport(&ready.state, CaptureTransport::Seek { position: ready.window.from, looped: false }, signal.clone()).await?;
        link.arm(tap, cap, false, Some(signal.clone())).await.map_err(|_| refused("ears_unavailable", "Kumi Ears did not arm."))?;
        *started = true;
        let deadline = Instant::now() + Duration::from_secs_f64(cap);
        let end = ready.window.from + ready.window.beats;
        let played = tokio::time::timeout_at(deadline, async {
            self.capture_transport(&ready.state, CaptureTransport::Continue, signal.clone()).await?;
            loop {
                signal.check()?;
                if let Some(now) = link.transport(tap, Some(signal.clone())).await.map_err(|_| unreadable())? {
                    if now.running && now.beats >= end { break; }
                }
                delay(20.0, signal.clone()).await?;
            }
            Ok::<_, RuntimeError>(())
        }).await;
        let stop_signal = abort::any([self.connection().lifetime.clone(), abort::timeout(5_000)]);
        if !self.capture_set_matches(&ready.state, stop_signal.clone()).await { return Err(unsure()); }
        // Keep `started` true until the independent after-read proves a stopped transport.
        self.capture_transport(&ready.state, CaptureTransport::Stop, stop_signal).await?;
        played.map_err(|_| refused("capture_deadline", "The capture reached its independent deadline."))??;
        let raw = RawFile(directory.join(format!("{}.raw", uuid::Uuid::new_v4())));
        let written = link.write(tap, &raw.0.to_string_lossy().replace('\\', "/"), Some(signal.clone())).await
            .map_err(|_| refused("capture_failed", "Kumi Ears could not write the capture."))?;
        if Path::new(&written.file) != raw.0 {
            return Err(refused("capture_failed", "Kumi Ears returned a file outside this call's requested capture."));
        }
        if written.channels != 4 || !written.sample_rate.is_finite() || written.sample_rate <= 0.0 {
            return Err(unreadable());
        }
        let meta = tokio::fs::symlink_metadata(&raw.0).await.map_err(|_| unreadable())?;
        let frames = meta.len() / (written.channels as u64 * 4);
        if !meta.is_file() || meta.len() % (written.channels as u64 * 4) != 0 || meta.len() > 150 * 1024 * 1024
            || frames as f64 / written.sample_rate > MAX_AUDIO_SECONDS
        { return Err(refused("capture_too_large", "The raw capture exceeded its bounded audio window.")); }
        let capture = read_capture(&raw.0, written.channels, written.sample_rate).await.map_err(|_| unreadable())?;
        let window = ready.window;
        let (capture, first, last) = tokio::task::spawn_blocking(move || {
            let anchors = Anchors { first: Some(written.beats), after_jump: Some(window.from) };
            let covered = cover(&runs(&capture, anchors), window.from, window.beats, capture.sample_rate).map_err(|_| unreadable())?;
            let first = frame_at(&covered, window.from).ok_or_else(unreadable)?;
            let last = (first as f64 + (window.beats * covered.samples_per_beat).round()) as usize;
            if last > covered.to || last <= first || (last - first) as f64 / capture.sample_rate > MAX_AUDIO_SECONDS
                || (last - first) as u64 * 8 + 44 > MAX_WAV_BYTES
            { return Err(unreadable()); }
            Ok::<_, RuntimeError>((capture, first, last))
        }).await.map_err(|_| unreadable())??;
        signal.check()?;
        let file = directory.join(format!("{}.wav", uuid::Uuid::new_v4()));
        let seconds = (last - first) as f64 / capture.sample_rate;
        write_capture_wav(&file, &capture, first as f64, last as f64).await.map_err(|_| unreadable())?;
        Ok((file, seconds))
    }
}
