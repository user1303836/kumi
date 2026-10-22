use ableton_mcp_server::{
    audio_diagnosis::{diagnose_audio_with_live_context_value, AudioSourceProvenance},
    host::{audio::*, helpers::canonical_mutation_identity, McpHost, McpHostOptions},
    live::*,
};
use base64::Engine;
use serde_json::{json, Value};
use std::rc::Rc;
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/host-audio-oracle.json")).unwrap()
}
fn same(got: &Value, expected: &Value, label: &str) {
    assert_eq!(canonical_mutation_identity(got).unwrap(), canonical_mutation_identity(expected).unwrap(), "{label}");
}
#[test]
fn source_pcm_metadata_and_worker_job_preparation_match() {
    let fixture = fixture();
    for row in fixture["count"].as_array().unwrap() {
        assert_eq!(json!(base64_float_count(&row["value"])), row["result"], "{row}");
    }
    for row in fixture["encoded"].as_array().unwrap() {
        let got = encoded_analysis_source(
            row.get("value").unwrap_or(&Value::Null),
            row["maxChannels"].as_u64().unwrap() as usize,
            row["allowFrameSize"] == true,
        );
        if row["undefined"] == true {
            assert!(got.is_none(), "{row}");
        } else {
            let got = got.unwrap_or_else(|| panic!("{row}"));
            same(&json!({"sampleCount":got.sample_count,"source":got.source}), &row["result"], &row.to_string());
        }
    }
    for row in fixture["requests"].as_array().unwrap() {
        let got = if row["tool"] == "audio_analyze" {
            Ok(prepare_audio_analyze(&json!(1), row.get("args")))
        } else {
            prepare_audio_compare(&json!(1), row.get("args"))
        };
        if let Some(error) = row.get("error") {
            assert_eq!(got.unwrap_err().message(), error.as_str().unwrap(), "{row}");
        } else {
            match got.unwrap_or_else(|e| panic!("{row}: {e}")) {
                PreparedAudioJob::Rejected(frame) => same(&frame, &row["result"], &row.to_string()),
                PreparedAudioJob::Job(job) => same(&job, &row["jobs"][0], &row.to_string()),
            }
        }
    }
}
#[test]
fn diagnosis_preserves_full_mixer_routing_and_source_hashes() {
    let fixture = fixture();
    let analysis = serde_json::from_value(fixture["analysis"].clone()).unwrap();
    for row in fixture["diagnoses"].as_array().unwrap() {
        let source: AudioSourceProvenance = serde_json::from_value(row["source"].clone()).unwrap();
        let got = diagnose_audio_with_live_context_value(
            &analysis,
            &row["snapshot"],
            1,
            row["trackRef"].as_str().unwrap(),
            &source,
            Some("2026-01-01T00:00:01.000Z".into()),
        );
        if let Some(error) = row.get("error") {
            assert_eq!(got.unwrap_err().to_string(), error.as_str().unwrap());
        } else {
            same(&got.unwrap(), &row["result"], row["variant"].as_str().unwrap());
        }
    }
}
fn pcm(sample: f32, frames: usize) -> Value {
    let bytes: Vec<u8> = (0..frames).flat_map(|_| sample.to_le_bytes()).collect();
    json!({"pcmBase64":base64::engine::general_purpose::STANDARD.encode(bytes),"sampleRate":48000})
}
fn body(frame: &Value) -> Value {
    serde_json::from_str(frame["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}
#[tokio::test]
async fn host_audio_runs_isolated_workers_retains_provenance_and_honors_cancellation() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let sim = Rc::new(DeterministicLiveSimulator::new());
            let before = sim.state.borrow().clone();
            let host = McpHost::new(sim.clone(), McpHostOptions::default()).unwrap();
            let source = pcm(0.1, 4096);
            let analyzed = host.audio_analyze_async(&json!(1), Some(&source), None).await.unwrap();
            assert_eq!(analyzed["result"]["isError"], false, "{analyzed}");
            assert_eq!(body(&analyzed)["version"], "pcm-analysis/v3");
            let compared = host
                .audio_compare_reference_async(
                    &json!(2),
                    Some(&json!({"project":source,"reference":source,"alignment":{"mode":"disabled"}})),
                    None,
                )
                .await
                .unwrap()
                .unwrap();
            assert_eq!(compared["result"]["isError"], false, "{compared}");
            let coerced = host
                .audio_compare_reference_async(
                    &json!(3),
                    Some(&json!({"project":source,"reference":source,"alignment":{"mode":["auto"]}})),
                    None,
                )
                .await
                .unwrap()
                .unwrap();
            assert_eq!(coerced["result"]["isError"], false, "{coerced}");
            assert_eq!(body(&coerced)["alignment"]["mode"], json!(["auto"]));
            let non_normalized = host.audio_analyze_async(&json!(4), Some(&pcm(2.0, 1024)), None).await.unwrap();
            assert_eq!(non_normalized["result"]["isError"], true);
            let signal = kumi_common::abort::Signal::new();
            signal.cancel();
            assert!(host.audio_analyze_async(&json!(5), Some(&source), Some(&signal)).await.is_none());
            let mut params = source;
            params["trackRef"] = json!("track:track-1");
            params["provenance"] = json!({"observedAt":"2026-01-01","description":"caller capture"});
            let diagnosed = host.audio_diagnose_live_context_async(&json!(6), &params, None).await.unwrap();
            assert_eq!(diagnosed["result"]["isError"], false, "{diagnosed}");
            let value = body(&diagnosed);
            assert_eq!(value["diagnosis"]["source"]["relationshipToLive"], "declared-by-caller-not-verified");
            assert_eq!(
                value["diagnosis"]["context"]["mixer"]["sendIdentities"],
                json!(["simulator:parameter:mixer:0:sends:0", "simulator:parameter:mixer:0:sends:1"])
            );
            assert_eq!(value["diagnosis"]["context"]["mixer"]["trackActivator"], true);
            assert_eq!(*sim.state.borrow(), before, "analysis and diagnosis must leave Live unchanged");
            let unavailable = McpHost::default().audio_diagnose_live_context_async(&json!(7), &params, None).await.unwrap();
            assert_eq!(body(&unavailable)["reason"], "fresh Live context is unavailable");
        })
        .await;
}

