#[path = "support/streams.rs"]
mod streams;

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::Rc;
use std::time::Duration;

use ableton_mcp_server::stdio::{serve_stdio, HandlerOutcome, Notify, RecordContext, RecordHandler, StdioOptions};
use futures::FutureExt;
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use streams::{Hold, Pipe, WriterHandle};
use tokio::sync::Notify as Gate;
use tokio::task::{spawn_local, LocalSet};

fn handler<F, Fut>(f: F) -> RecordHandler
where
    F: Fn(String, Option<RecordContext>) -> Fut + 'static,
    Fut: Future<Output = HandlerOutcome> + 'static,
{
    Rc::new(move |record, context| f(record, context).boxed_local())
}

async fn local<T>(future: impl Future<Output = T>) -> T {
    LocalSet::new().run_until(future).await
}

/// `await new Promise(setImmediate)`: every task that is ready runs.
async fn tick() {
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}

async fn until(mut condition: impl FnMut() -> bool) {
    for _ in 0..4000 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    panic!("the condition was not met in time");
}

fn parse(record: &str) -> Value {
    serde_json::from_str(record).unwrap_or(Value::Null)
}

fn id_of(record: &str) -> i64 {
    parse(record).get("id").and_then(Value::as_i64).unwrap_or(-1)
}

fn request(id: i64, method: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "method": method }).to_string()
}

fn cancel(id: i64) -> String {
    json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": id } }).to_string()
}

fn capture_notifier() -> (Rc<RefCell<Option<Notify>>>, Box<dyn FnOnce(Notify)>) {
    let slot: Rc<RefCell<Option<Notify>>> = Rc::new(RefCell::new(None));
    let registered = slot.clone();
    (slot, Box::new(move |notify| *registered.borrow_mut() = Some(notify)))
}

#[tokio::test]
async fn stdio_preserves_order_across_a_backpressured_writable() {
    local(async {
        let (input, reader) = Pipe::new();
        let (output, writer) = WriterHandle::holding(Hold::Write, |_, writes| writes == 1);
        let done = spawn_local(serve_stdio(reader, writer, handler(|record, _| async move { Ok(Some(record)) }), StdioOptions::default()));
        // The first write's callback comes 5 ms later.
        let release = output.clone();
        spawn_local(async move {
            until(|| release.is_holding()).await;
            tokio::time::sleep(Duration::from_millis(5)).await;
            release.release();
        });
        input.end_with("1\n2\n3\n");
        done.await.unwrap().unwrap();
        assert_eq!(output.received(), ["1", "2", "3"]);
    })
    .await;
}

#[tokio::test]
async fn stdio_runs_bounded_concurrent_work_and_writes_each_response_when_its_work_is_done_a_slow_one_holds_up_no_other() {
    local(async {
        let (input, reader) = Pipe::new();
        let (output, writer) = WriterHandle::new();
        let active = Rc::new(Cell::new(0usize));
        let peak = Rc::new(Cell::new(0usize));
        let (counted_active, counted_peak) = (active.clone(), peak.clone());
        let done = spawn_local(serve_stdio(
            reader,
            writer,
            handler(move |record, _| {
                let (active, peak) = (counted_active.clone(), counted_peak.clone());
                async move {
                    let id = id_of(&record);
                    active.set(active.get() + 1);
                    peak.set(peak.get().max(active.get()));
                    tokio::time::sleep(Duration::from_millis(if id == 1 { 300 } else { 1 })).await;
                    active.set(active.get() - 1);
                    Ok(Some(record))
                }
            }),
            StdioOptions { max_in_flight: Some(2), ..Default::default() },
        ));
        input.end_with("{\"jsonrpc\":\"2.0\",\"id\":1}\n{\"jsonrpc\":\"2.0\",\"id\":2}\n{\"jsonrpc\":\"2.0\",\"id\":3}\n");
        done.await.unwrap().unwrap();
        assert_eq!(peak.get(), 2);
        // Request 1 takes 300 ms; 2 and 3 finish first and are answered first (JSON-RPC matches answers by id). Windows'
        // 15.6 ms timer turns each 1 ms sleep into about 16 ms, so 3 ends near 32 ms: well before 1, where 20 ms wasn't.
        assert_eq!(output.received_ids(), vec![json!(2), json!(3), json!(1)]);
    })
    .await;
}

