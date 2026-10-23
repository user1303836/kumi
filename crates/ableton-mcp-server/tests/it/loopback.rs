use ableton_mcp_server::{live::*, loopback::*};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;
const SECRET: &str = "0123456789abcdef0123456789abcdef";
struct Adapter {
    listener: Rc<RefCell<Option<LiveListener>>>,
}
impl Adapter {
    fn new() -> Rc<Self> {
        Rc::new(Self { listener: Rc::new(RefCell::new(None)) })
    }
    fn emit(&self) {
        if let Some(listener) = self.listener.borrow().as_ref() {
            listener(&LiveEvent {
                epoch: 1,
                sequence: 1,
                event_type: LiveEventType::Reset,
                ref_: None,
                payload: json!({}),
                channel: None,
                coalesced: None,
            });
        }
    }
}
impl LiveAdapter for Adapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        let mut status = UnavailableLiveAdapter.status()?;
        status.operations = Some(vec!["browser.search".into()]);
        Ok(status)
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        Ok(LiveSnapshot::default())
    }
    fn get(&self, _: &LiveRef) -> Result<Option<Value>, LiveError> {
        Ok(None)
    }
    fn invoke(&self, _: &LiveInvocation) -> Result<Value, LiveError> {
        Ok(json!({"items":[]}))
    }
    fn subscribe(&self, listener: LiveListener) -> Result<Unsubscribe, LiveError> {
        *self.listener.borrow_mut() = Some(listener);
        let slot = self.listener.clone();
        Ok(Box::new(move || {
            slot.borrow_mut().take();
        }))
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.emit();
        self.status()
    }
}
fn request(server: &AuthenticatedLoopback, id: &str, method: &str) -> Value {
    server.authenticate(json!({"version":LOOPBACK_PROTOCOL_VERSION,"id":id,"method":method,"nonce":"0000000000000001"})).unwrap()
}
#[test]
fn loopback_authenticates_rejects_replay_and_tampering_and_forwards_subscriptions() {
    let live = Adapter::new();
    let events = Rc::new(RefCell::new(vec![]));
    let sink = events.clone();
    let server = AuthenticatedLoopback::new(live.clone(), SECRET, Some(Rc::new(move |event| sink.borrow_mut().push(event)))).unwrap();
    let first = request(&server, "one", "status");
    assert_eq!(server.handle(&first)["ok"], true);
    assert_eq!(server.handle(&first)["ok"], false);
    let mut tampered = request(&server, "two", "status");
    tampered["id"] = "changed".into();
    assert_eq!(server.handle(&tampered)["ok"], false);
    assert_eq!(server.handle(&request(&server, "sub", "subscribe"))["ok"], true);
    live.reconnect().unwrap();
    assert_eq!(events.borrow().len(), 1);
    server.close();
    live.reconnect().unwrap();
    assert_eq!(events.borrow().len(), 1);
}
#[test]
fn loopback_accepts_valid_nonces_out_of_order_and_rejects_unknown_fields() {
    let server = AuthenticatedLoopback::new(Adapter::new(), SECRET, None).unwrap();
    for (id, nonce) in [("first", "zzzzzzzzzzzzzzzz1"), ("second", "aaaaaaaaaaaaaaaa2")] {
        let request = server.authenticate(json!({"version":LOOPBACK_PROTOCOL_VERSION,"id":id,"method":"status","nonce":nonce})).unwrap();
        assert_eq!(server.handle(&request)["ok"], true);
    }
    let mut extra = request(&server, "third", "status");
    extra["unexpected"] = true.into();
    assert_eq!(server.handle(&extra)["ok"], false);
}
#[test]
fn loopback_rejects_oversized_nonces_and_signing_bounds() {
    let server = AuthenticatedLoopback::new(Adapter::new(), SECRET, None).unwrap();
    let oversized =
        server.authenticate(json!({"version":LOOPBACK_PROTOCOL_VERSION,"id":"large","method":"status","nonce":"x".repeat(257)})).unwrap();
    assert_eq!(server.handle(&oversized)["ok"], false);
    let large = json!({"version":LOOPBACK_PROTOCOL_VERSION,"id":"large","method":"invoke","operation":"browser.search","args":{"query":"x".repeat(1_048_577)},"nonce":"large-wire-value-0001"});
    assert_eq!(server.authenticate(large).unwrap_err().to_string(), "wire string is too large");
    let mut nested = json!("value");
    for _ in 0..257 {
        nested = json!({"value":nested});
    }
    assert_eq!(server.authenticate(json!({"args":nested})).unwrap_err().to_string(), "wire payload is too deeply nested");
    let notes = (0..20_000)
        .map(|index| json!({"pitch":index%128,"start":index as f64/4.0,"duration":0.25,"velocity":100,"channel":1}))
        .collect::<Vec<_>>();
    assert!(server.authenticate(json!({"version":LOOPBACK_PROTOCOL_VERSION,"id":"large-note-batch","method":"invoke","operation":"note.add-batch","args":{"ref":"1:clip:0:0","notes":notes},"nonce":"large-note-batch-0001"})).is_ok());
}
#[test]
fn loopback_retains_replay_protection_beyond_the_old_eviction_threshold() {
    let server = AuthenticatedLoopback::new(Adapter::new(), SECRET, None).unwrap();
    let first = request(&server, "first", "status");
    assert_eq!(server.handle(&first)["ok"], true);
    for i in 0..4096 {
        assert_eq!(server.handle(&request(&server, &format!("request-{i}"), "status"))["ok"], true);
    }
    assert_eq!(server.handle(&first)["ok"], false);
}
#[test]
fn loopback_client_authenticates_events_binds_responses_and_rejects_stale_events() {
    let live = Adapter::new();
    let events = Rc::new(RefCell::new(vec![]));
    let sink = events.clone();
    let server =
        Rc::new(AuthenticatedLoopback::new(live.clone(), SECRET, Some(Rc::new(move |event| sink.borrow_mut().push(event)))).unwrap());
    let exchange = server.clone();
    let client = LoopbackLiveAdapter::new(SECRET, Rc::new(move |request| exchange.handle(&request))).unwrap();
    assert_eq!(client.status().unwrap().adapter, LiveAdapterKind::Unavailable);
    assert_eq!(client.invoke(&LiveInvocation::new("browser.search", json!({"query":"kick"}))).unwrap(), json!({"items":[]}));
    let seen = Rc::new(RefCell::new(vec![]));
    let sink = seen.clone();
    let unsubscribe = client.subscribe(Rc::new(move |event| sink.borrow_mut().push(event.clone()))).unwrap();
    live.reconnect().unwrap();
    client.receive(&events.borrow()[0]).unwrap();
    assert_eq!(seen.borrow().len(), 1);
    assert_eq!(client.receive(&events.borrow()[0]).unwrap_err().to_string(), "stale loopback event");
    unsubscribe();
    let mut tampered = events.borrow()[0].clone();
    tampered["mac"] = "tampered".into();
    assert_eq!(client.receive(&tampered).unwrap_err().to_string(), "loopback response authentication failed");
    let wrong = LoopbackLiveAdapter::new(
        SECRET,
        Rc::new(move |request| {
            let mut response = server.handle(&request);
            response["id"] = "other".into();
            response
        }),
    )
    .unwrap();
    assert_eq!(wrong.status().unwrap_err().to_string(), "invalid loopback response");
    assert_eq!(
        client.invoke(&LiveInvocation::new("unknown", json!({}))).unwrap_err().to_string(),
        "loopback operation is not negotiated: unknown"
    );
}
