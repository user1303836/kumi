use std::f64::consts::PI;
use std::future::Future;
use std::time::Duration;

use ableton_mcp_server::analysis::MAX_ANALYSIS_SAMPLES;
use ableton_mcp_server::analysis_runner::{
    AnalysisJob, AnalysisRunner, AnalysisRunnerStatus, EncodedAnalysisSource, MAX_ANALYSIS_JOB_REQUEST_BYTES, MAX_CONCURRENT_ANALYSIS_JOBS,
    MAX_QUEUED_ANALYSIS_JOBS,
};
use base64::Engine;
use kumi_common::abort::Controller;
use tokio::task::{spawn_local, LocalSet};

fn encoded_tone(frames: usize) -> String {
    let mut bytes = Vec::with_capacity(frames * 4);
    for frame in 0..frames {
        bytes.extend_from_slice(&((0.1 * (2.0 * PI * 440.0 * frame as f64 / 48_000.0).sin()) as f32).to_le_bytes());
    }
    base64::engine::general_purpose::STANDARD.encode(&bytes)
}

fn analyze(pcm_base64: String) -> AnalysisJob {
    AnalysisJob::Analyze {
        source: EncodedAnalysisSource { pcm_base64, sample_rate: 48_000.0, channels: Some(1.0), channel_layout: None, frame_size: None },
    }
}

async fn local<T>(future: impl Future<Output = T>) -> T {
    LocalSet::new().run_until(future).await
}

async fn tick() {
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}

#[test]
fn worker_request_bound_contains_the_advertised_maximum_pcm_payload() {
    let maximum_base64_bytes = ((MAX_ANALYSIS_SAMPLES * 4) as f64 / 3.0).ceil() as usize * 4;
    assert!(maximum_base64_bytes + 1_024 < MAX_ANALYSIS_JOB_REQUEST_BYTES);
}

#[tokio::test]
async fn runs_standards_analysis_in_a_disposable_bounded_child_process() {
    local(async {
        let runner = AnalysisRunner::new();
        let result = runner.run(&analyze(encoded_tone(48_000)), None, None).await.unwrap();
        assert_eq!(result["version"], "pcm-analysis/v3");
        assert_eq!(result["privacy"]["rawAudioReturned"], false);
        assert_eq!(
            runner.status(),
            AnalysisRunnerStatus {
                active: 0,
                queued: 0,
                max_concurrent: MAX_CONCURRENT_ANALYSIS_JOBS,
                max_queued: MAX_QUEUED_ANALYSIS_JOBS
            }
        );
    })
    .await;
}

#[tokio::test]
async fn kills_an_isolated_worker_on_cancellation_and_releases_its_slot() {
    local(async {
        let runner = AnalysisRunner::new();
        let controller = Controller::new();
        let job = analyze(encoded_tone(500_000));
        let pending = {
            let runner = runner.clone();
            let signal = controller.signal.clone();
            spawn_local(async move { runner.run(&job, Some(signal), None).await })
        };
        tokio::time::sleep(Duration::from_millis(5)).await;
        controller.abort();
        let failure = pending.await.unwrap().unwrap_err();
        assert!(failure.0.contains("cancelled"), "{failure}");
        assert_eq!(runner.status().active, 0);
        assert_eq!(runner.status().queued, 0);
    })
    .await;
}

#[tokio::test]
async fn enforces_timeout_concurrency_and_queue_bounds() {
    local(async {
        let timeout_runner = AnalysisRunner::new();
        let failure = timeout_runner.run(&analyze(encoded_tone(100_000)), None, Some(1)).await.unwrap_err();
        assert!(failure.0.contains("exceeded"), "{failure}");
        assert_eq!(failure.0, "analysis job exceeded 1 ms");

        let runner = AnalysisRunner::new();
        let pcm_base64 = encoded_tone(300_000);
        let controllers: Vec<Controller> =
            (0..MAX_CONCURRENT_ANALYSIS_JOBS + MAX_QUEUED_ANALYSIS_JOBS + 1).map(|_| Controller::new()).collect();
        let calls: Vec<_> = controllers
            .iter()
            .map(|controller| {
                let runner = runner.clone();
                let job = analyze(pcm_base64.clone());
                let signal = controller.signal.clone();
                spawn_local(async move {
                    match runner.run(&job, Some(signal), None).await {
                        Ok(_) => "ok".to_string(),
                        Err(cause) => cause.0,
                    }
                })
            })
            .collect();
        tick().await;
        assert_eq!(
            runner.status(),
            AnalysisRunnerStatus {
                active: MAX_CONCURRENT_ANALYSIS_JOBS,
                queued: MAX_QUEUED_ANALYSIS_JOBS,
                max_concurrent: MAX_CONCURRENT_ANALYSIS_JOBS,
                max_queued: MAX_QUEUED_ANALYSIS_JOBS
            }
        );
        let mut outcomes = Vec::new();
        let last = calls.len() - 1;
        for (index, call) in calls.into_iter().enumerate() {
            if index == last {
                let queue_full = call.await.unwrap();
                assert!(queue_full.contains("queue is full"), "{queue_full}");
                for controller in &controllers {
                    controller.abort();
                }
            } else {
                outcomes.push(call);
            }
        }
        for call in outcomes {
            call.await.unwrap();
        }
        assert_eq!(runner.status().active, 0);
        assert_eq!(runner.status().queued, 0);
    })
    .await;
}

#[tokio::test]
async fn contains_malformed_worker_input_as_a_redacted_job_error() {
    local(async {
        let runner = AnalysisRunner::new();
        let failure = runner.run(&analyze("AAAA".to_string()), None, None).await.unwrap_err();
        assert!(failure.0.contains("float32") || failure.0.contains("normalized") || failure.0.contains("bounded"), "{failure}");
        assert_eq!(runner.status().active, 0);
    })
    .await;
}

#[tokio::test]
async fn refuses_a_job_whose_request_exceeds_the_worker_input_limit_before_spawning() {
    local(async {
        let runner = AnalysisRunner::new();
        let oversized = "A".repeat(MAX_ANALYSIS_JOB_REQUEST_BYTES);
        let failure = runner.run(&analyze(oversized), None, None).await.unwrap_err();
        assert_eq!(failure.0, "analysis job request exceeds the worker input limit");
        assert_eq!(runner.status().active, 0);
    })
    .await;
}