#[tokio::test]
async fn stdio_saturation_refuses_excess_work_without_stranding_cancellation_behind_backpressure() {
    local(async {
        let (input, reader) = Pipe::new();
        let (output, writer) = WriterHandle::holding(Hold::Write, |_, writes| writes == 1);
        let aborted = Rc::new(Cell::new(false));
        let observed = aborted.clone();
        let done = spawn_local(serve_stdio(
            reader,
            writer,
            handler(move |record, context| {
                let aborted = observed.clone();
                async move {
                    if id_of(&record) == 1 {
                        let signal = context.expect("request context").signal;
                        signal.cancelled().await;
                        aborted.set(true);
                    }
                    Ok(Some(record))
                }
            }),
            StdioOptions { max_in_flight: Some(1), ..Default::default() },
        ));
        let mut lines: Vec<String> = (1..=15).map(|id| request(id, "work")).collect();
        lines.push(cancel(1));
        input.end_with(&format!("{}\n", lines.join("\n")));
        for _ in 0..50 {
            if aborted.get() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert!(aborted.get());
        let busy = |id: i64| {
            output.received().iter().any(|value| {
                let frame = parse(value);
                frame["id"] == json!(id) && frame["error"]["code"] == json!(-32000)
            })
        };
        assert!(busy(5));
        output.release();
        done.await.unwrap().unwrap();
        assert!(busy(15));
    })
    .await;
}

#[tokio::test]
async fn stdio_rechecks_cancellation_after_a_completed_reply_waits_in_the_output_queue() {
    for rejects in [false, true] {
        local(async move {
            let (input, reader) = Pipe::new();
            let (output, writer) = WriterHandle::holding(Hold::Write, |chunk, _| id_of(chunk) == 5);
            let gate = Rc::new(Gate::new());
            let signal: Rc<RefCell<Option<Signal>>> = Rc::new(RefCell::new(None));
            let (handler_gate, handler_signal) = (gate.clone(), signal.clone());
            let done = spawn_local(serve_stdio(
                reader,
                writer,
                handler(move |record, context| {
                    let (gate, signal) = (handler_gate.clone(), handler_signal.clone());
                    async move {
                        let id = id_of(&record);
                        if id == 1 {
                            *signal.borrow_mut() = Some(context.expect("request context").signal);
                            gate.notified().await;
                            if rejects {
                                return Err("completed handler failed before output drained".to_string());
                            }
                        }
                        Ok(Some(json!({ "jsonrpc": "2.0", "id": id, "result": {} }).to_string()))
                    }
                }),
                StdioOptions { max_in_flight: Some(1), ..Default::default() },
            ));
            let lines: Vec<String> = (1..=5).map(|id| request(id, "work")).collect();
            input.write(&format!("{}\n", lines.join("\n")));
            until(|| output.received_ids().contains(&json!(5))).await;
            gate.notify_one();
            tick().await;
            assert_eq!(output.received_ids(), vec![json!(5)], "the busy reply holds stdout while request 1 is queued to write");
            assert!(!signal.borrow().as_ref().unwrap().is_cancelled());
            input.write(&format!("{}\n", cancel(1)));
            tick().await;
            assert!(signal.borrow().as_ref().unwrap().is_cancelled(), "completed but un-emitted replies retain cancellation ownership");
            gate.notify_one();
            output.release();
            input.end();
            done.await.unwrap().unwrap();
            assert_eq!(
                output.received_ids(),
                vec![json!(5), json!(2), json!(3), json!(4)],
                "cancelled success/error replies are omitted without blocking later responses"
            );
        })
        .await;
    }
}

#[tokio::test]
async fn stdio_emitted_response_cleanup_preserves_a_reused_ids_cancellation_ownership() {
    // A write held before it is accepted (highWaterMark 1) and one held after (65536: the callback).
    for kind in [Hold::Write, Hold::Flush] {
        local(async move {
            let (input, reader) = Pipe::new();
            let (output, writer) = WriterHandle::holding(kind, |_, writes| writes == 1);
            let signals: Rc<RefCell<Vec<Signal>>> = Rc::new(RefCell::new(Vec::new()));
            let second_gate = Rc::new(Gate::new());
            let (seen, gate) = (signals.clone(), second_gate.clone());
            let done = spawn_local(serve_stdio(
                reader,
                writer,
                handler(move |record, context| {
                    let (seen, gate) = (seen.clone(), gate.clone());
                    async move {
                        seen.borrow_mut().push(context.expect("request context").signal);
                        let second = seen.borrow().len() == 2;
                        if second {
                            gate.notified().await;
                        }
                        Ok(Some(json!({ "jsonrpc": "2.0", "id": id_of(&record), "result": {} }).to_string()))
                    }
                }),
                StdioOptions::default(),
            ));
            input.write("{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"work\"}\n");
            until(|| output.writes() == 1).await;
            input.write("{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"work-again\"}\n");
            tick().await;
            assert_eq!(signals.borrow().len(), 2, "an emitted reply frees its ID even before the write callback/drain");
            output.release();
            tick().await;
            input.write("{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":7}}\n");
            tick().await;
            assert!(signals.borrow()[1].is_cancelled(), "old-response cleanup must not delete the replacement controller");
            assert!(!signals.borrow()[0].is_cancelled());
            output.release();
            second_gate.notify_one();
            input.end();
            done.await.unwrap().unwrap();
            assert_eq!(output.received_ids(), vec![json!(7)]);
        })
        .await;
    }
}

