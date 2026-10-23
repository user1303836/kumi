//! Kumi's listening flow against a real REAPER or Renoise, through Kumi's host in process (a lab check, not a test):
//! listen to the mix and a track, audition two tracks, a judged round, a goal on one track (an EQ added, two generations
//! on a scratch copy, the best kept, the winner settled), then Kumi's undo of the settle, the kept best and the EQ.
//!
//! daw_listening_flow reaper|renoise <kumi-home> [track other]
//!
//! - The DAW runs with Kumi's bridge installed and connected to `<kumi-home>` (its discovery and secret), on a project
//!   whose tracks include `track` and `other` (REAPER default "Lead" and "Child", Renoise "Plain" and "Member"), stopped.
//! - The flow changes the project (and takes its changes back): use a lab project, never a producer's.
//! - Prints each step's answer, timing, what Kumi told the producer, HISTORY and the tracks after.
use daw_bridge::Daw;
use daw_mcp_server::host::Host;
use futures::FutureExt;
use kumi_common::abort::Signal;
use kumi_daw_own::bridge::{Connect, OwnOptions};
use kumi_daw_reaper::create_reaper_integration;
use kumi_daw_renoise::create_renoise_integration;
use kumi_runtime::core::{context::IntegrationContext, contracts::*};
use kumi_runtime::mcp::client::McpEndpoint;
use serde_json::{json, Value};
use std::{cell::Cell, cell::RefCell, rc::Rc, sync::Arc, time::Instant};

kumi_testkit::in_process!();
use in_process::InProcess;

fn input(value: Value) -> JsonObject {
    value.as_object().unwrap().clone()
}

fn tool(observed: &Observation, name: &str) -> Rc<dyn KernelTool> {
    observed.tools.iter().find(|tool| tool.name() == name).cloned().unwrap_or_else(|| panic!("no {name} tool"))
}