fn same_worker_value(got: &Value, expected: &Value, path: &str) {
    match (got, expected) {
        (Value::Number(a), Value::Number(b)) => {
            let a = a.as_f64().unwrap();
            let b = b.as_f64().unwrap();
            assert!((a - b).abs() <= 1e-8 * b.abs().max(1.0), "{path}: {a} != {b}");
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{path}");
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                same_worker_value(a, b, &format!("{path}[{i}]"));
            }
        }
        (Value::Object(a), Value::Object(b)) => {
            assert_eq!(a.len(), b.len(), "{path}: {got} != {expected}");
            for (key, b) in b {
                same_worker_value(a.get(key).unwrap_or(&Value::Null), b, &format!("{path}.{key}"));
            }
        }
        _ => assert_eq!(got, expected, "{path}"),
    }
}
#[tokio::test]
async fn worker_coercion_and_semantic_validation_match_source_process() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture: Value = serde_json::from_str(include_str!("fixtures/analysis-worker-oracle.json")).unwrap();
            let runner = ableton_mcp_server::analysis_runner::AnalysisRunner::new();
            for (i, row) in fixture["rows"].as_array().unwrap().iter().enumerate() {
                let text =
                    serde_json::to_string(&row["job"]).unwrap().replace("\"$pcm\"", &serde_json::to_string(&fixture["pcm"]).unwrap());
                let job = serde_json::from_str(&text).unwrap();
                let result = runner.run_value(&job, None, None).await;
                if row["output"]["ok"] == true {
                    same_worker_value(&result.unwrap_or_else(|e| panic!("row {i}: {e}")), &row["output"]["result"], &format!("row {i}"));
                } else {
                    assert_eq!(result.unwrap_err().0, row["output"]["error"].as_str().unwrap(), "row {i}");
                }
            }
        })
        .await;
}