#[tokio::test]
async fn stdio_output_failure_closes_authority_and_contains_writable_callback_errors() {
    local(async {
        let (input, reader) = Pipe::new();
        let (output, writer) = WriterHandle::failing_write("injected output failure");
        let (emit, notifier) = capture_notifier();
        let calls = Rc::new(Cell::new(0usize));
        let counted = calls.clone();
        let done = spawn_local(serve_stdio(
            reader,
            writer,
            handler(move |_, _| {
                counted.set(counted.get() + 1);
                async { Ok(Some("must-not-run".to_string())) }
            }),
            StdioOptions { notifier: Some(notifier), ..Default::default() },
        ));
        tick().await;
        let emit = emit.borrow().clone().expect("notifier registered");
        let failure = emit("event".to_string()).await.unwrap_err();
        assert!(failure.0.contains("injected output failure"), "{failure}");
        let terminated = done.await.unwrap().unwrap_err();
        assert!(
            terminated.0.contains("injected output failure") || terminated.0.contains("Premature close") || terminated.0.contains("output"),
            "{terminated}"
        );
        assert_eq!(calls.get(), 0);
        assert!(input.destroyed());
        assert!(output.destroyed());
    })
    .await;
}

#[tokio::test]
async fn stdio_treats_callback_only_writable_errors_as_authoritative_output_failure() {
    local(async {
        let (input, reader) = Pipe::new();
        let (output, writer) = WriterHandle::failing_flush("callback-only failure");
        let (emit, notifier) = capture_notifier();
        let done = spawn_local(serve_stdio(
            reader,
            writer,
            handler(|_, _| async { Ok(Some("must-not-run".to_string())) }),
            StdioOptions { notifier: Some(notifier), ..Default::default() },
        ));
        tick().await;
        let emit = emit.borrow().clone().expect("notifier registered");
        let failure = emit("event".to_string()).await.unwrap_err();
        assert!(failure.0.contains("callback-only failure"), "{failure}");
        let terminated = done.await.unwrap().unwrap_err();
        assert!(terminated.0.contains("callback-only failure") || terminated.0.contains("Premature close"), "{terminated}");
        assert!(input.destroyed());
        assert!(output.destroyed());
    })
    .await;
}

