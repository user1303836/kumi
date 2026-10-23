//! Exact read and immediate utility handlers from the host's asynchronous dispatch.
use super::*;
use crate::transactions::session_midi::discover_session;
use kumi_common::abort::Signal;
pub(super) const AUDITION_DEADLINE_MS: f64 = 15_000.0;
pub(super) const NOTE_PAGE: usize = 2000;
fn outcome(id: &Value, result: Result<Value, LiveError>, remediation: &str) -> Value {
    match result {
        Ok(result) => result,
        Err(cause) => adapter_tool_error(id, &cause, remediation),
    }
}
fn set_ref(snapshot: &LiveSnapshot) -> Result<&LiveRef, LiveError> {
    snapshot.set.as_ref().map(|set| &set.ref_).ok_or_else(|| LiveError::type_error("Cannot read properties of undefined (reading 'ref')"))
}
fn property<'a>(value: &'a Value, key: &str) -> Result<&'a Value, LiveError> {
    if value.is_null() {
        return Err(LiveError::type_error(format!("Cannot read properties of null (reading '{key}')")));
    }
    Ok(value.get(key).unwrap_or(&Value::Null))
}
fn session_read(status: &LiveStatus) -> Result<(), LiveError> {
    if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
        Err(LiveError::error("session read capability is unavailable"))
    } else {
        Ok(())
    }
}
fn has_operation(status: &LiveStatus, operation: &str, reason: &str) -> Result<(), LiveError> {
    if status.has_operation(operation) {
        Ok(())
    } else {
        Err(LiveError::error(reason))
    }
}
fn optional_string(params: &Value, key: &str, max: usize) -> bool {
    params.get(key).is_none_or(|v| is_non_empty_string(v, max))
}
impl McpHost {
    /// A recognized family yields its exact result; other names remain for the next dispatcher family.
    pub async fn dispatch_read_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Value, LiveError>> {
        let id = &call.id;
        let params = call.arguments.as_ref();
        let args = params.unwrap_or(&Value::Null);
        Some(if call.asynchronous {
            match call.name.as_str() {
                "live_status" => Ok(self.live_status_async(id).await),
                "live_snapshot" => self.live_snapshot_async(id, params).await,
                "live_discover" => self.live_discover_async(id, args).await,
                "live_note_read" => Ok(self.live_note_read_async(id, args).await),
                "live_key_estimate" => Ok(self.live_key_estimate_async(id, args).await),
                "live_observe_subscribe" => Ok(self.live_observe_subscribe_async(id, args).await),
                "live_observe_poll" => Ok(self.live_observe_poll_async(id, args).await),
                "live_observe_unsubscribe" => Ok(self.live_observe_unsubscribe_async(id, args).await),
                "live_browser_roots" => Ok(self.live_browser_roots_async(id, params).await),
                "live_song_state" => Ok(self.live_song_state_async(id, args).await),
                "live_performance_read" => Ok(self.live_performance_read_async(id, params).await),
                "live_data_read" => Ok(self.live_data_read_async(id, args).await),
                "live_automation_read" => Ok(self.live_automation_read_async(id, args).await),
                "live_device_read" => Ok(self.live_device_read_async(id, args).await),
                "live_clip_time_convert" => Ok(self.live_clip_time_convert_async(id, args).await),
                "live_message" => Ok(self.live_message_async(id, args).await),
                "live_run_python" => Ok(self.live_run_python_async(id, args, signal).await),
                "live_browser_preview" => Ok(self.live_browser_preview_async(id, args).await),
                "live_browser_preview_stop" => Ok(self.live_browser_preview_stop_async(id, args).await),
                _ => return None,
            }
        } else {
            match call.name.as_str() {
                "live_snapshot" => Ok(self.live_snapshot(id)),
                "live_discover" => Ok(self.live_discover(id, args)),
                _ => return None,
            }
        })
    }
    pub fn live_snapshot(&self, id: &Value) -> Value {
        outcome(
            id,
            (|| {
                let status = self.require_connected(Some("session.read"))?;
                Ok(success_text(id, &json!({"epoch":status.epoch,"snapshot":self.adapter.snapshot()?})))
            })(),
            "Snapshot unavailable. Verify the Live adapter connection and retry.",
        )
    }
    pub async fn live_snapshot_async(&self, id: &Value, params: Option<&Value>) -> Result<Value, LiveError> {
        if !utility_params(params) {
            return Ok(error(id, -32602, "Invalid live_snapshot parameters", None));
        }
        let status = self.require_connected(Some("session.read"))?;
        Ok(success_text(id, &json!({"epoch":status.epoch,"snapshot":self.views.whole_set(None,None).await?})))
    }
    pub fn live_discover(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["kind", "limit", "cursor"])
            || !params["kind"].as_str().is_some_and(|k| ["track", "scene", "clip", "note"].contains(&k))
            || params.get("limit").is_some_and(|v| !is_integer_in_range(v, 1.0, 100.0))
            || !optional_string(params, "cursor", 256)
        {
            return error(id, -32602, "kind, limit, and cursor are invalid", None);
        }
        outcome(
            id,
            discover_session(
                &*self.adapter,
                params["kind"].as_str().unwrap(),
                params["limit"].as_f64().unwrap_or(50.0),
                params["cursor"].as_str(),
            )
            .map(|result| success_text(id, &result)),
            "Discovery is unavailable; verify the Live adapter and request a fresh page.",
        )
    }
    pub async fn live_discover_async(&self, id: &Value, params: &Value) -> Result<Value, LiveError> {
        let kinds = [
            "set",
            "track",
            "return-track",
            "main-track",
            "scene",
            "clip-slot",
            "session-clip",
            "arrangement-clip",
            "note",
            "locator",
            "device",
            "parameter",
            "selection",
            "routing-choice",
            "session-playback",
        ];
        let kind = params["kind"].as_str().unwrap_or("");
        if !has_only(params, &["kind", "parent", "filter", "fields", "budget", "limit", "cursor"])
            || !kinds.contains(&kind)
            || (["clip-slot", "session-clip", "arrangement-clip", "note", "parameter", "routing-choice"].contains(&kind)
                && !is_non_empty_string(&params["parent"], 256))
            || !optional_string(params, "parent", 256)
            || params.get("filter").is_some_and(|v| !is_discovery_filter(v))
            || params
                .get("fields")
                .is_some_and(|v| !v.as_array().is_some_and(|v| v.len() <= 256 && v.iter().all(|v| is_non_empty_string(v, 64))))
            || params.get("budget").is_some_and(|v| !is_integer_in_range(v, 1.0, 10_000_000.0))
            || params.get("limit").is_some_and(|v| !is_integer_in_range(v, 1.0, 100_000.0))
            || !optional_string(params, "cursor", 1024)
        {
            return Ok(error(id, -32602, "kind, parent, filter, fields, budget, limit, and cursor are invalid", None));
        }
        let limit = params["limit"].as_f64().unwrap_or(50.0) as usize;
        let request = LiveDiscoveryRequest {
            kind: serde_json::from_value(json!(kind)).unwrap(),
            parent: params["parent"].as_str().map(str::to_owned),
            filter: params["filter"].as_object().cloned(),
            fields: params["fields"].as_array().map(|v| v.iter().map(|v| v.as_str().unwrap().into()).collect()),
            budget: Some(params["budget"].as_f64().unwrap_or(1000.0) as usize),
            limit: Some(if kind == "note" { limit.min(NOTE_PAGE) } else { limit }),
            cursor: params["cursor"].as_str().map(str::to_owned),
        };
        Ok(success_text(id, &self.adapter.discover_async(&request, None).await?.into_value()))
    }
    pub async fn live_note_read_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["clipRef", "noteIds", "selected"]) || !is_non_empty_string(&params["clipRef"], 256) {
            return error(id, -32602, "clipRef is required", None);
        }
        if params.get("noteIds").is_some() && params["selected"] == true {
            return error(id, -32602, "noteIds and selected are mutually exclusive", None);
        }
        outcome(
            id,
            async {
                let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
                session_read(&status)?;
                let context = LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS));
                if params["selected"] == true {
                    has_operation(&status, "note.read-selected", "selected note reads are unavailable")?;
                    return Ok(success_text(
                        id,
                        &self
                            .async_adapter()
                            .invoke_async(&LiveInvocation::new("note.read-selected", json!({"ref":params["clipRef"]})), Some(&context))
                            .await?,
                    ));
                }
                if !params["noteIds"]
                    .as_array()
                    .is_some_and(|v| !v.is_empty() && v.len() <= 10_000_000 && v.iter().all(|v| is_integer_in_range(v, 0.0, f64::MAX)))
                {
                    return Ok(error(id, -32602, "noteIds must be one or more non-negative integers (or selected=true)", None));
                }
                has_operation(&status, "note.read-by-id", "targeted note reads are unavailable")?;
                Ok(success_text(
                    id,
                    &self
                        .async_adapter()
                        .invoke_async(
                            &LiveInvocation::new("note.read-by-id", json!({"ref":params["clipRef"],"noteIds":params["noteIds"]})),
                            Some(&context),
                        )
                        .await?,
                ))
            }
            .await,
            "Note read requires fresh authoritative state.",
        )
    }
    pub async fn live_song_state_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["conversion", "smpteFormat"]) {
            return error(id, -32602, "only conversion and smpteFormat are accepted", None);
        }
        if params.get("conversion").is_some_and(|v| !v.as_str().is_some_and(|v| ["beats-loop", "current-smpte"].contains(&v))) {
            return error(id, -32602, "conversion is invalid", None);
        }
        if params
            .get("smpteFormat")
            .is_some_and(|v| !v.as_str().is_some_and(|v| ["smpte-24", "smpte-25", "smpte-29", "smpte-30", "smpte-30-drop"].contains(&v)))
        {
            return error(id, -32602, "smpteFormat is invalid", None);
        }
        outcome(
            id,
            async {
                let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
                session_read(&status)?;
                has_operation(&status, "song.read", "song state reads are unavailable")?;
                let adapter = self.async_adapter();
                let snapshot = self.views.view(None, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Set])).await?;
                let context = LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS));
                let mut state =
                    adapter.invoke_async(&LiveInvocation::new("song.read", json!({"setRef":set_ref(&snapshot)?})), Some(&context)).await?;
                if params.get("conversion").is_some() && status.has_operation("song.time-convert") {
                    let mut args = json!({"setRef":set_ref(&snapshot)?,"query":params["conversion"]});
                    if let Some(format) = params.get("smpteFormat") {
                        args["smpteFormat"] = format.clone();
                    }
                    let conversions = adapter.invoke_async(&LiveInvocation::new("song.time-convert", args), Some(&context)).await?;
                    if let Some(target) = state.as_object_mut() {
                        target.insert("conversions".into(), conversions);
                    } else if !state.is_array() {
                        return Err(LiveError::type_error(if state.is_null() {
                            "Cannot set properties of null (setting 'conversions')".into()
                        } else {
                            format!(
                                "Cannot create property 'conversions' on {} '{}'",
                                if state.is_boolean() {
                                    "boolean"
                                } else if state.is_number() {
                                    "number"
                                } else {
                                    "string"
                                },
                                js_string(&state)?
                            )
                        }));
                    }
                }
                Ok(success_text(id, &state))
            }
            .await,
            "Song state requires a fresh connection.",
        )
    }
    pub async fn live_performance_read_async(&self, id: &Value, params: Option<&Value>) -> Value {
        if !utility_params(params) {
            return error(id, -32602, "no arguments are accepted", None);
        }
        outcome(
            id,
            async {
                let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
                session_read(&status)?;
                has_operation(&status, "performance.read", "performance reads are unavailable")?;
                let snapshot = self.views.view(None, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Set])).await?;
                Ok(success_text(
                    id,
                    &self
                        .async_adapter()
                        .invoke_async(
                            &LiveInvocation::new("performance.read", json!({"setRef":set_ref(&snapshot)?})),
                            Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS))),
                        )
                        .await?,
                ))
            }
            .await,
            "Performance read requires a fresh connection.",
        )
    }
    pub async fn live_data_read_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["key", "trackRef"]) || !is_non_empty_string(&params["key"], 256) || !optional_string(params, "trackRef", 256)
        {
            return error(id, -32602, "key is required (1-256 characters); trackRef reads a track's own", None);
        }
        outcome(
            id,
            async {
                let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
                has_operation(&status, "data.get", "data saved in the Set is unavailable on this Live shape")?;
                let owner = if let Some(owner) = params["trackRef"].as_str() {
                    owner.to_owned()
                } else {
                    set_ref(&self.views.view(None, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Set])).await?)?.to_string()
                };
                let result = self
                    .async_adapter()
                    .invoke_async(
                        &LiveInvocation::new("data.get", json!({"ref":owner,"key":params["key"]})),
                        Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS))),
                    )
                    .await?;
                let value = property(&result, "value")?;
                Ok(success_text(id, &json!({"ref":owner,"key":params["key"],"value":if value.is_string() {value}else{&Value::Null}})))
            }
            .await,
            "Read it again from a fresh track reference.",
        )
    }
    pub async fn live_automation_read_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["clipRef", "parameterRef", "time"])
            || !is_non_empty_string(&params["clipRef"], 256)
            || !is_non_empty_string(&params["parameterRef"], 256)
        {
            return error(id, -32602, "clipRef and parameterRef are required", None);
        }
        if params.get("time").is_some_and(|v| !v.as_f64().is_some_and(|v| v.is_finite() && (0.0..=1e9).contains(&v))) {
            return error(id, -32602, "time is a beat in the clip (0 or more)", None);
        }
        outcome(id,async {
            let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            has_operation(&status,"automation.envelope.read","reading envelopes is unavailable on this Live shape")?;
            if params.get("time").is_some() {has_operation(&status,"automation.value-at","reading an envelope's value is unavailable on this Live shape")?;}
            let adapter=self.async_adapter();
            let context=LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS));
            let envelope=adapter.invoke_async(&LiveInvocation::new("automation.envelope.read",json!({"clipRef":params["clipRef"],"parameterRef":params["parameterRef"]})),Some(&context)).await?;
            let at=if params.get("time").is_some() {Some(adapter.invoke_async(&LiveInvocation::new("automation.value-at",params.clone()),Some(&context)).await?)}else{None};
            let exists=property(&envelope,"exists")?==true;
            let points=property(&envelope,"points")?;
            let mut result=json!({"clipRef":params["clipRef"],"parameterRef":params["parameterRef"],"exists":exists,"points":if points.is_array(){points.clone()}else{json!([])},"revision":property(&envelope,"revision")?});
            if let Some(at)=at.filter(|v|!v.is_null() && v!=false && v.as_f64()!=Some(0.0) && v!="") {
                let value=property(&at,"value")?;
                result["time"]=params["time"].clone();result["value"]=if value.is_number(){value.clone()}else{Value::Null};
            }
            Ok(success_text(id,&result))
        }.await,"Discover the clip and parameter again and read from fresh references.")
    }
    pub async fn live_device_read_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["deviceRef", "what", "begin", "end"])
            || !is_non_empty_string(&params["deviceRef"], 256)
            || !params["what"].as_str().is_some_and(|v| ["parameter-names", "banks"].contains(&v))
        {
            return error(id, -32602, "deviceRef and what (parameter-names or banks) are required", None);
        }
        if params["what"] == "banks" && (params.get("begin").is_some() || params.get("end").is_some()) {
            return error(id, -32602, "begin and end go with parameter-names", None);
        }
        if params.get("begin").is_some_and(|v| !is_integer_in_range(v, 0.0, 10_000_000.0))
            || params.get("end").is_some_and(|v| !is_integer_in_range(v, -1.0, 10_000_000.0))
            || params["end"].as_f64().is_some_and(|end| end != -1.0 && end < params["begin"].as_f64().unwrap_or(0.0))
        {
            return error(id, -32602, "begin is 0 or more; end is -1 (to the last) or at least begin", None);
        }
        outcome(
            id,
            async {
                let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
                let operation = if params["what"] == "banks" { "device.banks.read" } else { "plugin.parameter-names" };
                has_operation(&status, operation, &format!("{operation} is unavailable on this Live shape"))?;
                let mut args = json!({"ref":params["deviceRef"]});
                for key in ["begin", "end"] {
                    if let Some(value) = params.get(key) {
                        args[key] = value.clone();
                    }
                }
                let result = self
                    .async_adapter()
                    .invoke_async(
                        &LiveInvocation::new(operation, args),
                        Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS))),
                    )
                    .await?;
                let mut body = json!({"deviceRef":params["deviceRef"]});
                if let Some(fields) = result.as_object() {
                    body.as_object_mut().unwrap().extend(fields.clone());
                } else if let Some(values) = result.as_array() {
                    for (i, v) in values.iter().enumerate() {
                        body[i.to_string()] = v.clone();
                    }
                } else if let Some(text) = result.as_str() {
                    for (i, c) in text.chars().enumerate() {
                        body[i.to_string()] = json!(c.to_string());
                    }
                }
                Ok(success_text(id, &body))
            }
            .await,
            "Discover the device again and read from a fresh reference.",
        )
    }
    pub async fn live_clip_time_convert_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["clipRef", "from", "value"])
            || !is_non_empty_string(&params["clipRef"], 256)
            || !params["from"].as_str().is_some_and(|v| ["beats", "samples", "seconds"].contains(&v))
            || !params["value"].as_f64().is_some_and(|v| v.is_finite() && (-1e9..=1e12).contains(&v))
        {
            return error(id, -32602, "clipRef, from (beats, samples or seconds) and value are required", None);
        }
        outcome(
            id,
            async {
                let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
                has_operation(&status, "clip.time-convert", "converting a clip's time is unavailable on this Live shape")?;
                let result = self
                    .async_adapter()
                    .invoke_async(
                        &LiveInvocation::new(
                            "clip.time-convert",
                            json!({"ref":params["clipRef"],"from":params["from"],"value":params["value"]}),
                        ),
                        Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS))),
                    )
                    .await?;
                let mut body = json!({"clipRef":params["clipRef"]});
                for key in ["beats", "samples", "seconds"] {
                    let value = property(&result, key)?;
                    body[key] = if value.as_f64().is_some_and(f64::is_finite) { value.clone() } else { Value::Null };
                }
                Ok(success_text(id, &body))
            }
            .await,
            "Discover the clip again and convert from a fresh reference.",
        )
    }
    pub async fn live_message_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["text", "modal"])
            || !is_non_empty_string(&params["text"], 1024)
            || params.get("modal").is_some_and(|v| !v.is_boolean())
        {
            return error(id, -32602, "text (1-1024 characters) is required; modal is true or false", None);
        }
        outcome(
            id,
            async {
                let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
                has_operation(&status, "application.message", "messages in Live are unavailable on this Live shape")?;
                let result = self
                    .async_adapter()
                    .invoke_async(
                        &LiveInvocation::new("application.message", params.clone()),
                        Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS))),
                    )
                    .await?;
                if property(&result, "shown")? != true {
                    return Err(LiveError::error("Live didn't show the message"));
                }
                Ok(success_text(id, &json!({"shown":true,"modal":params["modal"]==true})))
            }
            .await,
            "The message wasn't shown.",
        )
    }
    pub async fn live_run_python_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        if !has_only(params, &["code", "mode", "ref", "timeoutMs"])
            || !is_non_empty_string(&params["code"], 65_536)
            || params.get("mode").is_some_and(|v| v != "eval" && v != "exec")
            || !optional_string(params, "ref", 256)
            || params.get("timeoutMs").is_some_and(|v| !is_integer_in_range(v, 1.0, 30_000.0))
        {
            return error(id, -32602, "code is required; mode is eval or exec, ref is optional, timeoutMs is 1–30000", None);
        }
        outcome(
            id,
            async {
                let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
                has_operation(&status, "python.run", "Python execution is unavailable on this Live shape")?;
                let timeout = params["timeoutMs"].as_f64().unwrap_or(5000.0);
                let mut args = json!({"code":params["code"],"mode":params.get("mode").unwrap_or(&json!("exec")),"timeoutMs":timeout});
                if let Some(reference) = params.get("ref") {
                    args["ref"] = reference.clone();
                }
                let context = LiveOperationContext {
                    signal: signal.cloned(),
                    deadline_ms: Some(self.deadline(AUDITION_DEADLINE_MS.max(timeout + 5000.0))),
                    ..Default::default()
                };
                Ok(success_text(id, &self.async_adapter().invoke_async(&LiveInvocation::new("python.run", args), Some(&context)).await?))
            }
            .await,
            "Read the Set again before continuing; a dispatched script may have changed it.",
        )
    }
    pub async fn live_browser_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["itemId"]) || !is_non_empty_string(&params["itemId"], 256) {
            return error(id, -32602, "itemId is required", None);
        }
        outcome(
            id,
            async {
                let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
                if !status.has_operation("browser.inspect") || !status.has_operation("browser.preview.start") {
                    return Err(LiveError::error("browser previews are unavailable on this Live shape"));
                }
                let adapter = self.async_adapter();
                let context = LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS));
                let item = adapter
                    .invoke_async(&LiveInvocation::new("browser.inspect", json!({"itemId":params["itemId"]})), Some(&context))
                    .await?;
                if property(&item, "id")? != &params["itemId"]
                    || !is_non_empty_string(property(&item, "objectIdentity")?, 256)
                    || !is_non_empty_string(property(&item, "name")?, 256)
                {
                    return Err(LiveError::error("browser item lacks exact authoritative identity"));
                }
                let started = adapter
                    .invoke_async(
                        &LiveInvocation::new(
                            "browser.preview.start",
                            json!({"itemId":params["itemId"],"expectedName":item["name"],"expectedItemIdentity":item["objectIdentity"]}),
                        ),
                        Some(&context),
                    )
                    .await?;
                if !is_non_empty_string(property(&started, "previewId")?, 256) {
                    return Err(LiveError::error("Live didn't say which preview plays"));
                }
                Ok(success_text(id, &json!({"previewId":started["previewId"],"item":{"id":item["id"],"name":item["name"]}})))
            }
            .await,
            "Nothing plays; search the browser again and preview from a fresh item id.",
        )
    }
    pub async fn live_browser_preview_stop_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["previewId"])
            || !params["previewId"].as_str().is_some_and(|v| (32..=256).contains(&kumi_common::js::string::utf16_len(v)))
        {
            return error(id, -32602, "previewId (from live_browser_preview) is required", None);
        }
        outcome(
            id,
            async {
                let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
                has_operation(&status, "browser.preview.stop", "browser previews are unavailable on this Live shape")?;
                let result = self
                    .async_adapter()
                    .invoke_async(
                        &LiveInvocation::new("browser.preview.stop", params.clone()),
                        Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS))),
                    )
                    .await?;
                if property(&result, "stopped")? != true {
                    return Err(LiveError::error("Live didn't say the preview stopped"));
                }
                Ok(success_text(id, &json!({"stopped":true})))
            }
            .await,
            "A later preview may be playing: stop it by its own previewId.",
        )
    }
    pub async fn live_observe_subscribe_async(&self, id: &Value, params: &Value) -> Value {
        let kinds = ["transport", "selection", "track", "clip", "device", "parameter", "groove", "tuning", "scene", "meters", "rack"];
        if !has_only(params, &["topics", "minIntervalMs"]) {
            return error(id, -32602, "topics is required", None);
        }
        let Some(topics) = params["topics"].as_array().filter(|v| (1..=64).contains(&v.len())) else {
            return error(id, -32602, "topics must be 1-64 entries", None);
        };
        for topic in topics {
            if !has_only(topic, &["kind", "ref"]) || !topic["kind"].as_str().is_some_and(|k| kinds.contains(&k)) {
                return error(id, -32602, "observe topic is invalid", None);
            }
            if !optional_string(topic, "ref", 256) {
                return error(id, -32602, "observe topic ref is invalid", None);
            }
        }
        if params.get("minIntervalMs").is_some_and(|v| !is_integer_in_range(v, 100.0, 60_000.0)) {
            return error(id, -32602, "minIntervalMs is invalid", None);
        }
        outcome(
            id,
            async {
                let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
                session_read(&status)?;
                has_operation(&status, "observe.subscribe", "observer subscriptions are unavailable")?;
                Ok(success_text(
                    id,
                    &self
                        .async_adapter()
                        .invoke_async(
                            &LiveInvocation::new("observe.subscribe", params.clone()),
                            Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS))),
                        )
                        .await?,
                ))
            }
            .await,
            "Observer subscription requires a fresh connection.",
        )
    }
    pub async fn live_observe_poll_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["subscriptionId"]) || !is_non_empty_string(&params["subscriptionId"], 128) {
            return error(id, -32602, "subscriptionId is required", None);
        }
        outcome(
            id,
            async {
                let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
                session_read(&status)?;
                has_operation(&status, "observe.poll", "observer polling is unavailable")?;
                Ok(success_text(
                    id,
                    &self
                        .async_adapter()
                        .invoke_async(
                            &LiveInvocation::new("observe.poll", params.clone()),
                            Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS))),
                        )
                        .await?,
                ))
            }
            .await,
            "Observer poll requires a fresh connection.",
        )
    }
    pub async fn live_observe_unsubscribe_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["subscriptionId"]) || !is_non_empty_string(&params["subscriptionId"], 128) {
            return error(id, -32602, "subscriptionId is required", None);
        }
        outcome(
            id,
            async {
                let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
                session_read(&status)?;
                has_operation(&status, "observe.unsubscribe", "observer unsubscribe is unavailable")?;
                Ok(success_text(
                    id,
                    &self
                        .async_adapter()
                        .invoke_async(
                            &LiveInvocation::new("observe.unsubscribe", params.clone()),
                            Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS))),
                        )
                        .await?,
                ))
            }
            .await,
            "Observer unsubscribe requires a fresh connection.",
        )
    }
    pub async fn live_browser_roots_async(&self, id: &Value, params: Option<&Value>) -> Value {
        if !utility_params(params) {
            return error(id, -32602, "no arguments are accepted", None);
        }
        outcome(
            id,
            async {
                let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
                session_read(&status)?;
                has_operation(&status, "browser.roots", "browser roots are unavailable")?;
                Ok(success_text(
                    id,
                    &self
                        .async_adapter()
                        .invoke_async(
                            &LiveInvocation::new("browser.roots", json!({})),
                            Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS))),
                        )
                        .await?,
                ))
            }
            .await,
            "Browser roots requires a fresh connection.",
        )
    }
    pub(super) fn clip_row(&self, snapshot: &LiveSnapshot, reference: &str) -> Result<ClipRow, LiveError> {
        let snapshot = serde_json::to_value(snapshot).unwrap();
        let tracks = snapshot["tracks"].as_array().ok_or_else(|| LiveError::type_error("snapshot.tracks is not iterable"))?;
        for track in tracks {
            let clips =
                track["clips"].as_array().ok_or_else(|| LiveError::type_error("Cannot read properties of undefined (reading 'filter')"))?;
            if let Some(clip) = clips.iter().find(|clip| clip.is_object() && clip["ref"] == reference) {
                return Ok(ClipRow { track: Some(track.clone()), clip: clip.clone(), arrangement: false, take_lane: None });
            }
            for lane in track["takeLanes"].as_array().into_iter().flatten().filter(|v| v.is_object()) {
                if let Some(clip) = lane["clips"].as_array().into_iter().flatten().find(|v| v.is_object() && v["ref"] == reference) {
                    return Ok(ClipRow {
                        track: Some(track.clone()),
                        clip: clip.clone(),
                        arrangement: true,
                        take_lane: Some(lane.clone()),
                    });
                }
            }
        }
        let arrangement =
            snapshot.get("arrangement").ok_or_else(|| LiveError::type_error("Cannot read properties of undefined (reading 'clips')"))?;
        let clips = property(arrangement, "clips")?;
        if let Some(clip) = clips.as_array().into_iter().flatten().find(|v| v.is_object() && v["ref"] == reference) {
            return Ok(ClipRow {
                track: tracks.iter().find(|t| t["ref"] == clip["trackRef"]).cloned(),
                clip: clip.clone(),
                arrangement: true,
                take_lane: None,
            });
        }
        Err(LiveError::error("clip reference is not authoritative"))
    }
    pub(super) async fn clip_notes_async(&self, reference: &str, context: Option<&LiveOperationContext>) -> Result<Vec<Value>, LiveError> {
        let default = LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS));
        let request = LiveDiscoveryRequest {
            parent: Some(reference.into()),
            limit: Some(NOTE_PAGE),
            ..LiveDiscoveryRequest::of(LiveDiscoveryKind::Note)
        };
        let items = self.views.discover_all(&request, context.or(Some(&default)), None).await?;
        Ok(items
            .into_iter()
            .map(|mut row| {
                row.shift_remove("ref");
                row.shift_remove("parentRef");
                Value::Object(row)
            })
            .collect())
    }
    pub async fn live_key_estimate_async(&self, id: &Value, params: &Value) -> Value {
        use crate::key_estimation::{estimate_key, KeyEstimateNote};
        let typed = |note: &Value| KeyEstimateNote {
            pitch: note["pitch"].as_f64().unwrap_or(f64::NAN),
            start: note["start"].as_f64().unwrap_or(f64::NAN),
            duration: note["duration"].as_f64().unwrap_or(f64::NAN),
            velocity: note["velocity"].as_f64(),
        };
        if !has_only(params, &["clipRef", "notes", "expectedNotesRevision"]) {
            return error(id, -32602, "key estimate arguments are invalid", None);
        }
        if let Some(notes) = params.get("notes") {
            if params.get("clipRef").is_some() || params.get("expectedNotesRevision").is_some() {
                return error(id, -32602, "notes is mutually exclusive with clipRef and expectedNotesRevision", None);
            }
            let Some(notes) = notes.as_array().filter(|n| n.len() <= 10_000_000) else {
                return error(id, -32602, "notes must be an array of note objects", None);
            };
            let mut parsed = Vec::new();
            for note in notes {
                if !has_only(note, &["pitch", "start", "duration", "velocity"]) {
                    return error(id, -32602, "note objects may only carry pitch, start, duration, and velocity", None);
                }
                if !is_integer_in_range(&note["pitch"], 0.0, 127.0) {
                    return error(id, -32602, "note pitch must be an integer in 0..127", None);
                }
                if !note["start"].as_f64().is_some_and(|v| v.is_finite() && (0.0..=1_000_000.0).contains(&v)) {
                    return error(id, -32602, "note start must be finite in 0..1000000 beats", None);
                }
                if !note["duration"].as_f64().is_some_and(|v| v.is_finite() && v > 0.0 && v <= 1_000_000.0) {
                    return error(id, -32602, "note duration must be finite in 0..1000000 beats, exclusive of zero", None);
                }
                if note.get("velocity").is_some_and(|v| !is_integer_in_range(v, 0.0, 127.0)) {
                    return error(id, -32602, "note velocity must be an integer in 0..127", None);
                }
                parsed.push(typed(note));
            }
            return success_text(id, &serde_json::to_value(estimate_key(&parsed)).unwrap());
        }
        if !is_non_empty_string(&params["clipRef"], 256) {
            return error(id, -32602, "clipRef or notes are required", None);
        }
        if params.get("expectedNotesRevision").is_some_and(|v| {
            !v.as_str().is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        }) {
            return error(id, -32602, "expectedNotesRevision must be a 64-character hex digest", None);
        }
        outcome(
            id,
            async {
                use sha2::{Digest, Sha256};
                let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
                session_read(&status)?;
                let snapshot = self.views.view_for(None, &[params["clipRef"].clone()], None, &[]).await?;
                let row = self.clip_row(&snapshot, params["clipRef"].as_str().unwrap())?;
                if row.clip["isAudio"] == true || row.clip["kind"] == "audio" {
                    return Ok(transaction_error(id, "key estimation requires a MIDI clip"));
                }
                let listed = if let Some(notes) = row.clip["notes"].as_array() {
                    notes.clone()
                } else {
                    self.clip_notes_async(params["clipRef"].as_str().unwrap(), None).await?
                };
                let notes: Vec<_> = listed
                    .iter()
                    .filter(|n| {
                        n["pitch"].as_f64().is_some_and(|n| n.is_finite() && n.fract() == 0.0)
                            && n["duration"].as_f64().is_some_and(|n| n > 0.0)
                    })
                    .cloned()
                    .collect();
                let revision = if let Some(revision) = row.clip["notesRevision"].as_str() {
                    revision.to_owned()
                } else {
                    hex::encode(Sha256::digest(
                        canonical_mutation_identity(&Value::Array(if row.clip["notes"].is_array() { notes.clone() } else { listed }))?
                            .as_bytes(),
                    ))
                };
                if params.get("expectedNotesRevision").is_some_and(|v| v != &revision) {
                    return Ok(transaction_error(id, "clip notes changed since the fenced revision"));
                }
                let mut estimate = serde_json::to_value(estimate_key(&notes.iter().map(typed).collect::<Vec<_>>())).unwrap();
                estimate["evidence"]["clipRef"] = params["clipRef"].clone();
                estimate["evidence"]["notesRevision"] = json!(revision);
                Ok(success_text(id, &estimate))
            }
            .await,
            "Key estimation requires fresh authoritative state.",
        )
    }
}
pub(super) struct ClipRow {
    pub track: Option<Value>,
    pub clip: Value,
    pub arrangement: bool,
    pub take_lane: Option<Value>,
}
