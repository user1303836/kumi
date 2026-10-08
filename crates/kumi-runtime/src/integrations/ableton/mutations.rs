//! Preview/apply changes and retire the references whose positions they moved.
use super::{
    change_context::{PreparationContext, SampleBank},
    changes::{laid_over, new_record, ChangeKind, CHANGES},
    connection::{ReadError, NO_CURRENT_LIVE},
    context::{self, ObservationError},
    history::{uncertain, Restore},
    observation::Observer,
    options::AbletonOptions,
    parameters::{ChangeOutcome, Parameters},
    references::Shift,
    views::ViewHost,
};
use crate::core::{contracts::*, errors::RuntimeError};
use indexmap::{IndexMap, IndexSet};
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::{
        json::stringify,
        number,
        string::{head, utf16_len},
    },
};
use regex::Regex;
use serde_json::{json, Value};
use std::{cell::RefCell, collections::HashSet, rc::Rc, sync::LazyLock};

pub struct Mutations {
    pub parameters: Rc<Parameters>,
    pub observer: Rc<Observer>,
    pub options: Rc<AbletonOptions>,
    pub samples: SampleBank,
    pub(crate) copied: RefCell<HashSet<String>>,
    /// The `@names` this answer's plans made (each the short ref of what its step made): a later plan in the answer
    /// uses them, so steps a plan refused are resent alone, not with everything they hang on (#259).
    pub names: RefCell<IndexMap<String, String>>,
}
impl Mutations {
    pub fn new(parameters: Rc<Parameters>, observer: Rc<Observer>, options: Rc<AbletonOptions>) -> Self {
        Self {
            parameters,
            observer,
            options,
            samples: SampleBank::default(),
            copied: RefCell::new(HashSet::new()),
            names: RefCell::new(IndexMap::new()),
        }
    }
    pub fn supported(&self, since: Option<&str>) -> bool {
        since.is_none_or(|since| super::bridge_version::at_least(self.parameters.history.connection.version().as_deref(), since))
    }
    /// What a change asks of the bridge that one from before Arrangement editing can't do (its clip move takes no
    /// keepSource), if it does. Said here, never sent: an older bridge would move an Arrangement clip on its own track
    /// whatever track it's given, and refuses an Arrangement clip's notes in words that would read like Kumi's fault.
    fn needs_newer_bridge(&self, kind: &ChangeKind, input: &JsonObject) -> Option<&'static str> {
        let tools = self.parameters.history.connection.tools()?;
        let move_tool = tools.tool("live_clip_move_preview");
        let edits_arrangement = move_tool
            .as_ref()
            .and_then(|tool| tool.input_schema.properties.as_ref())
            .is_some_and(|properties| properties.contains_key("keepSource"));
        if edits_arrangement || move_tool.is_none() {
            return None;
        }
        let arrangement = input.get("clipRef").and_then(Value::as_str).is_some_and(|clip| clip.contains(":arrangement_clip:"));
        match kind.tool.as_str() {
            "move_clip" if input.get("keepSource") == Some(&Value::Bool(true)) => Some("copying an Arrangement clip"),
            "move_clip" if arrangement && input.get("targetTrackRef").is_some_and(|track| !track.is_null()) => {
                Some("moving an Arrangement clip to another track")
            }
            "change_notes" | "delete_notes" | "edit_notes" | "transform_midi" if arrangement => Some("editing an Arrangement clip's notes"),
            _ => None,
        }
    }
    pub fn too_old(&self, since: Option<&str>) -> String {
        format!(
            "That needs the Ableton bridge {} or later; this one is {}. Tell the producer to update it (kumi doctor says how).",
            since.unwrap_or("undefined"),
            self.parameters.history.connection.version().as_deref().unwrap_or("older")
        )
    }
    pub async fn change(&self, kind: &ChangeKind, named: JsonObject, original: Signal, settled: bool) -> ChangeOutcome {
        // A change acting on a track's devices reads them again first, so what Kumi holds of them is current where it
        // acts, and is refused if a device it names isn't the one this turn showed. One that changes the devices
        // themselves (or renames one) has them read again on the next turn too, since Live tells of no device renamed.
        let acts = matches!(kind.family, ChangeFamily::Device | ChangeFamily::Parameter) || kind.tool == "set_chain_mixer";
        let changes = matches!(kind.family, ChangeFamily::Device | ChangeFamily::Rename);
        let (tracks, devices) =
            if acts || changes { self.observer.devices_named(&named, kind.family == ChangeFamily::Device) } else { (vec![], vec![]) };
        if acts {
            if let Err(text) = self.observer.refresh_devices(&tracks, &devices, original.clone()).await {
                return ChangeOutcome::error(text);
            }
        }
        let mut outcome = self.attempt(kind, named.clone(), original.clone(), settled).await;
        // Refused because what it fenced moved between its preview and its apply, nothing changed: a track or device
        // Live was still setting up (a default track's devices arriving, a Simpler taking its sample, #203). Asked
        // again once it's had a moment, it's previewed afresh (#259).
        if outcome.is_error && !outcome.stops && transient(&outcome.text) {
            tokio::select! { _ = original.cancelled() => {}, _ = tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)) => {} }
            if !original.is_cancelled() {
                if acts {
                    self.observer.devices_changed(&tracks);
                    if let Err(text) = self.observer.refresh_devices(&tracks, &devices, original.clone()).await {
                        return ChangeOutcome::error(text);
                    }
                }
                outcome = self.attempt(kind, named.clone(), original.clone(), true).await;
            }
        }
        // An audio clip's loop points are set_audio_clip's, which set_clip is refused for: asked there instead (#259).
        if kind.tool == "set_clip" && outcome.is_error && !outcome.stops && outcome.text.contains("audio clip loop editing uses") {
            if let (Some(audio), Some(loop_points)) = (CHANGES.iter().find(|k| k.tool == "set_audio_clip"), audio_loop_points(&named)) {
                outcome = self.attempt(audio, loop_points, original.clone(), settled).await;
                // set_audio_clip sets the points, not an audio clip's looping, and doesn't say whether it loops.
                if !outcome.is_error && named.get("looping") == Some(&json!(true)) {
                    if let Ok(Value::Object(mut reply)) = serde_json::from_str::<Value>(&outcome.text) {
                        reply.insert("looping".into(), json!(LOOPING_NOTE));
                        outcome.text = stringify(&json!(reply));
                    }
                }
            }
        }
        if changes {
            self.observer.devices_changed(&tracks);
        }
        if outcome.is_error {
            outcome.text = in_kumi_words(&outcome.text);
        }
        outcome
    }
    /// One try of a change. What fails here is Live's connection or the Set itself (gone, switched, unreadable), not
    /// this one change: a plan stops on it, where it goes on past a change Live refused.
    async fn attempt(&self, kind: &ChangeKind, named: JsonObject, original: Signal, settled: bool) -> ChangeOutcome {
        match self.try_change(kind, named, original, settled).await {
            Ok(result) => result,
            Err(ReadError::Observation(error)) => ChangeOutcome::stop(error.0),
            Err(ReadError::Other(_)) => {
                ChangeOutcome::stop("The change failed before anything happened in Live; discover again, then retry.")
            }
        }
    }
    async fn try_change(&self, kind: &ChangeKind, named: JsonObject, original: Signal, settled: bool) -> Result<ChangeOutcome, ReadError> {
        let history = &self.parameters.history;
        let connection = &history.connection;
        let signal = abort::any([original, connection.lifetime.clone()]);
        let input = context::object(&connection.references.borrow().lengthen(&json!(named)))?;
        let lease = connection.lease.get();
        signal.check()?;
        if !connection.available.get() || connection.lost.get() || connection.epoch.get().is_none() || connection.tools().is_none() {
            return Err(observation(NO_CURRENT_LIVE));
        }
        connection.ensure_catalog(signal.clone()).await?;
        connection.assert_lease(lease, &signal)?;
        if !kind.available(|tool| connection.has(tool)) {
            connection.guard_epoch(signal.clone(), connection.epoch.get().unwrap(), lease).await?;
            connection.tools().unwrap().refresh(signal.clone()).await?;
            connection.assert_lease(lease, &signal)?;
        }
        // Refusals of this change alone: a plan goes on past them.
        if !kind.available(|tool| connection.has(tool)) {
            return Ok(ChangeOutcome::error(
                kind.unavailable.as_deref().unwrap_or("That change isn't available for the open Set right now"),
            ));
        }
        if !self.supported(kind.since.as_deref()) {
            return Ok(ChangeOutcome::error(self.too_old(kind.since.as_deref())));
        }
        if let Some(what) = self.needs_newer_bridge(kind, &input) {
            return Ok(ChangeOutcome::error(format!(
                "That needs a newer Ableton bridge than this one ({}): {what}. Tell the producer to update it (kumi doctor says how).",
                connection.version().as_deref().unwrap_or("older")
            )));
        }
        if history.changes_this_turn.get() >= 5_000 {
            return Err(observation("That's 5000 changes in one answer; carry on in the next one"));
        }
        if let Err(stale) = connection.references.borrow().require_fresh_references(&input) {
            return Ok(ChangeOutcome::error(stale.0));
        }
        // Notes written in Kumi's notation become the notes Live takes; its mistakes come back as the change's error, and
        // the clips of a several-clip write that have none are written (#257).
        let super::notes::Expanded { mut input, fixed: read_for_itself, unwritten } =
            match super::notes::expand(&kind.tool, input, connection, self.observer.tempo.get(), &signal).await {
                Ok(expanded) => expanded,
                Err(text) => return Ok(ChangeOutcome::error(text)),
            };
        // Live's compressor sidechain is fed by a track (routingType) and Kumi sets nothing more there, so a channel
        // given with it is left out rather than the change refused (#259).
        let channel_left = (kind.tool == "set_sidechain" && input.get("action").and_then(Value::as_str) == Some("sidechain"))
            .then(|| input.remove("routingChannel"))
            .flatten();
        if kind.tool == super::modulation::MAP_MODULATOR {
            return self.map_modulator(kind, &input, signal).await;
        }
        if kind.family == ChangeFamily::Parameter && self.parameters.fast_on() {
            return Ok(self.parameters.fast_parameters(kind, &input, signal).await?);
        }
        let prepared = if kind.has("prepare") {
            match kind
                .prepare(&input, &PreparationContext { parameters: &self.parameters, samples: &self.samples, signal: signal.clone() })
                .await?
            {
                Ok(args) => args,
                Err(text) => return Ok(ChangeOutcome::error(text)),
            }
        } else {
            input
        };
        connection.assert_lease(lease, &signal)?;
        let epoch = connection.epoch.get().ok_or_else(|| observation(NO_CURRENT_LIVE))?;
        if !settled {
            connection.guard_epoch(signal.clone(), epoch, lease).await?;
        }
        let args = self.append_at_end(kind, prepared, signal.clone()).await?;
        connection.assert_lease(lease, &signal)?;
        let previewed = connection.call(&kind.preview, args.clone(), signal.clone()).await?;
        connection.assert_lease(lease, &signal)?;
        if previewed.is_error == Some(true) {
            let text = stringify(&serde_json::to_value(&previewed).unwrap());
            let more = if kind.has("explain") {
                kind.explain(&text, &args, &PreparationContext { parameters: &self.parameters, samples: &self.samples, signal })
                    .await
                    .ok()
                    .flatten()
                    .filter(|s| !s.is_empty())
            } else {
                None
            };
            return Ok(ChangeOutcome::error(format!("{text}{}", more.map(|s| format!(" {s}")).unwrap_or_default())));
        }
        let mut preview = context::payload(&previewed)?;
        if let Some(given) = preview.get("epoch") {
            connection.assert_epoch(Some(given), epoch)?;
        }
        let transaction = preview.get("transactionId").and_then(Value::as_str).filter(|s| !s.is_empty() && utf16_len(s) <= 256);
        let confirmation = preview.get("confirmation").and_then(Value::as_str).filter(|s| !s.is_empty() && utf16_len(s) <= 512);
        let (Some(transaction), Some(confirmation)) = (transaction, confirmation) else {
            return Ok(ChangeOutcome::error("The bridge's preview was malformed; nothing was changed"));
        };
        let (transaction, confirmation) = (transaction.to_owned(), confirmation.to_owned());
        let known = |reference: &Value| reference.as_str().and_then(|r| connection.references.borrow().known.get(r).cloned());
        // A new Arrangement clip is laid over the clips it lands on, as Live does: they're read first, to say which (an
        // audio clip's own length shows once it's made). A clip in a take lane lands on none: the bridge refuses a span
        // a clip in the lane is in.
        let main_lane = kind.tool == "add_arrangement_clip" && !args.contains_key("takeLaneRef");
        let under = if main_lane { self.arrangement_clips_of(&args, &signal).await } else { None };
        if let (Some(under), Some(start), Some(length)) = (&under, beats(args.get("position")), beats(args.get("length"))) {
            preview.insert("replaces".into(), json!(laid_over(under, start, start + length)));
        }
        // What the change will cut or delete, read first, so Kumi's undo can make it again (history v0).
        let cutting = super::cuts::before(history, kind, &args, &preview, under.as_deref(), &signal).await;
        let summary = kind.summarize(&preview, &args, &known, None);
        signal.check()?;
        history.changes_this_turn.set(history.changes_this_turn.get() + 1);
        let applied = match connection
            .call(
                &kind.apply,
                object(json!({"transactionId":transaction,"confirmation":confirmation,"idempotencyKey":uuid::Uuid::new_v4().to_string()})),
                history.change_signal_for(items_of(&args)),
            )
            .await
        {
            Ok(result) => result,
            Err(_) => {
                history.remember(
                    new_record(kind, summary, ChangeState::Unsure, connection.now().timestamp_millis()),
                    transaction.into(),
                    None,
                );
                return Ok(ChangeOutcome::stop("Live didn't confirm this change, so it may or may not have happened. Tell the producer to check Live; discover again before more changes."));
            }
        };
        if applied.is_error == Some(true) {
            let text = stringify(&serde_json::to_value(&applied).unwrap());
            if !uncertain(&applied) {
                return Ok(ChangeOutcome::error(text));
            }
            history.remember(new_record(kind, summary, ChangeState::Unsure, connection.now().timestamp_millis()), transaction.into(), None);
            return Ok(ChangeOutcome::stop(format!("Live couldn't confirm this change: {text}")));
        }
        let mut result = match context::payload(&applied) {
            Ok(value) => value,
            Err(_) => {
                history.remember(
                    new_record(kind, summary, ChangeState::Unsure, connection.now().timestamp_millis()),
                    transaction.into(),
                    None,
                );
                return Ok(ChangeOutcome::stop("Kumi couldn't read Live's answer to this change, so it can't confirm whether it happened. Tell the producer to check Live; discover again before more changes."));
            }
        };
        if main_lane {
            // Whether Kumi's own undo can take the new clip back hangs on what it cut: the track's clips are read again
            // and compared with the read before. A read that fails, or a clip cut past what that read explains, leaves
            // the undo to Live's (deleting the new clip wouldn't bring back what it cut).
            let after = self.arrangement_clips_of(&args, &signal).await;
            let made = result.get("result");
            match (&under, &after, beats(made.and_then(|m| m.get("start"))), beats(made.and_then(|m| m.get("length")))) {
                (Some(under), Some(after), Some(start), Some(length)) => {
                    let replaces = laid_over(under, start, start + length);
                    if replaces.is_empty() && cut(under, after) {
                        preview.insert("replacesUnknown".into(), json!(true));
                    }
                    preview.insert("replaces".into(), json!(replaces));
                }
                _ => {
                    preview.insert("replacesUnknown".into(), json!(true));
                }
            }
        }
        // A move or copy that cut something its preview didn't name (a clip put in its new place in between): the
        // bridge says so, and Kumi's undo can't put that back.
        if kind.tool == "move_clip" && result.get("replacesUnknown") == Some(&json!(true)) {
            preview.insert("replacesUnknown".into(), json!(true));
        }
        let final_summary = kind.summarize(&preview, &args, &known, Some(&result));
        let applied = result.get("state").and_then(Value::as_str) == Some("applied");
        // What Live left of what it cut, read now: with it, Kumi's undo makes the clips again, so the change isn't
        // Live's alone to take back. A cut past what the read before explains leaves it to Live.
        let kept = match cutting.filter(|_| applied && preview.get("replacesUnknown") != Some(&json!(true))) {
            Some(cutting) => {
                let made = match kind.tool.as_str() {
                    "add_arrangement_clip" => result.get("result"),
                    "move_clip" => result.get("created"),
                    _ => None,
                };
                let made_ref = made.and_then(|m| Some((m.get("ref")?.as_str()?, m.get("objectIdentity")?.as_str()?)));
                let span = (kind.tool == "add_arrangement_clip")
                    .then(|| Some((beats(made?.get("start"))?, beats(made?.get("start"))? + beats(made?.get("length"))?)))
                    .flatten();
                cutting.after(history, made_ref, span, &signal).await
            }
            None => None,
        };
        let permanent = if applied && kept.is_none() {
            kind.permanent(&args).or_else(|| kind.replaced(&preview)).filter(|s| !s.is_empty())
        } else {
            None
        };
        let mut record = new_record(
            kind,
            final_summary.clone(),
            if applied {
                if permanent.is_some() {
                    ChangeState::Kept
                } else {
                    ChangeState::Applied
                }
            } else {
                ChangeState::Unsure
            },
            connection.now().timestamp_millis(),
        );
        record.note = permanent.clone();
        let field = match kind.family {
            ChangeFamily::Rename => Some("name"),
            ChangeFamily::Color => Some("color"),
            _ => None,
        };
        let restore = field.and_then(|field| {
            args.get("ref").and_then(Value::as_str).and_then(|reference| {
                connection.references.borrow().known.get(reference).map(|track| Restore {
                    reference: reference.into(),
                    field: field.into(),
                    value: if field == "name" { Some(track.name.clone()) } else { track.color.clone() },
                })
            })
        });
        history.remember(record.clone(), transaction.into(), restore);
        history.made_by(&record.id, &kind.tool);
        // The device a load or a duplicate made, by Live's identity for it: what may be removed if Live won't undo it.
        if applied && matches!(kind.tool.as_str(), "load_device" | "duplicate_device" | "load_sample") {
            let identity = [
                result.get("deviceObjectIdentity"),
                record_of(result.get("created")).get("deviceObjectIdentity"),
                record_of(result.get("created")).get("objectIdentity"),
                record_of(result.get("result")).get("objectIdentity"),
            ]
            .into_iter()
            .flatten()
            .find_map(|value| match value {
                Value::String(text) if !text.is_empty() => Some(text.clone()),
                Value::Number(number) => Some(number.to_string()),
                _ => None,
            });
            if let Some(identity) = identity {
                history.made(&record.id, identity);
            }
        }
        if let Some((mut material, clips)) = kept {
            let view = json!({"change":record.id,"tool":kind.tool,"track":material.track,"remnants":material.remnants});
            let remember = &history.remember;
            let objects =
                remember.history.keep(remember.store.as_ref(), remember.current().as_deref(), &clips, &record.title, view, record.at);
            material.clips = super::cuts::kept(&clips, &objects);
            // The bridge's answer says only Live's undo brings this back. With what Kumi kept, its own undo does, and
            // what that won't bring back is said now.
            result.insert("kept".into(), json!(kumi_undo_words(&material.names(), &clips)));
            history.attach_material(&record.id, material);
        }
        if kind.tool == "set_tempo" && record.state == ChangeState::Applied {
            if let Some(tempo) = args.get("tempo").and_then(Value::as_f64) {
                self.observer.tempo.set(Some(tempo));
            }
        }
        if let Some(reference) = args.get("ref").and_then(Value::as_str) {
            if let Some(track) = connection.references.borrow_mut().known.get_mut(reference) {
                if kind.family == ChangeFamily::Rename {
                    if let Some(renamed) = summary.track {
                        track.name = renamed.name;
                    }
                }
                if kind.family == ChangeFamily::Color {
                    if let Some(colors) = &record.colors {
                        track.color = Some(colors.to.clone());
                    }
                }
            }
        }
        // Whether the refs past a restructure followed what they named (or were all retired), and whether every
        // track's sends were renumbered with the returns.
        let mut followed = false;
        let mut sends = false;
        if kind.restructures == Some(true) {
            let created: Vec<_> = result
                .get("created")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|v| v.as_object().cloned().unwrap_or_default())
                .collect();
            let shift = restructure_shift(&kind.tool, &args, &preview, &result, &created);
            followed = shift.is_some();
            sends = shift.as_ref().is_some_and(|shift| shift.sends);
            let mut book = connection.references.borrow_mut();
            book.cursors.clear();
            match &shift {
                // The refs past the change follow what they named, so a plan's later steps (deleting the next track,
                // setting a device on a track after the new one) still find theirs (#261, #253). So do the ones
                // HISTORY's undo names.
                Some(shift) => {
                    book.shift(shift);
                    book.mark_moved(history.shifted(shift));
                    history.restructured(&record.id, shift);
                }
                None => {
                    book.refs.clear();
                    book.known.clear();
                    book.clear_names();
                }
            }
            drop(book);
            if let Some(shift) = &shift {
                self.observer.shifted(shift);
            } else {
                self.observer.forget_devices();
            }
            let mut book = connection.references.borrow_mut();
            for row in &created {
                if let Some(reference) = row.get("ref").and_then(Value::as_str) {
                    book.retire(reference);
                }
            }
            for row in &created {
                if let Some(reference) = row.get("ref").and_then(Value::as_str).filter(|s| utf16_len(s) <= 256) {
                    if let Some(kind) = row.get("kind").and_then(Value::as_str).filter(|s| matches!(*s, "track" | "scene")) {
                        book.refs.insert(reference.into(), kind.into());
                        book.registered(reference);
                        if kind == "track" {
                            if let Some(name) = row.get("name").and_then(Value::as_str) {
                                book.known.insert(reference.into(), TrackChip { name: head(name, 256), color: None });
                            }
                        }
                    }
                }
            }
        }
        let shifted = matches!(kind.tool.as_str(), "move_device" | "move_device_to" | "delete_device");
        if shifted || kind.restructures == Some(true) {
            self.parameters.forget();
        }
        let mut devices_now = None;
        if shifted {
            let mut tracks = Vec::new();
            for key in ["deviceRef", "ref", "targetTrackRef", "targetChainRef"] {
                if let Some(index) = args.get(key).and_then(Value::as_str).and_then(track_index_of) {
                    if !tracks.contains(&index) {
                        tracks.push(index);
                    }
                }
            }
            {
                let mut book = connection.references.borrow_mut();
                let references: IndexSet<_> = book.refs.keys().cloned().chain(book.named_references()).collect();
                static DEVICE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r":(?:device|parameter|chain|drum_pad):").unwrap());
                for reference in references {
                    if DEVICE.is_match(&reference)
                        && !reference.contains(":mixer:")
                        && track_index_of(&reference).is_some_and(|index| tracks.contains(&index))
                    {
                        book.retire(&reference);
                    }
                }
            }
            devices_now = self.devices_of(&tracks, signal).await.ok().flatten();
        }
        let produced = kind.produces(&result);
        if let Some(produced) = &produced {
            if utf16_len(&produced.reference) <= 256 {
                let mut book = connection.references.borrow_mut();
                if kind.restructures != Some(true) {
                    book.unname(&produced.reference);
                }
                book.refs.insert(produced.reference.clone(), produced.kind.clone());
                book.registered(&produced.reference);
            }
        }
        let mut reply = object(json!({"changed":record.title,"change":record.id,"state":record.state}));
        if let Some(produced) = produced {
            reply.insert("ref".into(), json!(connection.references.borrow_mut().short_ref(&produced.reference)));
        }
        if let Some(lines) = final_summary.lines.filter(|lines| !lines.is_empty()) {
            reply.insert("lines".into(), json!(lines));
        }
        if kind.restructures == Some(true) {
            reply.insert(
                "note".into(),
                json!(if followed && sends {
                    "Tracks after this moved (returns and Main among them), and Kumi moved their references with them: references from earlier in this answer still name the same tracks, scenes, clips and devices, but every track's sends were renumbered with the returns, so earlier send references are retired; read a track's mixer again for them. The new ones in live.created are current."
                } else if followed {
                    "Tracks and scenes after this moved (returns and Main among them), and Kumi moved their references with them: references from earlier in this answer still name the same tracks, scenes, clips and devices (not what was deleted), and the new ones in live.created are current."
                } else {
                    "Track and scene positions moved; discover again before using earlier references (the new ones in live.created are current)."
                }),
            );
        }
        if shifted {
            if let Some(devices) = devices_now {
                reply.insert("devicesNow".into(), json!(devices));
                reply.insert("note".into(),json!("Devices on the tracks involved moved along their chains: devicesNow has each track's devices as they are now, with current references; earlier device references on those tracks are retired (discover inside racks again)."));
            } else {
                reply.insert("note".into(),json!("Devices on the tracks involved moved along their chains: discover them (and their parameters) again before using earlier references."));
            }
        }
        // A device file of the producer's or Kumi's, loaded from the User Library: what hides its face, which the
        // model can't see in Live (#179).
        if kind.tool == "load_device" && record.state == ChangeState::Applied {
            if let Some(item) = args.get("itemId").and_then(Value::as_str).map(str::to_owned) {
                let library = self.options.user_library.clone().unwrap_or_else(|| super::samples::user_library(None, None));
                // Read off the runtime's thread: a device with samples frozen in can be megabytes.
                let note = tokio::task::spawn_blocking(move || crate::devices::face::face_note(std::path::Path::new(&library), &item))
                    .await
                    .ok()
                    .flatten();
                if let Some(note) = note {
                    reply.insert("face".into(), json!(note));
                }
            }
        }
        if !read_for_itself.is_empty() {
            reply.insert("notation".into(), json!(read_for_itself));
        }
        if let Some(channel) = channel_left {
            reply.insert(
                "channel".into(),
                json!(format!(
                    "The sidechain takes the source track; Kumi doesn't set its channel, so it's Live's own choice, not {}.",
                    stringify(&channel)
                )),
            );
        }
        if !unwritten.is_empty() {
            reply.insert("missed".into(), json!(unwritten));
            reply.insert(
                "missedNote".into(),
                json!("These clips weren't written, for the mistakes in their notation; the others were. Fix them and write only those."),
            );
        }
        let mut full = reply.clone();
        full.insert("live".into(), connection.references.borrow_mut().shorten(&json!(result)));
        let text = stringify(&json!(full));
        Ok(ChangeOutcome {
            text: if text.len() <= 16 * 1024 { text } else { stringify(&json!(reply)) },
            is_error: record.state != ChangeState::Applied && permanent.is_none(),
            missed: (!unwritten.is_empty()).then_some(unwritten.len()),
            stops: record.state == ChangeState::Unsure,
        })
    }
    async fn append_at_end(&self, kind: &ChangeKind, input: JsonObject, signal: Signal) -> Result<JsonObject, ReadError> {
        let lacks = |key: &str| {
            input
                .get(key)
                .and_then(Value::as_array)
                .is_some_and(|items| items.iter().any(|item| item.is_object() && !item.as_object().unwrap().contains_key("index")))
        };
        if kind.family != ChangeFamily::Structure || (!lacks("tracks") && !lacks("scenes")) {
            return Ok(input);
        }
        let probe = self.parameters.history.connection.call(&kind.preview, input.clone(), signal).await?;
        if probe.is_error == Some(true) {
            return Ok(input);
        }
        let prior = context::object(context::payload(&probe)?.get("prior").unwrap_or(&Value::Null))?;
        let mut out = input;
        for key in ["tracks", "scenes"] {
            let count = prior.get(key).and_then(Value::as_array).map_or(0, Vec::len);
            if let Some(Value::Array(items)) = out.get_mut(key) {
                for (index, item) in items.iter_mut().enumerate() {
                    if let Some(row) = item.as_object_mut() {
                        if !row.contains_key("index") {
                            row.insert("index".into(), json!(count + index));
                        }
                    }
                }
            }
        }
        Ok(out)
    }
    /// The Arrangement clips on the track a change names (name and span), or None when they can't be read.
    async fn arrangement_clips_of(&self, args: &JsonObject, signal: &Signal) -> Option<Vec<JsonObject>> {
        let track = args.get("trackRef").and_then(Value::as_str)?;
        let read = object(json!({"parent":track,"fields":["name","start","endTime","length","objectIdentity"]}));
        self.parameters.history.connection.rows("arrangement-clip", read, signal.clone()).await.ok()
    }
    async fn devices_of(&self, tracks: &[f64], signal: Signal) -> Result<Option<JsonObject>, RuntimeError> {
        let connection = &self.parameters.history.connection;
        let Some(epoch) = connection.epoch.get() else { return Ok(None) };
        if tracks.is_empty() || tracks.len() > 4 {
            return Ok(None);
        }
        let mut now = JsonObject::new();
        for index in tracks {
            let reference = format!("{}:track:{}", number::to_string(epoch), number::to_string(*index));
            let read = connection
                .invoke(
                    "live_discover",
                    object(json!({"kind":"device","parent":reference,"fields":["ref","name","className"]})),
                    signal.clone(),
                )
                .await;
            if read.is_error {
                return Ok(None);
            }
            let parsed: Value = serde_json::from_str(&read.text).map_err(|e| RuntimeError::plain(e.to_string()))?;
            let live = context::object(parsed.get("live").unwrap_or(&Value::Null))?;
            let mut row = JsonObject::new();
            if let Some(name) =
                connection.references.borrow().known.get(&reference).map(|track| track.name.clone()).filter(|s| !s.is_empty())
            {
                row.insert("track".into(), json!(name));
            }
            row.insert("devices".into(), live.get("items").filter(|v| v.is_array()).cloned().unwrap_or(json!([])));
            let short = connection.references.borrow_mut().short_ref(&reference);
            now.insert(short, json!(row));
        }
        Ok(Some(now))
    }
}
/// Whether a clip read before a change is cut or gone in the read after (each told by its identity).
fn cut(before: &[JsonObject], after: &[JsonObject]) -> bool {
    let span = |clip: &JsonObject| (beats(clip.get("start")), beats(clip.get("endTime")));
    before.iter().any(|clip| {
        let identity = clip.get("objectIdentity").filter(|identity| identity.is_string());
        identity.is_none() || !after.iter().any(|other| other.get("objectIdentity") == identity && span(other) == span(clip))
    })
}
/// How long a refused change waits for Live to finish setting up what it fenced before it's asked again.
const SETTLE_MS: u64 = 600;
/// A refusal because what the change fenced moved between its preview and its apply, which a fresh preview a moment
/// later may not meet: Live still setting up a track or device it just made.
fn transient(text: &str) -> bool {
    ["changed since preview", "did not confirm the exact requested", "hierarchy is stale"].iter().any(|said| text.contains(said))
}
/// What a plan's done row says when set_clip asked an audio clip to loop: set_audio_clip sets only its loop points.
const LOOPING_NOTE: &str = "The loop points are set, but Kumi can't switch an audio clip's looping: if it isn't looping already, tell the producer to turn Loop on in its Clip View.";
/// set_clip's loop points for an audio clip, as set_audio_clip takes them, when that's all it changes (an audio clip's
/// looping itself isn't Kumi's to switch).
fn audio_loop_points(named: &JsonObject) -> Option<JsonObject> {
    let points = ["loopStart", "loopEnd"];
    let only =
        named.keys().all(|key| key == "clipRef" || points.contains(&key.as_str()) || (key == "looping" && named[key] == json!(true)));
    (only && points.iter().any(|key| named.contains_key(*key))).then(|| {
        named.iter().filter(|(key, _)| *key == "clipRef" || points.contains(&key.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect()
    })
}
/// A refusal in the model's words: the bridge's own tools it names (live_audio_clip_preview) are Kumi's tools that
/// make those changes (set_audio_clip), where one does, since the model can't call the bridge's (#259).
fn in_kumi_words(text: &str) -> String {
    static TOOLS: LazyLock<Vec<(String, String)>> = LazyLock::new(|| {
        let mut by_bridge: IndexMap<String, IndexSet<String>> = IndexMap::new();
        for kind in CHANGES.iter().filter(|kind| kind.internal != Some(true)) {
            for bridge in [&kind.preview, &kind.apply] {
                by_bridge.entry(bridge.clone()).or_default().insert(kind.tool.clone());
            }
        }
        by_bridge.into_iter().filter(|(_, tools)| tools.len() == 1).map(|(bridge, tools)| (bridge, tools[0].clone())).collect()
    });
    static NAMED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\blive_[a-z_]+_(?:preview|apply)\b").unwrap());
    let said = NAMED
        .replace_all(text, |found: &regex::Captures| {
            TOOLS.iter().find(|(bridge, _)| bridge == &found[0]).map_or_else(|| found[0].to_owned(), |(_, tool)| tool.clone())
        })
        .into_owned();
    // Live gives scripts only the 16 pads a Drum Rack shows (#259): the model can pick one of those in its next plan
    // rather than try the same note again.
    if said.contains("isn't among the rack's visible pads") && !said.contains("drumPads") {
        return format!("{said}. Kumi can reach only the 16 pads the rack shows in Live, the notes in its drumPads (C1 to D#2, 36 to 51, unless it's scrolled): use one of those, or ask the producer to scroll the rack's pads to this note in Live first.");
    }
    said
}
/// How many things a change makes or sets at once (tracks and scenes, pads, clips, values), for its time in Live.
fn items_of(args: &JsonObject) -> usize {
    ["tracks", "scenes", "pads", "clips", "values", "chains"]
        .iter()
        .filter_map(|key| args.get(*key).and_then(Value::as_array))
        .map(Vec::len)
        .sum::<usize>()
        .max(1)
}
/// A finite number of beats, if the value is one.
fn beats(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|n| n.is_finite())
}
/// What a restructure did to the Set's tracks and scenes, from what it was asked and what Live answered: None when
/// that can't be told exactly, and every ref is retired instead.
fn restructure_shift(tool: &str, args: &JsonObject, preview: &JsonObject, result: &JsonObject, created: &[JsonObject]) -> Option<Shift> {
    static TRACK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9]+:track:([0-9]+)$").unwrap());
    static SCENE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9]+:scene:([0-9]+)$").unwrap());
    let index = |reference: Option<&Value>, pattern: &Regex| {
        reference.and_then(Value::as_str).and_then(|reference| pattern.captures(reference)).and_then(|m| m[1].parse::<usize>().ok())
    };
    let mut shift = Shift::default();
    match tool {
        "add_tracks_and_scenes" | "group_tracks" => {
            for row in created {
                match (index(row.get("ref"), &TRACK), index(row.get("ref"), &SCENE)) {
                    (Some(track), _) => shift.tracks_made.push(track),
                    (_, Some(scene)) => shift.scenes_made.push(scene),
                    _ => return None,
                }
            }
        }
        "capture_scene" => shift.scenes_made.push(index(result.get("created").and_then(|made| made.get("sceneRef")), &SCENE)?),
        // A group track takes the tracks in it, which follow it.
        "delete_track" => {
            let at = index(args.get("trackRef"), &TRACK)?;
            let inside = preview.get("track").and_then(|track| track.get("alsoDeletes")).and_then(Value::as_array).map_or(0, Vec::len);
            shift.tracks_gone = (at..=at + inside).collect();
        }
        "delete_scene" => shift.scenes_gone.push(index(args.get("sceneRef"), &SCENE)?),
        "change_structure" => {
            let made = result.get("result").and_then(|made| made.get("ref"));
            match args.get("action").and_then(Value::as_str)? {
                "create-return" | "duplicate-track" => shift.tracks_made.push(index(made, &TRACK)?),
                "duplicate-scene" => shift.scenes_made.push(index(made, &SCENE)?),
                "delete-return" => {
                    shift.tracks_gone.push(index(args.get("ref"), &TRACK)?);
                    shift.sends = true;
                }
                _ => return None,
            }
        }
        _ => return None,
    }
    (!shift.is_empty()).then_some(shift)
}
/// Whether a device is on a track, by their refs read long (a short ref is a counter, not a place).
pub fn same_track(book: &super::references::References, device: &str, track: &str) -> bool {
    let long = book.lengthen(&json!({"deviceRef":device,"trackRef":track}));
    let (Some(device), Some(track)) = (long["deviceRef"].as_str(), long["trackRef"].as_str()) else { return false };
    track.contains(":track:") && track_index_of(device).is_some_and(|index| Some(index) == track_index_of(track))
}
pub fn track_index_of(reference: &str) -> Option<f64> {
    static MATCH: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r":(?:track|clip_slot|clip|arrangement_clip|device|chain|drum_pad|routing_choice|take_lane|mixer):([0-9]+)").unwrap()
    });
    MATCH.captures(reference).and_then(|m| number::parse(&m[1]))
}
fn observation(message: &str) -> ReadError {
    ReadError::Observation(ObservationError(message.into()))
}
fn object(value: Value) -> JsonObject {
    value.as_object().cloned().unwrap_or_default()
}