#[tokio::test]
async fn stdio_fail_closes_when_a_framing_response_saturates_the_output_queue() {
    local(async {
        let (input, reader) = Pipe::new();
        // The first write is held indefinitely.
        let (output, writer) = WriterHandle::holding(Hold::Write, |_, writes| writes == 1);
        let (emit, notifier) = capture_notifier();
        let done = spawn_local(serve_stdio(
            reader,
            writer,
            handler(|_, _| async { Ok(None) }),
            StdioOptions { max_in_flight: Some(1), notifier: Some(notifier), ..Default::default() },
        ));
        tick().await;
        let emit = emit.borrow().clone().expect("notifier registered");
        let queued: Vec<_> = (0..16)
            .map(|index| {
                let write = emit(format!("queued-{index}"));
                spawn_local(async move {
                    let _ = write.await;
                })
            })
            .collect();
        input.end_with_bytes(&[0xff, 0x0a]);
        let terminated = done.await.unwrap().unwrap_err();
        assert!(terminated.0.contains("bounded output queue is saturated"), "{terminated}");
        for task in queued {
            task.await.unwrap();
        }
        assert!(input.destroyed());
        assert!(output.destroyed());
    })
    .await;
}

#[tokio::test]
async fn stdio_fail_closes_when_a_notification_error_response_saturates_the_output_queue() {
    local(async {
        let (input, reader) = Pipe::new();
        let (output, writer) = WriterHandle::holding(Hold::Write, |_, writes| writes == 1);
        let (emit, notifier) = capture_notifier();
        let calls = Rc::new(Cell::new(0usize));
        let counted = calls.clone();
        let done = spawn_local(serve_stdio(
            reader,
            writer,
            handler(move |_, _| {
                counted.set(counted.get() + 1);
                async { Err("notification failure".to_string()) }
            }),
            StdioOptions { max_in_flight: Some(1), notifier: Some(notifier), ..Default::default() },
        ));
        tick().await;
        let emit = emit.borrow().clone().expect("notifier registered");
        let queued: Vec<_> = (0..16)
            .map(|index| {
                let write = emit(format!("queued-{index}"));
                spawn_local(async move {
                    let _ = write.await;
                })
            })
            .collect();
        input.end_with("{\"jsonrpc\":\"2.0\",\"method\":\"notification\"}\n");
        let terminated = done.await.unwrap().unwrap_err();
        assert!(terminated.0.contains("bounded output queue is saturated"), "{terminated}");
        for task in queued {
            task.await.unwrap();
        }
        assert_eq!(calls.get(), 1);
        assert!(input.destroyed());
        assert!(output.destroyed());
    })
    .await;
}

#[tokio::test]
async fn stdio_cancellation_aborts_the_matching_request_and_emits_no_response() {
    local(async {
        let (input, reader) = Pipe::new();
        let (output, writer) = WriterHandle::new();
        let aborted = Rc::new(Cell::new(false));
        let observed = aborted.clone();
        let done = spawn_local(serve_stdio(
            reader,
            writer,
            handler(move |record, context| {
                let aborted = observed.clone();
                async move {
                    if parse(&record)["method"] != json!("work") {
                        return Ok(Some(record));
                    }
                    let signal = context.expect("request context").signal;
                    signal.cancelled().await;
                    aborted.set(true);
                    Ok(Some("should-not-be-written".to_string()))
                }
            }),
            StdioOptions::default(),
        ));
        input.write("{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"work\"}\n");
        tick().await;
        input.write("{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":7}}\n");
        input.end();
        done.await.unwrap().unwrap();
        assert!(aborted.get());
        assert_eq!(output.received(), Vec::<String>::new());
    })
    .await;
}

