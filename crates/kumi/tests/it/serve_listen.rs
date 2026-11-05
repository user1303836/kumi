//! The real facade, adapters and DSP over MCP; only the DAW is simulated.
use super::*;
use daw_mcp_server::{renoise::simulator::RenoiseScript, simulator::listening::SimListening};
use std::path::Path;

async fn listening_lab(daw: Daw) -> (Lab, SimListening) {
    let (id, listening, config): (DawId, SimListening, Box<dyn FnOnce(daw_bridge::auth::Secret) -> SimulatorConfig>) = match daw {
        Daw::Reaper => {
            let script = ReaperScript::new(3, 0);
            let listening = script.listening().clone();
            (DawId::Reaper, listening, Box::new(move |secret| script.config(secret, "/lab/REAPER")))
        }
        Daw::Renoise => {
            let script = RenoiseScript::new();
            script.set_bpm(120.);
            let listening = script.listening().clone();
            (DawId::Renoise, listening, Box::new(move |secret| script.config(secret, "/lab/Renoise")))
        }
        _ => unreachable!(),
    };
    listening.set_render_preflight(true);
    listening.set_tempo(120.);
    let lab = Lab::new(id, daw, Some(config)).await;
    listening.set_folder(&BridgePaths::new(&lab.ctx.paths().kumi_home, daw).root.join("listening"));
    (lab, listening)
}

fn window(key: &Value, epoch: u64, track: Value) -> Value {
    json!({"source":{"track":track,"from_beat":0,"beats":4},
        "expected_project_key":key,"expected_epoch":epoch.to_string()})
}

fn approve(args: &mut Value, preview: &Value) {
    args["_listen"] = json!({"estimated_seconds":preview["estimated_seconds"],
        "prepare_seconds":preview["prepare_seconds"],"deadline_seconds":preview["deadline_seconds"]});
}

async fn approved_args(wire: &mut Wire, mut args: Value) -> Value {
    let mut ask = args.clone();
    ask["preview"] = json!(true);
    let (preview, error) = wire.call("listen", ask, None).await;
    assert!(!error && preview["state"] == "preview", "{preview}");
    approve(&mut args, &preview);
    args
}

fn no_audio(result: &Value) {
    assert!(result.get("metrics").is_none(), "{result}");
    assert!(result.get("artifact_id").is_none(), "{result}");
    assert!(result.get("media_tag").is_none(), "{result}");
    assert!(!result.to_string().contains("MEDIA:"), "{result}");
}

