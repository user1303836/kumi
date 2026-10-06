//! Read-only, revision-bound probes and their shared mutation authority evidence.
use super::reads::AUDITION_DEADLINE_MS;
use super::*;
use crate::registry::{canonical_json, CanonicalError, CanonicalLimits};
use base64::Engine;
use kumi_common::{
    abort::Signal,
    js::{json as js_json, number as js_number},
};
use sha2::{Digest, Sha256};
const MAX_SET_COLLECTION: usize = 10_000_000;
fn hash(value: &Value) -> Result<String, LiveError> {
    Ok(hex::encode(Sha256::digest(canonical_mutation_identity(value)?)))
}
fn rows(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten().filter(|v| v.is_object())
}
fn fields(value: &Value, keys: &[&str]) -> Value {
    Value::Object(keys.iter().filter_map(|k| value.get(*k).map(|v| ((*k).into(), v.clone()))).collect())
}
fn authority_fields(value: &Value, keys: &[&str]) -> Result<Value, LiveError> {
    if keys.iter().any(|k| value.get(*k).is_none()) {
        return Err(LiveError::error("mutation authority contains an unsupported value"));
    }
    Ok(fields(value, keys))
}
fn text_or_null(value: &Value) -> Value {
    if value.is_string() {
        value.clone()
    } else {
        Value::Null
    }
}
fn outcome(id: &Value, result: Result<Value, LiveError>, remediation: &str) -> Value {
    match result {
        Ok(v) => success_text(id, &v),
        Err(e) => adapter_tool_error(id, &e, remediation),
    }
}
fn capability(status: &LiveStatus, name: &str, reason: &str) -> Result<(), LiveError> {
    if status.connected && status.capabilities.iter().any(|c| c.as_str() == name) {
        Ok(())
    } else {
        Err(LiveError::error(reason))
    }
}
fn operation(status: &LiveStatus, name: &str, reason: &str) -> Result<(), LiveError> {
    if status.has_operation(name) {
        Ok(())
    } else {
        Err(LiveError::error(reason))
    }
}
fn property<'a>(value: &'a Value, key: &str) -> Result<&'a Value, LiveError> {
    if value.is_null() {
        Err(LiveError::type_error(format!("Cannot read properties of null (reading '{key}')")))
    } else {
        Ok(value.get(key).unwrap_or(&Value::Null))
    }
}
fn collection<'a>(read: &'a Value, key: &str, message: &str) -> Result<&'a Vec<Value>, LiveError> {
    property(read, key)?.as_array().filter(|v| v.len() <= MAX_SET_COLLECTION).ok_or_else(|| LiveError::error(message))
}
fn finite(value: &Value) -> bool {
    value.as_f64().is_some_and(f64::is_finite)
}
fn page_paging(page: &Value, key: &str, bound: usize) -> Value {
    let mut v = json!({"limit":page["returned"],"total":page["total"],"complete":page["complete"]});
    if let Some(c) = page.get("nextCursor") {
        v["nextCursor"] = c.clone();
    }
    v[key] = json!(bound);
    v
}
fn probe_envelope(status: &LiveStatus) -> Value {
    let s = serde_json::to_value(status).unwrap();
    let mut v = fields(&s, &["adapter", "epoch", "protocol"]);
    v["provenance"] = s.get("provenance").filter(|v| !v.is_null()).cloned().unwrap_or(json!("unknown"));
    v["environment"] = s["environment"].clone();
    v
}
pub(super) fn node_base64_decode(text: &str) -> Vec<u8> {
    let (mut out, mut bits, mut acc) = (vec![], 0, 0u32);
    for c in text.encode_utf16() {
        let n = match c as u8 {
            b'A'..=b'Z' => (c as u8 - b'A') as u32,
            b'a'..=b'z' => (c as u8 - b'a') as u32 + 26,
            b'0'..=b'9' => (c as u8 - b'0') as u32 + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => continue,
        };
        acc = (acc << 6) | n;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}
fn probe_page(items: &[Value], revision: &str, limit: Option<&Value>, cursor: Option<&Value>, max: usize) -> Result<Value, LiveError> {
    let limit = match limit {
        None => max,
        Some(v) if is_integer_in_range(v, 1.0, max as f64) => v.as_f64().unwrap() as usize,
        _ => return Err(LiveError::range_error(format!("limit must be an integer from 1 to {max}"))),
    };
    let offset = if let Some(cursor) = cursor {
        if !is_non_empty_string(cursor, 1024) {
            return Err(LiveError::range_error("cursor is invalid"));
        }
        let decoded: Value = serde_json::from_str(&String::from_utf8_lossy(&node_base64_decode(cursor.as_str().unwrap())))
            .map_err(|_| LiveError::range_error("cursor is invalid"))?;
        if !decoded.is_object()
            || decoded["revision"] != revision
            || !decoded["offset"].as_f64().is_some_and(|n| js_number::is_safe_integer(n) && n >= 0.0 && n <= items.len() as f64)
        {
            return Err(LiveError::error("probe cursor is stale; request a fresh first page"));
        }
        decoded["offset"].as_f64().unwrap() as usize
    } else {
        0
    };
    let next = (offset + limit).min(items.len());
    let page = &items[offset..next];
    let mut out = json!({"items":page,"total":items.len(),"returned":page.len(),"complete":next>=items.len()});
    if next < items.len() {
        out["nextCursor"] =
            json!(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(js_json::stringify(&json!({"revision":revision,"offset":next}))));
    }
    Ok(out)
}
impl McpHost {
    pub async fn dispatch_probe_tool(&self, call: &ToolCall, _signal: Option<&Signal>) -> Option<Result<Value, LiveError>> {
        if !call.asynchronous {
            return None;
        }
        let id = &call.id;
        let args = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_library_search" => return Some(self.live_library_search_async(id, args).await),
            "live_arrangement_automation_read" => self.live_arrangement_automation_read_async(id, args).await,
            "live_take_lane_read" => self.live_take_lane_read_async(id, args).await,
            "live_comp_read" => self.live_comp_read_async(id, args).await,
            "live_warp_marker_read" => self.live_warp_marker_read_async(id, args).await,
            "live_browser_inspect" => self.live_browser_inspect_async(id, args).await,
            _ => return None,
        }))
    }
    pub async fn live_arrangement_automation_read_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["clipRef", "parameterRef", "limit", "cursor"])
            || !is_non_empty_string(&params["clipRef"], 256)
            || !is_non_empty_string(&params["parameterRef"], 256)
        {
            return error(id, -32602, "clipRef and parameterRef are required", None);
        }
        let result = async {
            let status = self
                .fresh_status(Some(&LiveOperationContext::with_deadline(
                    self.deadline(AUDITION_DEADLINE_MS),
                )))
                .await?;
            capability(
                &status,
                "arrangement.read",
                "arrangement read capability is unavailable",
            )?;
            operation(
                &status,
                "arrangement.automation.read",
                "arrangement automation read is unavailable",
            )?;
            let context = LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS));
            let snapshot = self
                .views
                .view_for(
                    Some(&context),
                    &[params["clipRef"].clone(), params["parameterRef"].clone()],
                    None,
                    &[],
                )
                .await?;
            let located = self.clip_row(&snapshot, params["clipRef"].as_str().unwrap())?;
            if !located.arrangement {
                return Err(LiveError::error(
                    "arrangement automation requires an exact Arrangement clip reference",
                ));
            }
            if !is_non_empty_string(&located.clip["objectIdentity"], 256) {
                return Err(LiveError::error(
                    "arrangement clip identity is not authoritative",
                ));
            }
            let parameter =
                self.parameter_row(&snapshot, params["parameterRef"].as_str().unwrap())?;
            let read = self.async_adapter()
                .invoke_async(
                    &LiveInvocation::new(
                        "arrangement.automation.read",
                        fields(params, &["clipRef", "parameterRef"]),
                    ),
                    Some(&context),
                )
                .await?;
            property(&read, "available")?;
            let raw = collection(
                &read,
                "points",
                "arrangement automation read returned an unbounded or malformed result",
            )?;
            if !read["available"].is_boolean() || !read["exists"].is_boolean() {
                return Err(LiveError::error(
                    "arrangement automation read returned an unbounded or malformed result",
                ));
            }
            let mut points: Vec<_> = raw
                .iter()
                .filter(|v| v.is_object())
                .map(|v| fields(v, &["time", "value"]))
                .filter(|v| finite(&v["time"]) && finite(&v["value"]))
                .collect();
            if points.len() != raw.len() {
                return Err(LiveError::error(
                    "arrangement automation points are unreadable",
                ));
            }
            points.sort_by(|a, b| {
                a["time"]
                    .as_f64()
                    .unwrap()
                    .partial_cmp(&b["time"].as_f64().unwrap())
                    .unwrap()
            });
            let revision = hash(&json!({"clipRef":params["clipRef"],"clipIdentity":located.clip["objectIdentity"],"parameterRef":params["parameterRef"],"points":points}))?;
            let page = probe_page(
                &points,
                &revision,
                params.get("limit"),
                params.get("cursor"),
                512,
            )?;
            let s = serde_json::to_value(&snapshot).unwrap();
            let mut clip = fields(&located.clip, &["objectIdentity", "start", "length"]);
            clip["ref"] = params["clipRef"].clone();
            clip["arrangement"] = json!(true);
            let mut envelope = fields(&read, &["available", "exists"]);
            if read["available"] == false {
                // A Live that doesn't list an Arrangement clip's envelopes (before 12.4.15b5's automation_envelopes):
                // `exists: false` there isn't "no envelope".
                envelope["note"] = json!("This Live doesn't list an Arrangement clip's envelopes, so whether this parameter has one isn't known.");
            }
            Ok(json!({"clip":clip,"parameter":{"ref":params["parameterRef"],"name":text_or_null(&parameter["name"]),"ownerRef":text_or_null(&parameter["parentRef"]),"identity":text_or_null(&parameter["objectIdentity"])},"envelope":envelope,"range":if points.is_empty(){Value::Null}else{json!({"from":points[0]["time"],"to":points.last().unwrap()["time"]})},"points":page["items"],"paging":page_paging(&page,"pointBound",512),"curve":{"available":false,"reason":"curve shapes are not exposed by the negotiated arrangement.automation.read contract"},"revision":revision,"sessionState":{"arrangementOverdub":s["song"]["arrangementOverdub"],"sessionAutomationRecord":s["song"]["sessionAutomationRecord"],"reEnableAutomationEnabled":s["song"]["reEnableAutomationEnabled"],"note":if s.get("song").is_some_and(|v|!v.is_null()){"authoritative song automation-record state at read time"}else{"the adapter did not expose song automation-record state; external controller state is not authoritatively enumerable"}},"mutation":{"advertised":false,"note":"no arrangement automation create/delete/insert operation is advertised; mutation requires a separate reviewed issue with exact prior-state restoration evidence"},"probe":probe_envelope(&status)}))
        }
        .await;
        outcome(
            id,
            result,
            "Arrangement automation read requires a fresh authoritative shape; restart paging from the first page when a cursor is stale.",
        )
    }
    pub async fn live_take_lane_read_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["trackRef", "limit", "cursor"]) || !is_non_empty_string(&params["trackRef"], 256) {
            return error(id, -32602, "trackRef is required", None);
        }
        let result = async {
            let status = self
                .fresh_status(Some(&LiveOperationContext::with_deadline(
                    self.deadline(AUDITION_DEADLINE_MS),
                )))
                .await?;
            capability(&status, "takes", "take-lane read capability is unavailable")?;
            operation(
                &status,
                "audio.take-lane.read",
                "take-lane read is unavailable",
            )?;
            let context = LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS));
            let read = self.async_adapter()
                .invoke_async(
                    &LiveInvocation::new("audio.take-lane.read", fields(params, &["trackRef"])),
                    Some(&context),
                )
                .await?;
            let raw = collection(
                &read,
                "lanes",
                "take-lane read returned an unbounded or malformed result",
            )?;
            let advertised: Vec<_> = raw
                .iter()
                .filter(|v| v.is_object())
                .map(|v| fields(v, &["ref", "name"]))
                .collect();
            if advertised
                .iter()
                .any(|v| !is_non_empty_string(&v["ref"], 256) || !v["name"].is_string())
            {
                return Err(LiveError::error("take-lane identity is malformed"));
            }
            let snapshot = self
                .views
                .view_for(Some(&context), &[params["trackRef"].clone()], None, &[])
                .await?;
            let s = serde_json::to_value(&snapshot).unwrap();
            let track = rows(&s["tracks"])
                .find(|v| v["ref"] == params["trackRef"])
                .filter(|v| is_non_empty_string(&v["objectIdentity"], 256))
                .ok_or_else(|| LiveError::error("take-lane track identity is not authoritative"))?;
            let mut lanes = vec![];
            for lane in advertised {
                let row = rows(&track["takeLanes"])
                    .find(|v| v["ref"] == lane["ref"])
                    .unwrap_or(&Value::Null);
                let mut clips = vec![];
                for clip in rows(&row["clips"]) {
                    let mut c = fields(clip, &["ref", "start", "length"]);
                    c["objectIdentity"] = clip["objectIdentity"].clone();
                    let mut evidence = fields(clip, &["ref"]);
                    evidence["objectIdentity"] = clip["objectIdentity"].clone();
                    evidence["content"] = json!(self.bounded_clip_content_digest(clip)?);
                    c["fingerprint"] = json!(hash(&evidence)?);
                    clips.push(c);
                }
                lanes.push(json!({"ref":lane["ref"],"name":lane["name"],"index":if row["index"].is_number(){row["index"].clone()}else{Value::Null},"objectIdentity":text_or_null(&row["objectIdentity"]),"clipCount":clips.len(),"clips":clips}));
            }
            let revision = hash(&json!({"trackRef":params["trackRef"],"trackIdentity":track["objectIdentity"],"lanes":lanes}))?;
            let main: Vec<_> = rows(&s["arrangementClips"])
                .filter(|v| v["trackRef"] == params["trackRef"])
                .map(|v| {
                    let mut c = fields(&v["clip"], &["ref", "start", "length"]);
                    c["objectIdentity"] = v["clip"]["objectIdentity"].clone();
                    c
                })
                .collect();
            let page = probe_page(
                &lanes,
                &revision,
                params.get("limit"),
                params.get("cursor"),
                128,
            )?;
            Ok(json!({"track":{"ref":params["trackRef"],"objectIdentity":track["objectIdentity"]},"lanes":page["items"],"mainLane":{"clipCount":main.len(),"arrangementClips":main},"paging":page_paging(&page,"laneBound",128),"revision":revision,"relationships":{"compSourceSegments":"not enumerable through the public LOM; see live_comp_read for adapter-negotiated segment evidence","audition":"not started by this tool","mutation":"no lane creation/deletion, take promotion, or main-lane change is performed"},"probe":probe_envelope(&status)}))
        }
        .await;
        outcome(
            id,
            result,
            "Take-lane read requires a fresh authoritative track; restart paging from the first page when a cursor is stale.",
        )
    }
    pub async fn live_comp_read_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["clipRef", "limit", "cursor"]) || !is_non_empty_string(&params["clipRef"], 256) {
            return error(id, -32602, "clipRef is required", None);
        }
        let result = async {
   let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
capability(&status,"takes","comp read capability is unavailable")?;
operation(&status,"audio.comp.read","comp read is unavailable on this Live shape (the public LOM exposes no comp-region API)")?;
   let adapter=self.async_adapter();
   let context=LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS));
