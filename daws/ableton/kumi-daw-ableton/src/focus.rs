//! The producer's current selection, read without overlapping bridge requests.
use super::context::{object, payload};
use crate::{
    core::{
        contracts::{JsonObject, LiveFocus},
        errors::RuntimeError,
    },
    mcp::types::CallToolResult,
};
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_common::{
    abort::Signal,
    js::{number::is_safe_integer, string::head},
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    panic::{catch_unwind, AssertUnwindSafe},
    rc::Rc,
    time::Duration,
};
use tokio::{sync::Notify, time::sleep};

pub fn parse_focus(row: &JsonObject) -> Option<LiveFocus> {
    let text = |key: &str| row.get(key).and_then(Value::as_str).filter(|text| !text.is_empty()).map(|text| head(text, 256));
    let mut focus = JsonObject::new();
    let mut scene_index = None;
    if let Some(track) = text("focusTrackName") {
        let mut track = json!({"name":track}).as_object().unwrap().clone();
        if let Some(color) = row
            .get("focusTrackColor")
            .and_then(Value::as_str)
            .filter(|color| color.len() == 7 && color.starts_with('#') && color[1..].bytes().all(|b| b.is_ascii_hexdigit()))
        {
            track.insert("color".into(), json!(color));
        }
        if let Some(kind) =
            row.get("focusTrackKind").and_then(Value::as_str).filter(|kind| ["midi", "audio", "group", "return", "main"].contains(kind))
        {
            track.insert("kind".into(), json!(kind));
        }
        focus.insert("track".into(), Value::Object(track));
    }
    if focus.contains_key("track") {
        if let Some(reference) = text("selectedTrackRef") {
            focus.insert("trackRef".into(), json!(reference));
        }
    }
    for (key, field) in [("highlightedClipSlotRef", "slotRef"), ("selectedDeviceRef", "deviceRef")] {
        if let Some(value) = text(key) {
            focus.insert(field.into(), json!(value));
        }
    }
    if let Some(scene) = row
        .get("selectedSceneRef")
        .and_then(Value::as_str)
        .and_then(|s| s.rsplit_once(":scene:").map(|(_, n)| n))
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
    {
        scene_index = Some(scene.parse::<f64>().unwrap_or(f64::INFINITY));
        focus.insert("sceneIndex".into(), json!(scene_index));
    }
    for (key, field) in [("focusSceneName", "scene"), ("focusDeviceName", "device")] {
        if let Some(value) = text(key) {
            focus.insert(field.into(), json!(value));
        }
    }
    if let Some(clip) = row.get("focusClipName").and_then(Value::as_str) {
        focus.insert("clip".into(), json!(head(clip, 256)));
    }
    if let Some(parameter) = text("focusParameterName") {
        let mut parameter = json!({"name":parameter}).as_object().unwrap().clone();
        for (key, field) in [("focusParameterValue", "value"), ("focusParameterOwner", "owner")] {
            if let Some(value) = text(key) {
                parameter.insert(field.into(), json!(value));
            }
        }
        focus.insert("parameter".into(), Value::Object(parameter));
    }
    if let Some(chain) = text("focusChainName") {
        focus.insert("chain".into(), json!(chain));
    }
    for (key, field, allowed) in [("focusView", "view", ["Session", "Arrangement"]), ("focusDetail", "detail", ["Clip", "Device"])] {
        if let Some(value) = row.get(key).and_then(Value::as_str).filter(|s| allowed.contains(s)) {
            focus.insert(field.into(), json!(value));
        }
    }
    if let Some(browser) = row.get("focusBrowser").and_then(Value::as_bool) {
        focus.insert("browser".into(), json!(browser));
    }
    if let Some(notes) = row.get("focusSelectedNotes").and_then(Value::as_f64).filter(|n| is_safe_integer(*n) && *n >= 0.0) {
        focus.insert("selectedNotes".into(), json!(notes as usize));
    }
    if focus.is_empty() {
        None
    } else {
        {
            let mut parsed: LiveFocus = serde_json::from_value(Value::Object(focus)).expect("validated focus fields");
            parsed.scene_index = scene_index;
            Some(parsed)
        }
    }
}
pub struct FocusFeedOptions {
    pub read: Rc<dyn Fn(Signal) -> LocalBoxFuture<'static, Result<CallToolResult, RuntimeError>>>,
    pub on_focus: Rc<dyn Fn(Option<LiveFocus>)>,
    pub on_failure: Option<Rc<dyn Fn()>>,
    pub interval_ms: Option<u64>,
    pub timeout_ms: Option<u64>,
}
#[derive(Clone)]
pub struct FocusFeed(Rc<Inner>);
struct Inner {
    options: FocusFeedOptions,
    stopped: Cell<bool>,
    last: RefCell<String>,
    inflight: RefCell<Option<Signal>>,
    failures: Cell<usize>,
    again: Cell<bool>,
    interval: Cell<u64>,
    poke: Notify,
    stop: Signal,
}
impl Inner {
    fn report(&self, focus: Option<LiveFocus>) {
        let key = kumi_common::js::json::stringify(&serde_json::to_value(&focus).unwrap());
        if *self.last.borrow() == key {
            return;
        }
        *self.last.borrow_mut() = key;
        let _ = catch_unwind(AssertUnwindSafe(|| (self.options.on_focus)(focus)));
    }
}
pub fn start_focus_feed(options: FocusFeedOptions) -> FocusFeed {
    let inner = Rc::new(Inner {
        interval: Cell::new(options.interval_ms.unwrap_or(500)),
        options,
        stopped: Cell::new(false),
        last: RefCell::new("null".into()),
        inflight: RefCell::new(None),
        failures: Cell::new(0),
        again: Cell::new(false),
        poke: Notify::new(),
        stop: Signal::new(),
    });
    let running = inner.clone();
    tokio::task::spawn_local(async move {
        while !running.stopped.get() {
            let signal = Signal::new();
            *running.inflight.borrow_mut() = Some(signal.clone());
            let timeout_signal = signal.clone();
            let timeout_ms = running.options.timeout_ms.unwrap_or(2000);
            let timeout = tokio::task::spawn_local(async move {
                sleep(Duration::from_millis(timeout_ms)).await;
                timeout_signal.cancel();
            });
            let read = AssertUnwindSafe(async {
                let result = (running.options.read)(signal).await?;
                let page = payload(&result)?;
                let row = page.get("items").and_then(Value::as_array).and_then(|items| items.first());
                let focus = match row {
                    Some(row) => parse_focus(&object(row)?),
                    None => None,
                };
                Ok::<_, RuntimeError>(focus)
            })
            .catch_unwind()
            .await;
            timeout.abort();
            match read {
                Ok(Ok(focus)) => {
                    if !running.stopped.get() {
                        running.report(focus);
                    }
                    running.failures.set(0);
                }
                _ => {
                    if !running.stopped.get() {
                        let failures = running.failures.get() + 1;
                        running.failures.set(failures);
                        if failures == 2 {
                            if let Some(callback) = &running.options.on_failure {
                                let _ = catch_unwind(AssertUnwindSafe(|| callback()));
                            }
                        }
                    }
                }
            }
            *running.inflight.borrow_mut() = None;
            if running.stopped.get() {
                break;
            }
            let delay = if running.again.replace(false) { 0 } else { running.interval.get() };
            tokio::select! {_ = sleep(Duration::from_millis(delay))=>{},_=running.poke.notified()=>{},_=running.stop.cancelled()=>break}
        }
    });
    FocusFeed(inner)
}
impl FocusFeed {
    pub fn stop(&self) {
        if self.0.stopped.replace(true) {
            return;
        }
        self.0.stop.cancel();
        if let Some(signal) = self.0.inflight.borrow().as_ref() {
            signal.cancel();
        }
        self.0.report(None);
    }
    pub fn poke(&self) {
        if self.0.stopped.get() {
            return;
        }
        if self.0.inflight.borrow().is_some() {
            self.0.again.set(true);
        } else {
            self.0.poke.notify_one();
        }
    }
    pub fn slow(&self) {
        self.0.interval.set(self.0.interval.get().max(5000));
    }
}
