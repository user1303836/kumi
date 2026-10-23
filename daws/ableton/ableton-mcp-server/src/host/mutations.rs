//! Shared in-flight mutation ownership, preview fences, and Live's own undo history.
use super::*;
use futures::{
    future::{LocalBoxFuture, Shared},
    Future, FutureExt,
};
use kumi_common::{abort::Signal, js::json as js_json};
use sha2::{Digest, Sha256};

pub(super) type MutationOutcome = Result<Option<Value>, LiveError>;
pub(super) struct MutationFlight {
    idempotency_key: String,
    argument_digest: String,
    signal: Signal,
    waiters: Cell<usize>,
    settled: Cell<bool>,
    promise: Shared<LocalBoxFuture<'static, MutationOutcome>>,
}
/// What a flight marks while its operation runs, cleared however the flight ends: an operation that panics would
/// otherwise leave its transaction in flight for good (so finalization refuses it) and its flight joinable.
struct FlightCleanup {
    host: Rc<McpHost>,
    flight: Rc<MutationFlight>,
    identity: String,
    transaction_id: Option<String>,
    /// The operation came back: its own handler settled the transaction's state.
    finished: bool,
}
impl Drop for FlightCleanup {
    // This runs while a panic unwinds too, where a second panic aborts: every borrow here is a `try_borrow`.
    fn drop(&mut self) {
        let host = &self.host;
        host.active_async_operations.set(host.active_async_operations.get() - 1);
        self.flight.settled.set(true);
        if let Some(id) = &self.transaction_id {
            // A panic cut the work short past its handler, which would have made `applying` or `undoing` uncertain:
            // the transaction is uncertain now, for a same-key retry to reconcile or for finalization.
            if !self.finished {
                if let Some(mut record) = host.try_transaction_record(id).as_ref().and_then(|record| record.try_borrow_mut().ok()) {
                    if retention::ACTIVE_TRANSACTION_STATES.contains(&record["state"].as_str().unwrap_or("")) {
                        record["state"] = json!("uncertain");
                    }
                }
            }
            retention::clear_in_flight(id);
        }
        let ours = host
            .in_flight_mutations
            .try_borrow()
            .is_ok_and(|flights| flights.get(&self.identity).is_some_and(|flight| Rc::ptr_eq(flight, &self.flight)));
        if ours {
            if let Ok(mut flights) = host.in_flight_mutations.try_borrow_mut() {
                flights.remove(&self.identity);
            }
        }
    }
}
struct Waiter(Rc<MutationFlight>);
impl Drop for Waiter {
    fn drop(&mut self) {
        self.0.waiters.set(self.0.waiters.get() - 1);
        if self.0.waiters.get() == 0 && !self.0.settled.get() {
            self.0.signal.cancel();
        }
    }
}
struct Active<'a>(&'a Cell<usize>);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