let snapshot=self.views.view_for(Some(&context),&[params["clipRef"].clone()],None,&[]).await?;
let located=self.clip_row(&snapshot,params["clipRef"].as_str().unwrap())?;
if !is_non_empty_string(&located.clip["objectIdentity"],256){return Err(LiveError::error("comp read requires exact clip identity"));
}
   let read=adapter.invoke_async(&LiveInvocation::new("audio.comp.read",fields(params,&["clipRef"])),Some(&context)).await?;
let raw=collection(&read,"segments","comp read returned an unbounded or malformed result")?;
let track=located.track.as_ref().unwrap_or(&Value::Null);
let mut segments:Vec<_>=raw.iter().filter(|v|v.is_object()).map(|v|{let lane=rows(&track["takeLanes"]).find(|l|l["ref"]==v["laneRef"]).unwrap_or(&Value::Null);
let mut seg=fields(v,&["laneRef","from","to"]);
seg["laneName"]=text_or_null(&lane["name"]);
seg["laneIdentity"]=text_or_null(&lane["objectIdentity"]);
seg}).filter(|s|s["laneRef"].is_string()&&finite(&s["from"])&&finite(&s["to"])&&s["to"].as_f64()>s["from"].as_f64()).collect();
if segments.len()!=raw.len(){return Err(LiveError::error("comp segments are unreadable"));
}segments.sort_by(|a,b|a["from"].as_f64().unwrap().partial_cmp(&b["from"].as_f64().unwrap()).unwrap());
let revision=hash(&json!({"clipRef":params["clipRef"],"clipIdentity":located.clip["objectIdentity"],"segments":segments}))?;
let page=probe_page(&segments,&revision,params.get("limit"),params.get("cursor"),512)?;
let mut clip=fields(&located.clip,&["objectIdentity","start","length"]);
clip["ref"]=params["clipRef"].clone();
   Ok(json!({"clip":clip,"segments":page["items"],"paging":page_paging(&page,"segmentBound",512),"revision":revision,"relationships":{"sourceHighlightFidelity":"not inferred; the public LOM exposes no comp-promotion or source-highlight API","note":"segments are adapter-reported only when audio.comp.read is negotiated; no best-take ranking is performed"},"probe":probe_envelope(&status)}))
  }.await;
        outcome(
            id,
            result,
            "Comp read requires a fresh authoritative clip; unsupported relationships are reported explicitly, never inferred.",
        )
    }
    pub async fn live_warp_marker_read_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["clipRef", "limit", "cursor"]) || !is_non_empty_string(&params["clipRef"], 256) {
            return error(id, -32602, "clipRef is required", None);
        }
        let result = async {
            let status = self
                .fresh_status(Some(&LiveOperationContext::with_deadline(
                    self.deadline(AUDITION_DEADLINE_MS),
                )))
                .await?;
            capability(&status, "warp", "warp capability is unavailable")?;
            operation(
                &status,
                "audio.warp-marker.read",
                "warp-marker read is unavailable",
            )?;
            let context = LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS));
            let snapshot = self
                .views
                .view_for(Some(&context), &[params["clipRef"].clone()], None, &[])
                .await?;
            let located = self.clip_row(&snapshot, params["clipRef"].as_str().unwrap())?;
            if located.clip["kind"] != "audio" && located.clip["isAudio"] != true {
                return Err(LiveError::error("warp markers require an audio clip"));
            }
            let read = self.async_adapter()
                .invoke_async(
                    &LiveInvocation::new("audio.warp-marker.read", json!({"ref":params["clipRef"]})),
                    Some(&context),
                )
                .await?;
            property(&read, "revision")?;
            let raw = collection(
                &read,
                "markers",
                "warp-marker read returned an unbounded or malformed result",
            )?;
            if !is_non_empty_string(&read["revision"], 64) {
                return Err(LiveError::error(
                    "warp-marker read returned an unbounded or malformed result",
                ));
            }
            let mut markers: Vec<_> = raw
                .iter()
                .filter(|v| v.is_object())
                .map(|v| fields(v, &["beatTime", "sampleTime"]))
                .filter(|v| finite(&v["beatTime"]) && finite(&v["sampleTime"]))
                .collect();
            if markers.len() != raw.len() {
                return Err(LiveError::error("warp markers are unreadable"));
            }
            markers.sort_by(|a, b| {
                a["beatTime"]
                    .as_f64()
                    .unwrap()
                    .partial_cmp(&b["beatTime"].as_f64().unwrap())
                    .unwrap()
            });
            let beat = markers
                .windows(2)
                .all(|w| w[1]["beatTime"].as_f64() > w[0]["beatTime"].as_f64());
            let sample = markers
                .windows(2)
                .all(|w| w[1]["sampleTime"].as_f64() >= w[0]["sampleTime"].as_f64());
            let revision = self.warp_marker_collection_revision(&markers)?;
            let page = probe_page(
                &markers,
                &revision,
                params.get("limit"),
                params.get("cursor"),
                256,
            )?;
            Ok(json!({"clip":{"ref":params["clipRef"],"objectIdentity":text_or_null(&located.clip["objectIdentity"])},"markers":page["items"],"paging":page_paging(&page,"markerBound",256),"monotonic":{"beatTime":beat,"sampleTime":sample},"revisions":{"adapter":read["revision"],"collection":revision,"clipAuthority":self.clip_authority_digest(&snapshot,params["clipRef"].as_str().unwrap())?},"identity":{"addressedBy":"beatTime","stableMarkerIdsExposed":false,"note":"the API exposes no separate warp-marker identity; repeated reads prove only collection-revision stability"},"mutationFeasibility":{"add":status.has_operation("audio.warp-marker.add"),"move":status.has_operation("audio.warp-marker.move"),"delete":status.has_operation("audio.warp-marker.delete"),"advertisedByThisTool":false,"note":"read-only probe: mutation feasibility is reported from negotiated operations only; a guarded mutation requires a separate reviewed issue with complete prior-state restoration"},"probe":probe_envelope(&status)}))
        }
        .await;
        outcome(
            id,
            result,
            "Warp-marker read requires a fresh authoritative audio clip; restart paging from the first page when a cursor is stale.",
        )
    }
    pub async fn live_browser_inspect_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["itemId"]) || !is_non_empty_string(&params["itemId"], 256) {
            return error(id, -32602, "itemId is required", None);
        }
        let result = async {
            let status = self
                .fresh_status(Some(&LiveOperationContext::with_deadline(
                    self.deadline(AUDITION_DEADLINE_MS),
                )))
                .await?;
            capability(&status, "browser", "browser capability is unavailable")?;
            operation(
                &status,
                "browser.inspect",
                "browser item inspection is unavailable",
            )?;
            let item = self.async_adapter()
                .invoke_async(
                    &LiveInvocation::new("browser.inspect", fields(params, &["itemId"])),
                    Some(&LiveOperationContext::with_deadline(
                        self.deadline(AUDITION_DEADLINE_MS),
                    )),
                )
                .await?;
            if property(&item, "id")? != &params["itemId"]
                || !is_non_empty_string(&item["objectIdentity"], 256)
                || !item["name"].is_string()
                || !item["category"].is_string()
                || !item["isDevice"].is_boolean()
            {
                return Err(LiveError::error(
                    "browser item lacks exact authoritative identity",
                ));
            }
            let path = item["path"]
                .as_str()
                .filter(|p| {
                    let b = p.as_bytes();
                    !p.starts_with('/')
                        && !(b.len() >= 3
                            && b[0].is_ascii_alphabetic()
                            && b[1] == b':'
                            && matches!(b[2], b'/' | b'\\'))
                })
                .map(|p| json!(p))
                .unwrap_or(Value::Null);
            let mut identity = fields(
                &item,
                &["id", "objectIdentity", "name", "category", "isDevice"],
            );
            identity["path"] = path.clone();
            let revision = hash(&identity)?;
            let loadable = item["isDevice"] == true && status.has_operation("browser.load");
            let mut out_item = fields(&item, &["id", "name", "category", "isDevice"]);
            out_item["path"] = path;
            let status = serde_json::to_value(status).unwrap();
            let mut provenance = fields(&status, &["adapter", "epoch"]);
            provenance["operations"] = json!(["browser.inspect"]);
            Ok(json!({"item":out_item,"identity":{"objectIdentity":item["objectIdentity"],"revision":revision},"loadability":{"loadable":loadable,"reason":if loadable{"loadable through live_browser_load_preview with exact identity fencing"}else if item["isDevice"]==true{"browser.load is not negotiated on this Live shape"}else{"only device items are loadable; samples, clips, and packs report inspect-only"}},"provenance":provenance}))
        }
        .await;
        outcome(id, result, "Browser inspection requires an available Live Browser and an exact item id from a fresh search.")
    }
    pub(super) fn parameter_row(&self, snapshot: &LiveSnapshot, reference: &str) -> Result<Value, LiveError> {
        fn walk<'a>(devices: impl Iterator<Item = &'a Value>, reference: &str) -> Option<&'a Value> {
            for device in devices {
                if let Some(found) = rows(&device["parameters"]).chain(rows(&device["macros"])).find(|v| v["ref"] == reference) {
                    return Some(found);
                }
                for chain in rows(&device["chains"]) {
                    if let Some(found) = walk(rows(&chain["devices"]), reference) {
                        return Some(found);
                    }
                }
                for pad in rows(&device["drumPads"]) {
                    for chain in rows(&pad["chains"]) {
                        if let Some(found) = walk(rows(&chain["devices"]), reference) {
                            return Some(found);
                        }
                    }
                }
            }
            None
        }
        let s = serde_json::to_value(snapshot).unwrap();
        for track in rows(&s["tracks"]) {
            if let Some(found) = walk(rows(&track["devices"]), reference) {
                return Ok(found.clone());
            }
        }
        Err(LiveError::error("parameter reference is not authoritative"))
    }
    pub(super) fn bounded_clip_content_digest(&self, clip: &Value) -> Result<String, LiveError> {
        let keys = [
            "name",
            "start",
            "length",
            "muted",
            "looping",
            "colorIndex",
            "isAudio",
            "notesRevision",
            "gain",
            "pitchCoarse",
            "pitchFine",
            "warpMode",
            "warping",
            "fadeInLength",
            "fadeOutLength",
            "loopStart",
            "loopEnd",
            "filePath",
            "groove",
            "warpMarkers",
            "launchMode",
            "legato",
            "velocityAmount",
            "signatureNumerator",
            "signatureDenominator",
            "ramMode",
            "clipView",
        ];
        let value = Value::Object(keys.into_iter().map(|k| (k.into(), clip[k].clone())).collect());
        let text = canonical_json(
            &value,
            &CanonicalLimits {
                max_depth: 8,
                max_string_length: 1_048_576,
                max_array_length: MAX_SET_COLLECTION,
                max_object_properties: 1_000_000,
            },
        )
        .map_err(|e| {
            LiveError::error(match e {
                CanonicalError::TooDeep => "clip content is too deeply nested",
                CanonicalError::StringTooLarge => "clip content string is too large",
                CanonicalError::ArrayTooLarge => "clip content array exceeds its authoritative bound",
                CanonicalError::ObjectTooLarge => "clip content object is too large",
            })
        })?;
        Ok(hex::encode(Sha256::digest(text)))
    }
    pub(super) fn warp_marker_collection_revision(&self, markers: &[Value]) -> Result<String, LiveError> {
        let mut sorted = markers.to_vec();
        sorted.sort_by(|a, b| {
            a["beatTime"]
                .as_f64()
                .unwrap_or(f64::NAN)
                .partial_cmp(&b["beatTime"].as_f64().unwrap_or(f64::NAN))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hash(&Value::Array(sorted.iter().map(|v| authority_fields(v, &["beatTime", "sampleTime"])).collect::<Result<_, _>>()?))
    }
    pub(super) fn clip_authority_digest(&self, snapshot: &LiveSnapshot, reference: &str) -> Result<String, LiveError> {
        let row = self.clip_row(snapshot, reference)?;
        if let Some(lane) = row.take_lane {
            let siblings: Vec<_> =
                rows(&lane["clips"]).map(|v| authority_fields(v, &["ref", "objectIdentity"])).collect::<Result<_, _>>()?;
            let mut value = fields(&lane, &[]);
            value["laneIdentity"] =
                lane.get("objectIdentity").ok_or_else(|| LiveError::error("mutation authority contains an unsupported value"))?.clone();
            value["takeLaneRevision"] = json!(hash(&json!(siblings))?);
            return hash(&value);
        }
        let authority = self.clip_authority(snapshot, reference)?;
        if row.arrangement {
            Ok(authority["expectedAuthorityRevision"].as_str().unwrap().into())
        } else {
            hash(&authority)
        }
    }
    pub(super) fn arrangement_collection_revision(&self, snapshot: &LiveSnapshot, track: &str) -> Result<String, LiveError> {
        let s = serde_json::to_value(snapshot).unwrap();
        let clips: Vec<_> =
            rows(&s["arrangement"]["clips"]).filter(|v| v["trackRef"] == track).map(|v| fields(v, &["ref", "objectIdentity"])).collect();
        if clips.iter().any(|v| !is_non_empty_string(&v["ref"], 256) || !is_non_empty_string(&v["objectIdentity"], 256)) {
            return Err(LiveError::error("Arrangement clip collection authority is incomplete"));
        }
        hash(&json!(clips))
    }
    pub(super) fn arrangement_clip_authority(&self, snapshot: &LiveSnapshot, reference: &str) -> Result<Value, LiveError> {
        let row = self.clip_row(snapshot, reference)?;
        let track = row.track.as_ref().unwrap_or(&Value::Null);
        if !row.arrangement
            || !is_non_empty_string(&row.clip["objectIdentity"], 256)
            || !is_non_empty_string(&track["ref"], 256)
            || !is_non_empty_string(&track["objectIdentity"], 256)
        {
            return Err(LiveError::error("Arrangement clip hierarchy authority is incomplete"));
        }
        let s = serde_json::to_value(snapshot).unwrap();
        let siblings: Vec<_> = rows(&s["arrangement"]["clips"])
            .filter(|v| v["trackRef"] == track["ref"])
            .map(|v| authority_fields(v, &["ref", "objectIdentity"]))
            .collect::<Result<_, _>>()?;
        Ok(
            json!({"expectedObjectIdentity":row.clip["objectIdentity"],"expectedAuthorityRevision":hash(&json!({"clip":{"ref":reference,"objectIdentity":row.clip["objectIdentity"]},"owner":fields(track,&["ref","objectIdentity"]),"siblings":siblings}))?}),
        )
    }
    pub(super) fn clip_authority(&self, snapshot: &LiveSnapshot, reference: &str) -> Result<Value, LiveError> {
        let row = self.clip_row(snapshot, reference)?;
        if !is_non_empty_string(&row.clip["objectIdentity"], 256) {
            return Err(LiveError::error("clip lacks exact object identity"));
        }
        if row.arrangement {
            return self.arrangement_clip_authority(snapshot, reference);
        }
        let track = row.track.as_ref().unwrap_or(&Value::Null);
        if !is_non_empty_string(&track["ref"], 256) || !is_non_empty_string(&track["objectIdentity"], 256) || !track["clipSlots"].is_array()
        {
            return Err(LiveError::error("clip track authority is incomplete"));
        }
        let slot = rows(&track["clipSlots"]).find(|v| v["clipRef"] == reference).unwrap_or(&Value::Null);
        let s = serde_json::to_value(snapshot).unwrap();
        let scene = rows(&s["scenes"]).find(|v| v["index"].as_f64() == slot["sceneIndex"].as_f64()).unwrap_or(&Value::Null);
        if !is_non_empty_string(&slot["ref"], 256)
            || !is_non_empty_string(&slot["objectIdentity"], 256)
            || !is_non_empty_string(&scene["ref"], 256)
            || !is_non_empty_string(&scene["objectIdentity"], 256)
        {
            return Err(LiveError::error("clip slot or scene authority is incomplete"));
        }
        Ok(
            json!({"expectedObjectIdentity":row.clip["objectIdentity"],"expectedTrackRef":track["ref"],"expectedTrackIdentity":track["objectIdentity"],"expectedSlotRef":slot["ref"],"expectedSlotIdentity":slot["objectIdentity"],"expectedSceneRef":scene["ref"],"expectedSceneIdentity":scene["objectIdentity"]}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_authority_helpers_match_source_and_clip_bounds() {
        let f: Value = serde_json::from_str(include_str!("../../tests/support/host_probes_oracle.json")).unwrap();
        let host = McpHost::default();
        for (i, c) in f["helpers"].as_array().unwrap().iter().enumerate() {
            let mut snapshot = f["base"].clone();
            for path in c["remove"].as_array().into_iter().flatten() {
                let path = path.as_array().unwrap();
                let mut at = &mut snapshot;
                for k in &path[..path.len() - 1] {
                    at = if let Some(k) = k.as_str() { &mut at[k] } else { &mut at[k.as_u64().unwrap() as usize] };
                }
                at.as_object_mut().unwrap().shift_remove(path.last().unwrap().as_str().unwrap());
            }
            let s: LiveSnapshot = serde_json::from_value(snapshot).unwrap();
            let arg = &c["arg"];
            let r = match c["method"].as_str().unwrap() {
                "clipAuthority" => host.clip_authority(&s, arg.as_str().unwrap()),
                "arrangementClipAuthority" => host.arrangement_clip_authority(&s, arg.as_str().unwrap()),
                "clipAuthorityDigest" => host.clip_authority_digest(&s, arg.as_str().unwrap()).map(Value::String),
                "arrangementCollectionRevision" => host.arrangement_collection_revision(&s, arg.as_str().unwrap()).map(Value::String),
                "boundedClipContentDigest" => host.bounded_clip_content_digest(arg).map(Value::String),
                "warpMarkerCollectionRevision" => host.warp_marker_collection_revision(arg.as_array().unwrap()).map(Value::String),
                _ => unreachable!(),
            };
            match r {
                Ok(v) => assert_eq!(v, c["result"], "helper {i} {c}"),
                Err(e) => assert_eq!(Some(e.message()), c["error"].as_str(), "helper {i} {c}"),
            }
        }
        let mut value = json!(0);
        for _ in 0..8 {
            value = json!({"nested":value});
        }
        assert_eq!(
            host.bounded_clip_content_digest(&json!({"clipView":value})).unwrap_err().message(),
            "clip content is too deeply nested"
        );
        assert_eq!(
            host.bounded_clip_content_digest(&json!({"name":"a".repeat(1_048_577)})).unwrap_err().message(),
            "clip content string is too large"
        );
        assert!(host.bounded_clip_content_digest(&json!({"warpMarkers":vec![json!({"beatTime":0,"sampleTime":0});4096]})).is_ok());
    }
}
