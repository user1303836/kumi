//! Targeted selection, quantization, duplication, and range deletion of MIDI notes.
use super::*;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json as js_json,
};
use note_edit::{normalized_note, note_content_fence, note_fence, NoteClip};
fn operation(action: &str) -> &'static str {
    match action {
        "duplicate" => "note.duplicate",
        "select" => "note.select",
        "delete-range" => "note.delete-range",
        _ => "note.quantize",
    }
}

/// The grids Live quantizes to, in beats: 1/4, 1/8, 1/8T, 1/16, 1/16T and 1/32 notes. Live refuses any other.
const QUANTIZE_GRIDS: [f64; 6] = [1.0, 0.5, 1.0 / 3.0, 0.25, 1.0 / 6.0, 0.125];

fn key(value: Option<&Value>) -> String {
    value.map(js_json::stringify).unwrap_or_else(|| "$undefined".into())
}
fn ids(notes: &[Value]) -> HashSet<String> {
    notes.iter().map(|n| key(n.get("id"))).collect()
}
fn valid_ids(value: &Value) -> bool {
    value.as_array().is_some_and(|a| {
        !a.is_empty()
            && a.len() <= 10_000_000
            && a.iter().all(|v| v.as_f64().is_some_and(|n| n.is_finite() && n >= 0.0 && n.fract() == 0.0))
            && a.iter().map(|v| key(Some(v))).collect::<HashSet<_>>().len() == a.len()
    })
}

fn region(params: &Value) -> Option<Value> {
    if !is_integer_in_range(&params["fromPitch"], 0.0, 127.0)
        || !is_integer_in_range(&params["pitchSpan"], 1.0, 128.0)
        || !params["fromTime"].as_f64().is_some_and(|n| n.is_finite() && (0.0..=1e9).contains(&n))
        || !params["timeSpan"].as_f64().is_some_and(|n| n.is_finite() && (0.001..=1e9).contains(&n))
    {
        return None;
    }

    Some(
        json!({"fromPitch":params["fromPitch"],"pitchSpan":params["pitchSpan"],"fromTime":params["fromTime"],"timeSpan":params["timeSpan"]}),
    )
}
fn in_region(note: &Value, region: &Value, slack: f64) -> bool {
    let pitch = note["pitch"].as_f64().unwrap_or(f64::NAN);
    let start = note["start"].as_f64().unwrap_or(f64::NAN);
    let from = region["fromPitch"].as_f64().unwrap();
    let time = region["fromTime"].as_f64().unwrap();
    pitch >= from
        && pitch < from + region["pitchSpan"].as_f64().unwrap()
        && start >= time - slack
        && start < time + region["timeSpan"].as_f64().unwrap() + slack
}

fn fence(reference: &Value, clip: &NoteClip) -> String {
    js_json::stringify(&json!({
    "ref":reference,
    "notes":clip.notes,
    "notesRevision":clip.notes_revision,
    "authority":clip.authority}
    ))
}

fn counted(result: &Value, field: &str, count: usize) -> Result<bool, LiveError> {
    if result.is_null() {
        return Err(LiveError::type_error(format!("Cannot read properties of null (reading '{field}')")));
    }
    Ok(result[field].as_f64() == Some(count as f64))
}

