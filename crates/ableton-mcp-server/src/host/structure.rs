//! Session structure creation preserves identities across paging, retries, and cleanup.
use super::*;
use arrangement::capture_object_fingerprint;
use kumi_common::{abort::Signal, js::json as js_json};
use retention::TransactionRecord;
use sha2::{Digest, Sha256};
const STRUCTURE_STEP_DEADLINE_MS: f64 = 45000.0;
fn rows(snapshot: &LiveSnapshot, kind: &str) -> Vec<Value> {
    if kind == "track" {
        snapshot.tracks.iter().flatten().map(|r| serde_json::to_value(r).unwrap()).collect()
    } else {
        snapshot.scenes.iter().flatten().map(|r| serde_json::to_value(r).unwrap()).collect()
    }
}
fn validate_items(params: &Value) -> Option<(Vec<Value>, Vec<Value>)> {
    if !has_only(params, &["tracks", "scenes"]) {
        return None;
    }
    fn parse(items: &Value, kind: &str) -> Option<Vec<Value>> {
        let items = items.as_array()?;
        if items.len() > 1000 {
            return None;
        }
        let mut result = vec![];
        for (position, item) in items.iter().enumerate() {
            if !has_only(item, if kind == "track" { &["name", "kind", "index"] } else { &["name", "index"] })
                || !is_non_empty_string(&item["name"], 128)
                || (kind == "track" && !matches!(item["kind"].as_str(), Some("audio" | "midi")))
            {
                return None;
            }
            let index = item.get("index").cloned().unwrap_or(json!(position));
            if !is_integer_in_range(&index, 0.0, 100_000.0) {
                return None;
            }
            let mut row = json!({"kind":kind,"name":item["name"]});
            if kind == "track" {
                row["trackKind"] = item["kind"].clone();
            }
            row["index"] = index;
            result.push(row);
        }
        Some(result)
    }
    Some((parse(&params["tracks"], "track")?, parse(&params["scenes"], "scene")?))
}
fn create_args(item: &Value, revision: String) -> Value {
    let mut args = json!({"name":item["name"]});
    if item["kind"] == "track" {
        args["kind"] = item["trackKind"].clone();
    }
    args["index"] = item["index"].clone();
    args["expectedStructureRevision"] = json!(revision);
    args
}
fn create_operation(item: &Value) -> &'static str {
    if item["kind"] == "track" {
        "track.create"
    } else {
        "scene.create"
    }
}
fn delete_operation(item: &Value) -> &'static str {
    if item["kind"] == "track" {
        "track.delete"
    } else {
        "scene.delete"
    }
}
const CHANGED_BEFORE_COMPENSATION: &str = "transaction-owned Session structure changed before compensation";
/// How many reads past the first a new track gets to settle, and how far apart.
const SETTLE_READS: usize = 8;
const SETTLE_MS: u64 = 100;

/// A track's devices as a shape: each device's class, in order, with its chains' devices (racks, a Drum
/// Rack's pads), and nothing of their values or names. Live's late setup of a new track (a default track's
/// saved values, its routing) leaves it as it was; a device added, removed or swapped doesn't.
fn device_shape(row: &Value) -> Result<String, LiveError> {
    fn devices(list: &Value) -> Value {
        Value::Array(
            list.as_array()
                .into_iter()
                .flatten()
                .map(|device| {
                    let chains = |key: &str| -> Vec<Value> {
                        device[key]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|chain| if chain["devices"].is_array() { devices(&chain["devices"]) } else { devices(&chain["chains"]) })
                            .collect()
                    };
                    json!([device["className"].as_str().unwrap_or(""), chains("chains"), chains("drumPads")])
                })
                .collect(),
        )
    }
    capture_object_fingerprint(&devices(&row["devices"]))
}

