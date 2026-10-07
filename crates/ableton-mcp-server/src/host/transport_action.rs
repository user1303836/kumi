//! Momentary transport actions, including jumps inside a track's running clip.
use super::*;
use kumi_common::{abort::Signal, js::json as js_json, time::now_ms_f64};
const PARTS: &[LiveSnapshotPart] = &[LiveSnapshotPart::Set, LiveSnapshotPart::Playback];
impl McpHost {
    pub async fn dispatch_transport_action_tool(
        &self,
        call: &ToolCall,
        signal: Option<&Signal>,
    ) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_transport_action_preview" => Some(self.live_transport_action_preview_async(&call.id, p).await),
            "live_transport_action_apply" => self.live_transport_action_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_transport_action_preview_async(&self, id: &Value, p: &Value) -> Value {
        let actions = [
            "start",
            "continue",
            "stop",
            "play-selection",
            "scrub",
            "tap-tempo",
            "nudge-up",
            "nudge-down",
            "re-enable-automation",
            "trigger-session-record",
            "force-link-beat-time",
            "stop-all-clips",
            "back-to-arrangement",
            "jump-by",
            "jump-in-running-clip",
        ];
        let Some(action) = p["action"].as_str().filter(|a| actions.contains(a)) else {
            return error(id, -32602, "a valid action is required", None);
        };
        if !has_only(p, &["action", "beatTime", "beats", "trackRef"]) {
            return error(id, -32602, "a valid action is required", None);
        }
        if ["force-link-beat-time", "scrub"].contains(&action) && !p["beatTime"].as_f64().is_some_and(f64::is_finite) {
            return error(
                id,
                -32602,
                &format!("beatTime is required for {}", if action == "scrub" { "the scrub distance" } else { "force-link-beat-time" }),
                None,
            );
        }
        let jumping = ["jump-by", "jump-in-running-clip"].contains(&action);
        if jumping != p.get("beats").is_some() || (jumping && !p["beats"].as_f64().is_some_and(|n| n.is_finite() && n.abs() <= 1_000_000.))
        {
            return error(
                id,
                -32602,
                &if jumping {
                    format!("{action} takes beats: how far to jump (negative jumps back)")
                } else {
                    "beats goes with jump-by and jump-in-running-clip".into()
                },
                None,
            );
        }
        if (action == "jump-in-running-clip") != p.get("trackRef").is_some()
            || p.get("trackRef").is_some_and(|r| !is_non_empty_string(r, 256))
        {
            return error(id, -32602, "jump-in-running-clip takes a trackRef, and only it does", None);
        }
        let result=async{
            let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected||!status.capabilities.iter().any(|c|c.as_str()=="session.read"){return Err(LiveError::error("session read capability is unavailable"));}
            let (payload,fence,prior,playing)=if action=="jump-in-running-clip"{
                if !status.has_operation("track.action"){return Err(LiveError::error("jumping in a playing clip is unavailable on this Live shape"));}
                let context=LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS));let s=serde_json::to_value(self.views.view_for(Some(&context),&[p["trackRef"].clone()],None,&[]).await?).unwrap();
                let track=s["tracks"].as_array().into_iter().flatten().find(|t|t["ref"]==p["trackRef"]).filter(|t|is_non_empty_string(&t["objectIdentity"],256)).ok_or_else(||LiveError::error("track reference is not authoritative"))?;
                (json!({"ref":p["trackRef"],"action":action,"beats":p["beats"],"expectedObjectIdentity":track["objectIdentity"]}),js_json::stringify(&json!({"trackRef":p["trackRef"],"identity":track["objectIdentity"]})),json!({"playingSlotIndex":track["playingSlotIndex"]}),Some(track["playingSlotIndex"].is_number()))
            }else{
                if !status.has_operation("transport.action"){return Err(LiveError::error("transport actions are unavailable"));}
                let s=serde_json::to_value(self.views.view(None,LiveViewScope::Indices(vec![]),Some(PARTS)).await?).unwrap();
                if !is_non_empty_string(&s["set"]["objectIdentity"],256)||!is_non_empty_string(&s["playback"]["revision"],128){return Err(LiveError::error("transport identity is not authoritative"));}
                let mut payload=json!({"setRef":s["set"]["ref"],"action":action});for f in ["beatTime","beats"]{if let Some(v)=p.get(f){payload[f]=v.clone();}}payload["expectedObjectIdentity"]=s["set"]["objectIdentity"].clone();payload["expectedRevision"]=s["playback"]["revision"].clone();
                (payload,js_json::stringify(&json!({"setRef":s["set"]["ref"],"identity":s["set"]["objectIdentity"],"playbackRevision":s["playback"]["revision"]})),json!({"playing":s["playback"]["transport"]["playing"],"position":s["playback"]["transport"].get("position").filter(|v|!v.is_null()).unwrap_or(&json!(0))}),None)
            };
            let t=json!({"id":tempo::transaction_id("transportaction"),"epoch":status.epoch,"kind":"transport-action","fence":fence,"payload":payload,"prior":prior,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});self.retain_bounded_transaction(&self.clip_lifecycle_transactions,t.clone(),"transport action")?;
            let mut response=json!({"transactionId":t["id"],"epoch":t["epoch"],"action":action});if let Some(playing)=playing{response["trackRef"]=p["trackRef"].clone();response["playing"]=json!(playing);}response["impact"]=json!(if ["start","continue","play-selection","scrub","trigger-session-record","force-link-beat-time","jump-by","jump-in-running-clip"].contains(&action){"audible-transport-action-no-undo"}else{"momentary-transport-action-no-undo"});response["confirmation"]=json!("apply");response["expiresAt"]=t["expiresAt"].clone();Ok(success_text(id,&response))
        }.await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Transport-action preview requires fresh authoritative state."))
    }
    pub async fn live_transport_action_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()).filter(|r| {
            let t = r.borrow();
            t["kind"] == "transport-action" && !(t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(0.) <= now_ms_f64())
        }) else {
            return Some(transaction_error(id, "Unknown or expired transport-action transaction"));
        };
        let t = record.borrow().clone();
        if t["state"] == "applied" && t["applyKey"] == p["idempotencyKey"] {
            return Some(success_text(id, &json!({"transactionId":t["id"],"state":"applied","idempotent":true})));
        }
        let reconciliation = t["state"] == "uncertain" && t["applyKey"] == p["idempotencyKey"];
        if t["state"] != "previewed" && !reconciliation {
            return Some(transaction_error(id, "Transaction is no longer applicable"));
        }
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        let result=async{
            if reconciliation{self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;}let status=self.require_connected(Some("session.read"))?;if json!(status.epoch)!=t["epoch"]{return Ok(transaction_error(id,"Live connection epoch changed; preview again"));}
            let context=self.transaction_context(p,signal,reads::AUDITION_DEADLINE_MS);let jumping=t["payload"]["action"]=="jump-in-running-clip";
            if !reconciliation{
                if jumping{let s=serde_json::to_value(self.views.view_for(Some(&context),&[t["payload"]["ref"].clone()],None,&[]).await?).unwrap();let track=s["tracks"].as_array().into_iter().flatten().find(|r|r["ref"]==t["payload"]["ref"]);
                    if track.is_none_or(|r|js_json::stringify(&json!({"trackRef":t["payload"]["ref"],"identity":r["objectIdentity"]}))!=t["fence"]){return Ok(transaction_error(id,"the track changed since the preview; preview again"));}
                }else{let s=serde_json::to_value(self.views.view(Some(&context),LiveViewScope::Indices(vec![]),Some(PARTS)).await?).unwrap();
                    if js_json::stringify(&json!({"setRef":t["payload"]["setRef"],"identity":s["set"]["objectIdentity"],"playbackRevision":s["playback"]["revision"]}))!=t["fence"]{return Ok(transaction_error(id,"transport state changed since preview; preview again"));}
                }
            }
            record.borrow_mut()["state"]=json!("applying");record.borrow_mut()["applyKey"]=p["idempotencyKey"].clone();let result=self.async_adapter().invoke_async(&LiveInvocation::new(if jumping{"track.action"}else{"transport.action"},t["payload"].clone()),Some(&context)).await?;
            if result.is_null(){return Err(LiveError::type_error("Cannot read properties of null (reading 'done')"));}if result["done"]!=true{return Err(LiveError::error(if jumping{"the jump wasn't confirmed"}else{"transport action was not confirmed"}));}
            record.borrow_mut()["applyKey"]=p["idempotencyKey"].clone();record.borrow_mut()["state"]=json!("applied");let mut response=json!({"transactionId":t["id"],"state":"applied"});if !jumping{if let Some(revision)=result.get("revision"){response["revision"]=revision.clone();}}response["idempotent"]=json!(false);Ok(success_text(id,&response))
        }.await;
        Some(
            result.unwrap_or_else(|e| {
                apply_failed(id, &record, &e, "Transport state is uncertain; perform fresh discovery before retrying.")
            }),
        )
    }
}
