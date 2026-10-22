use ableton_mcp_server::benchmark::*;
use serde_json::{json, Value};
use std::time::Duration;

#[global_allocator]
static ALLOCATOR: memory::MeasuredAllocator = memory::MeasuredAllocator;

fn same(actual: Value, expected: &Value) {
    assert_eq!(kumi_common::js::json::stringify(&actual), kumi_common::js::json::stringify(expected));
}
fn metadata(measurements: &[BenchmarkMeasurement]) -> Value {
    json!(measurements.iter().map(|m| json!({"name":m.name,"unit":m.unit,"budget":m.budget})).collect::<Vec<_>>())
}
fn metric<'a>(rows: &'a [BenchmarkMeasurement], name: &str) -> &'a BenchmarkMeasurement {
    rows.iter().find(|m| m.name == name).unwrap()
}

// Keep allocator observations and deliberate retained-allocation regressions in one test so
// another benchmark in this executable cannot interfere with the before/after measurement.
#[tokio::test(flavor = "current_thread")]
async fn source_gates_and_native_resource_evidence() {
    tokio::task::LocalSet::new().run_until(async {
    let oracle:Value=serde_json::from_str(include_str!("fixtures/benchmark-oracle.json")).unwrap();
    same(json!(BENCHMARK_BUDGETS),&oracle["budgets"]);
    assert_eq!(ANALYSIS_MEASUREMENTS,oracle["analysisMeasurements"].as_u64().unwrap() as usize);
    for row in oracle["percentiles"].as_array().unwrap(){
        let values:Vec<_>=row["values"].as_array().unwrap().iter().map(|v|v.as_f64().unwrap()).collect();
        let actual=match percentile(&values,row["fraction"].as_f64().unwrap()){Ok(value)=>json!(value),Err(error)=>json!({"error":error})};same(actual,&row["result"]);
    }
    for row in oracle["measurements"].as_array().unwrap(){same(json!(measure("gate",row["value"].as_f64().unwrap(),"ms",row["budget"].as_f64().unwrap(),row["minimum"]==true)),&row["result"]);}
    for row in oracle["analysis"].as_array().unwrap(){
        let kind=row["kind"].as_str().unwrap();let mut calls=0;
        let result=measure_maximum_input_analysis_with(|input|{calls+=1;assert_eq!(input.samples.len(),10_000_000);assert_eq!(input.sample_rate,48_000.);assert!(input.channels.is_none());Ok(json!({"sampleCount":if kind=="short"{1}else{input.samples.len()},"safety":{"projectMutated":match kind{"unsafe"=>json!(true),"truthyUnsafe"=>json!("yes"),_=>json!(false)}},"peak":0.5}))});
        let result=match result{Ok(rows)=>metadata(&rows),Err(error)=>json!({"error":error})};same(result,&row["result"]);assert_eq!(calls,row["calls"].as_u64().unwrap());
    }
    let protocol=tokio::time::timeout(Duration::from_secs(10),measure_protocol()).await.expect("protocol benchmark completed").unwrap();
    assert_eq!(protocol.iter().map(|m|m.name.as_str()).collect::<Vec<_>>(),["rpc_ping_p95_latency","rpc_ping_throughput","ndjson_batch_p95_latency","ndjson_response_loss","cancellation_p95_latency","malformed_stream_recovery_latency","restart_resume_latency"]);
    assert_eq!(metric(&protocol,"ndjson_response_loss").value,0.);
    assert!(protocol.iter().all(|m|m.value.is_finite()&&m.value>=0.));
    // Gate the actual measured duration; no fabricated timing or test-only budget increase.
    let slow=measure_maximum_input_analysis_with(|input|{std::thread::sleep(Duration::from_millis(2050));Ok(json!({"sampleCount":input.samples.len(),"safety":{"projectMutated":false}}))}).unwrap();
    assert!(!metric(&slow,"pcm_analysis_p95_latency").passed);
    let mut retained:Vec<Vec<f64>>=vec![];
    let allocations=measure_maximum_input_analysis_with(|input|{retained.push(vec![0.;input.samples.len()]);retained.push(vec![0.;input.samples.len()]);Ok(json!({"sampleCount":input.samples.len(),"safety":{"projectMutated":false}}))}).unwrap();
    assert!(!metric(&allocations,"pcm_analysis_array_buffer_delta").passed);
    assert!(metric(&allocations,"pcm_analysis_array_buffer_delta").value>=160_000_000.);
    std::hint::black_box(&retained);drop(retained);
    let resources=IsolatedAnalysisResources{elapsed_milliseconds:10.,latency_measurements:vec![2.,1.,3.],peak_rss_bytes:512_000_001.,peak_heap_used_bytes:256_000_001.,peak_external_bytes:256_000_001.,peak_array_buffers_bytes:256_000_001.,output_bytes:2_000_001.};
    let gates=isolated_measurements(&resources,&IsolatedAnalysisOptions::default()).unwrap();
    assert_eq!(gates.len(),6);assert!(gates[0].passed);assert!(gates[1..].iter().all(|m|!m.passed));
    let injected=measure_isolated_maximum_input_analysis(&IsolatedAnalysisOptions{worker_path:Some(env!("CARGO_BIN_EXE_ableton-mcp-analysis-worker").into()),slow_milliseconds:Some(5.),allocate:true,latency_budget_milliseconds:Some(1.),array_buffers_budget_bytes:Some(1.)}).await.unwrap();
    assert!(!metric(&injected,"pcm_isolated_latency_p95").passed);
    assert!(!metric(&injected,"pcm_isolated_peak_array_buffers").passed);
    assert!(metric(&injected,"pcm_isolated_peak_array_buffers").value>=480_000_000.);
    assert!(metric(&injected,"pcm_isolated_peak_rss").value>0.);
 }).await;
}
