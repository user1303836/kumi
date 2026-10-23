use futures::FutureExt;
use kumi_common::abort::Signal;
use kumi_runtime::{
    core::contracts::LiveFocus,
    integrations::ableton::{focus::*, plan_stream::*},
    mcp::types::CallToolResult,
    RuntimeError,
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};
fn oracle() -> Value {
    serde_json::from_str(&kumi_testkit::oracle("kumi-runtime/focus-stream-oracle.json")).unwrap()
}
#[test]
fn focus_fields_and_incremental_step_delivery_match_source_outputs() {
    let data = oracle();
    for case in data["focus"].as_array().unwrap() {
        let result = parse_focus(case["row"].as_object().unwrap());
        assert_eq!(
            kumi_common::js::json::stringify(&serde_json::to_value(result).unwrap()),
            kumi_common::js::json::stringify(&case["value"]),
            "{case}"
        );
    }
    for case in data["scans"].as_array().unwrap() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let events = seen.clone();
        let chunk = Rc::new(Cell::new(0));
        let index = chunk.clone();
        let mut scan = step_scanner(move |step| events.borrow_mut().push(json!({"chunk":index.get(),"step":step})));
        for (i, delta) in case["chunks"].as_array().unwrap().iter().enumerate() {
            chunk.set(i);
            scan.push(delta.as_str().unwrap());
        }
        assert_eq!(json!(*seen.borrow()), case["events"]);
    }
}
fn reply(row: Value) -> CallToolResult {
    serde_json::from_value(json!({"content":[],"structuredContent":{"items":[row]}})).unwrap()
}
async fn tick(ms: u64) {
    tokio::time::advance(Duration::from_millis(ms)).await;
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
}
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn focus_feed_deduplicates_recovers_and_stops() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let reads = Rc::new(Cell::new(0));
            let calls = reads.clone();
            let seen = Rc::new(RefCell::new(Vec::<Option<LiveFocus>>::new()));
            let events = seen.clone();
            let failures = Rc::new(Cell::new(0));
            let failed = failures.clone();
            let feed = start_focus_feed(FocusFeedOptions {
                interval_ms: Some(5),
                timeout_ms: Some(100),
                on_focus: Rc::new(move |focus| events.borrow_mut().push(focus)),
                on_failure: Some(Rc::new(move || failed.set(failed.get() + 1))),
                read: Rc::new(move |_| {
                    let count = calls.get();
                    calls.set(count + 1);
                    async move {
                        match count {
                            2..=4 => Err(RuntimeError::plain("busy")),
                            0..=1 => Ok(reply(json!({"focusTrackName":"Bass"}))),
                            _ => Ok(reply(json!({"focusTrackName":"Keys","focusDetail":"Clip","focusClipName":"Verse"}))),
                        }
                    }
                    .boxed_local()
                }),
            });
            tick(0).await;
            for _ in 0..6 {
                tick(5).await;
            }
            assert_eq!(seen.borrow().len(), 2);
            assert_eq!(failures.get(), 1);
            feed.stop();
            feed.stop();
            assert!(seen.borrow().last().unwrap().is_none());
            let count = reads.get();
            tick(20).await;
            assert_eq!(reads.get(), count);
        })
        .await;
}
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn pokes_coalesce_without_overlapping_reads_and_slow_keeps_existing_timer() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let calls = Rc::new(Cell::new(0));
            let active = Rc::new(Cell::new(0));
            let gate = Rc::new(tokio::sync::Semaphore::new(0));
            let read_calls = calls.clone();
            let read_active = active.clone();
            let read_gate = gate.clone();
            let feed = start_focus_feed(FocusFeedOptions {
                interval_ms: Some(10),
                timeout_ms: None,
                on_failure: None,
                on_focus: Rc::new(|_| {}),
                read: Rc::new(move |_| {
                    let (calls, active, gate) = (read_calls.clone(), read_active.clone(), read_gate.clone());
                    async move {
                        assert_eq!(active.get(), 0);
                        active.set(1);
                        calls.set(calls.get() + 1);
                        gate.acquire().await.unwrap().forget();
                        active.set(0);
                        Ok(reply(json!({"focusTrackName":"Bass"})))
                    }
                    .boxed_local()
                }),
            });
            tick(0).await;
            assert_eq!(calls.get(), 1);
            feed.poke();
            feed.poke();
            feed.poke();
            assert_eq!(calls.get(), 1);
            gate.add_permits(1);
            tick(0).await;
            tick(1).await;
            assert_eq!(calls.get(), 2);
            gate.add_permits(1);
            tick(0).await;
            feed.slow();
            tick(10).await;
            assert_eq!(calls.get(), 3, "old timer keeps its deadline");
            gate.add_permits(1);
            tick(0).await;
            tick(4999).await;
            assert_eq!(calls.get(), 3);
            tick(1).await;
            assert_eq!(calls.get(), 4);
            feed.stop();
            gate.add_permits(1);
            tick(0).await;
        })
        .await;
}
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn read_deadlines_signal_abort_and_late_results_cannot_restore_stopped_focus() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let failures = Rc::new(Cell::new(0));
            let failed = failures.clone();
            let feed = start_focus_feed(FocusFeedOptions {
                interval_ms: Some(5),
                timeout_ms: Some(20),
                on_focus: Rc::new(|_| panic!("no successful reads")),
                on_failure: Some(Rc::new(move || failed.set(failed.get() + 1))),
                read: Rc::new(|signal| {
                    async move {
                        signal.cancelled().await;
                        Err(RuntimeError::Aborted)
                    }
                    .boxed_local()
                }),
            });
            tick(0).await;
            tick(20).await;
            tick(5).await;
            tick(20).await;
            assert_eq!(failures.get(), 1);
            feed.stop();
            let seen = Rc::new(RefCell::new(vec![]));
            let events = seen.clone();
            let gate = Rc::new(tokio::sync::Semaphore::new(0));
            let read_gate = gate.clone();
            let aborted = Rc::new(RefCell::new(None::<Signal>));
            let read_signal = aborted.clone();
            let feed = start_focus_feed(FocusFeedOptions {
                interval_ms: None,
                timeout_ms: None,
                on_failure: None,
                on_focus: Rc::new(move |focus| events.borrow_mut().push(focus)),
                read: Rc::new(move |signal| {
                    *read_signal.borrow_mut() = Some(signal);
                    let gate = read_gate.clone();
                    async move {
                        gate.acquire().await.unwrap().forget();
                        Ok(reply(json!({"focusTrackName":"Late"})))
                    }
                    .boxed_local()
                }),
            });
            tick(0).await;
            feed.stop();
            assert!(aborted.borrow().as_ref().unwrap().is_cancelled());
            gate.add_permits(1);
            tick(0).await;
            assert!(seen.borrow().is_empty());
        })
        .await;
}
