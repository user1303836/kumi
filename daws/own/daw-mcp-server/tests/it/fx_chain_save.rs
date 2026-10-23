//! m4-r-fxenv: REAPER's FX chain save reaches the bridge with only its own fields, and is REAPER's alone.
use daw_bridge::files::ensure_secret;
use daw_bridge::paths::BridgePaths;
use daw_bridge::Daw;
use daw_mcp_server::host::Host;
use daw_mcp_server::mcp::{handle, tools};
use daw_mcp_server::simulator::{Simulator, SimulatorConfig};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn an_fx_chain_save_goes_to_reapers_bridge_with_its_fields_and_comes_back_as_it_answered() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::Reaper);
    let secret = ensure_secret(&paths.secret()).unwrap();
    let asked = Arc::new(Mutex::new(Vec::<Value>::new()));
    let log = asked.clone();
    let respond: daw_mcp_server::simulator::Respond = Arc::new(move |method, params| {
        (method == "fx_chain_save").then(|| {
            log.lock().unwrap().push(params.clone());
            Ok(json!({"epoch": "1", "file": "/lab/REAPER/FXChains/Lead.RfxChain", "name": "Lead", "count": 2, "replaced": false}))
        })
    });
    let mut config = SimulatorConfig::reaper(secret, "/lab/REAPER").with_methods(&["fx_chain_save"], respond);
    config.discovery_dir = Some(paths.discovery());
    let simulator = Simulator::start(config).await.unwrap();
    let host = Host::new(Daw::Reaper, home.path());
    assert!(tools(&host).as_array().unwrap().iter().any(|tool| tool["name"] == "fx_chain_save"));
    let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "fx_chain_save", "arguments": {
        "ref": "r:track:{T0001}", "fxRefs": ["r:fx:{F1}"], "name": "Lead", "overwrite": true, "input": false, "epoch": "1",
    }}});
    let answer = handle(&host, &call).await.unwrap();
    assert_eq!(answer["result"]["isError"], false, "{answer}");
    assert_eq!(answer["result"]["structuredContent"]["count"], 2);
    assert_eq!(
        asked.lock().unwrap().clone(),
        [json!({"ref": "r:track:{T0001}", "fxRefs": ["r:fx:{F1}"], "name": "Lead", "overwrite": true, "input": false, "epoch": "1"})]
    );
    simulator.stop();
}

#[tokio::test]
async fn renoise_has_no_fx_chain_save() {
    let home = tempfile::tempdir().unwrap();
    let host = Host::new(Daw::Renoise, home.path());
    assert!(!tools(&host).as_array().unwrap().iter().any(|tool| tool["name"] == "fx_chain_save"));
    let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "fx_chain_save", "arguments": {"ref": "x", "name": "y"}}});
    assert_eq!(handle(&host, &call).await.unwrap()["error"]["code"], -32602);
}
