use super::*;
use crate::{
    journeys::{journey_resource, plan_user_journey, render_journey_prompt, JOURNEY_IDS, JOURNEY_PROMPTS},
    project::project_limitation,
};
use kumi_common::js::json as js_json;
use serde_json::Map;

fn unique(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values.into_iter().filter(|value| seen.insert(value.clone())).collect()
}
fn strings(table: &str) -> Vec<String> {
    DATA[table].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect()
}
impl McpHost {
    pub fn capability_catalog(&self) -> Result<Value, LiveError> {
        let live = self.safe_adapter_status();
        let capabilities: HashSet<_> = live.capabilities.iter().map(LiveCapability::as_str).collect();
        let rows = self.tool_visibility_rows()?;
        let live_rows: Vec<_> = rows.iter().filter(|row| !row.entry.local).collect();
        let available: Vec<_> = live_rows.iter().filter(|row| row.executable).map(|row| row.entry.name.clone()).collect();
        let unavailable: Vec<_> = live_rows.iter().filter(|row| !row.executable).map(|row| row.entry.name.clone()).collect();
        let visible: Vec<_> = rows.iter().filter(|row| row.visible).map(|row| row.entry.name.clone()).collect();
        let denied: Vec<_> = rows.iter().filter(|row| row.executable && !row.policy_allowed).map(|row| row.entry.name.clone()).collect();
        let classes: Map<String, Value> = rows.iter().map(|row| (row.entry.name.clone(), json!(row.entry.policy_class))).collect();
        let mut broad = Vec::new();
        if !tool_catalog::live_mutation_available(&live) {
            broad.push("live.mutations".into());
        }
        for (cap, name) in [("transport", "live.transport"), ("recording", "live.recording"), ("routing", "live.routing")] {
            if !capabilities.contains(cap) {
                broad.push(name.into());
            }
        }
        if !["audio", "audio.capture.resampling"].iter().any(|cap| capabilities.contains(cap)) {
            broad.push("live.audio".into());
        }
        if !["notes", "session.midi_note.read", "session.midi_note.write", "session.midi_clip.create"]
            .iter()
            .any(|cap| capabilities.contains(cap))
        {
            broad.push("live.midi".into());
        }
        if !capabilities.contains("realtime.events") {
            broad.push("realtime".into());
        }
        let live_unavailable = if live.connected {
            let mut values = strings("hostUnavailableCapabilities");
            values.extend(broad);
            values
                .extend(LIVE_UNAVAILABLE_CAPABILITIES.iter().filter(|c| !capabilities.contains(c.as_str())).map(|c| c.as_str().to_owned()));
            values
        } else {
            let mut values = strings("unavailableCapabilities");
            values.push("audio.diagnose.live-context".into());
            values.extend(LIVE_CAPABILITIES.iter().map(|c| c.as_str().to_owned()));
            values
        };
        let mut implemented: Vec<String> =
            ["server.status", "capabilities", "journeys.plan", "audio.analyze", "audio.analysis.standards", "audio.reference.compare"]
                .iter()
                .map(|s| (*s).into())
                .collect();
        implemented.extend(available.iter().map(|name| name.replace('_', ".")));
        let policy = self.tool_policy.borrow();
        let profile = tool_catalog::TOOL_POLICY_PROFILES
            .get(&policy.profile)
            .ok_or_else(|| LiveError::type_error("Cannot read properties of undefined (reading 'classes')"))?;
        let operations = if live.connected { live.operations.clone().unwrap_or_default() } else { Vec::new() };
        let reserved: Vec<_> = live_registry_operations().iter().filter(|operation| !operations.contains(operation)).cloned().collect();
        Ok(json!({
            "implemented":unique(implemented),"unavailable":unique(live_unavailable),
            "tools":{"available":available,"unavailable":unavailable,"visible":visible,"policyDenied":denied,"classes":classes},
            "policy":{"profile":policy.profile,"profileClasses":profile.classes,"allowOverrides":policy.allow,"denyOverrides":policy.deny},
            "limitations":[project_limitation("save"),project_limitation("open/new/export/collect/bounce")],
            "live":{"connected":live.connected,"adapter":live.adapter,"epoch":live.epoch,"protocol":live.protocol,"capabilities":live.capabilities},
            "operations":{"executable":operations,"reserved":reserved}
        }))
    }
    pub fn list_resources(&self, id: &Value, params: Option<&Value>) -> Value {
        if !utility_params(params) {
            return error(id, -32602, "Invalid resources/list parameters", None);
        }
        let mut resources = DATA["resources"].as_array().unwrap().clone();
        resources.push(DATA["liveResource"].clone());
        response(id, json!({"resources":resources}))
    }
    pub fn read_resource(&self, id: &Value, params: Option<&Value>) -> Result<Value, LiveError> {
        let params = params.unwrap_or(&Value::Null);
        if !has_only(params, &["uri"]) || !params["uri"].is_string() {
            return Ok(error(id, -32602, "Invalid resources/read parameters", None));
        }
        let uri = params["uri"].as_str().unwrap();
        let (mime, text) = match uri {
            "ableton://safety" => ("text/markdown", DATA["safetyResource"].as_str().unwrap().to_owned()),
            "ableton://capabilities" => ("application/json", js_json::stringify(&self.capability_catalog()?)),
            "ableton://max-extension" => (
                "application/json",
                js_json::stringify(&json!({
                    "version":"max-packet-extension/v1","available":false,"bundledDevice":false,"advertisedCapability":false,"channelLabel":"max","transport":"authenticated-loopback-udp",
                    "operations":["parameter.set","xy.set","emergency-stop"],"authority":["realtime.arm token","ttl","source port","exact parameter refs"],
                    "limits":{"packetBytes":512,"sustainedPerSecond":64,"burst":16},
                    "compatibility":"An operator-authored Max patch may emit this packet contract; device distribution and handshake require a separately versioned adapter."
                })),
            ),
            "ableton://journeys" => ("application/json", js_json::stringify(&journey_resource(&self.safe_adapter_status()))),
            "ableton://live-workflow" => ("text/markdown", DATA["liveWorkflowResource"].as_str().unwrap().to_owned()),
            _ => return Ok(error(id, -32002, "Resource not found", Some(json!({"uri":uri})))),
        };
        Ok(response(id, json!({"contents":[{"uri":uri,"mimeType":mime,"text":text}]})))
    }
    pub fn list_prompts(&self, id: &Value, params: Option<&Value>) -> Value {
        if !utility_params(params) {
            return error(id, -32602, "Invalid prompts/list parameters", None);
        }
        response(id, json!({"prompts":DATA["prompts"]}))
    }
    pub fn get_prompt(&self, id: &Value, params: Option<&Value>) -> Result<Value, LiveError> {
        let params = params.unwrap_or(&Value::Null);
        if !has_only(params, &["name", "arguments"])
            || !params["name"].is_string()
            || params.get("arguments").is_some_and(|v| !v.is_object())
        {
            return Ok(error(id, -32602, "Invalid prompts/get parameters", None));
        }
        let name = params["name"].as_str().unwrap();
        let args = params.get("arguments");
        if name == "change_tempo_safely" {
            if args.is_some_and(|args| !has_only(args, &[])) {
                return Ok(error(id, -32602, "Invalid prompt arguments", None));
            }
            return Ok(response(
                id,
                json!({"description":"Discover, preview, confirm, verify, and undo a tempo change","messages":[{"role":"user","content":text_content("Use live_status and live_snapshot, then live_tempo_preview, live_tempo_apply with explicit confirmation, live_snapshot for verification, and live_undo when restoration is requested.")}]}),
            ));
        }
        if let Some(prompt) = JOURNEY_PROMPTS.iter().find(|p| p["name"] == name) {
            let valid = args.is_some_and(|args| {
                has_only(args, &["traits", "experienceLevel", "bars"])
                    && args["traits"].is_string()
                    && args.get("experienceLevel").is_none_or(|v| v == "beginner" || v == "advanced")
                    && args.get("bars").is_none_or(|v| {
                        v.as_str().is_some_and(|v| {
                            matches!(
                                v,
                                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "10" | "11" | "12" | "13" | "14" | "15" | "16"
                            )
                        })
                    })
            });
            if !valid {
                return Ok(error(id, -32602, "Invalid journey prompt arguments", None));
            }
            let mut input = args.unwrap().clone();
            input["journey"] = json!(name.replace('_', "-"));
            if let Some(bars) = input.get("bars").and_then(Value::as_str) {
                input["bars"] = json!(bars.parse::<u8>().unwrap());
            }
            return Ok(match render_journey_prompt(&input, &self.safe_adapter_status()) {
                Ok(text) => {
                    response(id, json!({"description":prompt["description"],"messages":[{"role":"user","content":text_content(&text)}]}))
                }
                Err(cause) => error(id, -32602, &cause.to_string(), None),
            });
        }
        if name != "analyze_audio" {
            return Ok(error(id, -32002, "Prompt not found", Some(json!({"name":name}))));
        }
        if args.is_some_and(|args| !has_only(args, &["sampleRate", "channels"])) {
            return Ok(error(id, -32602, "Invalid prompt arguments", None));
        }
        let sample_rate = match args.and_then(|args| args.get("sampleRate")) {
            Some(v) => format!("Use sampleRate={} Hz.", js_string(v)?),
            None => "Provide sampleRate in Hz.".into(),
        };
        let channels = match args.and_then(|args| args.get("channels")) {
            Some(v) => format!("Use channels={}.", js_string(v)?),
            None => "Optionally provide channels.".into(),
        };
        let text =
            format!("Use tools/call with name audio_analyze and caller-supplied little-endian float32 PCM. {sample_rate} {channels}");
        Ok(response(id, json!({"description":"Safe local audio analysis","messages":[{"role":"user","content":text_content(&text)}]})))
    }
    pub fn plan_user_journey(&self, id: &Value, args: Option<&Value>) -> Value {
        let args = args.unwrap_or(&Value::Null);
        if !has_only(args, &["journey", "traits", "experienceLevel", "bars"])
            || !args["journey"].as_str().is_some_and(|j| JOURNEY_IDS.contains(&j))
            || !args["traits"].is_string()
            || args.get("experienceLevel").is_some_and(|v| v != "beginner" && v != "advanced")
            || args.get("bars").is_some_and(|v| !is_integer_in_range(v, 1.0, 16.0))
        {
            return error(id, -32602, "Invalid plan_user_journey arguments", None);
        }
        match plan_user_journey(args, &self.safe_adapter_status()) {
            Ok(plan) => success_text(id, &plan),
            Err(cause) => error(id, -32602, &cause.to_string(), None),
        }
    }
}
