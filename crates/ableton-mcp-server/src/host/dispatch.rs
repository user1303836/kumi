//! Public request execution around family handlers, recovery watches and shared mutations.
use super::*;
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_common::abort::Signal;
use mutations::result_body;
use sha2::{Digest, Sha256};

// Construct each family future in its own frame, before moving it to the heap. Boxing
// after construction at the dispatcher call site still reserves every temporary's stack
// slot in debug builds, even when only one family handles the request.
#[inline(never)]
fn boxed_operation<'a, F: std::future::Future + 'a>(create: impl FnOnce() -> F) -> LocalBoxFuture<'a, F::Output> {
    create().boxed_local()
}

const ASYNC_FAILURE: &str = "The asynchronous Live operation failed; inspect authoritative state before retrying.";
const FUSED_LIMIT: usize = 4096;
fn fused_refusal(name: &str) -> Option<&'static str> {
    Some(match name {
        "live_session_audition_preview" => {
            "an audition plays a scene out loud: its preview says what would sound and hands out a confirmation for the producer to give"
        }
        "live_clip_launch_preview" => {
            "launching a clip plays it out loud: its preview says what would sound and hands out a confirmation for the producer to give"
        }
        "live_audio_capture_preview" => {
            "a capture records what Live plays: its preview says what it would record and hands out a confirmation for the producer to give"
        }
        "live_recording_preview" => "recording arms tracks and records: the producer starts it having seen what would record",
        "live_realtime_arm_preview" => {
            "arming opens a network channel that moves parameters live: the producer arms it having seen its endpoint and targets"
        }
        "live_application_dialog_preview" => "answering one of Live's dialogs: the producer reads the dialog before its button is pressed",
        "live_fire_button_preview" => "pressing a launch button plays out loud: the producer presses it having seen what it launches",
        _ => return None,
    })
}
impl McpHost {
    pub async fn handle_async(self: &Rc<Self>, input: &Value, signal: Option<&Signal>) -> Result<Option<Value>, LiveError> {
        if signal.is_some_and(Signal::is_cancelled) {
            return Ok(None);
        }
        let request = self.begin_request(input, true)?;
        let RequestDecision::Tool(call) = request.decision.clone() else {
            return Ok(if signal.is_some_and(Signal::is_cancelled) { None } else { request.completed() });
        };
        let outcome = if call.asynchronous {
            let owner = self.clone();
            let invoke = call.clone();
            self.single_flight_mutation(
                &call.name,
                &call.id,
                call.arguments.as_ref().unwrap_or(&Value::Null),
                move |signal| async move {
                    let result = owner.dispatch_tool(invoke.clone(), signal).await?;
                    Ok(owner.expect_preview_state_digest(&invoke.name, result))
                },
                signal,
            )
            .await
            .unwrap_or_else(|cause| Some(adapter_tool_error(&call.id, &cause, ASYNC_FAILURE)))
        } else {
            Some(self.dispatch_sync_tool(&call)?)
        };
        Ok(if signal.is_some_and(Signal::is_cancelled) { None } else { request.finish(outcome) })
    }
    /// The source also exposes a synchronous boundary for synchronous adapters and local tools.
    pub fn handle(&self, input: &Value) -> Result<Option<Value>, LiveError> {
        let request = self.begin_request(input, false)?;
        let RequestDecision::Tool(call) = request.decision.clone() else { return Ok(request.completed()) };
        let result = self.dispatch_sync_tool(&call)?;
        Ok(request.finish(Some(result)))
    }
    fn dispatch_sync_tool(&self, call: &ToolCall) -> Result<Value, LiveError> {
        let id = &call.id;
        let args = call.arguments.as_ref().unwrap_or(&Value::Null);
        Ok(match call.name.as_str() {
            "live_snapshot" => self.live_snapshot(id),
            "live_discover" => self.live_discover(id, args),
            "live_device_parameter_preview" => self.live_device_parameter_preview(id, args),
            "live_device_parameter_apply" => self.live_device_parameter_apply(id, args),
            "live_session_structure_preview" => self.live_session_structure_preview(id, args),
            "live_session_structure_apply" => self.live_session_structure_apply(id, args),
            "live_midi_clip_preview" => self.live_midi_preview(id, args),
            "live_midi_clip_apply" => self.live_midi_apply(id, args),
            "live_arrangement_section_preview" => self.live_arrangement_preview(id, args),
            "live_arrangement_section_apply" => self.live_arrangement_apply(id, args),
            "live_tempo_preview" => self.live_tempo_preview(id, args),
            "live_tempo_apply" => self.live_tempo_apply(id, args),
            "live_transaction_release" => self.live_transaction_release(id, args),
            "live_undo" => self.undo_dispatch_sync(id, args),
            _ => error(id, -32601, "Tool not found", None),
        })
    }
    pub fn dispatch_tool(
        self: &Rc<Self>,
        call: ToolCall,
        signal: Option<Signal>,
    ) -> LocalBoxFuture<'static, Result<Option<Value>, LiveError>> {
        let owner = self.clone();
        async move {
            if signal.as_ref().is_some_and(Signal::is_cancelled) {
                return Ok(None);
            }
            owner.forget_live_failure();
            let args = call.arguments.as_ref().unwrap_or(&Value::Null);
            match call.name.as_str() {
                "live_change" => return owner.live_change(&call.id, args, signal.as_ref()).await,
                "live_undo" => {
                    return owner
                        .with_undo_watch(&call.id, args, boxed_operation(|| owner.undo_dispatch_async(&call.id, args, signal.as_ref())))
                        .await
                        .map(Some)
                }
                "live_recovery_finalize" => return owner.live_recovery_finalize_async(&call.id, args).await.map(Some),
                "live_subscribe" => return Ok(Some(owner.live_subscribe_async(&call.id, args).await)),
                "live_unsubscribe" => return Ok(Some(owner.live_unsubscribe_async(&call.id, args).await)),
                _ => {}
            }
            // Keep each tool family on the heap: one dispatcher must not embed every family
            // state machine in the caller's stack frame. The same applies to compound undo below.
            macro_rules! family {
                ($method:ident) => {
                    if let Some(outcome) = boxed_operation(|| owner.$method(&call, signal.as_ref())).await {
                        return outcome.map(Some);
                    }
                };
            }
            macro_rules! optional {
                ($method:ident) => {
                    if let Some(outcome) = boxed_operation(|| owner.$method(&call, signal.as_ref())).await {
                        return outcome;
                    }
                };
            }
            family!(dispatch_read_tool);
            family!(dispatch_project_tool);
            family!(dispatch_probe_tool);
            family!(dispatch_tempo_tool);
            family!(dispatch_structure_tool);
            family!(dispatch_rename_tool);
            family!(dispatch_device_parameter_tool);
            family!(dispatch_arrangement_tool);
            optional!(dispatch_audio_tool);
            optional!(dispatch_capture_tool);
            optional!(dispatch_audition_tool);
            optional!(dispatch_transport_tool);
            optional!(dispatch_transport_action_tool);
            optional!(dispatch_track_structure_tool);
            optional!(dispatch_deletion_tool);
            optional!(dispatch_dialog_tool);
            optional!(dispatch_object_view_tool);
            optional!(dispatch_selection_tool);
            optional!(dispatch_warp_marker_tool);
            optional!(dispatch_clip_action_tool);
            optional!(dispatch_drum_pad_tool);
            optional!(dispatch_automation_tool);

            optional!(dispatch_clip_launch_tool);
            optional!(dispatch_managed_tool);
            optional!(dispatch_device_state_tool);
            optional!(dispatch_history_tool);
            optional!(dispatch_note_edit_tool);
            optional!(dispatch_routing_tool);
            optional!(dispatch_session_capture_tool);
            optional!(dispatch_browser_render_tool);
            optional!(dispatch_clip_duplicate_tool);
            optional!(dispatch_clip_move_tool);
            optional!(dispatch_device_lifecycle_tool);
            optional!(dispatch_device_copy_tool);
            optional!(dispatch_device_basic_tool);
            optional!(dispatch_data_tool);
            optional!(dispatch_follow_tool);
            optional!(dispatch_midi_transform_tool);
            optional!(dispatch_advanced_device_tool);
            optional!(dispatch_willington_tool);
            optional!(dispatch_device_edit_tool);
            optional!(dispatch_rack_tool);
            optional!(dispatch_scene_tool);
            optional!(dispatch_track_view_tool);
            optional!(dispatch_track_properties_tool);
            optional!(dispatch_song_settings_tool);
            optional!(dispatch_fire_button_tool);
            optional!(dispatch_specialized_device_tool);
            optional!(dispatch_looper_tool);
            optional!(dispatch_groove_tool);
            optional!(dispatch_tuning_tool);
            optional!(dispatch_simpler_tool);
            optional!(dispatch_clip_properties_tool);
            optional!(dispatch_extended_mixer_tool);
            optional!(dispatch_audio_clip_tool);
            optional!(dispatch_audio_import_tool);
            optional!(dispatch_note_target_tool);
            optional!(dispatch_mixer_tool);
            optional!(dispatch_ui_tool);
            optional!(dispatch_arrangement_clip_tool);
            optional!(dispatch_arrangement_midi_tool);
            optional!(dispatch_recording_tool);
            optional!(dispatch_realtime_tool);
            // The source falls through to its guarded undo handler after its named routes.
            owner
                .with_undo_watch(&call.id, args, boxed_operation(|| owner.undo_dispatch_async(&call.id, args, signal.as_ref())))
                .await
                .map(Some)
        }
        .boxed_local()
    }
    async fn live_change(self: &Rc<Self>, id: &Value, params: &Value, signal: Option<&Signal>) -> Result<Option<Value>, LiveError> {
        static PREVIEW: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^live_[a-z0-9_]+_preview$").unwrap());
        if !has_only(params, &["tool", "args", "idempotencyKey"])
            || !params["tool"].as_str().is_some_and(|s| PREVIEW.is_match(s))
            || !params["args"].is_object()
            || params.get("idempotencyKey").is_some_and(|v| !is_idempotency_key(v))
        {
            return Ok(Some(error(id, -32602, "tool (a *_preview tool), its args, and optionally an idempotencyKey are required", None)));
        }
        let preview = params["tool"].as_str().unwrap();
        let apply = format!("{}_apply", preview.strip_suffix("_preview").unwrap());
        if let Some(refusal) = fused_refusal(preview) {
            return Ok(Some(reason_error(
                id,
                refusal,
                &format!("Preview it with {preview}, show the producer what it will do, then apply it with {apply}."),
            )));
        }
        if tool_catalog::tool_catalog_entry(preview).is_none() || tool_catalog::tool_catalog_entry(&apply).is_none() {
            return Ok(Some(error(id, -32602, &format!("{preview} isn't a preview with an apply of its own"), None)));
        }
        for name in [preview, apply.as_str()] {
            if !self.tool_callable(name)? {
                return self.tool_gate_error(id, name).map(Some);
            }
        }
        let key = params["idempotencyKey"].as_str().map(str::to_owned).unwrap_or_else(|| tempo::transaction_id("change"));
        let digest = hex::encode(Sha256::digest(canonical_mutation_identity(&json!({"tool":preview,"args":params["args"]}))?.as_bytes()));
        let mut recorded = self.fused_changes.borrow().iter().find(|(name, _)| name == &key).map(|(_, row)| row.clone());
        if recorded.as_ref().is_some_and(|row| row["digest"] != digest) {
            return Ok(Some(error(id, -32602, "this idempotencyKey already made another change", None)));
        }
        if recorded.is_none() {
            let outcome = self
                .dispatch_tool(
                    ToolCall { id: id.clone(), name: preview.into(), arguments: Some(params["args"].clone()), asynchronous: true },
                    signal.cloned(),
                )
                .await?;
            let outcome = self.expect_preview_state_digest(preview, outcome);
            if outcome.as_ref().is_none_or(|v| !v["result"].is_object() || v["result"]["isError"] != false) {
                return Ok(outcome);
            }
            let body = outcome.as_ref().and_then(result_body);
            if body.as_ref().is_none_or(|b| !is_non_empty_string(&b["transactionId"], 128)) {
                return Ok(Some(reason_error(
                    id,
                    &format!("{preview} made no transaction to apply"),
                    "Use the family's preview and apply tools.",
                )));
            }
            let body = body.unwrap();
            let confirmation = if is_non_empty_string(&body["confirmation"], 128) { body["confirmation"].clone() } else { json!("apply") };
            let row = json!({"digest":digest,"transactionId":body["transactionId"],"confirmation":confirmation,"preview":body});
            let mut entries = self.fused_changes.borrow_mut();
            while entries.len() >= FUSED_LIMIT {
                entries.pop_front();
            }
            entries.push_back((key.clone(), row.clone()));
            recorded = Some(row);
        }
        let recorded = recorded.unwrap();
        let args = json!({"transactionId":recorded["transactionId"],"confirmation":recorded["confirmation"],"idempotencyKey":key});
        let invoke = ToolCall { id: id.clone(), name: apply.clone(), arguments: Some(args.clone()), asynchronous: true };
        let owner = self.clone();
        let applied = self.single_flight_mutation(&apply, id, &args, move |signal| owner.dispatch_tool(invoke, signal), signal).await?;
        let Some(body) = applied.as_ref().and_then(result_body).filter(|_| applied.as_ref().is_some_and(|a| a["result"].is_object()))
        else {
            return Ok(applied);
        };
        let mut body = body.as_object().unwrap().clone();
        if body.get("transactionId").is_none_or(Value::is_null) {
            body.insert("transactionId".into(), recorded["transactionId"].clone());
        }
        body.insert("change".into(), json!({"preview":preview,"apply":apply,"idempotencyKey":key}));
        body.insert("preview".into(), recorded["preview"].clone());
        Ok(Some(response(
            id,
            json!({"content":[{"type":"text","text":kumi_common::js::json::stringify(&json!(body))}],"isError":applied.unwrap()["result"]["isError"]==true}),
        )))
    }
    fn undo_dispatch_sync(&self, id: &Value, params: &Value) -> Value {
        if !valid_transaction_params(params, "undo") {
            return error(id, -32602, "transactionId, confirmation=undo, and idempotencyKey are required", None);
        }
        let tx = params["transactionId"].as_str().unwrap();
        if self.transactions.get(tx).is_none() {
            if tx.starts_with("parameter_") {
                return self.undo_device_parameter(id, params);
            }
            if tx.starts_with("structure_") {
                return self.undo_structure(id, params);
            }
            if tx.starts_with("arrangement_") {
                return self.undo_arrangement(id, params);
            }
            if tx.starts_with("midi_") {
                return self
                    .midi_transactions
                    .undo(tx, &params["confirmation"], params["idempotencyKey"].as_str().unwrap())
                    .map(|v| success_text(id, &v))
                    .unwrap_or_else(|cause| {
                        adapter_tool_error(id, &cause, "MIDI undo refused; inspect the target clip and connection epoch.")
                    });
            }
        }
        self.undo_tempo(id, params)
    }
    async fn undo_dispatch_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Result<Value, LiveError> {
        if !valid_transaction_params(params, "undo") {
            return Ok(error(id, -32602, "transactionId, confirmation=undo, and idempotencyKey are required", None));
        }
        let tx = params["transactionId"].as_str().unwrap();
        let record = self.transaction_record(tx);
        let kind = record.as_ref().and_then(|r| r.borrow()["kind"].as_str().map(str::to_owned));
        if !self.policy_allows_tool(Self::transaction_owner_tool(tx, kind.as_deref()))? {
            return Ok(transaction_error(id,"The current deployment policy no longer allows this transaction's tool domain; reconcile manually or restore the policy before undo."));
        }
        if self.transactions.get(tx).is_none() {
            let key = params["idempotencyKey"].as_str().unwrap();
            let context =
                self.transaction_context(params, signal, if tx.starts_with("midi_") { 30_000.0 } else { reads::AUDITION_DEADLINE_MS });
            if tx.starts_with("midi_") {
                return self
                    .midi_transactions
                    .undo_async(tx, &params["confirmation"], key, Some(&context))
                    .await
                    .map(|v| success_text(id, &v));
            }
            if tx.starts_with("batch_") {
                return self
                    .batch_transactions
                    .undo_async(tx, &params["confirmation"], key, Some(&context))
                    .await
                    .map(|v| success_text(id, &v));
            }
            if tx.starts_with("devstate_") {
                return self
                    .device_state_transactions
                    .undo_async(tx, &params["confirmation"], key, Some(&context))
                    .await
                    .map(|v| success_text(id, &v));
            }
            if tx.starts_with("transport_") {
                return Ok(boxed_operation(|| self.undo_transport_async(id, params, signal)).await);
            }
            if tx.starts_with("noteupdate_") || tx.starts_with("notedelete_") {
                return Ok(boxed_operation(|| self.undo_note_edit_async(id, params, signal)).await);
            }
            if tx.starts_with("capturemidi_") || tx.starts_with("scenecapture_") {
                return Ok(boxed_operation(|| self.undo_session_capture_async(id, params, signal)).await);
            }
            if tx.starts_with("routing_") {
                return Ok(boxed_operation(|| self.undo_routing_async(id, params, signal)).await);
            }
            if tx.starts_with("arrmidi_") {
                return Ok(boxed_operation(|| self.undo_arrangement_midi_async(id, params, signal)).await);
            }
            if tx.starts_with("arrclip_") {
                return Ok(boxed_operation(|| self.undo_arrangement_clip_async(id, params, signal)).await);
            }
            if tx.starts_with("recording_") {
                return Ok(self.undo_recording_async(id, params).await);
            }
            if tx.starts_with("clipmove_") {
                return Ok(boxed_operation(|| self.undo_clip_move_async(id, params, signal)).await);
            }
            if tx.starts_with("audioimport_") {
                return Ok(boxed_operation(|| self.undo_audio_import_async(id, params, signal)).await);
            }
            if tx.starts_with("audioclip_") {
                return Ok(boxed_operation(|| self.undo_audio_clip_async(id, params, signal)).await);
            }
            if tx.starts_with("noteedit_") {
                return Ok(boxed_operation(|| self.undo_note_target_async(id, params, signal)).await);
            }
            if tx.starts_with("mixer_") {
                return Ok(boxed_operation(|| self.undo_mixer_async(id, params, signal)).await);
            }
            if tx.starts_with("clipset_") {
                return Ok(boxed_operation(|| self.undo_clip_properties_async(id, params, signal)).await);
            }
            if ["mixerext_", "chainmix_", "devio_"].iter().any(|prefix| tx.starts_with(prefix)) {
                return Ok(boxed_operation(|| self.undo_extended_mixer_async(id, params, signal)).await);
            }
            if tx.starts_with("miditransform_") {
                return Ok(boxed_operation(|| self.undo_midi_transform_async(id, params, signal)).await);
            }
            if tx.starts_with("devadv_") {
                return Ok(boxed_operation(|| self.undo_device_advanced_async(id, params, signal)).await);
            }
            if tx.starts_with("chainset_") {
                return Ok(boxed_operation(|| self.undo_chain_async(id, params, signal)).await);
            }
            if tx.starts_with("willington_") {
                return Ok(boxed_operation(|| self.undo_willington_async(id, params, signal)).await);
            }
            if tx.starts_with("devedit_") {
                return Ok(boxed_operation(|| self.undo_device_edit_async(id, params, signal)).await);
            }
            if tx.starts_with("rack_") {
                return Ok(boxed_operation(|| self.undo_rack_async(id, params, signal)).await);
            }
            if tx.starts_with("rackview_") {
                return Ok(boxed_operation(|| self.undo_rack_async(id, params, signal)).await);
            }
            if tx.starts_with("sceneset_") {
                return Ok(boxed_operation(|| self.undo_scene_async(id, params, signal)).await);
            }
            if tx.starts_with("trackview_") {
                return Ok(boxed_operation(|| self.undo_track_view_async(id, params, signal)).await);
            }
            if tx.starts_with("trackset_") {
                return Ok(boxed_operation(|| self.undo_track_properties_async(id, params, signal)).await);
            }
            if tx.starts_with("songset_") {
                return Ok(boxed_operation(|| self.undo_song_settings_async(id, params, signal)).await);
            }
            if tx.starts_with("trackstruct_") {
                return Ok(boxed_operation(|| self.undo_track_structure_async(id, params, signal)).await);
            }
            if tx.starts_with("clipview_") {
                return Ok(boxed_operation(|| self.undo_clip_view_async(id, params, signal)).await);
            }
            if tx.starts_with("devview_") {
                return Ok(boxed_operation(|| self.undo_device_view_async(id, params, signal)).await);
            }
            if tx.starts_with("selection_") {
                return Ok(boxed_operation(|| self.undo_selection_async(id, params, signal)).await);
            }
            if tx.starts_with("warp_") {
                return Ok(boxed_operation(|| self.undo_warp_marker_async(id, params, signal)).await);
            }
            if tx.starts_with("drumpad_") {
                return Ok(boxed_operation(|| self.undo_drum_pad_async(id, params, signal)).await);
            }
            if tx.starts_with("automation_") {
                return Ok(boxed_operation(|| self.undo_automation_async(id, params, signal)).await);
            }
            if tx.starts_with("firebutton_") {
                return Ok(self.undo_fire_button(id));
            }
            if tx.starts_with("devspec_") {
                return Ok(boxed_operation(|| self.undo_specialized_device_async(id, params, signal)).await);
            }
            if tx.starts_with("looper_") {
                return Ok(boxed_operation(|| self.undo_looper_async(id, params, signal)).await);
            }
            if tx.starts_with("groove_") {
                return Ok(boxed_operation(|| self.undo_groove_async(id, params, signal)).await);
            }
            if tx.starts_with("tuning_") {
                return Ok(boxed_operation(|| self.undo_tuning_async(id, params, signal)).await);
            }
            if tx.starts_with("simpler_") {
                return Ok(boxed_operation(|| self.undo_simpler_async(id, params, signal)).await);
            }
            if tx.starts_with("data_") {
                return Ok(boxed_operation(|| self.undo_data_async(id, params, signal)).await);
            }
            if tx.starts_with("follow_") {
                return Ok(boxed_operation(|| self.undo_follow_actions_async(id, params, signal)).await);
            }
            if tx.starts_with("device_") {
                return Ok(boxed_operation(|| self.undo_device_basic_async(id, params, signal)).await);
            }
            if tx.starts_with("devdup_") {
                return Ok(boxed_operation(|| self.undo_device_copy_async(id, params, signal)).await);
            }
            if tx.starts_with("browserload_") {
                return Ok(boxed_operation(|| self.undo_browser_load_async(id, params, signal)).await);
            }
            if tx.starts_with("clipdup_") {
                return Ok(boxed_operation(|| self.undo_clip_duplicate_async(id, params, signal)).await);
            }
            if tx.starts_with("parameter_") {
                return Ok(boxed_operation(|| self.undo_device_parameter_async(id, params, signal)).await);
            }
            if tx.starts_with("parameters_") {
                return Ok(boxed_operation(|| self.undo_device_parameters_async(id, params, signal)).await);
            }
            if tx.starts_with("structure_") {
                return boxed_operation(|| self.undo_structure_async(id, params, signal)).await;
            }
            if tx.starts_with("arrangement_") {
                return Ok(boxed_operation(|| self.undo_arrangement_async(id, params, signal)).await);
            }
            if tx.starts_with("rename_") {
                return Ok(boxed_operation(|| self.undo_rename_async(id, params, signal)).await);
            }
            if kind.as_deref().is_some_and(|k| {
                ["device-delete", "clip-delete", "scene-delete", "track-delete", "locator-delete", "clip-clear-range"].contains(&k)
            }) {
                return Ok(reason_error(
                    id,
                    "Kumi can't bring this back; Live's undo can.",
                    "If the producer wants it back, Live's own undo can bring it (Cmd-Z in Live).",
                ));
            }
        }
        Ok(boxed_operation(|| self.undo_tempo_async(id, params, signal)).await)
    }
    async fn live_subscribe_async(&self, id: &Value, params: &Value) -> Value {
        let names: Vec<_> = REMOTE_SCRIPT_EVENT_TYPES.iter().map(LiveEventType::as_str).collect();
        let valid = params.get("types").is_none_or(|v| {
            v.as_array().is_some_and(|types| {
                types.len() <= names.len()
                    && types.iter().filter_map(Value::as_str).collect::<HashSet<_>>().len() == types.len()
                    && types.iter().all(|v| v.as_str().is_some_and(|s| names.contains(&s)))
            })
        });
        if !has_only(params, &["types"]) || !valid {
            return error(id, -32602, &format!("types must be a unique subset of {}", names.join(", ")), None);
        }
        let result=async{
            let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected||!status.capabilities.iter().any(|c|c.as_str()=="subscriptions"){return Err(LiveError::error("subscriptions are unavailable"));}
            if !status.has_operation("subscribe"){return Err(LiveError::error("subscription operation is unavailable"));}
            let result=self.adapter.invoke_async(&LiveInvocation::new("subscribe",params.clone()),Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if result["subscribed"]!=true||!result["subscriptionId"].is_string(){return Err(LiveError::error("subscription was not confirmed"));}
            Ok(success_text(id,&json!({"subscribed":true,"subscriptionId":result["subscriptionId"],"epoch":status.epoch,"resnapshot":"use live_snapshot for a fresh authoritative state at any point"})))
        }.await;
        result.unwrap_or_else(|cause| adapter_tool_error(id, &cause, "Subscription requires a connected Live adapter."))
    }
    async fn live_unsubscribe_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &[]) {
            return error(id, -32602, "no arguments accepted", None);
        }
        self.async_adapter()
            .invoke_async(
                &LiveInvocation::new("subscribe", json!({"types":[]})),
                Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS))),
            )
            .await
            .and_then(|r| {
                if r.is_null() {
                    Err(LiveError::type_error("Cannot read properties of null (reading 'subscribed')"))
                } else {
                    Ok(success_text(id, &json!({"subscribed":r["subscribed"]==true})))
                }
            })
            .unwrap_or_else(|cause| adapter_tool_error(id, &cause, "Unsubscribe failed."))
    }
}
