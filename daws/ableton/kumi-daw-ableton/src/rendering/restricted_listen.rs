//! One approved Arrangement window through preinstalled Ears. No setup, recording-track
//! fallback, preroll, second attempt, shared-cache sweep or implicit audition.
use super::ears::{object_identity, RawFile};
use super::rig::Window;
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
const SELECTION_FIELDS: [&str; 7] = ["trackRef", "sceneRef", "slotRef", "detailClipRef", "deviceRef", "parameterRef", "chainRef"];

fn selection_row(value: &Value) -> bool {
    value.as_object().is_some_and(|row| {
        row.len() == SELECTION_FIELDS.len()
            && SELECTION_FIELDS
                .iter()
                .all(|key| row.get(*key).is_some_and(|value| value.is_null() || value.as_str().is_some_and(|text| !text.is_empty())))
    })
}

/// Private bridge receipt: never inserted into HISTORY or returned to the model.
struct CaptureDevice {
    transaction: String,
    devices: Option<Vec<String>>,
}

fn remaining_ms(deadline: Instant) -> u64 {
    deadline.saturating_duration_since(Instant::now()).as_millis().min(u64::MAX as u128) as u64
}

fn uncertainty(code: &'static str, reason: &str) -> RuntimeError {
    HearFailure::Unsure { code: Some(code), reason: reason.into(), restored: false, marked_modified: true }.into()
}

fn modified(error: RuntimeError, touched: bool) -> RuntimeError {
    match error {
        RuntimeError::Hear(HearFailure::Refused { code, reason, marked_modified }) => {
            HearFailure::Refused { code, reason, marked_modified: marked_modified || touched }.into()
        }
        other => other,
    }
}

#[derive(Debug, PartialEq)]
struct CaptureState {
    identity: String,
    epoch: f64,
    tempo: f64,
    transport: Value,
    song: Value,
    tracks: Vec<Value>,
    devices: Vec<String>,
}

enum CaptureTransport {
    Prepare,
    Restore,
    RestoreStart,
    Continue,
    Stop,
}

struct Ready {
    state: CaptureState,
    target: String,
    label: String,
    window: Window,
    seconds: f64,
    seconds_limit: f64,
    owned_transport: RefCell<Value>,
}

fn refused(code: &'static str, reason: impl Into<String>) -> RuntimeError {
    HearFailure::Refused { code, reason: reason.into(), marked_modified: false }.into()
}

fn unsure() -> RuntimeError {
    HearFailure::Unsure {
        code: None,
        reason:
            "Live's capture did not verify restoration. Observe again and check the transport and Kumi Ears device before another action."
                .into(),
        restored: false,
        marked_modified: true,
    }
    .into()
}

fn unreadable() -> RuntimeError {
    refused(
        "unsupported",
        "This Live bridge cannot read every state needed to verify a bounded capture. Update its bridge and restart Live.",
    )
}

fn number(row: &JsonObject, key: &str) -> Result<f64, RuntimeError> {
    row.get(key).and_then(Value::as_f64).filter(|n| n.is_finite()).ok_or_else(unreadable)
}

fn flag(row: &JsonObject, key: &str) -> Result<bool, RuntimeError> {
    row.get(key).and_then(Value::as_bool).ok_or_else(unreadable)
}

fn only(mut rows: Vec<JsonObject>) -> Result<JsonObject, RuntimeError> {
    if rows.len() != 1 {
        return Err(unreadable());
    }
    Ok(rows.pop().expect("one row"))
}

fn window(request: &HearRequest) -> Result<Window, RuntimeError> {
    let from = request.from_beat.filter(|n| n.is_finite() && *n >= 0.0);
    let beats = request.beats.filter(|n| n.is_finite() && *n > 0.0 && *n <= 256.0);
    let source_ok = if request.mix == Some(true) { request.tracks.is_empty() } else { request.tracks.len() == 1 };
    match (from, beats) {
        (Some(from), Some(beats))
            if source_ok
                && (from + beats).is_finite()
                && from + beats > from
                && request.seconds.is_none()
                && request.whole != Some(true) =>
        {
            Ok(Window { from, beats })
        }
        _ => Err(refused(
            "invalid_window",
            "Give one fresh track ref or the mix, from_beat and beats (at most 256); there is no implicit range.",
        )),
    }
}

impl LiveRenderer {
    /// The same pure preflight is used by the approval preview and again under the write lease.
    pub(super) async fn preview_restricted_hear(&self, request: &HearRequest, signal: Signal) -> Result<HearPreview, RuntimeError> {
        let ready = self.restricted_preflight(request, signal).await?;
        Ok(HearPreview {
            label: ready.label, seconds: ready.seconds, method: HearMethod::Ears, deadline_seconds: CALL_DEADLINE_SECONDS,
            disclosure: "Live captures Arrangement audio after the target's devices, before its mixer (volume, pan and sends); Main's fader is not captured. Live may show the Set as modified, and its own Undo history includes the listen's temporary device steps (pressing Undo in Live can bring the Ears device back)",
        })
    }

