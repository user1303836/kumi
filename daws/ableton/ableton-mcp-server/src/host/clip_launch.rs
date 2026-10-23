//! Individual clip launch and exact-target stop with separate uncertain phases.
use super::*;
use audition::{active_targets, random_confirmation, target_key};
use kumi_common::abort::{Signal, SignalExt};
use retention::TransactionRecord;
const TARGET_FIELDS: &[&str] =
    &["slotRef", "trackRef", "sceneRef", "sceneIndex", "clipRef", "trackIdentity", "sceneIdentity", "slotIdentity", "clipIdentity"];
const APPLY_REMEDIATION:&str="Clip-launch state is uncertain; reconcile only with the exact original key or use the exact stop workflow after fresh playback discovery.";
const STOP_REMEDIATION: &str = "Clip-launch stop is uncertain; perform fresh playback discovery.";
fn rows(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn valid_params(params: &Value) -> bool {
    has_only(params, &["transactionId", "confirmation", "idempotencyKey"])
        && is_non_empty_string(&params["transactionId"], 128)
        && is_non_empty_string(&params["confirmation"], 128)
        && is_idempotency_key(&params["idempotencyKey"])
}
fn target_fields(t: &Value) -> Value {
    let mut fields = json!({});
    for field in TARGET_FIELDS {
        fields[*field] = t[*field].clone();
    }
    fields
}
fn uncertain_phase(t: &Value) -> &str {
    t["uncertainPhase"].as_str().unwrap_or_else(|| if arrangement::truthy(&t["stopKey"]) { "stop" } else { "apply" })
}
fn identity_matches(snapshot: &Value, t: &Value) -> bool {
    let Some(track) = rows(&snapshot["tracks"]).iter().find(|r| r["ref"] == t["trackRef"] && r["objectIdentity"] == t["trackIdentity"])
    else {
        return false;
    };
    rows(&track["clipSlots"]).iter().any(|r| {
        r["ref"] == t["slotRef"]
            && r["objectIdentity"] == t["slotIdentity"]
            && r["clipRef"] == t["clipRef"]
            && r["sceneIndex"].as_f64() == t["sceneIndex"].as_f64()
    }) && rows(&track["clips"]).iter().any(|r| r["ref"] == t["clipRef"] && r["objectIdentity"] == t["clipIdentity"])
        && rows(&snapshot["scenes"]).iter().any(|r| {
            r["ref"] == t["sceneRef"] && r["objectIdentity"] == t["sceneIdentity"] && r["index"].as_f64() == t["sceneIndex"].as_f64()
        })
}
fn complete(record: &TransactionRecord, state: &str) {
    let mut row = record.borrow_mut();
    row["state"] = json!(state);
    row.as_object_mut().unwrap().remove("uncertainPhase");
}
impl McpHost {
    pub async fn dispatch_clip_launch_tool(
        self: &Rc<Self>,
        call: &ToolCall,
        signal: Option<&Signal>,
    ) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(match call.name.as_str() {
            "live_clip_launch_preview" => Ok(Some(self.live_clip_launch_preview_async(&call.id, p).await)),
            "live_clip_launch_apply" => self.live_clip_launch_apply_async(&call.id, p, signal).await,
            "live_clip_launch_stop" => Ok(self.live_clip_launch_stop_async(&call.id, p, signal).await),
            _ => return None,
        })
    }
    pub async fn live_clip_launch_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["slotRef", "outputSafety"]) || !is_non_empty_string(&params["slotRef"], 256) {
            return error(id, -32602, "slotRef and outputSafety evidence are required", None);
        }
        let result=async{
            let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected||!status.capabilities.iter().any(|c|c.as_str()=="session.read"){return Err(LiveError::error("session read capability is unavailable"))}
            if !status.has_operation("session.clip-launch"){return Err(LiveError::error("clip launch operation is unavailable"))}
            let snapshot=serde_json::to_value(self.views.view_for(None,&[params["slotRef"].clone()],None,&[]).await?).unwrap();
            if snapshot["playback"]["transport"].is_null(){return Err(LiveError::error("authoritative playback state is unavailable"))}
            let target=rows(&snapshot["tracks"]).iter().find_map(|track|rows(&track["clipSlots"]).iter().find(|s|s["ref"]==params["slotRef"]).map(|slot|(track,slot)));
            let Some((track,slot))=target.filter(|(track,slot)|slot["clipRef"].is_string()&&slot["sceneIndex"].is_number()&&track["ref"].is_string())else{return Err(LiveError::error("clip slot with an authoritative clip is required"))};
            let scene=rows(&snapshot["scenes"]).iter().find(|s|s["index"].as_f64()==slot["sceneIndex"].as_f64());let clip=rows(&track["clips"]).iter().find(|c|c["ref"]==slot["clipRef"]);
            let Some((scene,
clip))=scene.zip(clip).filter(|(scene,
clip)|[&track["objectIdentity"],
&scene["objectIdentity"],
&slot["objectIdentity"],
&clip["objectIdentity"]].iter().all(|v|v.is_string()))else{
return Err(LiveError::error("clip-launch target lacks exact authoritative object identity"))}
;

            let target_key=format!("{}|{}|{}",track["ref"].as_str().unwrap(),slot["ref"].as_str().unwrap(),scene["ref"].as_str().unwrap());if target_key.split('|').count()!=3{return Err(LiveError::error("clip references are not encodable as a target key"))}
            let t=json!({
"id":tempo::transaction_id("cliplaunch"),
"epoch":status.epoch,
"slotRef":params["slotRef"],
"trackRef":track["ref"],
"sceneRef":scene["ref"],
"sceneIndex":scene["index"],
"clipRef":slot["clipRef"],
"trackIdentity":track["objectIdentity"],
"sceneIdentity":scene["objectIdentity"],
"slotIdentity":slot["objectIdentity"],
"clipIdentity":clip["objectIdentity"],
"targetKey":target_key,
"playbackRevision":snapshot["playback"]["revision"],
"outputSafety":output_safety_of(&params["outputSafety"]),
"confirmation":random_confirmation(),
"stopConfirmation":random_confirmation(),
"expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,
"state":"previewed"}
);

            self.retain_bounded_transaction(&self.clip_launch_transactions,t.clone(),"clip launch")?;let mut target=target_fields(&t);target["targetKey"]=json!(target_key);
            Ok(success_text(id,&json!({"transactionId":t["id"],"epoch":t["epoch"],"target":target,"playbackRevision":t["playbackRevision"],"audibleImpact":"potentially-audible-clip-launch","confirmation":t["confirmation"],"stopConfirmation":t["stopConfirmation"],"expiresAt":t["expiresAt"]})))
        }.await;
        result.unwrap_or_else(|e| {
            adapter_tool_error(
                id,
                &e,
                "Nothing launched: fix what the reason says (a clip in that slot, from fresh discovery) and preview again.",
            )
        })
    }
    pub async fn live_clip_launch_apply_async(
        self: &Rc<Self>,
        id: &Value,
        params: &Value,
        signal: Option<&Signal>,
    ) -> Result<Option<Value>, LiveError> {
        if !valid_params(params) {
            return Ok(Some(error(id, -32602, "transactionId, exact confirmation, and idempotencyKey are required", None)));
        }
        let Some(record) = self.clip_launch_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return Ok(Some(transaction_error(id, "Unknown or expired clip-launch transaction")));
        };
        let t = record.borrow().clone();
        if t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()) {
            return Ok(Some(transaction_error(id, "Unknown or expired clip-launch transaction")));
        }
        if matches!(t["state"].as_str(), Some("applied" | "stopped"))
            && t["applyKey"] == params["idempotencyKey"]
            && t["confirmation"] == params["confirmation"]
        {
            return Ok(Some(success_text(
                id,
                &json!({"transactionId":t["id"],"state":t["state"],"stopConfirmation":t["stopConfirmation"],"idempotent":true}),
            )));
        }
        if t["state"] == "applying" {
            let promise = self.record_operation(&record);
            if t["applyKey"] != params["idempotencyKey"] || t["confirmation"] != params["confirmation"] || promise.is_none() {
                return Ok(Some(transaction_error(id, "Clip-launch apply is already in progress with a different request")));
            }
            return Ok(Some(match promise.unwrap().await {
                Ok(mut outcome) => {
                    outcome["idempotent"] = json!(true);
                    success_text(id, &outcome)
                }
                Err(e) => adapter_tool_error(id, &e, APPLY_REMEDIATION),
            }));
        }

        let reconciliation = t["state"] == "uncertain"
            && uncertain_phase(&t) == "apply"
            && t["applyKey"] == params["idempotencyKey"]
            && t["confirmation"] == params["confirmation"];
        if t["state"] == "uncertain" && !reconciliation {
            return Ok(Some(transaction_error(
                id,
                "Uncertain clip-launch apply requires the exact original confirmation and idempotency key",
            )));
        }
        if (t["state"] != "previewed" && !reconciliation) || t["confirmation"] != params["confirmation"] {
            return Ok(Some(transaction_error(id, "Exact clip-launch confirmation is required")));
        }
        if signal.is_some_and(Signal::aborted) {
            return Ok(None);
        }
        if reconciliation {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await;
            tokio::task::yield_now().await;
            let status = status?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(Some(transaction_error(id, "Live connection epoch changed; clip-launch reconciliation refused")));
            }
        }

        {
            let mut row = record.borrow_mut();
            row["state"] = json!("applying");
            row["applyKey"] = params["idempotencyKey"].clone();
        }
        let host = self.clone();
        let held = record.clone();
        let signal = signal.cloned();
        let promise = self
            .start_record_operation(&record, async move { host.dispatch_clip_launch_apply(&held, signal.as_ref(), reconciliation).await });
        Ok(Some(match promise.await {
            Ok(mut outcome) => {
                outcome["idempotent"] = json!(false);
                success_text(id, &outcome)
            }
            Err(e) => adapter_tool_error(id, &e, APPLY_REMEDIATION),
        }))
    }
    async fn dispatch_clip_launch_apply(
        &self,
        record: &TransactionRecord,
        signal: Option<&Signal>,
        reconciliation: bool,
    ) -> Result<Value, LiveError> {
        let t = record.borrow().clone();
        let result=async{
            let status=self.require_connected(Some("session.read"))?;if json!(status.epoch)!=t["epoch"]{return Err(LiveError::error("Live connection epoch changed; preview again"))}
            let adapter=self.async_adapter();let context=self.transaction_context(&json!({"transactionId":t["id"],"idempotencyKey":t["applyKey"]}),signal,reads::AUDITION_DEADLINE_MS);
            let snapshot=self.views.view_for(Some(&context),&[t["trackRef"].clone(),t["slotRef"].clone()],None,&[]).await;tokio::task::yield_now().await;let snapshot=serde_json::to_value(snapshot?).unwrap();
            if snapshot["playback"]["transport"].is_null(){return Err(LiveError::error("authoritative playback state is unavailable"))}
            let ours=active_targets(&snapshot["playback"]).iter().any(|target|json!(target_key(target))==t["targetKey"]);
            if !identity_matches(&snapshot,&t){return Err(LiveError::error("clip-launch target identity changed since preview"))}
            if reconciliation&&ours{complete(record,"applied");return Ok(json!({"transactionId":t["id"],"state":"applied","verified":{"targetKey":t["targetKey"],"firedOrPlaying":true},"stopConfirmation":t["stopConfirmation"],"reconciled":true}))}
            if signal.is_some_and(Signal::aborted){return Err(LiveError::error("clip launch cancelled before dispatch"))}
            let mut args=target_fields(&t);args["playbackRevision"]=t["playbackRevision"].clone();args["outputSafety"]=t["outputSafety"].clone();let result=adapter.invoke_async(&LiveInvocation::new("session.clip-launch",args),Some(&context)).await?;
            if result.is_null(){return Err(LiveError::type_error("Cannot read properties of null (reading 'launched')"))}if result["launched"]!=t["slotRef"]{return Err(LiveError::error("clip launch result does not match the previewed slot"))}
            let mut verified=false;
while kumi_common::time::now_ms_f64()<context.deadline_ms.unwrap()-250.0{
let after=serde_json::to_value(self.views.playback(Some(&context)).await?).unwrap();
let active=active_targets(&after);
if active.iter().any(|target|json!(target_key(target))==t["targetKey"]){
verified=true;
break}
if reconciliation&&after["transport"]["playing"]==false&&after["transport"]["arrangementRecord"]==false&&after["transport"]["sessionRecord"]==false&&active.is_empty(){
complete(record,
"stopped");
return Ok(json!({
"transactionId":t["id"],
"state":"stopped",
"verified":{
"targetKey":t["targetKey"],
"firedOrPlaying":false,
"launchEnded":true}
,
"stopConfirmation":t["stopConfirmation"],
"reconciled":true}
))}
tokio::time::sleep(std::time::Duration::from_millis(100)).await;
}

            if !verified{return Err(LiveError::error("clip launch was not confirmed by fresh fired or playing target evidence"))}
            complete(record,"applied");let mut outcome=json!({"transactionId":t["id"],"state":"applied","verified":{"targetKey":t["targetKey"],"firedOrPlaying":true},"stopConfirmation":t["stopConfirmation"]});if reconciliation{outcome["reconciled"]=json!(true);}Ok(outcome)
        }.await;
        if let Err(cause) = &result {
            let safely_cancelled = !reconciliation && cause.message().contains("cancelled before dispatch");
            let mut row = record.borrow_mut();
            row["state"] = json!(if safely_cancelled { "previewed" } else { "uncertain" });
            if safely_cancelled {
                row.as_object_mut().unwrap().remove("applyKey");
                row.as_object_mut().unwrap().remove("uncertainPhase");
            } else {
                row["uncertainPhase"] = json!("apply");
            }
        }

        result
    }
    pub async fn live_clip_launch_stop_async(self: &Rc<Self>, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_params(params) {
            return Some(error(id, -32602, "transactionId, exact stop confirmation, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_launch_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown clip-launch transaction"));
        };
        let t = record.borrow().clone();
        if t["state"] == "stopped" && t["stopKey"] == params["idempotencyKey"] && params["confirmation"] == t["stopConfirmation"] {
            return Some(success_text(id, &json!({"transactionId":t["id"],"state":"stopped","idempotent":true})));
        }
        if params["confirmation"] != t["stopConfirmation"] {
            return Some(transaction_error(id, "Exact clip-launch stop confirmation is required"));
        }
        if t["state"] == "stopping" {
            let promise = self.record_operation(&record);
            if t["stopKey"] != params["idempotencyKey"] || promise.is_none() {
                return Some(transaction_error(id, "Clip-launch stop is already in progress with a different request"));
            }
            return Some(match promise.unwrap().await {
                Ok(mut outcome) => {
                    outcome["idempotent"] = json!(true);
                    success_text(id, &outcome)
                }
                Err(e) => adapter_tool_error(id, &e, STOP_REMEDIATION),
            });
        }

        if !matches!(t["state"].as_str(), Some("applied" | "uncertain")) {
            return Some(transaction_error(id, "Only an applied or uncertain clip launch can be stopped"));
        }
        let phase = uncertain_phase(&t);
        let stopping_uncertain_apply = t["state"] == "uncertain" && phase == "apply";
        if t["state"] == "uncertain" && phase == "stop" && t["stopKey"] != params["idempotencyKey"] {
            return Some(transaction_error(id, "Uncertain clip stop requires the exact original idempotency key"));
        }
        if signal.is_some_and(Signal::aborted) {
            return None;
        }
        {
            let mut row = record.borrow_mut();
            row["state"] = json!("stopping");
            row["stopKey"] = params["idempotencyKey"].clone();
        }
        let host = self.clone();
        let held = record.clone();
        let signal = signal.cloned();
        let promise = self.start_record_operation(&record, async move {
            host.dispatch_clip_launch_stop(&held, signal.as_ref(), stopping_uncertain_apply).await
        });
        Some(match promise.await {
            Ok(mut outcome) => {
                outcome["idempotent"] = json!(false);
                success_text(id, &outcome)
            }
            Err(e) => adapter_tool_error(id, &e, STOP_REMEDIATION),
        })
    }
    async fn dispatch_clip_launch_stop(
        &self,
        record: &TransactionRecord,
        signal: Option<&Signal>,
        stopping_uncertain_apply: bool,
    ) -> Result<Value, LiveError> {
        let t = record.borrow().clone();
        let mut dispatched = false;
        let result = async {
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Err(LiveError::error("Live connection epoch changed; stop refused"));
            }
            if !status.has_operation("session.clip-stop") {
                return Err(LiveError::error("track stop operation is unavailable"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(
                &json!({"transactionId":t["id"],"idempotencyKey":t["stopKey"]}),
                signal,
                reads::AUDITION_DEADLINE_MS,
            );
            let before = self.views.view_for(Some(&context), &[t["trackRef"].clone(), t["slotRef"].clone()], None, &[]).await;
            tokio::task::yield_now().await;
            let before = serde_json::to_value(before?).unwrap();
            let ours = active_targets(&before["playback"]).iter().any(|target| json!(target_key(target)) == t["targetKey"]);
            if ours {
                if !identity_matches(&before, &t) {
                    return Err(LiveError::error("clip-stop target identity changed; guarded stop refused"));
                }
                if signal.is_some_and(Signal::aborted) {
                    return Err(LiveError::error("clip-launch stop cancelled before dispatch"));
                }
                dispatched = true;
                adapter.invoke_async(&LiveInvocation::new("session.clip-stop", target_fields(&t)), Some(&context)).await?;
            }

            let mut confirmed = false;
            while kumi_common::time::now_ms_f64() < context.deadline_ms.unwrap() - 250.0 {
                let after = serde_json::to_value(self.views.playback(Some(&context)).await?).unwrap();
                let still = active_targets(&after).iter().any(|target| json!(target_key(target)) == t["targetKey"]);
                if !still {
                    confirmed = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }

            if !confirmed {
                return Err(LiveError::error("clip stop was not confirmed by fresh authoritative state"));
            }
            complete(record, "stopped");
            Ok(json!({"transactionId":t["id"],"state":"stopped","targetCleared":true}))
        }
        .await;
        if result.is_err() {
            if dispatched {
                let mut row = record.borrow_mut();
                row["state"] = json!("uncertain");
                row["uncertainPhase"] = json!("stop");
            } else if stopping_uncertain_apply {
                let mut row = record.borrow_mut();
                row["state"] = json!("uncertain");
                row["uncertainPhase"] = json!("apply");
            } else {
                complete(record, "applied");
            }
        }

        result
    }
}