/// What Kumi's undo of a change brings back, as the change's answer says it: by name, and what of them it won't.
fn kumi_undo_words(names: &str, clips: &[super::snapshots::Captured]) -> String {
    let short: Vec<String> = clips
        .iter()
        .filter(|clip| !clip.short_of().is_empty())
        .map(|clip| format!("\u{201c}{}\u{201d} without {}", clip.name(), super::snapshots::join(&clip.short_of())))
        .collect();
    if short.is_empty() {
        return format!("Kumi's undo brings back {names}.");
    }
    format!(
        "Kumi's undo brings back {names}, {} (Live doesn't give Kumi those). Live's own undo (Cmd-Z in Live), right away, brings back all of it.",
        short.join("; ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clip_cut_or_gone_since_the_read_before_is_seen() {
        let clip = |identity: &str, start: f64, end: f64| object(json!({"objectIdentity":identity,"start":start,"endTime":end}));
        let before = [clip("a", 0., 8.), clip("b", 16., 24.)];
        // The same clips, and a new one: nothing cut.
        assert!(!cut(&before, &[clip("a", 0., 8.), clip("b", 16., 24.), clip("new", 8., 12.)]));
        // A clip cut back, or gone: cut.
        assert!(cut(&before, &[clip("a", 0., 8.), clip("b", 16., 20.)]));
        assert!(cut(&before, &[clip("a", 0., 8.)]));
        // A clip read without its identity can't be told: taken as cut.
        assert!(cut(&[object(json!({"start":0.,"endTime":8.}))], &[clip("a", 0., 8.)]));
    }

    #[test]
    fn a_refusal_is_in_the_models_words_and_a_fence_that_moved_is_asked_again() {
        // The bridge's own tool, which the model can't call, is the Kumi tool that makes that change (#259).
        assert_eq!(in_kumi_words("audio clip loop editing uses live_audio_clip_preview"), "audio clip loop editing uses set_audio_clip");
        // One several of Kumi's tools share is left as it is, and so are the tools the model has.
        assert_eq!(in_kumi_words("use live_device_preview"), "use live_device_preview");
        assert_eq!(in_kumi_words("read it with live_discover"), "read it with live_discover");
        // A pad Live doesn't show says which ones Kumi can reach.
        let pad = in_kumi_words("drum pad note 70 isn't among the rack's visible pads");
        assert!(
            pad.starts_with("drum pad note 70 isn't among the rack's visible pads. Kumi can reach only the 16 pads")
                && pad.contains("drumPads"),
            "{pad}"
        );
        // What moved between preview and apply is asked again; a refusal on its own merits isn't.
        assert!(transient("device insertion did not confirm the exact requested name, index, and siblings"));
        assert!(transient("simpler sample state changed since preview"));
        assert!(!transient("a note at 9|1 ends after the clip"));
        // An audio clip's loop points go to set_audio_clip when they're all set_clip was asked; looping off, or another
        // setting, can't go with them.
        let points = |value: Value| audio_loop_points(&object(value));
        assert_eq!(
            points(json!({"clipRef":"c","looping":true,"loopStart":0,"loopEnd":4})),
            Some(object(json!({"clipRef":"c","loopStart":0,"loopEnd":4})))
        );
        assert_eq!(points(json!({"clipRef":"c","loopEnd":4,"muted":true})), None);
        assert_eq!(points(json!({"clipRef":"c","looping":false,"loopEnd":4})), None);
        assert_eq!(points(json!({"clipRef":"c","looping":true})), None);
    }

    #[test]
    fn a_change_of_many_things_gets_time_for_each() {
        let items = |value: Value| items_of(&object(value));
        assert_eq!(items(json!({"tracks":[{}, {}], "scenes":[{}]})), 3);
        assert_eq!(items(json!({"pads":(0..16).map(|n| json!({"note":n})).collect::<Vec<_>>()})), 16);
        assert_eq!(items(json!({"tempo":124})), 1);
    }
}

/// A JSON value as an object (an empty one when it isn't).
fn record_of(value: Option<&Value>) -> JsonObject {
    value.and_then(Value::as_object).cloned().unwrap_or_default()
}