/// Whether a track this transaction made holds nothing a producer added since: the name it was given,
/// no clips (Session or Arrangement) on it, and the device shape it settled with. Live goes on setting a
/// new track up after making it, its routing and a default track's devices with their saved values, so
/// its fingerprint can differ without anyone touching it (#203). A rename, a clip or a device added or
/// taken away still keeps it from cleanup. A scene isn't set up late: its fingerprint must match.
fn untouched_since_made(snapshot: &LiveSnapshot, item: &Value) -> bool {
    let Some(row) = rows(snapshot, "track").into_iter().find(|r| item["kind"] == "track" && r["ref"] == item["ref"]) else {
        return false;
    };
    if row["objectIdentity"] != item["objectIdentity"] || item.get("name").is_some_and(|name| row["name"] != *name) {
        return false;
    }
    if !item["shape"].is_string() || device_shape(&row).ok().as_deref() != item["shape"].as_str() {
        return false;
    }
    let clips = row["clips"].as_array().is_some_and(|clips| !clips.is_empty())
        || row["clipSlots"].as_array().into_iter().flatten().any(|slot| !slot["clipRef"].is_null());
    let arranged = snapshot.arrangement.as_ref().is_some_and(|arrangement| {
        arrangement
            .clips
            .iter()
            .flatten()
            .any(|clip| clip.get("trackRef") == Some(&item["ref"]) || clip.get("parentRef") == Some(&item["ref"]))
    });
    !clips && !arranged
}

/// What an apply made, as the model gets it: each track's device shape stays in the transaction, for
/// cleanup to check.
fn reported(created: &Value) -> Value {
    let mut created = created.clone();
    for item in created.as_array_mut().into_iter().flatten() {
        if let Some(item) = item.as_object_mut() {
            item.remove("shape");
        }
    }
    created
}

/// What a failed apply left in Live, by ref and name as `snapshot` has it, for the model to clean up.
fn left_in_live(snapshot: &LiveSnapshot, created: &[Value]) -> String {
    let left: Vec<String> = created
        .iter()
        .filter_map(|item| {
            rows(snapshot, item["kind"].as_str().unwrap_or("")).into_iter().find(|row| row["objectIdentity"] == item["objectIdentity"])
        })
        .map(|row| format!("{} {}", row["ref"].as_str().unwrap_or("?"), js_json::stringify(&row["name"])))
        .collect();
    if left.is_empty() {
        "nothing".into()
    } else {
        left.join(", ")
    }
}

fn created_item(item: &Value, result: &Value) -> Value {
    let mut row = json!({"ref":result["ref"],"objectIdentity":result["objectIdentity"],"kind":item["kind"]});
    if let Some(name) = result.get("name") {
        row["name"] = name.clone();
    }
    row["index"] = result.get("index").filter(|v| !v.is_null()).unwrap_or(&item["index"]).clone();
    row["fingerprint"] = result["createdFingerprint"].clone();
    row
}