case!(preview_is_read_only_and_a_restored_capture_is_retained_once, async {
    for daw in [Daw::Reaper, Daw::Renoise] {
        let (lab, listening) = listening_lab(daw).await;
        let mut wire = Wire::start(lab.ctx.clone()).await;
        let (key, epoch, reference, _) = looked(&mut wire).await;
        let mut args = window(&key, epoch, json!({"ref":reference}));
        let mut preview_args = args.clone();
        preview_args["preview"] = json!(true);
        let (preview, error) = wire.call("listen", preview_args, Some("listen-once")).await;
        assert!(!error, "{preview}");
        assert_eq!(preview["state"], "preview", "{preview}");
        assert_eq!(preview["estimated_seconds"], 2.);
        assert!(preview["deadline_seconds"].as_u64().is_some_and(|seconds| (120..=360).contains(&seconds)));
        assert_eq!(preview["prepare_seconds"], 0);
        no_audio(&preview);
        assert!(listening.renders().is_empty());
        assert!(!lab.ctx.paths().kumi_home.join("serve").join(format!("{}.lease", daw.id())).exists(), "preview never takes the writer lease");
        assert!(!lab.home.path().join("state/scratch/listen").exists());
        assert!(!lab.home.path().join("state/quarantine").exists());
        assert!(lab.ctx.history().rows(10).is_empty());
        approve(&mut args, &preview);

        // A neighboring cache file is not this call's staging and must survive even though the bridge normally tidies its cache.
        kumi_types::private::create_dir_all(&listening.folder()).unwrap();
        let neighbor = listening.folder().join("unrelated.wav");
        daw_mcp_server::simulator::listening::write_noise(&neighbor, 0.1, 0.2, 42).unwrap();
        let neighbor_bytes = std::fs::read(&neighbor).unwrap();
        let (heard, error) = wire.call("listen", args.clone(), Some("listen-once")).await;
        assert!(!error, "{heard}");
        assert_eq!(heard["state"], "done", "{heard}");
        assert_eq!(heard["capture"]["cleanup"], "restored");
        assert_eq!(heard["capture"]["method"], "offline");
        assert_eq!(heard["capture"]["seconds"], 2.);
        assert_eq!(heard["metrics"]["seconds"], 2.);
        assert!(heard["capture"].get("note").is_none(), "no notice for a full clean capture");
        let media = heard["media_tag"].as_str().unwrap().strip_prefix("MEDIA:").unwrap().trim();
        let retained = Path::new(media);
        assert!(retained.is_file(), "playback names a retained file, not the removed scratch");
        assert!(retained.starts_with(lab.ctx.paths().state_dir.join("artifacts")));
        assert_eq!(std::fs::read(&neighbor).unwrap(), neighbor_bytes);
        let cache: Vec<_> = std::fs::read_dir(listening.folder()).unwrap().map(|entry| entry.unwrap().file_name()).collect();
        assert_eq!(cache, [neighbor.file_name().unwrap()]);
        assert_eq!(std::fs::read_dir(lab.home.path().join("state/scratch/listen")).unwrap().count(), 0);
        assert_eq!(lab.ctx.link().project().unwrap().epoch, epoch, "restored capture does not stale the observed refs");
        assert!(lab.ctx.history().rows(10).is_empty(), "capture is not an undoable change");
        assert_eq!(listening.restored(), 1);
        assert!(listening.previews().is_empty(), "an offline render is never auditioned");

        let (again, error) = wire.call("listen", args, Some("listen-once")).await;
        assert!(!error, "{again}");
        assert_eq!(again["artifact_id"], heard["artifact_id"]);
        assert_eq!(listening.renders().len(), 1, "retrying delivery never starts another render");
        let (file, error) = wire.call("listen", json!({"source":{"artifact_id":heard["artifact_id"]}}), None).await;
        assert!(!error, "{file}");
        assert_eq!(file["metrics"]["seconds"], heard["metrics"]["seconds"]);
        assert!(file.get("capture").is_none(), "file listening stays a file read");
        assert_eq!(listening.renders().len(), 1);
        wire.end().await;
    }
});

case!(bad_reference_and_stale_epoch_do_not_spend_a_playback, async {
    let (lab, listening) = listening_lab(Daw::Reaper).await;
    let mut wire = Wire::start(lab.ctx.clone()).await;
    let (key, epoch, _, _) = looked(&mut wire).await;
    let mut args = approved_args(&mut wire, window(&key, epoch, json!({"mix":true}))).await;
    args["compare_to"] = json!({"file":lab.home.path().join("not-approved.wav")});
    args["_listen"]["prepare_seconds"] = json!(120);
    let (denied, error) = wire.call("listen", args, Some("bad-reference")).await;
    assert!(error, "{denied}");
    assert_ne!(denied["error"]["code"], "not_approved", "the reference itself was refused");
    no_audio(&denied);
    let (stale, error) = wire.call("listen", window(&key, epoch + 1, json!({"mix":true})), Some("stale-window")).await;
    assert!(error, "{stale}");
    assert_eq!(stale["error"]["code"], "stale");
    no_audio(&stale);
    assert!(listening.renders().is_empty());
    assert!(!lab.home.path().join("state/artifacts.json").exists());
    wire.end().await;
});

case!(an_old_component_refuses_instead_of_using_an_unverified_renderer, async {
    for daw in [Daw::Reaper, Daw::Renoise] {
        let (lab, listening) = listening_lab(daw).await;
        listening.set_render_preflight(false);
        let mut wire = Wire::start(lab.ctx.clone()).await;
        let (key, epoch, _, _) = looked(&mut wire).await;
        let mut args = window(&key, epoch, json!({"mix":true}));
        args["preview"] = json!(true);
        let (refused, error) = wire.call("listen", args, Some("old-component")).await;
        assert!(!error, "{refused}");
        assert_eq!((refused["state"].clone(), refused["code"].clone()), (json!("refused"), json!("unsupported")));
        no_audio(&refused);
        assert!(listening.renders().is_empty());
        assert!(lab.ctx.history().rows(10).is_empty());
        wire.end().await;
    }
});

