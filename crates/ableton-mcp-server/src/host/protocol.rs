//! Lifecycle and request bookkeeping precede every tool family. A pending tool retains its wire lease.
use super::*;
use crate::mcp_protocol::{format_mcp_response, prepare_mcp_request, LEGACY_PROTOCOL_VERSION, SUPPORTED_PROTOCOL_VERSIONS};
use kumi_common::js::json as js_json;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: Value,
    pub name: String,
    pub arguments: Option<Value>,
    pub asynchronous: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub enum RequestDecision {
    Complete(Option<Value>),
    Tool(ToolCall),
}
struct RequestLease {
    ids: Rc<RefCell<HashSet<String>>>,
    key: String,
}
impl Drop for RequestLease {
    fn drop(&mut self) {
        self.ids.borrow_mut().remove(&self.key);
    }
}
pub struct HostRequest {
    pub input: Value,
    pub modern: bool,
    pub decision: RequestDecision,
    format: bool,
    version: String,
    _lease: Option<RequestLease>,
}
impl HostRequest {
    /// Format the completed family result; dropping this request releases its modern in-flight ID.
    pub fn finish(self, frame: Option<Value>) -> Option<Value> {
        if self.format {
            format_mcp_response(frame, &self.input, self.modern, &json!({"name":"ableton-mcp-host","version":self.version}))
        } else {
            frame
        }
    }
    pub fn completed(self) -> Option<Value> {
        let RequestDecision::Complete(frame) = &self.decision else { panic!("tool request needs its family result") };
        let frame = frame.clone();
        self.finish(frame)
    }
}
fn key(id: &Value) -> String {
    format!(
        "{}:{}",
        if id.is_string() { "string" } else { "number" },
        if let Value::String(v) = id { v.clone() } else { js_json::stringify(id) }
    )
}
impl McpHost {
    /// Validate a request and resolve lifecycle/local operations, leaving exact tool dispatch to its family.
    /// This boundary is also used directly by protocol conformance tests.
    pub fn begin_request(&self, input: &Value, asynchronous: bool) -> Result<HostRequest, LiveError> {
        let wire = prepare_mcp_request(input, self.protocol_era.get());
        let mut request = HostRequest {
            input: wire.input,
            modern: wire.modern,
            decision: RequestDecision::Complete(None),
            format: false,
            version: self.server_version().to_owned(),
            _lease: None,
        };
        if let Some(error) = wire.error {
            request.decision = RequestDecision::Complete(Some(error));
            return Ok(request);
        }
        if wire.modern && input.is_object() && input.get("id").is_some_and(is_id) {
            let key = key(&input["id"]);
            if self.modern_in_flight_ids.borrow().contains(&key) {
                request.decision =
                    RequestDecision::Complete(Some(error(&input["id"], -32600, "Duplicate in-flight request identifier", None)));
                return Ok(request);
            }
            self.modern_in_flight_ids.borrow_mut().insert(key.clone());
            request._lease = Some(RequestLease { ids: self.modern_in_flight_ids.clone(), key });
            if input["method"] != "server/discover" {
                self.protocol_era.set(Some(ProtocolEra::Modern));
            }
        }
        request.format = true;
        request.decision = self.decide_request(&request.input, asynchronous, wire.modern)?;
        Ok(request)
    }
    fn remember_id(&self, id: &Value, modern: bool) -> Option<Value> {
        if modern {
            return None;
        }
        let key = key(id);
        let mut ids = self.seen_ids.borrow_mut();
        if !ids.insert(key.clone()) {
            return Some(error(id, -32600, "Duplicate request identifier", None));
        }
        let mut order = self.id_order.borrow_mut();
        order.push_back(key);
        if order.len() > MAX_TRACKED_REQUEST_IDS {
            if let Some(expired) = order.pop_front() {
                ids.remove(&expired);
            }
        }
        None
    }
    fn decide_request(&self, input: &Value, asynchronous: bool, modern: bool) -> Result<RequestDecision, LiveError> {
        let complete = |frame| Ok(RequestDecision::Complete(frame));
        let name = input.get("params").and_then(|p| p.get("name")).and_then(Value::as_str);
        // Async recognized tools have their own source validator (including live_status during initialization).
        if asynchronous
            && input.get("id").is_some()
            && input["method"] == "tools/call"
            && input["params"].is_object()
            && name.is_some_and(|name| table_has("asyncHandledTools", name) || table_has("asyncOnlyTools", name))
        {
            let id = request_id(input.get("id"));
            if id.is_null() || input["jsonrpc"] != "2.0" || !has_only(input, &["jsonrpc", "id", "method", "params", "_meta"]) {
                return complete(Some(error(&Value::Null, -32600, "Invalid Request", None)));
            }
            if let Some(error) = self.remember_id(&id, modern) {
                return complete(Some(error));
            }
            if self.shutting_down.get() {
                return complete(Some(error(&id, -32600, "Server is shutting down", None)));
            }
            if !modern && !self.initialized.get() {
                return complete(Some(error(&id, -32002, "Server has not been initialized", None)));
            }
            let name = name.unwrap();
            if !modern && !self.initialized_notification.get() && name != "live_status" {
                return complete(Some(error(&id, -32002, "Server has not received initialized notification", None)));
            }
            self.note_tool_list_changed()?;
            if !self.tool_callable(name)? {
                return complete(Some(self.tool_gate_error(&id, name)?));
            }
            return Ok(RequestDecision::Tool(ToolCall {
                id,
                name: name.into(),
                arguments: input["params"].get("arguments").cloned(),
                asynchronous: true,
            }));
        }
        if !input.is_object() || input["jsonrpc"] != "2.0" || !has_only(input, &["jsonrpc", "id", "method", "params", "_meta"]) {
            return complete(Some(error(&Value::Null, -32600, "Invalid Request", None)));
        }
        let Some(method) = input["method"].as_str() else {
            return complete(Some(error(&request_id(input.get("id")), -32600, "Invalid Request", None)));
        };
        let params = input.get("params");
        let Some(id) = input.get("id") else {
            if method == "notifications/initialized" && self.initialized.get() && !self.initialized_notification.get() && params.is_none() {
                self.initialized_notification.set(true);
            }
            if method == "exit" {
                self.shutting_down.set(true);
            }
            return complete(None);
        };
        if !is_id(id) {
            return complete(Some(error(&Value::Null, -32600, "Invalid Request", None)));
        }
        if let Some(error) = self.remember_id(id, modern) {
            return complete(Some(error));
        }
        if self.shutting_down.get() && method != "exit" {
            return complete(Some(error(id, -32600, "Server is shutting down", None)));
        }
        if method == "notifications/initialized" {
            if !self.initialized_notification.get() && self.initialized.get() && params.is_none() {
                self.initialized_notification.set(true);
            }
            return complete(None);
        }
        if method == "notifications/cancelled" {
            return complete(None);
        }
        if method == "exit" {
            self.shutting_down.set(true);
            return complete(Some(response(id, json!({}))));
        }
        if !modern && !self.initialized.get() && method != "initialize" {
            return complete(Some(error(id, -32002, "Server has not been initialized", None)));
        }
        if !modern && !self.initialized_notification.get() && method != "initialize" && method != "ping" {
            return complete(Some(error(id, -32002, "Server has not received initialized notification", None)));
        }
        let frame = match method {
            "server/discover" => {
                if modern && utility_params(params) {
                    response(
                        id,
                        json!({
                            "supportedVersions":SUPPORTED_PROTOCOL_VERSIONS,"capabilities":{"tools":{},"resources":{},"prompts":{}},
                            "instructions":"Preview and explicitly confirm edits; preserve transaction IDs and exact idempotency keys for recovery. RPC retries are not permission to repeat writes. Application handles are process-local and expire. Modern stdio has no push subscriptions; use snapshot or observe/poll. Discovery permits a later legacy initialize; otherwise do not mix eras in one process."
                        }),
                    )
                } else {
                    error(id, -32602, "Invalid server/discover parameters", None)
                }
            }
            "initialize" => self.initialize(id, params),
            "ping" => {
                if utility_params(params) {
                    response(id, json!({}))
                } else {
                    error(id, -32602, "Invalid ping parameters", None)
                }
            }
            "tools/list" => {
                if utility_params(params) {
                    self.note_tool_list_changed()?;
                    response(
                        id,
                        json!({"tools":tool_catalog::visible_tool_descriptors(&self.safe_adapter_status(),&self.tool_policy.borrow()).map_err(policy_error)?}),
                    )
                } else {
                    error(id, -32602, "Invalid tools/list parameters", None)
                }
            }
            "tools/call" => return self.decide_sync_tool(id, params),
            "resources/list" => self.list_resources(id, params),
            "resources/read" => self.read_resource(id, params)?,
            "prompts/list" => self.list_prompts(id, params),
            "prompts/get" => self.get_prompt(id, params)?,
            _ => error(id, -32601, "Method not found", None),
        };
        complete(Some(frame))
    }
    fn initialize(&self, id: &Value, params: Option<&Value>) -> Value {
        if self.initialized.get() {
            return error(id, -32600, "Already initialized", None);
        }
        let params = params.unwrap_or(&Value::Null);
        if !has_only(params, &["protocolVersion", "capabilities", "clientInfo", "_meta"])
            || params["protocolVersion"] != LEGACY_PROTOCOL_VERSION
            || !params["capabilities"].is_object()
            || !params["clientInfo"].is_object()
            || !is_non_empty_string(&params["clientInfo"]["name"], 256)
            || !is_non_empty_string(&params["clientInfo"]["version"], 64)
            || !has_only(&params["clientInfo"], &["name", "version", "title", "description", "websiteUrl", "icons"])
        {
            return error(id, -32602, "Invalid initialize parameters", None);
        }
        self.initialized.set(true);
        self.protocol_era.set(Some(ProtocolEra::Legacy));
        response(
            id,
            json!({"protocolVersion":LEGACY_PROTOCOL_VERSION,"capabilities":{"tools":{"listChanged":true},"resources":{},"prompts":{}},"serverInfo":{"name":"ableton-mcp-host","version":self.server_version()}}),
        )
    }
    fn decide_sync_tool(&self, id: &Value, params: Option<&Value>) -> Result<RequestDecision, LiveError> {
        let complete = |frame| Ok(RequestDecision::Complete(Some(frame)));
        let params = params.unwrap_or(&Value::Null);
        if !has_only(params, &["name", "arguments", "_meta"]) || !params["name"].is_string() {
            return complete(error(id, -32602, "Invalid tools/call parameters", None));
        }
        let args = params.get("arguments");
        if args.is_some_and(|v| !v.is_object()) {
            return complete(error(id, -32602, "Tool arguments must be an object", None));
        }
        let name = params["name"].as_str().unwrap();
        self.note_tool_list_changed()?;
        if !self.tool_callable(name)? {
            return complete(self.tool_gate_error(id, name)?);
        }
        if self.recovery_finalization_in_flight.get()
            && ![
                "server_status",
                "capabilities",
                "plan_user_journey",
                "live_status",
                "live_snapshot",
                "live_discover",
                "live_project_snapshot_export",
                "live_project_snapshot_diff",
            ]
            .contains(&name)
        {
            return complete(adapter_tool_error(
                id,
                &LiveError::error("recovery finalization safety barrier is in progress"),
                "Wait for terminal recovery finalization before any synchronous mutation.",
            ));
        }
        if !table_has("argumentTools", name)
            && !table_has("asyncOnlyTools", name)
            && args.is_some_and(|v| !v.as_object().unwrap().is_empty())
        {
            return complete(error(id, -32602, "Tool arguments must be an empty object", None));
        }
        let frame = match name {
            "server_status" => success_text(id, &json!({"host":"ready","live":self.safe_adapter_status()})),
            "capabilities" => success_text(id, &self.capability_catalog()?),
            "plan_user_journey" => self.plan_user_journey(id, args),
            "live_status" => self.live_status(id),
            _ => {
                if table_has("syncAsyncRequiredTools", name) || table_has("asyncOnlyTools", name) {
                    return complete(error(id, -32001, "This operation requires the asynchronous host request path", None));
                }
                if name == "live_object_rename_preview" || name == "live_object_rename_apply" {
                    return complete(adapter_tool_error(
                        id,
                        &LiveError::error("rename requires the asynchronous production adapter boundary"),
                        "Use McpHost.handleAsync for guarded rename operations.",
                    ));
                }
                return Ok(RequestDecision::Tool(ToolCall {
                    id: id.clone(),
                    name: name.into(),
                    arguments: args.cloned(),
                    asynchronous: false,
                }));
            }
        };
        complete(frame)
    }
}
