use ableton_mcp_server::live::{MixerState, RoutingState};
use serde_json::json;

#[test]
fn observed_mixer_and_routing_keep_property_order_nulls_and_updated_typed_values() {
    let mixer = json!({"sendRefs":[],"volumeRef":null,"sends":[],"volume":0.5,"pan":null,"cueVolume":1,"mute":false,"solo":false,"panRef":null,"cueRef":null,"trackActivatorRef":"p:active","trackActivator":true,"futureField":{"b":2,"a":1},"futureNull":null});
    let mut typed: MixerState = serde_json::from_value(mixer.clone()).unwrap();
    assert_eq!(kumi_common::js::json::stringify(&serde_json::to_value(&typed).unwrap()), kumi_common::js::json::stringify(&mixer));
    typed.volume = Some(0.75);
    let updated = serde_json::to_value(&typed).unwrap();
    assert_eq!(updated["volume"], 0.75);
    assert_eq!(updated.as_object().unwrap().keys().collect::<Vec<_>>(), mixer.as_object().unwrap().keys().collect::<Vec<_>>());
    typed.extra.remove("futureNull");
    assert!(serde_json::to_value(&typed).unwrap().get("futureNull").is_none());
    let route = json!({"outputType":"Main","inputType":null,"availableInputTypes":null,"future":true});
    let typed: RoutingState = serde_json::from_value(route.clone()).unwrap();
    assert_eq!(kumi_common::js::json::stringify(&serde_json::to_value(&typed).unwrap()), kumi_common::js::json::stringify(&route));
}

#[test]
fn clip_groove_retains_observed_extensions_and_property_order() {
    let original = serde_json::json!({"ref":"groove:g","objectIdentity":"groove:identity","name":"Swing","future":null,"base":3});
    let mut groove: ableton_mcp_server::live::ClipGroove = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(serde_json::to_string(&groove).unwrap(), serde_json::to_string(&original).unwrap());
    groove.name = ableton_mcp_server::live::Maybe::some(json!("Edited"));
    let mut edited = original;
    edited["name"] = serde_json::json!("Edited");
    assert_eq!(serde_json::to_string(&groove).unwrap(), serde_json::to_string(&edited).unwrap());
}

#[test]
fn device_view_preserves_observed_values_for_host_restoration_checks() {
    for original in [json!({}), json!({"isCollapsed":null}), json!({"isCollapsed":true}), json!({"isCollapsed":"bad","future":null})] {
        let view: ableton_mcp_server::live::DeviceView = serde_json::from_value(original.clone()).unwrap();
        assert_eq!(serde_json::to_value(view).unwrap(), original);
    }
}

#[test]
fn clip_and_track_views_keep_unknown_observations_until_host_validation() {
    let clip = json!({"gridQuantization":"bad","gridIsTriplet":5,"future":null});
    let observed: ableton_mcp_server::live::ClipView = serde_json::from_value(clip.clone()).unwrap();
    assert_eq!(serde_json::to_value(observed).unwrap(), clip);
    let track = json!({"deviceInsertMode":"bad","isCollapsed":5,"isShowingChains":[],"selectedDeviceRef":false,"future":null});
    let observed: ableton_mcp_server::live::TrackView = serde_json::from_value(track.clone()).unwrap();
    assert_eq!(serde_json::to_value(observed).unwrap(), track);
}

#[test]
fn observed_clip_grooves_preserve_incomplete_and_malformed_fields() {
    for original in [json!({}), json!({"ref":"groove:g"}), json!({"name":"Swing"}), json!({"ref":null,"name":false,"future":null})] {
        let groove: ableton_mcp_server::live::ClipGroove = serde_json::from_value(original.clone()).unwrap();
        assert_eq!(serde_json::to_string(&groove).unwrap(), serde_json::to_string(&original).unwrap());
    }
}