case!(a_short_restored_render_reports_measured_seconds_and_a_bounded_notice, async {
    let (lab, listening) = listening_lab(Daw::Renoise).await;
    listening.set_render_seconds(Some(1.));
    let mut wire = Wire::start(lab.ctx.clone()).await;
    let (key, epoch, _, _) = looked(&mut wire).await;
    let args = approved_args(&mut wire, window(&key, epoch, json!({"mix":true}))).await;
    let (heard, error) = wire.call("listen", args, Some("short-window")).await;
    assert!(!error, "{heard}");
    assert_eq!(heard["state"], "done", "{heard}");
    assert_eq!(heard["capture"]["cleanup"], "restored");
    assert_eq!(heard["capture"]["seconds"], 1.);
    assert_eq!(heard["metrics"]["seconds"], 1.);
    let note = heard["capture"]["note"].as_str().unwrap();
    assert!(!note.is_empty() && note.chars().count() <= 200);
    assert!(!note.contains('/') && !note.contains('\\'));
    assert_eq!(lab.ctx.link().project().unwrap().epoch, epoch);
    assert_eq!(listening.restored(), 1);
    wire.end().await;
});

case!(overlong_audio_is_unsure_discarded_and_audited_without_media, async {
    let (lab, listening) = listening_lab(Daw::Reaper).await;
    listening.set_render_seconds(Some(66.));
    let mut wire = Wire::start(lab.ctx.clone()).await;
    let (key, epoch, _, _) = looked(&mut wire).await;
    let args = approved_args(&mut wire, window(&key, epoch, json!({"mix":true}))).await;
    let (unsure, error) = wire.call("listen", args, Some("overlong-window")).await;
    assert!(!error, "{unsure}");
    assert_eq!(unsure["state"], "unsure", "{unsure}");
    no_audio(&unsure);
    assert!(!lab.home.path().join("state/artifacts.json").exists());
    assert_eq!(std::fs::read_dir(listening.folder()).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(lab.home.path().join("state/scratch/listen")).unwrap().count(), 0);
    assert!(lab.ctx.link().project().unwrap().epoch > epoch);
    let rows = serde_json::to_value(lab.ctx.history().rows(10)).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["state"], "unsure");
    assert_eq!(rows[0]["undo_available"], false);
    assert!(!lab.state("status.json")["unsure"].is_null());
    wire.end().await;
});

case!(capture_refuses_missing_or_invalid_host_bounds_before_rendering, async {
    let (lab, listening) = listening_lab(Daw::Reaper).await;
    let mut wire = Wire::start(lab.ctx.clone()).await;
    let (key, epoch, _, _) = looked(&mut wire).await;
    let args = window(&key, epoch, json!({"mix":true}));
    let (missing, error) = wire.call("listen", args.clone(), Some("missing-budget")).await;
    assert!(error && missing["error"]["code"] == "not_approved", "{missing}");
    no_audio(&missing);
    let args = approved_args(&mut wire, args).await;
    for (index, (field, value)) in [
        ("estimated_seconds", json!(0)), ("estimated_seconds", json!(60.001)),
        ("deadline_seconds", json!(0)), ("deadline_seconds", json!(361)),
        ("prepare_seconds", json!(121)), ("prepare_seconds", json!(0.5)),
        ("prepare_seconds", json!(1)),
    ].into_iter().enumerate() {
        let mut invalid = args.clone();
        invalid["_listen"][field] = value;
        let (refused, error) = wire.call("listen", invalid, Some(&format!("invalid-budget-{index}"))).await;
        assert!(error && refused["error"]["code"] == "invalid", "{refused}");
        no_audio(&refused);
    }
    let (unhosted, error) = wire.call("listen", args, None).await;
    assert!(error && unhosted["error"]["code"] == "invalid", "{unhosted}");
    no_audio(&unhosted);
    assert!(listening.renders().is_empty());
    assert!(!lab.home.path().join("state/artifacts.json").exists());
    wire.end().await;
});

case!(the_approved_native_deadline_is_not_replaced_by_a_fresh_plan, async {
    let (lab, listening) = listening_lab(Daw::Reaper).await;
    let mut wire = Wire::start(lab.ctx.clone()).await;
    let (key, epoch, _, _) = looked(&mut wire).await;
    let mut args = approved_args(&mut wire, window(&key, epoch, json!({"mix":true}))).await;
    args["_listen"]["deadline_seconds"] = json!(239);
    let (stale, error) = wire.call("listen", args, Some("short-budget")).await;
    assert!(!error && stale["state"] == "refused" && stale["code"] == "stale", "{stale}");
    no_audio(&stale);
    assert!(listening.renders().is_empty());
    assert_eq!(lab.ctx.link().project().unwrap().epoch, epoch);
    wire.end().await;
});

