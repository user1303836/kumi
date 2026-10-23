use super::*;
use futures::future::LocalBoxFuture;
use kumi_common::js::{json as js_json, number as js_number};
use sha2::{Digest, Sha256};

pub const MAX_QUEUED_EVENTS: usize = 65_536;
pub type EventEmitter = Rc<dyn Fn(String) -> LocalBoxFuture<'static, Result<(), LiveError>>>;
#[derive(Default)]
pub(super) struct EventState {
    emitter: RefCell<Option<EventEmitter>>,
    queue: RefCell<VecDeque<String>>,
    overflow: Cell<u64>,
    flushing: Cell<bool>,
    failed: Cell<bool>,
}
impl EventState {
    fn enqueue(&self, line: String) {
        if self.queue.borrow().len() >= MAX_QUEUED_EVENTS {
            self.overflow.set((self.overflow.get() + 1).min(9_007_199_254_740_991));
        } else {
            self.queue.borrow_mut().push_back(line);
        }
    }
    fn next(&self, adapter: &dyn AsyncLiveAdapter) -> Option<String> {
        if let Some(line) = self.queue.borrow_mut().pop_front() {
            return Some(line);
        }
        let dropped = self.overflow.replace(0);
        (dropped>0).then(||js_json::stringify(&json!({"jsonrpc":"2.0","method":"notifications/live_event_overflow","params":{"epoch":safe_adapter_status(adapter).epoch,"dropped":dropped,"resnapshot":true}})))
    }
    fn schedule(self: &Rc<Self>, adapter: Rc<dyn AsyncLiveAdapter>) {
        if self.flushing.get() || self.failed.get() || self.emitter.borrow().is_none() {
            return;
        }
        self.flushing.set(true);
        // The source removes the first queued event before its first asynchronous wait.
        let first = self.next(&*adapter);
        let state = self.clone();
        tokio::task::spawn_local(async move {
            let mut line = first;
            loop {
                if let Some(line) = line {
                    let emitter = state.emitter.borrow().clone().expect("registered emitter");
                    if emitter(line).await.is_err() {
                        state.failed.set(true);
                        state.queue.borrow_mut().clear();
                        state.overflow.set(0);
                        break;
                    }
                }
                line = state.next(&*adapter);
                if line.is_none() {
                    break;
                }
            }
            state.flushing.set(false);
            if !state.failed.get() && (!state.queue.borrow().is_empty() || state.overflow.get() > 0) {
                state.schedule(adapter);
            }
        });
    }
}
impl McpHost {
    pub fn set_event_emitter(self: &Rc<Self>, emitter: EventEmitter) -> Result<(), LiveError> {
        *self.events.emitter.borrow_mut() = Some(emitter);
        self.events.failed.set(false);
        let weak = Rc::downgrade(self);
        let _ = self.adapter.subscribe(Rc::new(move |event| {
            if let Some(host) = weak.upgrade() {
                let _ = host.on_live_event(event);
            }
        }))?;
        if self.adapter.has_subscribe_status() {
            let weak = Rc::downgrade(self);
            let _ = self.adapter.subscribe_status(Rc::new(move |_| {
                if let Some(host) = weak.upgrade() {
                    let _ = host.note_tool_list_changed();
                }
            }));
        }
        Ok(())
    }
    pub fn note_tool_list_changed(&self) -> Result<(), LiveError> {
        if !self.initialized_notification.get() || self.events.emitter.borrow().is_none() {
            *self.tool_list_fingerprint.borrow_mut() = None;
            return Ok(());
        }
        let status = self.safe_adapter_status();
        let fingerprint = hex::encode(Sha256::digest(
            canonical_mutation_identity(&json!({
                "connected":status.connected,"epoch":status.epoch,"adapter":status.adapter,
                "operations":status.operations.unwrap_or_default(),"capabilities":status.capabilities,"policy":*self.tool_policy.borrow()
            }))?
            .as_bytes(),
        ));
        let prior = self.tool_list_fingerprint.replace(Some(fingerprint.clone()));
        if prior.is_none_or(|prior| prior == fingerprint) {
            return Ok(());
        }
        self.events.enqueue(js_json::stringify(&json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"})));
        self.events.schedule(self.adapter.clone());
        Ok(())
    }
    pub fn on_live_event(&self, event: &LiveEvent) -> Result<(), LiveError> {
        if self.protocol_era.get() != Some(ProtocolEra::Legacy) {
            return Ok(());
        }
        let mut params = serde_json::to_value(event).expect("Live event JSON");
        if event.channel.is_none() {
            params["channel"] = json!("remote-script");
        }
        if event.event_type == LiveEventType::Pointed && event.payload.is_object() {
            params["payload"] = self.pointed_with_refs(&event.payload);
        }
        self.events.enqueue(js_json::stringify(&json!({"jsonrpc":"2.0","method":"notifications/live_event","params":params})));
        if event.event_type == LiveEventType::Reset {
            self.note_tool_list_changed()?;
        }
        self.events.schedule(self.adapter.clone());
        Ok(())
    }
    pub fn pointed_with_refs(&self, payload: &Value) -> Value {
        let epoch = self.safe_adapter_status().epoch;
        let reference = |located: &Value| -> Option<String> {
            let epoch = epoch?;
            let kind = located.get("kind")?.as_str()?;
            if kind.is_empty() || !kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
                return None;
            }
            let path = located.get("path")?.as_array()?;
            let mut indices = Vec::new();
            for step in path {
                let step = step.as_f64()?;
                if !step.is_finite() || step.fract() != 0.0 || step < 0.0 {
                    return None;
                }
                indices.push(js_number::to_string(step));
            }
            Some(format!("{epoch}:{}:{}", if kind == "sample" { "device" } else { kind }, indices.join(":")))
        };
        let with_ref = |located: &Value| {
            let mut row = located.clone();
            if let Some(reference) = reference(located) {
                row["ref"] = json!(reference);
            }
            row
        };
        let mut row = with_ref(payload);
        for key in ["lanes", "slots"] {
            if let Some(rows) = payload.get(key).and_then(Value::as_array) {
                row[key] = Value::Array(rows.iter().map(with_ref).collect());
            }
        }
        row
    }
}