pub fn result_body(outcome: &Value) -> Option<Value> {
    let body: Value = serde_json::from_str(outcome.get("result")?.get("content")?.as_array()?.first()?.get("text")?.as_str()?).ok()?;
    body.is_object().then_some(body)
}
fn copied_fields(output: &mut Value, source: &Value, keys: &[(&str, &str)]) {
    for (to, from) in keys {
        if let Some(value) = source.get(*from) {
            output[*to] = value.clone();
        }
    }
}
pub fn parameter_mutation_args(transaction: &Value, value: f64, expected_revision: f64) -> Value {
    let mut args = json!({});
    copied_fields(&mut args, transaction, &[("ref", "parameterRef")]);
    args["value"] = json!(value);
    args["expectedRevision"] = json!(expected_revision);
    copied_fields(
        &mut args,
        &transaction["authority"],
        &[
            ("expectedObjectIdentity", "parameterIdentity"),
            ("expectedOwnerRef", "ownerRef"),
            ("expectedOwnerIdentity", "ownerIdentity"),
            ("expectedTrackRef", "trackRef"),
            ("expectedTrackIdentity", "trackIdentity"),
        ],
    );
    args["expectedSiblings"] = transaction["authority"]["siblings"].as_array().cloned().map(Value::Array).unwrap_or(json!([]));
    args
}
pub fn parameters_mutation_args(transaction: &Value, values: impl Fn(&Value, usize) -> (f64, f64)) -> Result<Value, LiveError> {
    let parameters =
        transaction["parameters"].as_array().ok_or_else(|| LiveError::type_error("Cannot read properties of undefined (reading '0')"))?;
    let shared = parameters.first().ok_or_else(|| LiveError::type_error("Cannot read properties of undefined (reading 'authority')"))?;
    let mut args = json!({});
    copied_fields(
        &mut args,
        &shared["authority"],
        &[
            ("expectedOwnerRef", "ownerRef"),
            ("expectedOwnerIdentity", "ownerIdentity"),
            ("expectedTrackRef", "trackRef"),
            ("expectedTrackIdentity", "trackIdentity"),
        ],
    );
    args["expectedSiblings"] = shared["authority"]["siblings"].as_array().cloned().map(Value::Array).unwrap_or(json!([]));
    args["parameters"] = Value::Array(
        parameters
            .iter()
            .enumerate()
            .map(|(index, parameter)| {
                let (value, revision) = values(parameter, index);
                let mut row = json!({"value":value,"expectedRevision":revision});
                copied_fields(&mut row, parameter, &[("ref", "ref")]);
                copied_fields(&mut row, &parameter["authority"], &[("expectedObjectIdentity", "parameterIdentity")]);
                row
            })
            .collect(),
    );
    Ok(args)
}
pub fn preview_change(name: &str, record: &Value) -> Result<Option<LiveInvocation>, LiveError> {
    let pair = match name {
        "live_mixer_preview" => Some(("mixer.set", "mixer-set")),
        "live_mixer_extended_preview" => Some(("mixer.extended.set", "mixer-extended")),
        "live_chain_mixer_preview" => Some(("chain-mixer.set", "chain-mixer")),
        "live_chain_preview" => Some(("chain.set", "chain-set")),
        "live_routing_preview" => Some(("routing.set", "routing-set")),
        "live_clip_properties_preview" => Some(("clip.set", "clip-set")),
        "live_clip_action_preview" => Some(("clip.action", "clip-action")),
        "live_clip_view_preview" => Some(("clip.view.set", "clip-view")),
        "live_clip_duplicate_preview" => Some(("clip.duplicate", "duplicate")),
        "live_audio_clip_preview" => Some(("audio.clip.set", "audio-set")),
        "live_track_properties_preview" => Some(("track.set", "track-set")),
        "live_scene_preview" => Some(("scene.set", "scene-set")),
        "live_song_settings_preview" => Some(("song.set", "song-set")),
        "live_tuning_preview" => Some(("tuning.set", "tuning")),
        "live_device_view_preview" => Some(("device.view.set", "device-view")),
        "live_rack_view_preview" => Some(("rack.view.set", "rack-view")),
        "live_device_delete_preview" => Some(("device.delete", "device-delete")),
        "live_data_preview" => Some(("data.set", "data-set")),
        "live_simpler_preview" => Some(("simpler.replace-sample", "simpler")),
        "live_browser_load_preview" => Some(("browser.load", "browser-load")),
        _ => None,
    };
    if let Some((operation, kind)) = pair {
        return Ok(
            (record["kind"] == kind && record["payload"].is_object()).then(|| LiveInvocation::new(operation, record["payload"].clone()))
        );
    }
    let payload = &record["payload"];
    Ok(match name {
        "live_device_edit_preview" if record["kind"] == "device-edit" && payload["args"].is_object() => {
            payload["operation"].as_str().map(|operation| LiveInvocation::new(operation, payload["args"].clone()))
        }
        "live_clip_delete_preview" | "live_scene_delete_preview" | "live_track_delete_preview" | "live_locator_delete_preview" => {
            let kind = name.strip_prefix("live_").unwrap().strip_suffix("_preview").unwrap().replace('_', "-");
            if record["kind"] == kind {
                payload["operation"].as_str().map(|operation| {
                    let mut args = payload.clone();
                    args.as_object_mut().unwrap().remove("operation");
                    LiveInvocation::new(operation, args)
                })
            } else {
                None
            }
        }
        "live_object_rename_preview" if record["kind"] == "rename" && payload.is_object() && record["clipRef"].is_string() => {
            let kind = payload.get("kind").map(js_string).transpose()?.unwrap_or("undefined".into());
            Some(LiveInvocation::new(
                &if kind == "takeLane" { "take-lane.rename".into() } else { format!("{kind}.rename") },
                json!({"ref":record["clipRef"]}),
            ))
        }
        "live_device_parameter_preview" if record["parameters"].is_array() => {
            Some(LiveInvocation::new("device.parameters.set", parameters_mutation_args(record, |_, _| (0.0, 1.0))?))
        }
        "live_device_parameter_preview" if record["parameterRef"].is_string() && record["authority"].is_object() => {
            Some(LiveInvocation::new("device.parameter.set", parameter_mutation_args(record, 0.0, 1.0)))
        }
        _ => None,
    })
}
impl McpHost {
    pub async fn dispatch_history_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        if !call.asynchronous {
            return None;
        }
        let args = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_undo_step_begin" => Some(self.live_undo_step_begin_async(&call.id, args).await),
            "live_undo_step_end" => Some(self.live_undo_step_end_async(&call.id, args).await),
            "live_song_undo" => self.live_song_history_async(&call.id, args, false, signal).await,
            "live_song_redo" => self.live_song_history_async(&call.id, args, true, signal).await,
            _ => return None,
        }))
    }
    pub async fn single_flight_mutation<F, Fut>(
        self: &Rc<Self>,
        name: &str,
        id: &Value,
        args: &Value,
        execute: F,
        caller_signal: Option<&Signal>,
    ) -> MutationOutcome
    where
        F: FnOnce(Option<Signal>) -> Fut + 'static,
        Fut: Future<Output = MutationOutcome> + 'static,
    {
        if self.recovery_finalization_in_flight.get() && name != "live_recovery_finalize" {
            return Err(LiveError::error("recovery finalization safety barrier is in progress"));
        }
        if name == "live_recovery_finalize" {
            return execute(caller_signal.cloned()).await;
        }
        if !args.is_object() || !is_non_empty_string(&args["idempotencyKey"], 128) {
            self.active_async_operations.set(self.active_async_operations.get() + 1);
            let _active = Active(&self.active_async_operations);
            return execute(caller_signal.cloned()).await;
        }
        let transaction_id = ["transactionId", "captureId"]
            .iter()
            .find_map(|key| is_non_empty_string(&args[*key], 128).then(|| args[*key].as_str().unwrap().to_owned()));
        let key = args["idempotencyKey"].as_str().unwrap();
        let identity = if let Some(transaction_id) = &transaction_id {
            format!("operation:{name}:transaction:{transaction_id}")
        } else {
            format!("operation:{name}:key:{key}")
        };
        let digest = hex::encode(Sha256::digest(canonical_mutation_identity(args)?.as_bytes()));
        let previous = self.in_flight_mutations.borrow().get(&identity).cloned();
        let joined = previous.is_some();
        if previous.as_ref().is_some_and(|flight| flight.idempotency_key != key || flight.argument_digest != digest) {
            return Err(LiveError::error("operation is already applying with different idempotency or authority arguments"));
        }
        if previous.is_none() && transaction_id.as_deref().is_some_and(retention::is_in_flight) {
            return Err(LiveError::error("transaction recovery or finalization is already in progress"));
        }
        let flight = if let Some(flight) = previous {
            flight
        } else {
            let signal = Signal::new();
            let (send, receive) = tokio::sync::oneshot::channel::<MutationOutcome>();
            let flight = Rc::new(MutationFlight {
                idempotency_key: key.into(),
                argument_digest: digest,
                signal: signal.clone(),
                waiters: Cell::new(0),
                settled: Cell::new(false),
                promise: async move { receive.await.unwrap_or_else(|_| Err(LiveError::error("mutation execution ended unexpectedly"))) }
                    .boxed_local()
                    .shared(),
            });
            if let Some(id) = &transaction_id {
                retention::mark_in_flight(id);
            }
            self.active_async_operations.set(self.active_async_operations.get() + 1);
            let mut cleanup = FlightCleanup {
                host: self.clone(),
                flight: flight.clone(),
                identity: identity.clone(),
                transaction_id: transaction_id.clone(),
                finished: false,
            };
            let host = self.clone();
            let mut operation = execute(Some(signal)).boxed_local();
            // Calling an async JavaScript function runs its first synchronous stretch immediately.
            let immediate = operation.as_mut().now_or_never();
            self.in_flight_mutations.borrow_mut().insert(identity.clone(), flight.clone());
            tokio::task::spawn_local(async move {
                let outcome = match immediate {
                    Some(outcome) => outcome,
                    None => operation.await,
                };
                if let (Some(transaction_id), Ok(Some(outcome))) = (&transaction_id, &outcome) {
                    if outcome["result"]["isError"] == false
                        && host.adapter.has_retire_transaction_async()
                        && !host.adapter.retires_on_its_own()
                    {
                        let _ = host
                            .adapter
                            .retire_transaction_async(
                                transaction_id,
                                Some(&LiveOperationContext::with_deadline(kumi_common::time::now_ms_f64() + 5000.0)),
                                false,
                            )
                            .await;
                    }
                }
                cleanup.finished = true;
                drop(cleanup);
                let _ = send.send(outcome);
            });
            flight
        };
        flight.waiters.set(flight.waiters.get() + 1);
        let _waiter = Waiter(flight.clone());
        let result = if let Some(signal) = caller_signal {
            tokio::select! { biased; result=flight.promise.clone()=>result, _=signal.cancelled()=>return Ok(None) }
        } else {
            flight.promise.clone().await
        };
        let Some(mut outcome) = result? else { return Ok(None) };
        if joined {
            outcome["id"] = id.clone();
            if let Some(mut body) = result_body(&outcome) {
                body["idempotent"] = json!(true);
                outcome["result"]["content"][0]["text"] = json!(js_json::stringify(&body));
            }
        }
        Ok(Some(outcome))
    }
    pub fn active_async_operations(&self) -> usize {
        self.active_async_operations.get()
    }
    /// A transaction's record from whichever map or manager holds it, without waiting on a borrow.
    fn try_transaction_record(&self, id: &str) -> Option<retention::TransactionRecord> {
        [
            &self.transactions,
            &self.audio_capture_transactions,
            &self.arrangement_transactions,
            &self.session_structure_transactions,
            &self.device_parameter_transactions,
            &self.device_parameters_transactions,
            &self.audition_transactions,
            &self.transport_transactions,
            &self.clip_launch_transactions,
            &self.note_edit_transactions,
            &self.clip_lifecycle_transactions,
        ]
        .iter()
        .find_map(|map| map.try_get(id))
        .or_else(|| self.batch_transactions.try_record(id))
        .or_else(|| self.device_state_transactions.try_record(id))
        .or_else(|| self.midi_transactions.try_record(id))
    }
    pub fn transaction_record(&self, id: &str) -> Option<retention::TransactionRecord> {
        [
            &self.transactions,
            &self.audio_capture_transactions,
            &self.arrangement_transactions,
            &self.session_structure_transactions,
            &self.device_parameter_transactions,
            &self.device_parameters_transactions,
            &self.audition_transactions,
            &self.transport_transactions,
            &self.clip_launch_transactions,
            &self.note_edit_transactions,
            &self.clip_lifecycle_transactions,
        ]
        .iter()
        .find_map(|map| map.get(id))
    }
    pub fn expect_preview_state_digest(&self, name: &str, outcome: Option<Value>) -> Option<Value> {
        if self.adapter.has_expect_state_digest() {
            if let Some(outcome) = &outcome {
                if outcome["result"]["isError"] == false {
                    if let Some(id) = result_body(outcome).and_then(|body| body["transactionId"].as_str().map(str::to_owned)) {
                        if let Some(record) = self.transaction_record(&id) {
                            if let Ok(Some(invocation)) = preview_change(name, &record.borrow()) {
                                self.adapter.expect_state_digest(&id, &invocation);
                            }
                        }
                    }
                }
            }
        }
        outcome
    }
    pub async fn require_operation(&self, operation: &str) -> Result<LiveStatus, LiveError> {
        let mut status = self.require_connected(None)?;
        if !status.has_operation(operation) {
            status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
        }
        if !status.has_operation(operation) {
            return Err(LiveError::error(format!("{operation} is unavailable on this Live shape")));
        }
        Ok(status)
    }
    pub async fn live_undo_step_begin_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["label", "timeoutMs"])
            || params.get("label").is_some_and(|v| !is_non_empty_string(v, 256))
            || params.get("timeoutMs").is_some_and(|v| !is_integer_in_range(v, 1000.0, 3_600_000.0))
        {
            return error(id, -32602, "label (1-256 characters) and timeoutMs (1000-3600000) are optional", None);
        }
        let result = async {
            self.require_operation("undo.step.begin").await?;
            let opened = self
                .async_adapter()
                .invoke_async(
                    &LiveInvocation::new("undo.step.begin", params.clone()),
                    Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS))),
                )
                .await?;
            if opened["open"] != true || !is_non_empty_string(&opened["stepId"], 128) || !opened["expiresAt"].is_number() {
                return Err(LiveError::error("Live didn't confirm the undo step"));
            }
            *self.open_undo_step.borrow_mut() = Some(json!({"stepId":opened["stepId"],"expiresAt":opened["expiresAt"]}));
            Ok(success_text(id, &opened))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "No undo step is open: each change still undoes on its own in Live."))
    }
    pub async fn live_undo_step_end_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["stepId"])
            || params.get("stepId").is_some_and(|v| !v.as_str().is_some_and(|v| (8..=128).contains(&kumi_common::js::string::utf16_len(v))))
        {
            return error(id, -32602, "stepId (from live_undo_step_begin) is optional", None);
        }
        let result = async {
            self.require_operation("undo.step.end").await?;
            self.end_undo_step_async(params["stepId"].as_str(), self.deadline(reads::AUDITION_DEADLINE_MS)).await
        }
        .await;
        match result {
            Ok(value) => success_text(id, &value),
            Err(e) => adapter_tool_error(id, &e, "The undo step may still be open; Live closes it itself when its time runs out."),
        }
    }
    pub async fn end_undo_step_async(&self, step_id: Option<&str>, deadline_ms: f64) -> Result<Value, LiveError> {
        let ended = self
            .async_adapter()
            .invoke_async(
                &LiveInvocation::new("undo.step.end", step_id.filter(|s| !s.is_empty()).map(|s| json!({"stepId":s})).unwrap_or(json!({}))),
                Some(&LiveOperationContext::with_deadline(deadline_ms)),
            )
            .await?;
        if ended["reason"] != "other-step" || ended.get("stepId") != self.open_undo_step.borrow().as_ref().and_then(|v| v.get("stepId")) {
            *self.open_undo_step.borrow_mut() = None;
        }
        Ok(ended)
    }
    pub async fn close_open_undo_step(&self) {
        let step = self.open_undo_step.borrow().clone();
        if let Some(step) = step {
            if step["expiresAt"].as_f64().is_some_and(|expires| expires > kumi_common::time::now_ms_f64()) {
                let _ = self.end_undo_step_async(step["stepId"].as_str(), kumi_common::time::now_ms_f64() + 2000.0).await;
            }
            *self.open_undo_step.borrow_mut() = None;
        }
    }
    pub async fn live_song_history_async(&self, id: &Value, params: &Value, redo: bool, signal: Option<&Signal>) -> Option<Value> {
        let operation = if redo { "song.redo" } else { "song.undo" };
        let confirmation = if redo { "redo-in-live" } else { "undo-in-live" };
        if !has_only(params, &["confirmation", "idempotencyKey"])
            || params["confirmation"] != confirmation
            || !is_idempotency_key(&params["idempotencyKey"])
        {
            return Some(error(id, -32602, &format!("confirmation={confirmation} and an idempotencyKey are required"), None));
        }
        let idempotency_key = params["idempotencyKey"].as_str().unwrap();
        let key = format!("{operation}\0{idempotency_key}");
        if let Some((_, recorded)) = self.song_history_calls.borrow().iter().find(|(candidate, _)| candidate == &key) {
            let mut recorded = recorded.clone();
            recorded["idempotent"] = json!(true);
            return Some(success_text(id, &recorded));
        }
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        let result = async {
            use base64::Engine;
            self.require_operation(operation).await?;
            let digest = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(idempotency_key.as_bytes()));
            let transaction_id = format!("{}_{}",if redo {"song-redo"}else{"song-undo"},&digest[..32]);
            let result = self.async_adapter().invoke_async(&LiveInvocation::new(operation,json!({})),Some(&LiveOperationContext{signal:signal.cloned(),deadline_ms:Some(self.deadline(reads::AUDITION_DEADLINE_MS)),idempotency_key:Some(idempotency_key.into()),transaction_id:Some(transaction_id),..Default::default()})).await?;
            if result["done"] == true { *self.open_undo_step.borrow_mut() = None; }
            let mut answer = json!({"operation":operation,"done":result["done"]==true,"canUndo":result.get("canUndo").unwrap_or(&Value::Null),"canRedo":result.get("canRedo").unwrap_or(&Value::Null)});
            { let mut history = self.song_history_calls.borrow_mut(); while history.len() >= 4096 { history.pop_front(); } history.push_back((key,answer.clone())); }
            answer["idempotent"] = json!(false); Ok(success_text(id,&answer))
        }.await;
        Some(result.unwrap_or_else(|e| {
            adapter_tool_error(id, &e, "Live's own history may or may not have moved; look at the Set before trying again.")
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn recovery_barrier_only_admits_finalization_without_counting_it() {
        let host = Rc::new(McpHost::default());
        host.recovery_finalization_in_flight.set(true);
        let refused =
            host.single_flight_mutation("change", &json!(1), &json!({}), |_| async { panic!("barrier dispatched work") }, None).await;
        assert_eq!(refused.unwrap_err().message(), "recovery finalization safety barrier is in progress");
        let owned = host.clone();
        let result = host
            .single_flight_mutation(
                "live_recovery_finalize",
                &json!(2),
                &json!({"idempotencyKey":"finalize"}),
                move |_| async move {
                    assert_eq!(owned.active_async_operations(), 0);
                    Ok(Some(json!({"finished":true})))
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(result, Some(json!({"finished":true})));
        assert_eq!(host.active_async_operations(), 0);
    }
    #[tokio::test]
    async fn last_waiter_cancels_work_but_keeps_transaction_owned_until_cleanup_finishes() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let host = Rc::new(McpHost::default());
                let caller = Signal::new();
                let (send, recv) = tokio::sync::oneshot::channel();
                let cancelled = Rc::new(Cell::new(false));
                let observed = cancelled.clone();
                let args = json!({"transactionId":"cancellation-owned","idempotencyKey":"apply-key"});
                let id = json!(1);
                let mut operation = host
                    .single_flight_mutation(
                        "change",
                        &id,
                        &args,
                        move |signal| async move {
                            signal.unwrap().cancelled().await;
                            observed.set(true);
                            let _ = recv.await;
                            Ok(None)
                        },
                        Some(&caller),
                    )
                    .boxed_local();
                assert!(operation.as_mut().now_or_never().is_none());
                assert!(retention::is_in_flight("cancellation-owned"));
                caller.cancel();
                assert_eq!(operation.await.unwrap(), None);
                for _ in 0..8 {
                    tokio::task::yield_now().await;
                }
                assert!(cancelled.get());
                assert!(retention::is_in_flight("cancellation-owned"));
                assert_eq!(host.active_async_operations(), 1);
                send.send(()).unwrap();
                for _ in 0..8 {
                    tokio::task::yield_now().await;
                }
                assert!(!retention::is_in_flight("cancellation-owned"));
                assert_eq!(host.active_async_operations(), 0);
            })
            .await;
    }
}