case!(a_slower_tempo_after_approval_is_stale_without_starting_a_render, async {
    let script = ReaperScript::new(3, 0);
    let listening = script.listening();
    listening.set_render_preflight(true);
    let bridge = script.clone();
    let lab = Lab::new(DawId::Reaper, Daw::Reaper, Some(Box::new(move |secret| bridge.config(secret, "/lab/REAPER")))).await;
    listening.set_folder(&BridgePaths::new(&lab.ctx.paths().kumi_home, Daw::Reaper).root.join("listening"));
    let mut wire = Wire::start(lab.ctx.clone()).await;
    let (key, epoch, _, _) = looked(&mut wire).await;
    let args = approved_args(&mut wire, window(&key, epoch, json!({"mix":true}))).await;
    script.set_tempo(60.);
    let (stale, error) = wire.call("listen", args, Some("tempo-drift")).await;
    assert!(!error && stale["state"] == "refused" && stale["code"] == "stale", "{stale}");
    no_audio(&stale);
    assert!(listening.renders().is_empty());
    assert_eq!(lab.ctx.link().project().unwrap().epoch, epoch);
    assert!(lab.ctx.history().rows(10).is_empty());
    wire.end().await;
});

case!(a_native_queued_window_refusal_keeps_the_producers_new_tempo, async {
    let script = ReaperScript::new(3, 0);
    let listening = script.listening();
    listening.set_render_preflight(true);
    let bridge = script.clone();
    let lab = Lab::new(DawId::Reaper, Daw::Reaper, Some(Box::new(move |secret| {
        let mut config = bridge.config(secret, "/lab/REAPER");
        let inner = config.respond.take().unwrap();
        config.respond = Some(Arc::new(move |method, params| {
            if method == "listen_status" { bridge.set_tempo(60.); }
            inner(method, params)
        }));
        config
    }))).await;
    listening.set_folder(&BridgePaths::new(&lab.ctx.paths().kumi_home, Daw::Reaper).root.join("listening"));
    let mut wire = Wire::start(lab.ctx.clone()).await;
    let (key, epoch, _, _) = looked(&mut wire).await;
    let args = approved_args(&mut wire, window(&key, epoch, json!({"mix":true}))).await;
    listening.fail_next("stale", "the queued window exceeds its approved seconds");
    let (stale, error) = wire.call("listen", args, Some("queued-tempo-drift")).await;
    assert!(!error && stale["state"] == "refused" && stale["code"] == "stale", "{stale}");
    no_audio(&stale);
    let mut preview = window(&key, epoch, json!({"mix":true}));
    preview["preview"] = json!(true);
    let (current, error) = wire.call("listen", preview, None).await;
    assert!(!error && current["estimated_seconds"] == 4., "{current}");
    assert!(lab.ctx.history().rows(10).is_empty());
    wire.end().await;
});

case!(reference_preparation_finishes_before_capture_and_an_expired_allowance_never_renders, async {
    let (lab, listening) = listening_lab(Daw::Reaper).await;
    let mut wire = Wire::start(lab.ctx.clone()).await;
    let (key, epoch, _, _) = looked(&mut wire).await;
    let args = approved_args(&mut wire, window(&key, epoch, json!({"mix":true}))).await;
    let (first, error) = wire.call("listen", args, Some("reference-capture")).await;
    assert!(!error && first["state"] == "done", "{first}");
    let mut args = window(&key, epoch, json!({"mix":true}));
    args["compare_to"] = json!({"artifact_id":first["artifact_id"]});
    let mut args = approved_args(&mut wire, args).await;
    assert!(args["_listen"]["prepare_seconds"].as_u64().is_some_and(|seconds| seconds > 0 && seconds <= 120));
    let (compared, error) = wire.call("listen", args.clone(), Some("compared-capture")).await;
    assert!(!error && compared["state"] == "done", "{compared}");
    assert_eq!(compared["compare"]["artifact_id"], first["artifact_id"]);
    assert_eq!(compared["compare"]["metrics"]["seconds"], first["metrics"]["seconds"]);
    assert_eq!(listening.renders().len(), 2);
    args["_listen"]["prepare_seconds"] = json!(0);
    let (expired, error) = wire.call("listen", args, Some("expired-preparation")).await;
    assert!(error && expired["error"]["code"] == "prepare_timeout", "{expired}");
    no_audio(&expired);
    assert_eq!(listening.renders().len(), 2, "reference preparation never enters native capture");
    assert_eq!(lab.ctx.link().project().unwrap().epoch, epoch);
    assert!(lab.ctx.history().rows(10).is_empty());
    wire.end().await;
});
