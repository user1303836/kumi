use super::simulator_views::fields;
use super::*;
use serde_json::json;
#[derive(Clone)]
pub(super) struct ObserveTopic {
    kind: String,
    reference: Option<String>,
    revision: String,
}
pub(super) struct ObserveSubscription {
    topics: Vec<ObserveTopic>,
    min_interval: i64,
    sequence: u64,
    last_poll: i64,
}
fn base36(mut value: u64) -> String {
    let mut text = Vec::new();
    loop {
        text.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(value % 36) as usize]);
        value /= 36;
        if value == 0 {
            break;
        }
    }
    text.reverse();
    String::from_utf8(text).unwrap()
}
impl DeterministicLiveSimulator {
    fn observe_topic_digest(&self, kind: &str, reference: Option<&str>) -> Result<String, LiveError> {
        let state = self.state.borrow();
        let track = || array(&state["tracks"]).iter().find(|t| t["ref"].as_str() == reference);
        let device = || array(&state["tracks"]).iter().flat_map(|t| array(&t["devices"])).find(|d| d["ref"].as_str() == reference);
        let value = match kind {
            "transport" => {
                json!({"transport":state["playback"]["transport"],"tempo":state["set"]["tempo"],"signature":[state["song"]["signatureNumerator"],state["song"]["signatureDenominator"]]})
            }
            "selection" => state["selection"].clone(),
            "track" => {
                let track = track().ok_or_else(|| LiveError::error("track topics require a track ref"))?;
                json!({"arm":track["armed"],"mute":track["mute"],"solo":track["solo"],"fold":track["foldState"],"frozen":track["isFrozen"],"routing":[track["routing"].get("inputType").filter(|v|!v.is_null()).cloned().unwrap_or_else(||json!("")),track["routing"].get("outputType").filter(|v|!v.is_null()).cloned().unwrap_or_else(||json!(""))]})
            }
            "clip" => {
                let path = clip_path(&state, reference.unwrap_or("undefined"))?;
                let clip = state.pointer(&path).unwrap();
                json!({"playing":clip["isPlaying"],"loopStart":clip["loopStart"],"loopEnd":clip["loopEnd"],"looping":clip["looping"],"notes":clip["notes"],"warpMarkers":clip.get("warpMarkers").filter(|v|!v.is_null()).cloned().unwrap_or_else(||json!([])),"launchMode":clip["launchMode"]})
            }
            "device" => {
                let device = device().ok_or_else(|| LiveError::error("device topics require a device ref"))?;
                json!({"enabled":device["enabled"],"bank":device["parameterBank"],"view":device["view"]["isCollapsed"],"parameters":array(&device["parameters"]).iter().map(|p|p["value"].clone()).collect::<Vec<_>>()})
            }
            "parameter" => {
                let p = array(&state["tracks"])
                    .iter()
                    .flat_map(|t| array(&t["devices"]))
                    .flat_map(|d| array(&d["parameters"]))
                    .find(|p| p["ref"].as_str() == reference)
                    .ok_or_else(|| LiveError::error("parameter topics require a parameter ref"))?;
                json!({"value":p["value"]})
            }
            "groove" => state["groovePool"].clone(),
            "tuning" => state["tuning"].clone(),
            "scene" => {
                let scene = array(&state["scenes"])
                    .iter()
                    .find(|s| s["ref"].as_str() == reference)
                    .ok_or_else(|| LiveError::error("scene topics require a scene ref"))?;
                fields(
                    scene,
                    &[
                        "colorIndex",
                        "tempo",
                        "tempoEnabled",
                        "signatureNumerator",
                        "signatureDenominator",
                        "timeSignatureEnabled",
                        "isTriggered",
                    ],
                )
            }
            "meters" => {
                let track = track().ok_or_else(|| LiveError::error("meter topics require a track ref"))?;
                json!({"in":[track["inputMeterLeft"],track["inputMeterRight"]],"out":[track["outputMeterLeft"],track["outputMeterRight"]],"impact":track["performanceImpact"]})
            }
            "rack" => {
                let d = device().ok_or_else(|| LiveError::error("rack topics require a device ref"))?;
                json!({"macros":array(&d["macros"]).iter().map(|m|m["objectIdentity"].clone()).collect::<Vec<_>>(),"chains":array(&d["chains"]).iter().map(|c|c["objectIdentity"].clone()).collect::<Vec<_>>(),"variationCount":d["variationCount"],"selectedVariationIndex":d["selectedVariationIndex"],"visibleMacroCount":d["visibleMacroCount"]})
            }
            _ => return Err(LiveError::error("observe topic kind is invalid")),
        };
        Ok(simulator_revision(&value))
    }
    pub(super) fn invoke_observe(&self, operation: &str, args: &Map<String, Value>) -> Result<Value, LiveError> {
        match operation {
            "observe.subscribe" => {
                let topics = args
                    .get("topics")
                    .and_then(Value::as_array)
                    .filter(|a| !a.is_empty() && a.len() <= 64)
                    .ok_or_else(|| LiveError::range_error("topics are invalid"))?;
                let interval = ranged_number(
                    args.get("minIntervalMs").filter(|v| !v.is_null()).unwrap_or(&json!(250)),
                    100.,
                    60000.,
                    true,
                    "minIntervalMs is invalid",
                )? as i64;
                if self.observe_subscriptions.borrow().len() >= 8 {
                    return Err(LiveError::error("observe subscription quota is exhausted"));
                }
                let mut seen = std::collections::HashSet::new();
                let mut validated = Vec::new();
                for topic in topics {
                    let kind = topic
                        .get("kind")
                        .and_then(Value::as_str)
                        .filter(|s| {
                            [
                                "transport",
                                "selection",
                                "track",
                                "clip",
                                "device",
                                "parameter",
                                "groove",
                                "tuning",
                                "scene",
                                "meters",
                                "rack",
                            ]
                            .contains(s)
                        })
                        .ok_or_else(|| LiveError::range_error("observe topic is invalid"))?;
                    let reference = if let Some(value) = topic.get("ref") {
                        Some(value.as_str().ok_or_else(|| LiveError::range_error("observe topic ref is invalid"))?.to_string())
                    } else {
                        None
                    };
                    let key = format!("{kind}:{}", reference.as_deref().unwrap_or(""));
                    if !seen.insert(key) {
                        return Err(LiveError::range_error("duplicate observe topic"));
                    }
                    let revision = self.observe_topic_digest(kind, reference.as_deref())?;
                    validated.push(ObserveTopic { kind: kind.into(), reference, revision });
                }
                self.observe_sequence.set(self.observe_sequence.get() + 1);
                let id = format!("obs_{}_{}", base36(self.observe_sequence.get()), base36(rand::random::<u64>() >> 11));
                let revisions = validated
                    .iter()
                    .map(|t| (format!("{}:{}", t.kind, t.reference.as_deref().unwrap_or("")), json!(t.revision)))
                    .collect::<Map<_, _>>();
                let topics = validated
                    .iter()
                    .map(|t| {
                        let mut row = json!({"kind":t.kind});
                        if let Some(reference) = &t.reference {
                            row["ref"] = reference.clone().into();
                        }
                        row
                    })
                    .collect::<Vec<_>>();
                self.observe_subscriptions.borrow_mut().insert(
                    id.clone(),
                    ObserveSubscription { topics: validated, min_interval: interval, sequence: 0, last_poll: -interval },
                );
                Ok(json!({"subscriptionId":id,"topics":topics,"minIntervalMs":interval,"revisions":revisions}))
            }
            "observe.poll" => {
                let id =
                    args.get("subscriptionId").and_then(Value::as_str).ok_or_else(|| LiveError::type_error("subscriptionId is invalid"))?;
                let mut subscriptions = self.observe_subscriptions.borrow_mut();
                let subscription =
                    subscriptions.get_mut(id).ok_or_else(|| LiveError::error("observe subscription is unknown or expired"))?;
                let now = kumi_common::time::now_ms();
                if now - subscription.last_poll < subscription.min_interval {
                    return Err(LiveError::error("observe poll is faster than the negotiated minimum interval"));
                }
                subscription.last_poll = now;
                let mut events = Vec::new();
                let mut overflow = false;
                for topic in &mut subscription.topics {
                    let digest = self.observe_topic_digest(&topic.kind, topic.reference.as_deref())?;
                    if digest != topic.revision {
                        if events.len() >= 64 {
                            overflow = true;
                            break;
                        }
                        let fallback = [topic.kind.as_str()];
                        let fields: &[&str] = match topic.kind.as_str() {
                            "transport" => &["playing", "position", "loop"],
                            "clip" => &["playing", "notesRevision", "markers", "loop"],
                            "device" => &["enabled", "bank"],
                            "parameter" => &["value"],
                            "groove" => &["grooveAmount"],
                            "tuning" => &["referencePitch", "rootNote"],
                            "scene" => &["isTriggered", "tempo"],
                            "rack" => &["macros", "chains", "variationCount", "selectedVariationIndex"],
                            _ => &fallback,
                        };
                        events.push(json!({"kind":topic.kind,"ref":topic.reference,"revision":digest,"changedFields":fields}));
                        topic.revision = digest;
                    }
                }
                if overflow {
                    for topic in &mut subscription.topics {
                        topic.revision = self.observe_topic_digest(&topic.kind, topic.reference.as_deref())?;
                    }
                }
                subscription.sequence += 1;
                Ok(json!({"events":events,"overflow":overflow,"sequence":subscription.sequence}))
            }
            "observe.unsubscribe" => {
                let id =
                    args.get("subscriptionId").and_then(Value::as_str).ok_or_else(|| LiveError::type_error("subscriptionId is invalid"))?;
                if self.observe_subscriptions.borrow_mut().remove(id).is_none() {
                    return Err(LiveError::error("observe subscription is unknown or expired"));
                }
                Ok(json!({"unsubscribed":true}))
            }
            _ => unreachable!("observe dispatcher routes only implemented operations"),
        }
    }
}