    async fn restricted_preflight(&self, request: &HearRequest, signal: Signal) -> Result<Ready, RuntimeError> {
        let window = window(request)?;
        if !self.available() {
            return Err(refused("not_connected", "Connect to Live and observe the Set first."));
        }
        if self.rendering.is_on() {
            return Err(refused("busy", "Kumi is already listening; wait for that call to settle."));
        }
        if !self.supported(crate::bridge_version::EARS_BRIDGE)
            || [
                "live_song_state",
                "live_discover",
                "live_browser_load_preview",
                "live_browser_load_apply",
                "live_undo",
                "live_undo_step_begin",
                "live_undo_step_end",
                "live_transport_preview",
                "live_transport_apply",
                "live_transport_action_preview",
                "live_transport_action_apply",
            ]
            .iter()
            .any(|method| !self.connection().has(method))
        {
            return Err(unreadable());
        }
        if request.mix != Some(true) {
            let args = object(self.connection().references.borrow().lengthen(&json!({"trackRef":request.tracks[0]})));
            self.connection()
                .references
                .borrow()
                .require_fresh_references(&args)
                .map_err(|_| refused("stale", "That track ref is stale; observe again."))?;
        }
        let (state, target, label) = self.capture_state(request, signal.clone()).await.map_err(|error| match error {
            RuntimeError::Hear(HearFailure::Refused { .. }) | RuntimeError::Aborted => error,
            _ => unreadable(),
        })?;
        if state.transport["playing"] != false || state.transport["arrangementRecord"] != false || state.transport["sessionRecord"] != false
        {
            return Err(refused("transport_running", "Stop playback and recording before listening to an Arrangement window."));
        }
        if state.song["backToArranger"] != false {
            return Err(refused(
                "back_to_arrangement",
                "Live is not following the Arrangement. Resolve Back to Arrangement yourself, then observe again.",
            ));
        }
        let seconds = window.beats * 60.0 / state.tempo;
        if request.approval.is_some_and(|approval| !approval.covers(seconds, CALL_DEADLINE_SECONDS)) {
            return Err(refused("stale", "The current window exceeds its approved duration or deadline; preview again before listening."));
        }
        if !seconds.is_finite() || seconds <= 0.0 || seconds > 60.0 {
            return Err(refused("window_too_long", "The approved beat window must be at most 60 seconds at the current tempo."));
        }
        if self.ears_disabled || std::env::var("KUMI_EARS").ok().as_deref() == Some("0") {
            return Err(refused("ears_missing", "Kumi Ears is unavailable. It requires Max for Live; listening never installs it."));
        }
        // A caller-provided link is a trusted integration/test dependency, not a model argument.
        if self.open_ears.is_none() {
            if !self.connection().has("live_browser_inspect") {
                return Err(unreadable());
            }
            let found = self
                .connection()
                .call("live_browser_inspect", object(json!({"itemId":EARS_ITEM})), signal.clone())
                .await
                .map_err(|error| if matches!(error, RuntimeError::Aborted) { error } else { unreadable() })?;
            if found.is_error == Some(true) {
                return Err(refused(
                    "ears_missing",
                    "The preinstalled Kumi Ears device is missing. It requires Max for Live; listening never installs it.",
                ));
            }
        }
        // These read-only previews also reject old bridges that expose the method names but
        // not the preserved-selection/start-time contracts, before a device can be created.
        self.capture_load_preview(&state, &target, signal.clone()).await?;
        let probe =
            self.connection().call("live_transport_preview", object(json!({"startTime":state.transport["startTime"]})), signal).await?;
        if probe.is_error == Some(true) {
            return Err(unreadable());
        }
        let probe = payload(&probe)?;
        if probe.get("epoch").and_then(Value::as_f64) != Some(state.epoch)
            || probe.get("confirmation").and_then(Value::as_str) != Some("apply")
        {
            return Err(unreadable());
        }
        Ok(Ready {
            owned_transport: RefCell::new(state.transport.clone()),
            state,
            target,
            label,
            window,
            seconds,
            seconds_limit: request.approval.map_or(seconds, |approval| approval.seconds),
        })
    }

    /// Validate the exact observed Set before any rows from this response enter the ref book.
    async fn capture_set_at(&self, epoch: f64, identity: &str, signal: Signal) -> Result<JsonObject, RuntimeError> {
        let args = crate::context::discovery_args(&object(json!({"kind":"set","fields":["objectIdentity","tempo"]})))?;
        let answer = self.connection().call("live_discover", args, signal).await?;
        let mut page = crate::context::discovery_payload(&answer, "set", epoch)
            .map_err(|_| refused("stale", "Live did not confirm the observed Set epoch; observe again before listening."))?;
        if page["truncated"] != false || page.contains_key("nextCursor") {
            return Err(unreadable());
        }
        let Some(Value::Array(mut rows)) = page.remove("items") else { return Err(unreadable()) };
        if rows.len() != 1 {
            return Err(unreadable());
        }
        let Some(Value::Object(set)) = rows.pop() else { return Err(unreadable()) };
        if !set.get("objectIdentity").and_then(Value::as_str).is_some_and(|id| !id.is_empty()) {
            return Err(unreadable());
        }
        if crate::context::set_identity(&set)? != identity {
            return Err(refused("stale", "The observed Set identity changed; observe again before listening."));
        }
        Ok(set)
    }