#[tokio::test]
async fn stdio_rejects_a_duplicate_in_flight_id_without_replacing_cancellation_ownership() {
    local(async {
        let (input, reader) = Pipe::new();
        let (output, writer) = WriterHandle::new();
        let calls = Rc::new(Cell::new(0usize));
        let first_aborted = Rc::new(Cell::new(false));
        let (counted, observed) = (calls.clone(), first_aborted.clone());
        let done = spawn_local(serve_stdio(
            reader,
            writer,
            handler(move |_, context| {
                counted.set(counted.get() + 1);
                let aborted = observed.clone();
                async move {
                    let signal = context.expect("request context").signal;
                    signal.cancelled().await;
                    aborted.set(true);
                    Ok(Some("must-not-be-written".to_string()))
                }
            }),
            StdioOptions::default(),
        ));
        input.write("{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"work\"}\n");
        tick().await;
        input.write("{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"work-again\"}\n");
        input.write("{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":7}}\n");
        input.end();
        done.await.unwrap().unwrap();
        assert_eq!(calls.get(), 1);
        assert!(first_aborted.get());
        let received = output.received();
        assert_eq!(received.len(), 1);
        assert_eq!(
            parse(&received[0]),
            json!({ "jsonrpc": "2.0", "id": 7, "error": { "code": -32600, "message": "Duplicate in-flight request identifier" } })
        );
    })
    .await;
}

#[tokio::test]
async fn stdio_contains_delayed_handler_rejection_and_correlates_it_to_its_own_request_id() {
    local(async {
        let (input, reader) = Pipe::new();
        let (output, writer) = WriterHandle::new();
        let release_first = Rc::new(Gate::new());
        let gate = release_first.clone();
        let done = spawn_local(serve_stdio(
            reader,
            writer,
            handler(move |record, _| {
                let gate = gate.clone();
                async move {
                    if id_of(&record) == 1 {
                        gate.notified().await;
                        return Ok(Some(record));
                    }
                    Err("delayed failure".to_string())
                }
            }),
            StdioOptions { max_in_flight: Some(2), ..Default::default() },
        ));
        input.write("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"slow\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"fail\"}\n");
        tick().await;
        release_first.notify_one();
        input.end();
        done.await.unwrap().unwrap();
        // The failure is answered first (it finished first), each answer under its own id.
        let frames: Vec<Value> = output.received().iter().map(|line| parse(line)).collect();
        assert_eq!(
            frames,
            vec![
                json!({ "jsonrpc": "2.0", "id": 2, "error": { "code": -32603, "message": "Internal error" } }),
                json!({ "jsonrpc": "2.0", "id": 1, "method": "slow" }),
            ]
        );
    })
    .await;
}