fn short(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

fn track_ref(context: &Value, name: &str) -> String {
    let tracks = context["tracks"].as_array().unwrap();
    tracks
        .iter()
        .find_map(|t| match t {
            Value::Object(_) if t["name"] == name => t["ref"].as_str().map(str::to_string),
            Value::String(line) => {
                let mut words = line.split_whitespace();
                let (reference, title) = (words.next()?, words.next()?);
                (title.trim_matches('"') == name).then(|| reference.to_string())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no track {name}: {}", short(&context["tracks"].to_string(), 600)))
}

fn names(context: &Value) -> Vec<String> {
    context["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| match t {
            Value::Object(_) => t["name"].as_str().unwrap_or_default().to_string(),
            other => other.to_string(),
        })
        .collect()
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let (Some(which), Some(home)) = (argv.first(), argv.get(1)) else {
        eprintln!("daw_listening_flow reaper|renoise <kumi-home> [track other]");
        std::process::exit(2);
    };
    let reaper = match which.as_str() {
        "reaper" => true,
        "renoise" => false,
        _ => {
            eprintln!("the DAW is reaper or renoise");
            std::process::exit(2);
        }
    };
    let (daw, eq, eq_field, eq_name) = if reaper {
        (Daw::Reaper, "fx_add", "name", "ReaEQ (Cockos)")
    } else {
        (Daw::Renoise, "device_add", "path", "Audio/Effects/Native/EQ 10")
    };
    let (lead, other) = match (argv.get(2), argv.get(3)) {
        (Some(track), Some(other)) => (track.clone(), other.clone()),
        _ if reaper => ("Lead".to_owned(), "Child".to_owned()),
        _ => ("Plain".to_owned(), "Member".to_owned()),
    };
    let (lead, other) = (lead.as_str(), other.as_str());
    let home = std::path::PathBuf::from(home);
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    tokio::task::LocalSet::new().block_on(&runtime, async {
        let endpoint = Rc::new(InProcess { host: Arc::new(Host::new(daw, &home)), id: Cell::new(0) });
        let connect: Connect = Rc::new(move |_| {
            let endpoint = endpoint.clone();
            async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
        });
        let rows: Rc<RefCell<Vec<ChangeRecord>>> = Rc::default();
        let told: Rc<RefCell<Vec<String>>> = Rc::default();
        let (into_rows, into_told) = (rows.clone(), told.clone());
        let mut context = IntegrationContext::new(Rc::new(|_, _| {}));
        context.on_change = Some(Rc::new(move |record| {
            let mut rows = into_rows.borrow_mut();
            match rows.iter_mut().find(|r| r.id == record.id) {
                Some(row) => *row = record,
                None => rows.push(record),
            }
        }));
        context.on_action = Some(Rc::new(move |event: ActionEvent| into_told.borrow_mut().push(event.title)));
        context.focus_interval_ms = Some(200);
        let options = OwnOptions { context, kumi_home: home.clone(), connect: Some(connect) };
        let daw_ = if reaper { create_reaper_integration(options) } else { create_renoise_integration(options) };
        let t = Instant::now();
        daw_.start(Signal::new()).await.unwrap();
        let observed = daw_.observe(Signal::new(), None).await.unwrap();
        println!("== 0. connected and observed in {} ms", t.elapsed().as_millis());
        let listening: Vec<&str> = observed
            .tools
            .iter()
            .map(|tool| tool.name())
            .filter(|name| ["audition", "judge", "tune", "sound", "form", "groove", "render"].contains(name))
            .collect();
        println!("   listening tools {listening:?}; hear {} audition {} goal {}", daw_.has_hear(), daw_.has_audition(), daw_.has_goal());
        let ctx: Value = serde_json::from_str(&observed.context).unwrap();
        println!("   tracks {:?}", names(&ctx));
        let (lead_ref, other_ref) = (track_ref(&ctx, lead), track_ref(&ctx, other));
        let said = |from: usize| told.borrow()[from..].join(" | ");

        println!("== 1. listen: the mix, beats 0-8");
        let (t, n) = (Instant::now(), told.borrow().len());
        let mix = daw_.hear(&HearRequest { mix: Some(true), from_beat: Some(0.), beats: Some(8.), ..Default::default() }, Signal::new()).await;
        println!("   {} ms: {}", t.elapsed().as_millis(), short(&format!("{mix:?}"), 700));
        println!("   told: {}", said(n));
        let reference = mix.ok().and_then(|r| r.ok()).and_then(|takes| takes.first().map(|take| take.file.clone()));

        println!("== 2. listen: {lead}, beats 0-8");
        let (t, n) = (Instant::now(), told.borrow().len());
        let one = daw_
            .hear(&HearRequest { tracks: vec![lead_ref.clone()], from_beat: Some(0.), beats: Some(8.), ..Default::default() }, Signal::new())
            .await;
        println!("   {} ms: {}", t.elapsed().as_millis(), short(&format!("{one:?}"), 700));
        println!("   told: {}", said(n));

        println!("== 3. audition: {lead} and {other}");
        let (t, n) = (Instant::now(), told.borrow().len());
        let audition = tool(&observed, "audition")
            .execute(
                input(json!({"candidates": [{"track": lead_ref, "label": "lead"}, {"track": other_ref, "label": "other"}], "from_beat": 0, "beats": 8})),
                Signal::new(),
            )
            .await
            .unwrap();
        println!("   {} ms, error {}: {}", t.elapsed().as_millis(), audition.is_error, short(&audition.text, 900));
        println!("   told: {}", said(n));

        println!("== 4. judge: a loudness goal, {other} down 3 dB, one round");
        let judge = tool(&observed, "judge");
        let (t, n) = (Instant::now(), told.borrow().len());
        let start = judge
            .execute(input(json!({"goal": {"loudness": -14, "problems": false}, "from_beat": 0, "beats": 8})), Signal::new())
            .await
            .unwrap();
        println!("   start {} ms, error {}: {}", t.elapsed().as_millis(), start.is_error, short(&start.text, 700));
        let volume = tool(&observed, "track_volume").execute(input(json!({"ref": other_ref, "volumeDb": -3})), Signal::new()).await.unwrap();
        println!("   track_volume error {}: {}", volume.is_error, short(&volume.text, 300));
        let t = Instant::now();
        let round = judge.execute(input(json!({"change": format!("{other} down 3 dB")})), Signal::new()).await.unwrap();
        println!("   round {} ms, error {}: {}", t.elapsed().as_millis(), round.is_error, short(&round.text, 900));
        println!("   told: {}", said(n));

        println!("== 5. goal: an EQ on {lead}, one slot on a scratch copy, two generations, keep the best, settle");
        let added = tool(&observed, eq).execute(input(json!({"ref": lead_ref, eq_field: eq_name})), Signal::new()).await.unwrap();
        println!("   {eq} error {}: {}", added.is_error, short(&added.text, 300));
        let reference = reference.expect("the mix's render as the reference");
        let request = AuditionRequest {
            candidates: vec![AuditionCandidate { track: lead.into(), clip: None, label: Some("lead".into()), mix: None }],
            from_beat: Some(0.),
            beats: Some(8.),
            reference: Some(reference),
            reference_from: None,
            reference_seconds: None,
            focus: None,
        };
        let (t, n) = (Instant::now(), told.borrow().len());
        let rig = match daw_.goal(&request, Signal::new()).await.unwrap() {
            Ok(rig) => rig,
            Err(why) => panic!("no goal: {why}"),
        };
        println!("   goal opened {} ms; screens {}", t.elapsed().as_millis(), rig.screens());
        let slots = rig.slots();
        let slot = slots[0].clone();
        println!("   slot {} chain {} knobs {}", slot.name, slot.chain, slot.knobs.len());
        let knob = slot
            .knobs
            .iter()
            .find(|k| k.name == "Gain-Band 2" || k.name.starts_with("Gain 2"))
            .or_else(|| slot.knobs.iter().find(|k| k.name.contains("Gain")))
            .cloned()
            .expect("a gain knob");
        println!("   knob {} [{}..{}] at {}", knob.name, knob.min, knob.max, knob.value);
        let at = |f: f64| knob.min + (knob.max - knob.min) * f;
        let mut best = (f64::MIN, 0.);
        for f in [0.3, 0.7] {
            let trial = GenerationTrial { slot: slot.name.clone(), knobs: vec![knob.clone()], values: vec![at(f)], fresh: None };
            let t = Instant::now();
            let generation = rig.generation(&[trial], Signal::new(), None).await.unwrap();
            let score = generation.scores.get(&slot.name).copied().unwrap_or(f64::MIN);
            println!("   generation at {:.3}: {} ms, score {score:.3}, silent {:?}", at(f), t.elapsed().as_millis(), generation.silent);
            if score > best.0 {
                best = (score, at(f));
            }
        }
        let t = Instant::now();
        let kept = rig.keep_best(&slot.name, std::slice::from_ref(&knob), &[best.1], Signal::new()).await.unwrap();
        println!("   keep_best {} ms: {kept}", t.elapsed().as_millis());
        let t = Instant::now();
        let settled = rig.settle(&slot.name, std::slice::from_ref(&knob), &[best.1], Signal::new()).await.unwrap();
        println!("   settle {} ms: {settled:?}", t.elapsed().as_millis());
        let closed = rig.close().await.unwrap();
        println!("   close: {closed:?}");
        println!("   told: {}", said(n));
        let again = daw_.observe(Signal::new(), None).await.unwrap();
        let ctx: Value = serde_json::from_str(&again.context).unwrap();
        println!("   tracks now {:?}", names(&ctx));
        println!("   HISTORY: {}", rows.borrow().iter().map(|r| format!("{} [{:?}] {}", r.id, r.state, r.title)).collect::<Vec<_>>().join(" | "));

        println!("== 6. Kumi's undo: the settle, the kept best, then the EQ added before the goal");
        for _ in 0..3 {
            let t = Instant::now();
            let undo = tool(&again, "undo_change").execute(input(json!({"change": "last"})), Signal::new()).await.unwrap();
            println!("   undo {} ms, error {}: {}", t.elapsed().as_millis(), undo.is_error, short(&undo.text, 400));
        }
        let after = daw_.observe(Signal::new(), None).await.unwrap();
        let ctx: Value = serde_json::from_str(&after.context).unwrap();
        println!("   tracks after {:?}", names(&ctx));
        println!("   HISTORY: {}", rows.borrow().iter().map(|r| format!("{} [{:?}] {}", r.id, r.state, r.title)).collect::<Vec<_>>().join(" | "));
        daw_.close().await.ok();
    });
}