impl McpHost {
    pub async fn dispatch_note_target_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_note_edit_preview" => Some(self.live_note_target_preview_async(&call.id, p).await),
            "live_note_edit_apply" => self.live_note_target_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_note_target_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(
            params,
            &["clipRef", "action", "noteIds", "grid", "amount", "pitch", "all", "none", "fromPitch", "pitchSpan", "fromTime", "timeSpan"],
        ) || !is_non_empty_string(&params["clipRef"], 256)
            || !matches!(params["action"].as_str(), Some("quantize" | "quantize-pitch" | "duplicate" | "select" | "delete-range"))
        {
            return error(id, -32602, "clipRef and a valid action are required", None);
        }

        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }

            let action = params["action"].as_str().unwrap();
            let operation = operation(action);
            if !status.has_operation(operation) {
                return Err(LiveError::error(format!("{operation} is unavailable")));
            }

            let snapshot = self.views.view_for(None, &[params["clipRef"].clone()], None, &[]).await?;
            let clip = self.note_clip(&snapshot, None, params["clipRef"].as_str().unwrap()).await?;

            let present: HashSet<_> =
                clip.notes.iter().filter_map(|n| n.get("id")).filter(|v| v.is_number()).map(|v| key(Some(v))).collect();
            let mut payload = json!({
            "ref":params["clipRef"],
            "expectedClipAuthority":clip.authority,
            "expectedNotesRevision":clip.notes_revision}
            );
            let mut affected = None;

            match action {
                "duplicate" => {
                    if !valid_ids(&params["noteIds"]) {
                        return Ok(error(id, -32602, "noteIds must be one or more unique non-negative integers", None));
                    }
                    if params["noteIds"].as_array().unwrap().iter().any(|v| !present.contains(&key(Some(v)))) {
                        return Ok(transaction_error(id, "note id is not present in the clip"));
                    }
                    payload["noteIds"] = params["noteIds"].clone();
                }
                "select" => {
                    let modes: Vec<_> = ["noteIds", "all", "none"].iter().filter(|k| params.get(**k).is_some()).copied().collect();
                    if modes.len() != 1 || (modes[0] != "noteIds" && params[modes[0]] != true) {
                        return Ok(error(id, -32602, "name exactly one of noteIds, all: true or none: true", None));
                    }

                    if params.get("noteIds").is_some() {
                        if !valid_ids(&params["noteIds"]) {
                            return Ok(error(id, -32602, "noteIds must be one or more unique non-negative integers", None));
                        }
                        if params["noteIds"].as_array().unwrap().iter().any(|v| !present.contains(&key(Some(v)))) {
                            return Ok(transaction_error(id, "note id is not present in the clip"));
                        }
                        payload["noteIds"] = params["noteIds"].clone();
                    } else {
                        payload[modes[0]] = json!(true);
                    }

                    payload.as_object_mut().unwrap().remove("expectedNotesRevision");
                    affected = Some(if params["all"] == true {
                        clip.notes.len()
                    } else if params["none"] == true {
                        0
                    } else {
                        payload["noteIds"].as_array().unwrap().len()
                    });
                }
                "delete-range" => {
                    let Some(region) = region(params) else {
                        return Ok(error(
                            id,
                            -32602,
                            "fromPitch (0-127), pitchSpan (1-128), fromTime (0 or more) and timeSpan (more than 0) are required",
                            None,
                        ));
                    };

                    for (k, v) in region.as_object().unwrap() {
                        payload[k] = v.clone();
                    }
                    affected = Some(clip.notes.iter().filter(|n| in_region(n, &region, 0.0)).count());
                }
                _ => {
                    if !params["grid"].as_f64().is_some_and(|n| n.is_finite() && n > 0.0)
                        || !params["amount"].as_f64().is_some_and(|n| n.is_finite() && (0.0..=1.0).contains(&n))
                    {
                        return Ok(error(id, -32602, "grid and amount are required for quantization", None));
                    }
                    // Within 0.1% of one, as Live takes it (1/3 written as 0.3333 is the 1/8 triplet).
                    let grid = params["grid"].as_f64().unwrap();
                    if !QUANTIZE_GRIDS.iter().any(|beats| (grid - beats).abs() <= 1e-3 * beats) {
                        return Ok(error(
                            id,
                            -32602,
                            "grid must be one Live quantizes to: 1, 0.5, 1/3, 0.25, 1/6 or 0.125 beats (1/4, 1/8, 1/8T, 1/16, 1/16T or 1/32 notes)",
                            None,
                        ));
                    }

                    payload["grid"] = params["grid"].clone();
                    payload["amount"] = params["amount"].clone();
                    if action == "quantize-pitch" {
                        if !is_integer_in_range(&params["pitch"], 0.0, 127.0) {
                            return Ok(error(id, -32602, "pitch is required for quantize-pitch", None));
                        }
                        payload["pitch"] = params["pitch"].clone();
                    }
                }
            }
            let mut full_payload = json!({"action":action});
            for (k, v) in payload.as_object().unwrap() {
                full_payload[k] = v.clone();
            }
            let t = json!({
            "id":tempo::transaction_id("noteedit"),
            "epoch":status.epoch,
            "kind":"note-target",
            "fence":fence(&params["clipRef"],
            &clip),
            "clipRef":params["clipRef"],
            "payload":full_payload,
            "prior":{
            "notes":clip.notes}
            ,
            "expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,
            "state":"previewed"}
            );

            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "note edit")?;
            let mut value = json!({
            "transactionId":t["id"],
            "epoch":t["epoch"],
            "action":action,
            "clipRef":params["clipRef"]}
            );
            if let Some(n) = affected {
                value["notes"] = json!(n);
            }
            value["impact"] = json!(match action {
                "duplicate" => "duplicates-midi-notes",
                "select" => "selects-notes",
                "delete-range" => "deletes-midi-notes",
                _ => "quantizes-midi-notes",
            });
            value["confirmation"] = json!("apply");
            value["expiresAt"] = t["expiresAt"].clone();
            Ok(success_text(id, &value))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Note-edit preview requires fresh authoritative clip state."))
    }
    pub async fn live_note_target_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(params, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }

        let Some(record) = self.clip_lifecycle_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired note-edit transaction"));
        };
        let t = record.borrow().clone();

        if t["kind"] != "note-target"
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Some(transaction_error(id, "Unknown or expired note-edit transaction"));
        }

        if t["state"] == "applied" && t["applyKey"] == params["idempotencyKey"] {
            return Some(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "state":"applied",
                "idempotent":true}
                ),
            ));
        }

        let reconciliation = t["state"] == "uncertain" && t["applyKey"] == params["idempotencyKey"];
        if t["state"] != "previewed" && !reconciliation {
            return Some(transaction_error(id, "Transaction is no longer applicable"));
        }
        if signal.is_some_and(Signal::aborted) {
            return None;
        }

        let result=async{
            if reconciliation{self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;}
            let status=self.require_connected(Some("session.read"))?;
if json!(status.epoch)!=t["epoch"]{
return Ok(transaction_error(id,
"Live connection epoch changed; preview again"));
}

            let adapter=self.async_adapter();
let context=self.transaction_context(params,
signal,
reads::AUDITION_DEADLINE_MS);
let reference=t["clipRef"].as_str().unwrap();
let prior=t["prior"]["notes"].as_array().unwrap();
let before_count=prior.len();

            if !reconciliation{
let current=self.note_clip(&self.views.view_for(Some(&context),
&[t["clipRef"].clone()],
None,
&[]).await?, Some(&context),
reference).await?;
if fence(&t["clipRef"],
&current)!=t["fence"]{
return Ok(transaction_error(id,
"clip identity or notes changed since preview; preview again"));
}
}

            let action=t["payload"]["action"].as_str().unwrap();
{
let mut row=record.borrow_mut();
row["state"]=json!("applying");
row["applyKey"]=params["idempotencyKey"].clone();
}

            let mut args=t["payload"].clone();args.as_object_mut().unwrap().remove("action");
            let result=adapter.invoke_async(&LiveInvocation::new(operation(action),args),Some(&context)).await?;
            let verified=self.note_clip(&self.views.view_for(Some(&context),&[t["clipRef"].clone()],None,&[]).await?, Some(&context),reference).await?;
            match action{
                "duplicate"=>{
                    let count=t["payload"]["noteIds"].as_array().unwrap().len();
if !counted(&result,
"duplicated",
count)?||verified.notes.len()!=before_count+count{
return Err(LiveError::error("note duplication postcondition was not confirmed"));
}

                    let prior_ids=ids(prior);
record.borrow_mut()["created"]=json!({
"duplicatedIds":verified.notes.iter().filter_map(|n|n.get("id")).filter(|v|v.is_number()&&!prior_ids.contains(&key(Some(v)))).cloned().collect::<Vec<_>>()}
);

                }
                "select"=>{
let expected=if t["payload"]["all"]==true{
before_count}
else if t["payload"]["none"]==true{
0}
else{
t["payload"]["noteIds"].as_array().unwrap().len()}
;
if !counted(&result,
"selected",
expected)?||verified.notes.len()!=before_count{
return Err(LiveError::error("note selection was not confirmed"));
}
}

                "delete-range"=>{
                    let region=region(&t["payload"]).unwrap();
let kept=ids(&verified.notes);
let prior_ids=ids(prior);
let removed:Vec<_>=prior.iter().filter(|n|!kept.contains(&key(n.get("id")))).collect();

                    if !counted(&result,
"deleted",
removed.len())?||verified.notes.iter().any(|n|!prior_ids.contains(&key(n.get("id"))))||removed.iter().any(|n|!in_region(n,
&region,
1e-6))||verified.notes.iter().any(|n|in_region(n,
&region,
-1e-6)){
return Err(LiveError::error("note range deletion was not confirmed"));
}

                    record.borrow_mut()["created"]=json!({"removedIds":removed.iter().map(|n|n["id"].clone()).collect::<Vec<_>>()});
                }
                _=>{
if result.is_null(){
return Err(LiveError::type_error("Cannot read properties of null (reading 'changed')"));
}
if result["changed"]!=true||verified.notes.len()!=before_count{
return Err(LiveError::error("quantization postcondition was not confirmed"));
}
let prior_ids=ids(prior);
if verified.notes.iter().any(|n|!prior_ids.contains(&key(n.get("id")))){
return Err(LiveError::error("quantization changed the note identity set"));
}
// What its undo checks before putting the notes back.
record.borrow_mut()["appliedFence"]=json!(note_fence(&verified.notes));
}

            }
            {let mut row=record.borrow_mut();row["applyKey"]=params["idempotencyKey"].clone();row["state"]=json!("applied");}
            Ok(success_text(id,&json!({"transactionId":t["id"],"state":"applied","result":result,"idempotent":false})))
        }.await;
        Some(result.unwrap_or_else(|e| {
            self.apply_failed(id, &record, &e, "Note-edit state is uncertain; perform fresh discovery before retrying.")
        }))
    }
    pub async fn undo_note_target_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = params["transactionId"]
            .as_str()
            .and_then(|id| self.clip_lifecycle_transactions.get(id))
            .filter(|r| r.borrow()["kind"] == "note-target")
        else {
            return transaction_error(id, "Unknown or expired note-edit transaction");
        };
        let t = record.borrow().clone();

        if t["payload"]["action"] == "select" {
            return reason_error(
                id,
                "Selecting notes changes no notes: there's nothing to undo.",
                "Select other notes with live_note_edit_preview (action select).",
            );
        }

        if t["state"] == "undone" && t["undoKey"] == params["idempotencyKey"] {
            return success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "state":"undone",
                "idempotent":true}
                ),
            );
        }

        let reconciliation = t["state"] == "uncertain" && t["undoKey"] == params["idempotencyKey"];
        if (t["state"] != "applied" && !reconciliation) || !arrangement::truthy(&t["clipRef"]) {
            return transaction_error(id, "Only an applied or exact-key uncertain note-edit transaction can be undone");
        }

        let result = async {
            self.begin_undo_recovery(&record, params["idempotencyKey"].as_str().unwrap())?;
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, reads::AUDITION_DEADLINE_MS);
            let reference = t["clipRef"].as_str().unwrap();

            record.borrow_mut()["undoKey"] = params["idempotencyKey"].clone();
            if reconciliation {
                self.replay_undo_recovery(&record, adapter.as_ref(), &context).await?;
            }

            let current = self
                .note_clip(&self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?, Some(&context), reference)
                .await?;
            if let Some(moved) = self.undo_target_moved(
                id,
                &record.borrow(),
                "clip",
                &t["clipRef"],
                current.authority.get("expectedObjectIdentity"),
                t["payload"]["expectedClipAuthority"].get("expectedObjectIdentity"),
            )? {
                return Ok(moved);
            }

            let prior = t["prior"]["notes"].as_array().unwrap();
            match t["payload"]["action"].as_str().unwrap() {
                "duplicate" => {
                    let created = t["created"]["duplicatedIds"].as_array().cloned().unwrap_or_default();
                    if created.is_empty() {
                        return Ok(transaction_error(id, "note duplication lacks exact created identity"));
                    }

                    let prior_ids = ids(prior);
                    let created_ids: HashSet<_> = created.iter().map(|v| key(Some(v))).collect();
                    if current.notes.iter().any(|n| !prior_ids.contains(&key(n.get("id"))) && !created_ids.contains(&key(n.get("id")))) {
                        return Ok(transaction_error(id, "notes changed after apply; undo refused"));
                    }

                    record.borrow_mut()["state"] = json!("undoing");
                    let result = self
                        .invoke_undo_recovery(
                            &record,
                            adapter.as_ref(),
                            "note.delete",
                            &json!({
                            "ref":t["clipRef"],
                            "noteIds":created,
                            "expectedClipAuthority":current.authority,
                            "expectedNotesRevision":current.notes_revision}
                            ),
                            &context,
                        )
                        .await?;

                    if !counted(&result, "deleted", created.len())? {
                        return Err(LiveError::error("note duplication undo did not delete the exact created batch"));
                    }

                    let verified = self
                        .note_clip(
                            &self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?,
                            Some(&context),
                            reference,
                        )
                        .await?;
                    if note_content_fence(&verified.notes) != note_content_fence(prior) {
                        return Err(LiveError::error("note duplication undo did not restore exact prior content"));
                    }
                }
                "delete-range" => {
                    let removed: HashSet<_> = t["created"]["removedIds"].as_array().into_iter().flatten().map(|v| key(Some(v))).collect();
                    if !(reconciliation && note_content_fence(&current.notes) == note_content_fence(prior)) {
                        let kept: Vec<_> = prior.iter().filter(|n| !removed.contains(&key(n.get("id")))).cloned().collect();
                        if note_fence(&current.notes) != note_fence(&kept) {
                            return Ok(transaction_error(id, "notes changed after apply; undo refused"));
                        }
                        record.borrow_mut()["state"] = json!("undoing");
                        let notes: Vec<_> = prior
                            .iter()
                            .filter(|n| removed.contains(&key(n.get("id"))))
                            .map(|n| normalized_note(n, true, false, true))
                            .collect();

                        if !notes.is_empty() {
                            let added = self
                                .invoke_undo_recovery(
                                    &record,
                                    adapter.as_ref(),
                                    "note.add-batch",
                                    &json!({
                                    "ref":t["clipRef"],
                                    "notes":notes,
                                    "expectedClipAuthority":current.authority,
                                    "expectedNotesRevision":current.notes_revision}
                                    ),
                                    &context,
                                )
                                .await?;
                            if !counted(&added, "added", notes.len())? {
                                return Err(LiveError::error("note range undo did not re-add every note"));
                            }
                        }

                        let verified = self
                            .note_clip(
                                &self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?,
                                Some(&context),
                                reference,
                            )
                            .await?;
                        if note_content_fence(&verified.notes) != note_content_fence(prior) {
                            return Err(LiveError::error("note range undo did not restore exact prior content"));
                        }
                    }
                }
                _ => {
                    let current_ids = ids(&current.notes);
                    // Every field goes back to before the quantize, so a later edit (the same notes, moved or
                    // changed) refuses it, as every other note undo does.
                    let edited = !reconciliation && t["appliedFence"].as_str().is_some_and(|fence| note_fence(&current.notes) != fence);
                    if current_ids.len() != prior.len() || prior.iter().any(|n| !current_ids.contains(&key(n.get("id")))) || edited {
                        return Ok(transaction_error(id, "notes changed after apply; undo refused"));
                    }

                    record.borrow_mut()["state"] = json!("undoing");
                    let restore: Vec<_> = prior.iter().map(|n| normalized_note(n, true, true, false)).collect();

                    self.invoke_undo_recovery(
                        &record,
                        adapter.as_ref(),
                        "note.update",
                        &json!({
                        "ref":t["clipRef"],
                        "notes":restore,
                        "expectedClipAuthority":current.authority,
                        "expectedNotesRevision":current.notes_revision}
                        ),
                        &context,
                    )
                    .await?;

                    let verified = self
                        .note_clip(
                            &self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?,
                            Some(&context),
                            reference,
                        )
                        .await?;
                    if note_fence(&verified.notes) != note_fence(prior) {
                        return Err(LiveError::error("quantization undo did not restore exact prior notes"));
                    }
                }
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Note-edit undo is uncertain; perform fresh discovery.")
        })
    }
}