impl McpHost {
    pub async fn dispatch_structure_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Value, LiveError>> {
        let args = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match (call.name.as_str(), call.asynchronous) {
            ("live_session_structure_preview", true) => self.live_session_structure_preview_async(&call.id, args).await,
            ("live_session_structure_preview", false) => self.live_session_structure_preview(&call.id, args),
            ("live_session_structure_apply", true) => self.live_session_structure_apply_async(&call.id, args, signal).await,
            ("live_session_structure_apply", false) => self.live_session_structure_apply(&call.id, args),
            _ => return None,
        }))
    }
    pub(super) fn structure_revision(&self, snapshot: &LiveSnapshot) -> String {
        let tracks: Vec<_> = rows(snapshot, "track")
            .iter()
            .enumerate()
            .map(|(i, r)| json!([r["ref"], r["objectIdentity"], r["name"], r["kind"], i]))
            .collect();
        let scenes: Vec<_> =
            rows(snapshot, "scene").iter().enumerate().map(|(i, r)| json!([r["ref"], r["objectIdentity"], r["name"], i])).collect();
        hex::encode(Sha256::digest(js_json::stringify(&json!({"tracks":tracks,"scenes":scenes})).as_bytes()))
    }
    pub(super) async fn structure_view(&self, context: Option<&LiveOperationContext>) -> Result<LiveSnapshot, LiveError> {
        self.views.view(context, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Tracks, LiveSnapshotPart::Scenes])).await
    }
    pub(super) async fn structure_owned_view(
        &self,
        context: Option<&LiveOperationContext>,
        items: &[Value],
    ) -> Result<LiveSnapshot, LiveError> {
        if items.iter().any(|i| i["kind"] == "scene") {
            return self.views.whole_set(context, None).await;
        }
        let refs: Vec<_> = items.iter().map(|i| i["ref"].clone()).collect();
        let indices: Vec<_> =
            items.iter().filter_map(|i| i["index"].as_f64()).filter(|n| *n >= 0.0 && n.fract() == 0.0).map(|n| n as usize).collect();
        self.views.view_for(context, &refs, None, &indices).await
    }
    /// The fingerprint a track or scene this transaction just made settles at, which cleanup and undo
    /// check against. Live sets a new track up after making it (its routing, a default track's devices
    /// and their saved values), so the Remote Script's fingerprint from the moment it made the track can
    /// be gone by the first read. When the first read differs from it, the track is read again until two
    /// reads in a row agree, still the one made, with the name asked for (#203); a track still changing
    /// after that keeps the latest read.
    /// A scene isn't set up late, so its first read must match as it was made, as before.
    async fn settled_structure(
        &self,
        context: &LiveOperationContext,
        owned: &Value,
        name: &Value,
    ) -> Result<(String, Option<String>), LiveError> {
        let kind = owned["kind"].as_str().unwrap();
        let mut previous: Option<(String, Option<String>)> = None;
        for attempt in 0..=SETTLE_READS {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
            }
            let observed = self.structure_owned_view(Some(context), std::slice::from_ref(owned)).await?;
            let row = rows(&observed, kind).into_iter().find(|r| r["ref"] == owned["ref"]);
            let Some(row) = row.filter(|row| row["objectIdentity"] == owned["objectIdentity"] && row["name"] == *name) else {
                return Err(LiveError::error("created Session structure changed after atomic creation"));
            };
            let fingerprint = self.session_structure_created_fingerprint(&observed, kind, &owned["ref"])?;
            if kind != "track" {
                if json!(fingerprint) != owned["fingerprint"] {
                    return Err(LiveError::error("created Session structure changed after atomic creation"));
                }
                return Ok((fingerprint, None));
            }
            let settled = (fingerprint, Some(device_shape(&row)?));
            if (attempt == 0 && json!(settled.0) == owned["fingerprint"]) || previous.as_ref() == Some(&settled) {
                return Ok(settled);
            }
            previous = Some(settled);
        }
        Ok(previous.unwrap())
    }
    pub(super) fn session_structure_owned_row(&self, snapshot: &LiveSnapshot, item: &Value) -> Result<Option<Value>, LiveError> {
        let matches: Vec<_> =
            rows(snapshot, item["kind"].as_str().unwrap()).into_iter().filter(|r| r["objectIdentity"] == item["objectIdentity"]).collect();
        if matches.len() > 1 {
            return Err(LiveError::error("transaction-owned Session structure identity is ambiguous"));
        }
        if matches.first().is_some_and(|r| r["ref"] != item["ref"]) {
            return Err(LiveError::error("transaction-owned Session structure shifted from its exact reference"));
        }
        Ok(matches.into_iter().next())
    }
    pub(super) fn session_structure_created_fingerprint(
        &self,
        snapshot: &LiveSnapshot,
        kind: &str,
        reference: &Value,
    ) -> Result<String, LiveError> {
        if kind == "track" {
            let track = snapshot
                .tracks
                .iter()
                .flatten()
                .find(|t| json!(t.ref_) == *reference)
                .ok_or_else(|| LiveError::error("created track fingerprint is unavailable"))?;
            let arrangement = snapshot
                .arrangement
                .as_ref()
                .ok_or_else(|| LiveError::type_error("Cannot read properties of undefined (reading 'clips')"))?;
            let clips: Vec<_> = arrangement
                .clips
                .iter()
                .flatten()
                .filter(|c| c.get("trackRef") == Some(reference) || c.get("parentRef") == Some(reference))
                .collect();
            return capture_object_fingerprint(&json!({"track":owned_track_fingerprint_row(track),"arrangementClips":clips}));
        }
        let scenes = rows(snapshot, "scene");
        let scene =
            scenes.iter().find(|r| r["ref"] == *reference).ok_or_else(|| LiveError::error("created scene fingerprint is unavailable"))?;
        let identity = json!({"ref":scene["ref"],"parentRef":scene["parentRef"],"objectIdentity":scene["objectIdentity"],"name":scene["name"],"triggerable":scene["triggerable"]});
        let contents:Vec<_>=rows(snapshot,"track").iter().map(|t|{let slot=t["clipSlots"].as_array().and_then(|slots|slots.iter().find(|s|s["sceneIndex"].as_f64()==scene["index"].as_f64()));let clip=slot.and_then(|s|s.get("clipRef")).filter(|v|v.is_string()).and_then(|r|t["clips"].as_array().and_then(|clips|clips.iter().find(|c|c["ref"]==*r)));let owned=slot.map(|s|json!({"ref":s["ref"],"parentRef":s["parentRef"],"trackRef":s["trackRef"],"objectIdentity":s["objectIdentity"],"clipRef":s["clipRef"],"empty":s["empty"]}));json!({"trackRef":t["ref"],"trackIdentity":t["objectIdentity"],"slot":owned,"clip":clip})}).collect();
        capture_object_fingerprint(&json!({"scene":identity,"contents":contents}))
    }
    fn make_structure_preview(
        &self,
        id: &Value,
        proposed: (Vec<Value>, Vec<Value>),
        status: &LiveStatus,
        snapshot: &LiveSnapshot,
    ) -> Result<Value, LiveError> {
        let (tracks, scenes) = proposed;
        let regular: Vec<_> =
            rows(snapshot, "track").into_iter().filter(|r| !matches!(r["kind"].as_str(), Some("return" | "main" | "master"))).collect();
        let prior_scenes = rows(snapshot, "scene");
        for (i, item) in tracks.iter().enumerate() {
            if item["index"].as_f64().unwrap() > (regular.len() + i) as f64 {
                return Ok(error(id, -32602, "track index exceeds the current regular-track collection", None));
            }
        }
        for (i, item) in scenes.iter().enumerate() {
            if item["index"].as_f64().unwrap() > (prior_scenes.len() + i) as f64 {
                return Ok(error(id, -32602, "scene index exceeds the current scene collection", None));
            }
        }
        let prior_tracks: Vec<_> =
            regular.iter().enumerate().map(|(i, r)| json!({"ref":r["ref"],"name":r["name"],"kind":r["kind"],"index":i})).collect();
        let prior_scenes: Vec<_> =
            prior_scenes.iter().enumerate().map(|(i, r)| json!({"ref":r["ref"],"name":r["name"],"index":i})).collect();
        let proposed: Vec<_> = tracks.into_iter().chain(scenes).collect();
        let t = json!({"id":tempo::transaction_id("structure"),"epoch":status.epoch,"revision":self.structure_revision(snapshot),"proposed":proposed,"priorTracks":prior_tracks,"priorScenes":prior_scenes,"expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
        self.session_structure_transactions.insert(t["id"].as_str().unwrap(), t.clone())?;
        Ok(success_text(
            id,
            &json!({"transactionId":t["id"],"epoch":t["epoch"],"revision":t["revision"],"prior":{"tracks":t["priorTracks"],"scenes":t["priorScenes"]},"proposed":proposed,"impact":"creates-session-structure","confirmation":"apply","expiresAt":t["expiresAt"]}),
        ))
    }
    pub fn live_session_structure_preview(&self, id: &Value, params: &Value) -> Value {
        let Some(proposed) = validate_items(params) else {
            return error(id, -32602, "tracks and scenes must contain bounded, valid entries", None);
        };
        let result = (|| {
            let status = self.require_connected(Some("session.structure"))?;
            self.make_structure_preview(id, proposed, &status, &self.adapter.snapshot()?)
        })();
        result.unwrap_or_else(|e| {
            adapter_tool_error(id, &e, "Session structure preview failed without mutation; discover current names and ordering.")
        })
    }
    pub async fn live_session_structure_preview_async(&self, id: &Value, params: &Value) -> Value {
        let Some(proposed) = validate_items(params) else {
            return error(id, -32602, "tracks and scenes must contain bounded, valid entries", None);
        };
        let result = async {
            let status = self.require_connected(Some("session.structure"))?;
            self.make_structure_preview(id, proposed, &status, &self.structure_view(None).await?)
        }
        .await;
        result.unwrap_or_else(|e| {
            adapter_tool_error(id, &e, "Session structure preview failed without mutation; discover current names and ordering.")
        })
    }
    fn structure_apply_record(&self, id: &Value, params: &Value, asynchronous: bool) -> Result<(TransactionRecord, bool), Value> {
        if !valid_transaction_params(params, "apply") {
            return Err(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let record = self
            .session_structure_transactions
            .get(params["transactionId"].as_str().unwrap())
            .ok_or_else(|| transaction_error(id, "Unknown or expired Session-structure transaction"))?;
        let t = record.borrow();
        if t["state"] == "applied" && t["applyKey"] == params["idempotencyKey"] {
            return Err(success_text(
                id,
                &json!({"transactionId":t["id"],"state":"applied","created":reported(&t["created"]),"idempotent":true}),
            ));
        }
        let reconciliation = asynchronous && t["state"] == "uncertain" && t["applyKey"] == params["idempotencyKey"];
        if (t["state"] != "previewed" && !reconciliation)
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Err(transaction_error(id, "Session-structure preview expired or is no longer applicable"));
        }
        drop(t);
        Ok((record, reconciliation))
    }
    fn confirm_structure_created(&self, snapshot: &LiveSnapshot, created: &[Value]) -> Result<bool, LiveError> {
        for item in created {
            if !rows(snapshot, item["kind"].as_str().unwrap())
                .iter()
                .any(|r| r["ref"] == item["ref"] && r["objectIdentity"] == item["objectIdentity"] && r["name"] == item["name"])
                || self.session_structure_created_fingerprint(snapshot, item["kind"].as_str().unwrap(), &item["ref"])?
                    != item["fingerprint"]
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub fn live_session_structure_apply(&self, id: &Value, params: &Value) -> Value {
        let (record, _) = match self.structure_apply_record(id, params, false) {
            Ok(v) => v,
            Err(v) => return v,
        };
        let t = record.borrow().clone();
        let result = (|| {
            let status = self.require_connected(Some("session.structure"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            if self.structure_revision(&self.adapter.snapshot()?) != t["revision"] {
                return Ok(transaction_error(id, "Session structure changed since preview"));
            }
            let mut created = vec![];
            let dispatched = (|| {
                for item in t["proposed"].as_array().unwrap() {
                    let result = self.adapter.invoke(&LiveInvocation::new(
                        create_operation(item),
                        create_args(item, self.structure_revision(&self.adapter.snapshot()?)),
                    ))?;
                    if !arrangement::truthy(&result["ref"])
                        || !result["objectIdentity"].is_string()
                        || result["name"] != item["name"]
                        || !result["createdFingerprint"].is_string()
                    {
                        return Err(LiveError::error(format!("Live did not confirm atomically owned {}", item["kind"].as_str().unwrap())));
                    }
                    created.push(created_item(item, &result));
                }
                if !self.confirm_structure_created(&self.adapter.snapshot()?, &created)? {
                    return Err(LiveError::error("Live did not confirm unchanged atomically owned Session structure"));
                }
                Ok(())
            })();
            if let Err(cause) = dispatched {
                for item in created.iter().rev() {
                    let compensated = (|| {
                        let revision = self.structure_revision(&self.adapter.snapshot()?);
                        self.adapter.invoke(&LiveInvocation::new(
                            delete_operation(item),
                            json!({"ref":item["ref"],"expectedStructureRevision":revision,"expectedObjectIdentity":item["objectIdentity"]}),
                        ))
                    })();
                    if compensated.is_err() {
                        let mut r = record.borrow_mut();
                        r["state"] = json!("uncertain");
                        r["created"] = json!(created);
                        return Err(LiveError::error(
                            "Session-structure apply compensation failed; read authoritative structure before retrying",
                        ));
                    }
                }
                return Err(cause);
            }
            {
                let mut r = record.borrow_mut();
                r["created"] = json!(created);
                r["applyKey"] = params["idempotencyKey"].clone();
                r["state"] = json!("applied");
            }
            Ok(success_text(
                id,
                &json!({"transactionId":t["id"],"state":"applied","created":created,"epoch":t["epoch"],"idempotent":false}),
            ))
        })();
        result.unwrap_or_else(|e| {
            adapter_tool_error(id, &e, "Session-structure apply is uncertain; read authoritative tracks and scenes before retrying.")
        })
    }
    async fn compensate_structure_async(
        &self,
        record: &TransactionRecord,
        adapter: &dyn AsyncLiveAdapter,
        context: &LiveOperationContext,
    ) -> Result<(), LiveError> {
        let bounded = || {
            let mut c = context.clone();
            c.deadline_ms = Some(self.deadline(STRUCTURE_STEP_DEADLINE_MS));
            c
        };
        let created = record.borrow()["created"].as_array().cloned().unwrap_or_default();
        {
            let mut r = record.borrow_mut();
            if r["compensationSteps"].is_null() {
                r["compensationSteps"] = json!([]);
            }
            r["recoveryMode"] = json!("compensate");
        }
        for (index, item) in created.iter().rev().enumerate() {
            let mut step = record.borrow()["compensationSteps"].get(index).cloned().unwrap_or(Value::Null);
            if step.is_null() {
                let snapshot = self.structure_owned_view(Some(&bounded()), std::slice::from_ref(item)).await?;
                if self.session_structure_owned_row(&snapshot, item)?.is_none() {
                    continue;
                }
                if self.session_structure_created_fingerprint(&snapshot, item["kind"].as_str().unwrap(), &item["ref"])?
                    != item["fingerprint"]
                    && !untouched_since_made(&snapshot, item)
                {
                    return Err(LiveError::error(CHANGED_BEFORE_COMPENSATION));
                }
                step = json!({"operation":delete_operation(item),"args":{"ref":item["ref"],"expectedStructureRevision":self.structure_revision(&snapshot),"expectedObjectIdentity":item["objectIdentity"]},"completed":false});
                let mut r = record.borrow_mut();
                let steps = r["compensationSteps"].as_array_mut().unwrap();
                if steps.len() <= index {
                    steps.resize(index + 1, Value::Null);
                }
                steps[index] = step.clone();
            }
            if step["completed"] != true {
                adapter
                    .invoke_async(&LiveInvocation::new(step["operation"].as_str().unwrap(), step["args"].clone()), Some(&bounded()))
                    .await?;
                record.borrow_mut()["compensationSteps"][index]["completed"] = json!(true);
            }
        }
        let after = self.structure_view(Some(&bounded())).await?;
        for item in &created {
            if self.session_structure_owned_row(&after, item)?.is_some() {
                return Err(LiveError::error("Session-structure compensation left transaction-owned objects"));
            }
        }
        Ok(())
    }
    pub async fn live_session_structure_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let (record, reconciliation) = match self.structure_apply_record(id, params, true) {
            Ok(v) => v,
            Err(v) => return v,
        };
        let t = record.borrow().clone();
        let result=async{if reconciliation{self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;}let status=self.require_connected(Some("session.structure"))?;if json!(status.epoch)!=t["epoch"]{return Ok(transaction_error(id,"Live connection epoch changed; preview again"));}let adapter=self.async_adapter();let context=||self.transaction_context(params,signal,STRUCTURE_STEP_DEADLINE_MS);if reconciliation&&t["recoveryMode"]=="compensate"{return Ok(match self.compensate_structure_async(&record,&*adapter,&context()).await{Ok(())=>{record.borrow_mut()["state"]=json!("undone");success_text(id,&json!({"transactionId":t["id"],"state":"compensated","residuals":[],"idempotent":false}))},Err(e)=>{record.borrow_mut()["state"]=json!("uncertain");adapter_tool_error(id,&e,"Session-structure compensation remains uncertain; inspect authoritative structure.")}});}let current=self.structure_view(Some(&context())).await?;if !reconciliation&&self.structure_revision(&current)!=t["revision"]{return Ok(transaction_error(id,"Session structure changed since preview"));}let mut created=t["created"].as_array().cloned().unwrap_or_default();let mut dispatch_ambiguous=false;{let mut r=record.borrow_mut();if r["recoverySteps"].is_null(){r["recoverySteps"]=json!([]);}r["recoveryMode"]=json!("apply");r["state"]=json!("applying");r["applyKey"]=params["idempotencyKey"].clone();}
 let dispatched=async{for(index,item)in t["proposed"].as_array().unwrap().iter().enumerate(){let mut step=record.borrow()["recoverySteps"].get(index).cloned().unwrap_or(Value::Null);if step.is_null(){step=json!({"operation":create_operation(item),"args":create_args(item,self.structure_revision(&self.structure_view(Some(&context())).await?))});let mut r=record.borrow_mut();let steps=r["recoverySteps"].as_array_mut().unwrap();if steps.len()<=index{steps.resize(index+1,Value::Null);}steps[index]=step.clone();}let mut result=step["result"].clone();if !arrangement::truthy(&result){dispatch_ambiguous=true;result=adapter.invoke_async(&LiveInvocation::new(step["operation"].as_str().unwrap(),step["args"].clone()),Some(&context())).await?;dispatch_ambiguous=false;}if !arrangement::truthy(&result["ref"])||!result["objectIdentity"].is_string()||!is_non_empty_string(&result["createdFingerprint"],64){return Err(LiveError::error(format!("Live did not return atomic created {} ownership evidence",item["kind"].as_str().unwrap())));}let mut held=json!({"ref":result["ref"],"objectIdentity":result["objectIdentity"]});if let Some(name)=result.get("name"){held["name"]=name.clone();}held["index"]=result.get("index").filter(|v|!v.is_null()).unwrap_or(&item["index"]).clone();held["createdFingerprint"]=result["createdFingerprint"].clone();record.borrow_mut()["recoverySteps"][index]["result"]=held;if !created.iter().any(|i|i["ref"]==result["ref"]){created.push(created_item(item,&result));}record.borrow_mut()["created"]=json!(created);let at=created.iter().position(|i|i["ref"]==result["ref"]).unwrap();let(settled,shape)=self.settled_structure(&context(),&created[at],&item["name"]).await?;if json!(settled)!=created[at]["fingerprint"]{created[at]["fingerprint"]=json!(settled);record.borrow_mut()["recoverySteps"][index]["result"]["createdFingerprint"]=json!(settled);}if let Some(shape)=shape{created[at]["shape"]=json!(shape);}record.borrow_mut()["created"]=json!(created);if result["name"]!=item["name"]{return Err(LiveError::error(format!("Live did not confirm created {}",item["kind"].as_str().unwrap())));}}
 if !self.confirm_structure_created(&self.structure_owned_view(Some(&context()),&created).await?,&created)?{return Err(LiveError::error("Live did not confirm unchanged atomically owned Session structure"));}Ok(())}.await;
 if let Err(cause)=dispatched{record.borrow_mut()["created"]=json!(created);if dispatch_ambiguous{record.borrow_mut()["recoveryMode"]=json!("apply");return Err(cause);}match self.compensate_structure_async(&record,&*adapter,&context()).await{Ok(())=>record.borrow_mut()["state"]=json!("undone"),Err(failure)=>{{let mut r=record.borrow_mut();r["state"]=json!("uncertain");r["recoveryMode"]=json!("compensate");}let left=match self.structure_view(Some(&context())).await{Ok(view)=>left_in_live(&view,&created),Err(_)=>"read Live to see".into()};return Err(LiveError::error(if failure.message()==CHANGED_BEFORE_COMPENSATION{format!("Session-structure apply failed ({}). Something it made was changed in Live since (a new name, a clip or a device), so Kumi stopped cleaning up. Left in Live: {left}. A retry won't remove them; ask the producer before deleting any of them.",cause.message())}else{format!("Session-structure apply compensation failed; retry the exact key to reconcile cleanup. The apply failed ({}), then its cleanup ({}); left in Live: {left}",cause.message(),failure.message())}));}}return Err(cause);}{let mut r=record.borrow_mut();r["created"]=json!(created);r["applyKey"]=params["idempotencyKey"].clone();r["state"]=json!("applied");}Ok(success_text(id,&json!({"transactionId":t["id"],"state":"applied","created":reported(&json!(created)),"epoch":t["epoch"],"idempotent":false})))
 }.await;
        result.unwrap_or_else(|e| {
            if record.borrow()["state"] == "applying" {
                record.borrow_mut()["state"] = json!("uncertain");
            }
            adapter_tool_error(id, &e, "Session-structure apply is uncertain; read authoritative tracks and scenes before retrying.")
        })
    }
    pub fn undo_structure(&self, id: &Value, params: &Value) -> Value {
        let Some(record) = params["transactionId"].as_str().and_then(|id| self.session_structure_transactions.get(id)) else {
            return transaction_error(id, "Session-structure state is uncertain; read authoritative tracks and scenes before undo");
        };
        let t = record.borrow().clone();
        if t["state"] == "uncertain" {
            return transaction_error(id, "Session-structure state is uncertain; read authoritative tracks and scenes before undo");
        }
        if t["state"] != "applied" || !arrangement::truthy(&t["created"]) {
            return transaction_error(id, "Only an applied Session-structure transaction can be undone");
        }
        if t["undoKey"] == params["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let result = (|| {
            let status = self.require_connected(Some("session.structure"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let current = self.adapter.snapshot()?;
            let created = t["created"].as_array().unwrap();
            if !created.iter().all(|i| {
                rows(&current, i["kind"].as_str().unwrap())
                    .iter()
                    .any(|r| r["ref"] == i["ref"] && r["objectIdentity"] == i["objectIdentity"] && r["name"] == i["name"])
            }) {
                return Ok(transaction_error(id, "Session structure changed after apply; undo refused"));
            }
            for item in created.iter().rev() {
                let revision = self.structure_revision(&self.adapter.snapshot()?);
                self.adapter.invoke(&LiveInvocation::new(
                    delete_operation(item),
                    json!({"ref":item["ref"],"expectedStructureRevision":revision,"expectedObjectIdentity":item["objectIdentity"]}),
                ))?;
            }
            {
                let mut r = record.borrow_mut();
                r["state"] = json!("undone");
                r["undoKey"] = params["idempotencyKey"].clone();
            }
            Ok(success_text(
                id,
                &json!({"transactionId":t["id"],"state":"undone","restored":{"tracks":t["priorTracks"],"scenes":t["priorScenes"]},"idempotent":false}),
            ))
        })();
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Session-structure undo is uncertain; inspect authoritative tracks and scenes.")
        })
    }
    pub async fn undo_structure_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Result<Value, LiveError> {
        let Some(record) = params["transactionId"].as_str().and_then(|id| self.session_structure_transactions.get(id)) else {
            return Ok(transaction_error(id, "Only an applied or exact-key uncertain Session-structure transaction can be undone"));
        };
        let t = record.borrow().clone();
        let reconciliation = t["state"] == "uncertain" && t["undoKey"] == params["idempotencyKey"];
        if t["state"] == "undone" && t["undoKey"] == params["idempotencyKey"] {
            return Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true})));
        }
        if (t["state"] != "applied" && !reconciliation) || !arrangement::truthy(&t["created"]) {
            return Ok(transaction_error(id, "Only an applied or exact-key uncertain Session-structure transaction can be undone"));
        }
        let status = self.require_connected(Some("session.structure"))?;
        if json!(status.epoch) != t["epoch"] {
            return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
        }
        let adapter = self.async_adapter();
        let context = || self.transaction_context(params, signal, STRUCTURE_STEP_DEADLINE_MS);
        self.begin_undo_recovery(&record, params["idempotencyKey"].as_str().unwrap())?;
        record.borrow_mut()["undoKey"] = params["idempotencyKey"].clone();
        let created = t["created"].as_array().unwrap();
        let result=async{if reconciliation{self.replay_undo_recovery(&record,&*adapter,&context()).await?;}self.structure_owned_view(Some(&context()),created).await?;record.borrow_mut()["state"]=json!("undoing");for item in created.iter().rev(){let current=self.structure_owned_view(Some(&context()),std::slice::from_ref(item)).await?;if self.session_structure_owned_row(&current,item)?.is_none(){continue;}let mut args=json!({"ref":item["ref"],"expectedStructureRevision":self.structure_revision(&current),"expectedObjectIdentity":item["objectIdentity"]});if item["kind"]=="track"{args["discardChanges"]=json!(true);}self.invoke_undo_recovery(&record,&*adapter,delete_operation(item),&args,&context()).await?;}let after=self.structure_view(Some(&context())).await?;for item in created{if self.session_structure_owned_row(&after,item)?.is_some(){return Err(LiveError::error("Session-structure undo left transaction-owned objects"));}}Ok(())}.await;
        if let Err(cause) = result {
            if record.borrow()["state"] == "applied" && cause.message().contains("was modified after apply") {
                self.delete_undo_plan(&record);
                return Ok(adapter_tool_error(id, &cause, "Session-structure undo refused; nothing changed."));
            }
            record.borrow_mut()["state"] = json!("uncertain");
            return Ok(adapter_tool_error(id, &cause, "Session-structure undo is uncertain; inspect authoritative tracks and scenes."));
        }
        record.borrow_mut()["state"] = json!("undone");
        Ok(success_text(
            id,
            &json!({"transactionId":t["id"],"state":"undone","restored":{"tracks":t["priorTracks"],"scenes":t["priorScenes"]},"idempotent":false}),
        ))
    }
}