/// Live's status, until `fault` is set: then a panic inside the bridge.
struct Faulting {
    fault: Cell<bool>,
}
impl ableton_mcp_server::live::LiveAdapter for Faulting {
    fn status(&self) -> Result<ableton_mcp_server::live::LiveStatus, ableton_mcp_server::live::LiveError> {
        assert!(!self.fault.get(), "a fault inside the bridge");
        ableton_mcp_server::live::UnavailableLiveAdapter.status()
    }
    fn snapshot(&self) -> Result<ableton_mcp_server::live::LiveSnapshot, ableton_mcp_server::live::LiveError> {
        ableton_mcp_server::live::UnavailableLiveAdapter.snapshot()
    }
    fn get(&self, r: &ableton_mcp_server::live::LiveRef) -> Result<Option<Value>, ableton_mcp_server::live::LiveError> {
        ableton_mcp_server::live::UnavailableLiveAdapter.get(r)
    }
    fn invoke(&self, i: &ableton_mcp_server::live::LiveInvocation) -> Result<Value, ableton_mcp_server::live::LiveError> {
        ableton_mcp_server::live::UnavailableLiveAdapter.invoke(i)
    }
    fn subscribe(
        &self,
        l: ableton_mcp_server::live::LiveListener,
    ) -> Result<ableton_mcp_server::live::Unsubscribe, ableton_mcp_server::live::LiveError> {
        ableton_mcp_server::live::UnavailableLiveAdapter.subscribe(l)
    }
    fn reconnect(&self) -> Result<ableton_mcp_server::live::LiveStatus, ableton_mcp_server::live::LiveError> {
        self.status()
    }
}
#[async_trait::async_trait(?Send)]
impl ableton_mcp_server::live::AsyncLiveAdapter for Faulting {
    async fn snapshot_async(
        &self,
        c: Option<&ableton_mcp_server::live::LiveOperationContext>,
        r: Option<&ableton_mcp_server::live::LiveSnapshotRequest>,
    ) -> Result<ableton_mcp_server::live::LiveSnapshot, ableton_mcp_server::live::LiveError> {
        ableton_mcp_server::live::UnavailableLiveAdapter.snapshot_async(c, r).await
    }
    async fn discover_async(
        &self,
        r: &ableton_mcp_server::live::LiveDiscoveryRequest,
        c: Option<&ableton_mcp_server::live::LiveOperationContext>,
    ) -> Result<ableton_mcp_server::live::LiveDiscoveryResult, ableton_mcp_server::live::LiveError> {
        ableton_mcp_server::live::UnavailableLiveAdapter.discover_async(r, c).await
    }
    async fn get_async(
        &self,
        r: &ableton_mcp_server::live::LiveRef,
        c: Option<&ableton_mcp_server::live::LiveOperationContext>,
    ) -> Result<Option<Value>, ableton_mcp_server::live::LiveError> {
        ableton_mcp_server::live::UnavailableLiveAdapter.get_async(r, c).await
    }
    async fn invoke_async(
        &self,
        i: &ableton_mcp_server::live::LiveInvocation,
        c: Option<&ableton_mcp_server::live::LiveOperationContext>,
    ) -> Result<Value, ableton_mcp_server::live::LiveError> {
        ableton_mcp_server::live::UnavailableLiveAdapter.invoke_async(i, c).await
    }
    async fn reconnect_async(
        &self,
        _: Option<&ableton_mcp_server::live::LiveOperationContext>,
    ) -> Result<ableton_mcp_server::live::LiveStatus, ableton_mcp_server::live::LiveError> {
        ableton_mcp_server::live::LiveAdapter::status(self)
    }
    async fn close(&self) -> Result<(), ableton_mcp_server::live::LiveError> {
        Ok(())
    }
}

#[tokio::test]
async fn a_fault_inside_the_bridge_is_answered_under_the_requests_own_id() {
    local(async {
        let (input, reader) = Pipe::new();
        let (output, writer) = WriterHandle::new();
        let (_diagnostics, log) = WriterHandle::new();
        let adapter = Rc::new(Faulting { fault: Cell::new(false) });
        let served: Rc<dyn ableton_mcp_server::live::AsyncLiveAdapter> = adapter.clone();
        let done = spawn_local(ableton_mcp_server::serve::serve(reader, writer, log, Some(served), Default::default()));
        input.write(&format!(
            "{}\n{}\n",
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        ));
        until(|| output.received().iter().any(|line| id_of(line) == 1)).await;
        adapter.fault.set(true);
        input.write(&format!("{}\n", json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"live_status","arguments":{}}})));
        until(|| output.received().iter().any(|line| id_of(line) == 7)).await;
        let answer = output.received().iter().map(|line| parse(line)).find(|frame| frame["id"] == 7).unwrap();
        assert_eq!(answer["error"], json!({"code":-32603,"message":"Internal error"}), "{answer}");
        assert!(!output.received().iter().any(|line| parse(line)["id"].is_null()), "an answer to null");
        input.end();
        let _ = done.await;
    })
    .await;
}

#[tokio::test]
async fn stdio_refuses_an_in_flight_bound_outside_1_to_64() {
    local(async {
        for bound in [0, 65] {
            let (_input, reader) = Pipe::new();
            let (_output, writer) = WriterHandle::new();
            let failure = serve_stdio(
                reader,
                writer,
                handler(|_, _| async { Ok(None) }),
                StdioOptions { max_in_flight: Some(bound), ..Default::default() },
            )
            .await
            .unwrap_err();
            assert_eq!(failure.0, "maxInFlight must be an integer from 1 to 64");
        }
    })
    .await;
}