    /// Only these transport steps belong to a capture. Native transactions fence the preview
    /// through apply; never accept a preview from a newer Set or fall back to an emergency stop.
    async fn capture_transport(&self, ready: &Ready, step: CaptureTransport, signal: Signal) -> Result<(), RuntimeError> {
        let state = &ready.state;
        let begins = matches!(step, CaptureTransport::Prepare | CaptureTransport::Continue);
        let needs_stopped = !matches!(step, CaptureTransport::Stop);
        let (preview_name, apply_name, input) = match step {
            CaptureTransport::Prepare => {
                ("live_transport_preview", "live_transport_apply", json!({"position":ready.window.from,"loopEnabled":false}))
            }
            CaptureTransport::Restore => (
                "live_transport_preview",
                "live_transport_apply",
                json!({
                    "position":state.transport["position"],"loopEnabled":state.transport["loop"]["enabled"],
                }),
            ),
            CaptureTransport::RestoreStart => {
                ("live_transport_preview", "live_transport_apply", json!({"startTime":state.transport["startTime"]}))
            }
            CaptureTransport::Continue => ("live_transport_action_preview", "live_transport_action_apply", json!({"action":"continue"})),
            CaptureTransport::Stop => ("live_transport_action_preview", "live_transport_action_apply", json!({"action":"stop"})),
        };
        signal.check()?;
        let prepared = self.connection().call(preview_name, object(input), signal.clone()).await?;
        if prepared.is_error == Some(true) {
            return Err(unsure());
        }
        let preview = payload(&prepared)?;
        if preview.get("epoch").and_then(Value::as_f64) != Some(state.epoch) {
            return Err(refused("stale", "The transport preview belongs to another Set epoch; no transport action was applied."));
        }
        let transaction = preview.get("transactionId").and_then(Value::as_str).filter(|id| !id.is_empty()).ok_or_else(unreadable)?;
        if preview.get("confirmation").and_then(Value::as_str) != Some("apply") {
            return Err(unreadable());
        }
        let set = self.capture_set_at(state.epoch, &state.identity, signal.clone()).await?;
        let seconds = ready.window.beats * 60.0 / number(&set, "tempo")?;
        if begins && (!seconds.is_finite() || seconds <= 0. || seconds > ready.seconds_limit) {
            return Err(refused("stale", "Live's tempo changed beyond the approved window before playback; preview again."));
        }
        // Read AFTER preview: the native playback revision advances when recording/playing
        // state changes, and apply fences that revision again on Live's main thread.
        let (playback, song) = futures::try_join!(self.rows("session-playback", json!({}), signal.clone()), async {
            payload(&self.connection().call("live_song_state", JsonObject::new(), signal.clone()).await?).map_err(RuntimeError::from)
        },)?;
        let playback = only(playback)?;
        let transport = playback.get("transport").and_then(Value::as_object).ok_or_else(unreadable)?;
        if flag(transport, "arrangementRecord")?
            || flag(transport, "sessionRecord")?
            || (needs_stopped && flag(transport, "playing")?)
            || flag(&song, "backToArranger")?
        {
            return Err(refused(
                "transport_changed",
                "Live's playback or recording state changed; the capture transport step was not applied.",
            ));
        }
        {
            let owned = ready.owned_transport.borrow();
            let changed = ["playing", "arrangementRecord", "sessionRecord", "startTime", "loop", "punchIn", "punchOut"]
                .iter()
                .any(|key| transport.get(*key) != owned.get(*key))
                || ["firedTargets", "playingTargets"].iter().any(|key| playback.get(*key) != owned.get(*key))
                || (owned["playing"] != true
                    && (number(transport, "position")? - owned["position"].as_f64().ok_or_else(unreadable)?).abs() > 1e-6)
                || state.song.as_object().ok_or_else(unreadable)?.iter().any(|(key, value)| song.get(key) != Some(value));
            if changed {
                return Err(refused("transport_changed", "Live's producer state changed; Kumi did not overwrite it."));
            }
        }
        signal.check()?;
        let applied = self
            .connection()
            .call(
                apply_name,
                object(json!({"transactionId":transaction,"confirmation":"apply","idempotencyKey":uuid::Uuid::new_v4().to_string()})),
                signal.clone(),
            )
            .await?;
        if applied.is_error == Some(true) || payload(&applied)?.get("state").and_then(Value::as_str) != Some("applied") {
            return Err(unsure());
        }
        // Live accepts a playhead/loop write before its next tick applies it. Starting
        // from the old cursor can make Ears report the requested end immediately.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let playback = only(self.rows("session-playback", json!({}), signal.clone()).await?)?;
            let transport = playback.get("transport").and_then(Value::as_object).ok_or_else(unreadable)?;
            let playing = flag(transport, "playing")?;
            if flag(transport, "arrangementRecord")?
                || flag(transport, "sessionRecord")?
                || (matches!(step, CaptureTransport::Prepare | CaptureTransport::Restore | CaptureTransport::RestoreStart) && playing)
            {
                return Err(refused("transport_changed", "Live's transport changed while its capture step was settling."));
            }
            let settled = match step {
                CaptureTransport::Prepare | CaptureTransport::Restore => {
                    let (position, looped) = if matches!(step, CaptureTransport::Prepare) {
                        (ready.window.from, false)
                    } else {
                        (
                            state.transport["position"].as_f64().ok_or_else(unreadable)?,
                            state.transport["loop"]["enabled"].as_bool().ok_or_else(unreadable)?,
                        )
                    };
                    let loop_state = transport.get("loop").and_then(Value::as_object).ok_or_else(unreadable)?;
                    (number(transport, "position")? - position).abs() <= 1e-6 && flag(loop_state, "enabled")? == looped
                }
                CaptureTransport::RestoreStart => {
                    (number(transport, "startTime")? - state.transport["startTime"].as_f64().ok_or_else(unreadable)?).abs() <= 1e-6
                }
                CaptureTransport::Continue => playing,
                CaptureTransport::Stop => !playing,
            };
            if settled {
                let mut owned = Value::Object(transport.clone());
                owned["firedTargets"] = playback["firedTargets"].clone();
                owned["playingTargets"] = playback["playingTargets"].clone();
                *ready.owned_transport.borrow_mut() = owned;
                self.capture_set_at(state.epoch, &state.identity, signal).await?;
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(refused("capture_incomplete", "Live did not confirm its requested capture transport state."));
            }
            delay(20., signal.clone()).await?;
        }
    }

    async fn capture_load_preview(&self, state: &CaptureState, target: &str, signal: Signal) -> Result<String, RuntimeError> {
        let answer = self
            .connection()
            .call(
                "live_browser_load_preview",
                object(json!({"itemId":EARS_ITEM,"trackRef":target,"preserveSelection":true})),
                signal.clone(),
            )
            .await?;
        if answer.is_error == Some(true) {
            return Err(refused(
                "unsupported",
                "Live did not confirm a selection-preserving Ears load preview. Nothing was loaded; no fallback load was attempted.",
            ));
        }
        let preview = payload(&answer)?;
        if preview.get("epoch").and_then(Value::as_f64) != Some(state.epoch)
            || preview.get("confirmation").and_then(Value::as_str) != Some("apply")
            || !preview.get("selection").is_some_and(selection_row)
        {
            return Err(unreadable());
        }
        self.capture_set_at(state.epoch, &state.identity, signal).await?;
        preview.get("transactionId").and_then(Value::as_str).filter(|id| !id.is_empty()).map(str::to_owned).ok_or_else(unreadable)
    }

    async fn capture_place_tap(
        &self,
        ready: &Ready,
        link: &Rc<dyn EarsLink>,
        receipt: &mut Option<CaptureDevice>,
        signal: Signal,
    ) -> Result<Tap, RuntimeError> {
        let location = format!("{} devices ", self.lom_track_path(&ready.target, signal.clone()).await?);
        let before: Vec<_> = link.taps().iter().map(|tap| tap.id).collect();
        let transaction = self.capture_load_preview(&ready.state, &ready.target, signal.clone()).await?;
        signal.check()?;
        // Keep the transaction even if apply's reply is lost: cleanup must account for a
        // potentially-created device rather than treating a transport error as no mutation.
        *receipt = Some(CaptureDevice { transaction: transaction.clone(), devices: None });
        let answer = self
            .connection()
            .call(
                "live_browser_load_apply",
                object(json!({"transactionId":transaction,"confirmation":"apply","idempotencyKey":uuid::Uuid::new_v4().to_string()})),
                signal.clone(),
            )
            .await?;
        if answer.is_error == Some(true) {
            return Err(unsure());
        }
        let applied = payload(&answer)?;
        if applied.get("state").and_then(Value::as_str) != Some("applied")
            || applied.get("transactionId").and_then(Value::as_str) != Some(transaction.as_str())
            || !applied.get("deviceRef").and_then(Value::as_str).is_some_and(|id| !id.is_empty())
        {
            return Err(unsure());
        }
        let created = applied.get("deviceObjectIdentity").and_then(Value::as_str).filter(|id| !id.is_empty()).ok_or_else(unsure)?;
        let placement = applied.get("placement").ok_or_else(unsure)?;
        let index = placement["index"].as_u64().ok_or_else(unsure)?;
        if placement["owner"] != "track" {
            return Err(unsure());
        }
        let devices: Vec<_> = self
            .rows("device", json!({"parent":ready.target,"fields":["objectIdentity"]}), signal.clone())
            .await?
            .iter()
            .map(object_identity)
            .collect();
        // The tap must hear the whole existing device chain, including Main's limiter.
        if devices.iter().filter(|identity| identity.as_str() == created).count() != 1
            || devices.iter().filter(|identity| identity.as_str() != created).ne(ready.state.devices.iter())
            || index != ready.state.devices.len() as u64
            || devices.last().map(String::as_str) != Some(created)
        {
            return Err(unsure());
        }
        receipt.as_mut().expect("load receipt").devices = Some(devices);
        let location = format!("{location}{index}");
        link.wait_for(Rc::new(move |candidate| !before.contains(&candidate.id) && candidate.path == location), 6000, Some(signal))
            .await
            .ok_or_else(|| {
                refused("ears_missing", "The preinstalled Kumi Ears did not start. Max for Live is required; listening never installs it.")
            })
    }

    /// The only restricted-created-device removal path. Never hold a Live undo step
    /// across capture, and never abandon this future between begin and end.
    async fn capture_remove_tap(&self, ready: &Ready, receipt: &CaptureDevice, deadline: Instant) -> Result<(), RuntimeError> {
        let begin_failed = || {
            uncertainty("undo_step_begin_failed",
            "Kumi could not open Live's cleanup undo step, so the Ears device was left in place. Live's own undo, or deleting it by hand, removes it. No audio was published.")
        };
        if remaining_ms(deadline) < 10_000 {
            return Err(begin_failed());
        }
        self.capture_set_at(
            ready.state.epoch,
            &ready.state.identity,
            abort::any([self.connection().lifetime.clone(), abort::timeout(remaining_ms(deadline).min(5_000))]),
        )
        .await?;
        if remaining_ms(deadline) < 10_000 {
            return Err(begin_failed());
        }
        let opened = self
            .connection()
            .call(
                "live_undo_step_begin",
                object(json!({"label":"Kumi: listen","timeoutMs":30000})),
                abort::any([self.connection().lifetime.clone(), abort::timeout(remaining_ms(deadline).min(5_000))]),
            )
            .await
            .map_err(|_| begin_failed())?;
        if opened.is_error == Some(true) {
            return Err(begin_failed());
        }
        let opened = payload(&opened).map_err(|_| begin_failed())?;
        let step = opened.get("stepId").and_then(Value::as_str).filter(|id| !id.is_empty()).ok_or_else(begin_failed)?;
        // Even malformed success with an identifiable step is closed, but never used to delete.
        let deletion = if opened.get("open") == Some(&Value::Bool(true)) {
            let deletion_signal =
                abort::any([self.connection().lifetime.clone(), abort::timeout(remaining_ms(deadline).saturating_sub(5_000).min(15_000))]);
            self.connection()
                .call(
                    "live_undo",
                    object(json!({"transactionId":receipt.transaction,
                "confirmation":"undo","idempotencyKey":uuid::Uuid::new_v4().to_string(),"preserveSelection":true})),
                    deletion_signal,
                )
                .await
        } else {
            Err(begin_failed())
        };
        // No `?`, timeout wrapper, or caller-cancellation select may bypass this await.
        // Its allowance is reserved from the original native deadline, not restarted.
        let ended = self
            .connection()
            .call("live_undo_step_end", object(json!({"stepId":step})), abort::timeout(remaining_ms(deadline).min(5_000)))
            .await;
        let closed = ended.is_ok_and(|answer| {
            answer.is_error != Some(true)
                && payload(&answer).is_ok_and(|body| {
                    body.get("closed") == Some(&Value::Bool(true)) && body.get("stepId").and_then(Value::as_str) == Some(step)
                })
        });
        if !closed {
            return Err(uncertainty(
                "undo_step_end_failed",
                "Live did not confirm the cleanup undo step closed. Its state is uncertain; no audio was published.",
            ));
        }
        let answer = deletion.map_err(|error| match error {
            RuntimeError::Hear(_) => error,
            _ => uncertainty("cleanup_failed", "Live did not confirm removal of the Ears device. Live's own undo, or deleting it by hand, removes it. No audio was published."),
        })?;
        let body = payload(&answer).map_err(|_| unsure())?;
        if answer.is_error == Some(true) {
            return Err(if body.get("code").and_then(Value::as_str) == Some("selection_not_kept") {
                uncertainty("selection_not_kept", "Live could not keep the producer's selection, so Ears remains in place. Live's own undo, or deleting it by hand, removes it. No audio was published.")
            } else {
                unsure()
            });
        }
        let selection = body.get("selection").ok_or_else(unsure)?;
        if body.get("state").and_then(Value::as_str) != Some("undone")
            || body.get("deleted") != Some(&Value::Bool(true))
            || selection["restored"] != true
            || !selection["previous"].is_boolean()
            || !selection_row(&selection["before"])
            || !selection_row(&selection["after"])
        {
            return Err(unsure());
        }
        // The bridge compares identities in the native step, then reads selection again.
        // Refs can move on deletion; no client-side neighbour or restoration inference.
        Ok(())
    }

    async fn capture_state(&self, request: &HearRequest, signal: Signal) -> Result<(CaptureState, String, String), RuntimeError> {
        let epoch = request
            .expected_native_epoch
            .as_deref()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|epoch| epoch.is_finite() && self.connection().epoch.get() == Some(*epoch))
            .ok_or_else(|| refused("stale", "The current Live connection is not the request's observed Set epoch; observe again."))?;
        let identity =
            self.connection().set.borrow().clone().ok_or_else(|| refused("stale", "Observe the current Set before listening."))?;
        let set = self.capture_set_at(epoch, &identity, signal.clone()).await?;
        let light = json!({"fields":["objectIdentity","name","armed","monitoringState"]});
        let (playback, regular, returns, main, song) = futures::try_join!(
            self.rows("session-playback", json!({}), signal.clone()),
            self.rows("track", light.clone(), signal.clone()),
            self.rows("return-track", light.clone(), signal.clone()),
            self.rows("main-track", light, signal.clone()),
            async {
                payload(&self.connection().call("live_song_state", JsonObject::new(), signal.clone()).await?).map_err(RuntimeError::from)
            },
        )?;
        let tempo = number(&set, "tempo")?;
        if tempo <= 0.0 {
            return Err(unreadable());
        }
        let playback = only(playback)?;
        let transport = playback.get("transport").and_then(Value::as_object).ok_or_else(unreadable)?;
        for key in ["playing", "arrangementRecord", "sessionRecord", "punchIn", "punchOut"] {
            flag(transport, key)?;
        }
        if number(transport, "position")? < 0.0 || number(transport, "startTime")? < 0.0 {
            return Err(unreadable());
        }
        let looped = transport.get("loop").and_then(Value::as_object).ok_or_else(unreadable)?;
        flag(looped, "enabled")?;
        number(looped, "start")?;
        if number(looped, "length")? <= 0.0 {
            return Err(unreadable());
        }
        if !playback.get("firedTargets").is_some_and(Value::is_array) || !playback.get("playingTargets").is_some_and(Value::is_array) {
            return Err(unreadable());
        }
        let transport = json!({"playing":transport["playing"],"arrangementRecord":transport["arrangementRecord"],
            "sessionRecord":transport["sessionRecord"],"position":transport["position"],"startTime":transport["startTime"],"loop":looped,
            "punchIn":transport["punchIn"],"punchOut":transport["punchOut"],
            "firedTargets":playback["firedTargets"],"playingTargets":playback["playingTargets"]});
        for key in
            ["backToArranger", "sessionRecord", "sessionAutomationRecord", "overdub", "arrangementOverdub", "reEnableAutomationEnabled"]
        {
            flag(&song, key)?;
        }
        let song = json!({"backToArranger":song["backToArranger"],"sessionRecord":song["sessionRecord"],
            "sessionAutomationRecord":song["sessionAutomationRecord"],"overdub":song["overdub"],
            "arrangementOverdub":song["arrangementOverdub"],"reEnableAutomationEnabled":song["reEnableAutomationEnabled"]});
        let main = only(main)?;
        let all: Vec<_> = regular.into_iter().chain(returns).chain(std::iter::once(main)).collect();
        let target = if request.mix == Some(true) {
            all.last().ok_or_else(unreadable)?
        } else {
            let references = self.connection().references.borrow();
            let requested = &request.tracks[0];
            let short = references.short.get(requested).map(String::as_str).unwrap_or(requested);
            all.iter()
                .find(|row| row.get("ref").and_then(Value::as_str) == Some(short))
                .ok_or_else(|| refused("stale", "The requested track is not in the current Set; observe again."))?
        };
        let reference = target.get("ref").and_then(Value::as_str).ok_or_else(unreadable)?.to_owned();
        let label = if request.mix == Some(true) {
            "The whole mix".to_owned()
        } else {
            target.get("name").and_then(Value::as_str).ok_or_else(unreadable)?.to_owned()
        };
        let native = self.connection().references.borrow().lengthen(&json!({"trackRef":reference}));
        let reference = native["trackRef"].as_str().ok_or_else(unreadable)?.to_owned();
        let mut tracks = Vec::with_capacity(all.len());
        for row in &all {
            let identity = object_identity(row);
            if identity.is_empty() || !row.contains_key("armed") || !row.contains_key("monitoringState") {
                return Err(unreadable());
            }
            tracks.push(json!({"identity":identity,"armed":row["armed"],"monitoringState":row["monitoringState"]}));
        }
        let devices = self.rows("device", json!({"parent":reference,"fields":["objectIdentity"]}), signal.clone()).await?;
        let devices: Vec<_> = devices.iter().map(object_identity).collect();
        if devices.iter().any(String::is_empty) {
            return Err(unreadable());
        }
        self.capture_set_at(epoch, &identity, signal).await?;
        Ok((CaptureState { identity, epoch, tempo, transport, song, tracks, devices }, reference, label))
    }

    async fn capture_set_matches(&self, state: &CaptureState, signal: Signal) -> bool {
        self.capture_set_at(state.epoch, &state.identity, signal).await.is_ok()
    }

    pub(super) async fn hear_restricted(
        self: &Rc<Self>,
        request: &HearRequest,
        original: Signal,
    ) -> Result<Result<Vec<HeardTake>, String>, RuntimeError> {
        let Some(approval) = request.approval else {
            return Err(refused("not_approved", "A restricted capture needs the host's approved window and native deadline."));
        };
        let approved_ms = u64::from(approval.deadline_seconds) * 1000;
        let native_ms = request
            .native_deadline_unix_ms
            .map(|deadline| deadline.saturating_sub(now_ms().max(0) as u64).min(approved_ms))
            .filter(|remaining| *remaining > 0)
            .ok_or_else(|| refused("not_approved", "The host's original native deadline is missing or expired."))?;
        // Leave the final fifth to the host's measurement; never create a fresh allowance.
        let cleanup_deadline = Instant::now() + Duration::from_millis(native_ms * 4 / 5);
        let signal = abort::any([original.clone(), self.connection().lifetime.clone(), abort::timeout(native_ms * 2 / 5)]);
        let cleanup = abort::any([self.connection().lifetime.clone(), abort::timeout(native_ms * 4 / 5)]);
        let directory = self.listening_policy.scratch_dir(request).await?;
        let ready = self.restricted_preflight(request, signal.clone()).await?;
        // No installation, cache sweep, detached setup, or fallback recording rig.
        let link = if let Some(open) = &self.open_ears {
            open().await?
        } else {
            open_ears_link(EarsOptions::default())
                .await
                .map_err(|_| refused("ears_unavailable", "Kumi could not open its local Ears link."))?
        };
        let rendering = self.rendering_now();
        let mut receipt = None;
        let mut tap = None;
        let mut started = false;
        let mut transport_touched = false;
        let result = async {
            let placed = self.capture_place_tap(&ready, &link, &mut receipt, signal.clone()).await?;
            tap = Some(placed.clone());
            let (now, _, _) = self.capture_state(request, signal.clone()).await?;
            if now.identity != ready.state.identity
                || now.epoch != ready.state.epoch
                || now.tracks != ready.state.tracks
                || now.song != ready.state.song
                || now.transport != ready.state.transport
                || now.tempo != ready.state.tempo
                || receipt.as_ref().and_then(|receipt| receipt.devices.as_ref()) != Some(&now.devices)
            {
                return Err(refused("transport_changed", "Live's producer state changed while Ears was loaded; no playback was started."));
            }
            if placed.version != EARS_VERSION as f64 {
                return Err(refused("ears_missing", "The preinstalled Kumi Ears has the wrong version. Listening never installs it."));
            }
            self.capture_window(&ready, &link, &placed, &directory, &mut started, &mut transport_touched, signal.clone(), cleanup.clone())
                .await
        }
        .await;
        let marked_modified = receipt.is_some();
        // Disarm our local tap even if the Set/lifetime changed; do not mutate the new Set.
        if let Some(tap) = &tap {
            link.stop(tap);
        }
        let same_set = self.capture_set_matches(&ready.state, cleanup.clone()).await;
        let mut cleanup_error = None;
        if same_set {
            if started {
                if let Err(error) = self.capture_transport(&ready, CaptureTransport::Stop, cleanup.clone()).await {
                    cleanup_error = Some(error);
                }
            }
            if transport_touched {
                // Live's deferred playhead write must settle FIRST; restore startTime LAST.
                let restored = async {
                    self.capture_transport(&ready, CaptureTransport::Restore, cleanup.clone()).await?;
                    self.capture_transport(&ready, CaptureTransport::RestoreStart, cleanup.clone()).await
                }
                .await;
                if let Err(error) = restored {
                    if cleanup_error.is_none() {
                        cleanup_error = Some(error);
                    }
                }
            }
            if let Some(receipt) = &receipt {
                if let Err(error) = self.capture_remove_tap(&ready, receipt, cleanup_deadline).await {
                    cleanup_error = Some(error);
                }
            }
        }
        link.close().await;
        let after = if same_set { self.capture_state(request, cleanup).await } else { Err(unsure()) };
        let restored = same_set && after.is_ok_and(|(after, _, _)| after == ready.state);
        drop(rendering);
        if let Some(error) = cleanup_error {
            return Err(match error {
                RuntimeError::Hear(HearFailure::Unsure { .. }) => error,
                _ => unsure(),
            });
        }
        if !restored {
            return Err(unsure());
        }
        if original.is_cancelled() {
            return Err(HearFailure::Cancelled { marked_modified }.into());
        }
        if remaining_ms(cleanup_deadline) == 0 {
            return Err(modified(
                refused("capture_deadline", "The original native capture deadline expired. No audio was published."),
                marked_modified,
            ));
        }
        let (mut file, seconds) = result.map_err(|error| {
            modified(
                match error {
                    RuntimeError::Hear(HearFailure::Refused { .. }) => error,
                    _ => refused(
                        "capture_incomplete",
                        "The approved window was not captured completely; temporary state was restored. No audio was published.",
                    ),
                },
                marked_modified,
            )
        })?;
        Ok(Ok(vec![HeardTake {
            label: ready.label,
            file: std::mem::take(&mut file.0).to_string_lossy().into_owned(),
            start: 0.0,
            seconds: Some(seconds),
            live: true,
            note: Some(
                "Captured after the target's devices, before its mixer (volume, pan and sends). Main's fader is not captured; changing it will not change these measurements.".into(),
            ),
            marked_modified,
        }]))
    }

    async fn capture_window(
        &self,
        ready: &Ready,
        link: &Rc<dyn EarsLink>,
        tap: &Tap,
        directory: &Path,
        started: &mut bool,
        transport_touched: &mut bool,
        signal: Signal,
        stop_signal: Signal,
    ) -> Result<(RawFile, f64), RuntimeError> {
        // Re-read immediately before playback; a successful load did not authorize stopping
        // someone else's playback or changing to Session clips.
        let playback = only(self.rows("session-playback", json!({}), signal.clone()).await?)?;
        let transport = playback.get("transport").and_then(Value::as_object).ok_or_else(unreadable)?;
        if flag(transport, "playing")? || flag(transport, "arrangementRecord")? || flag(transport, "sessionRecord")? {
            return Err(refused("transport_running", "Playback or recording started while Kumi prepared its capture."));
        }
        let song = payload(&self.connection().call("live_song_state", JsonObject::new(), signal.clone()).await?)?;
        if flag(&song, "backToArranger")? {
            return Err(refused("back_to_arrangement", "Live stopped following the Arrangement."));
        }
        let cap = (ready.seconds + 5.0).min(MAX_AUDIO_SECONDS);
        if !tap.sample_rate.is_finite()
            || tap.sample_rate <= 0.0
            || ready.seconds * tap.sample_rate * 8.0 + 44.0 > MAX_WAV_BYTES as f64
            || cap * tap.sample_rate * 16.0 > (150 * 1024 * 1024) as f64
        {
            return Err(refused("capture_too_large", "This window at Live's sample rate would exceed the private audio size limit."));
        }
        *transport_touched = true;
        self.capture_transport(ready, CaptureTransport::Prepare, signal.clone()).await?;
        // Live's control-thread seek can settle before Max's audio clock. Do not arm
        // from the old cursor, and never "fix" a missed start by playing another window.
        let settled = Instant::now() + Duration::from_secs(5);
        let beat_tolerance = (ready.state.tempo / 60.0 / tap.sample_rate).max(1e-6);
        loop {
            signal.check()?;
            let clock = link.transport(tap, Some(signal.clone())).await.map_err(|_| unreadable())?;
            if clock.is_some_and(|clock| !clock.running && (clock.beats - ready.window.from).abs() <= beat_tolerance) {
                break;
            }
            if Instant::now() >= settled {
                return Err(refused("capture_incomplete", "Kumi Ears did not confirm the stopped start of the approved window."));
            }
            delay(20., signal.clone()).await?;
        }
        let armed =
            link.arm(tap, cap, false, Some(signal.clone())).await.map_err(|_| refused("ears_unavailable", "Kumi Ears did not arm."))?;
        if armed.running
            || !armed.beats.is_finite()
            || (armed.beats - ready.window.from).abs() > beat_tolerance
            || armed.sample_rate != tap.sample_rate
        {
            return Err(refused("capture_incomplete", "Kumi Ears armed outside the approved stopped window."));
        }
        *started = true;
        let deadline = Instant::now() + Duration::from_secs_f64(cap);
        let end = ready.window.from + ready.window.beats;
        let played = tokio::time::timeout_at(deadline, async {
            self.capture_transport(ready, CaptureTransport::Continue, signal.clone()).await?;
            loop {
                signal.check()?;
                if let Some(now) = link.transport(tap, Some(signal.clone())).await.map_err(|_| unreadable())? {
                    if now.running && now.beats >= end {
                        let playback = only(self.rows("session-playback", json!({}), signal.clone()).await?)?;
                        let transport = playback.get("transport").and_then(Value::as_object).ok_or_else(unreadable)?;
                        if !flag(transport, "playing")?
                            || number(transport, "position")? < end
                            || flag(transport, "arrangementRecord")?
                            || flag(transport, "sessionRecord")?
                        {
                            return Err(refused("capture_incomplete", "Kumi Ears reported an end the native playhead did not confirm."));
                        }
                        break;
                    }
                }
                delay(20.0, signal.clone()).await?;
            }
            Ok::<_, RuntimeError>(())
        })
        .await;
        if !self.capture_set_matches(&ready.state, stop_signal.clone()).await {
            return Err(unsure());
        }
        self.capture_transport(ready, CaptureTransport::Stop, stop_signal).await?;
        *started = false;
        played.map_err(|_| refused("capture_deadline", "The capture reached its independent deadline."))??;
        let raw = RawFile(directory.join(format!("{}.raw", uuid::Uuid::new_v4())));
        let written = link
            .write(tap, &raw.0.to_string_lossy().replace('\\', "/"), Some(signal.clone()))
            .await
            .map_err(|_| refused("capture_failed", "Kumi Ears could not write the capture."))?;
        if Path::new(&written.file) != raw.0 {
            return Err(refused("capture_failed", "Kumi Ears returned a file outside this call's requested capture."));
        }
        if written.channels != 4
            || !written.sample_rate.is_finite()
            || written.sample_rate <= 0.0
            || written.sample_rate != tap.sample_rate
            || !written.beats.is_finite()
        {
            return Err(unreadable());
        }
        let meta = tokio::fs::symlink_metadata(&raw.0).await.map_err(|_| unreadable())?;
        let frames = meta.len() / (written.channels as u64 * 4);
        if !meta.is_file()
            || meta.len() % (written.channels as u64 * 4) != 0
            || meta.len() > 150 * 1024 * 1024
            || frames as f64 / written.sample_rate > MAX_AUDIO_SECONDS
        {
            return Err(refused("capture_too_large", "The raw capture exceeded its bounded audio window."));
        }
        let capture = read_capture(&raw.0, written.channels, written.sample_rate).await.map_err(|_| unreadable())?;
        let window = ready.window;
        let (capture, first, last) = tokio::task::spawn_blocking(move || {
            let anchors = Anchors { first: Some(written.beats), after_jump: Some(window.from) };
            // Live needs verified beat coverage, so a short capture is refused rather
            // than published with A3's optional short-capture success notice.
            let covered = cover(&runs(&capture, anchors), window.from, window.beats, capture.sample_rate).map_err(|_| {
                refused("capture_incomplete", "Kumi Ears did not capture the complete approved beat window. No audio was published.")
            })?;
            let first = frame_at(&covered, window.from).ok_or_else(unreadable)?;
            let last = (first as f64 + (window.beats * covered.samples_per_beat).round()) as usize;
            if last > covered.to
                || last <= first
                || (last - first) as f64 / capture.sample_rate > MAX_AUDIO_SECONDS
                || (last - first) as u64 * 8 + 44 > MAX_WAV_BYTES
            {
                return Err(unreadable());
            }
            Ok::<_, RuntimeError>((capture, first, last))
        })
        .await
        .map_err(|_| unreadable())??;
        signal.check()?;
        let file = RawFile(directory.join(format!("{}.wav", uuid::Uuid::new_v4())));
        let seconds = (last - first) as f64 / capture.sample_rate;
        write_capture_wav(&file.0, &capture, first as f64, last as f64).await.map_err(|_| unreadable())?;
        Ok((file, seconds))
    }
}
