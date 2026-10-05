use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use futures::future::{FutureExt, LocalBoxFuture, Shared};
use indexmap::IndexMap;
use kumi_common::abort::{Signal, SignalExt};
use kumi_common::js::{json::stringify, string::utf16_len};
use serde_json::{json, Value};
use tokio::sync::oneshot;

use super::client::{McpEndpoint, MAX_BRIDGE_MESSAGE_BYTES};
use super::types::{CallToolResult, Tool};
use crate::core::{contracts::JsonObject, errors::RuntimeError, timing};

/// Tools the model may call directly: reads.
// Not live_snapshot: a whole big Set in one answer is more than the link carries (discovery pages instead).
pub const MODEL_TOOLS: [&str; 16] = [
    "server_status",
    "live_status",
    "live_discover",
    "live_browser_search",
    "live_note_read",
    "live_song_state",
    "live_performance_read",
    "live_key_estimate",
    "live_take_lane_read",
    "live_warp_marker_read",
    "live_arrangement_automation_read",
    "live_browser_roots",
    "live_browser_inspect",
    // Bridge 1.0.58: a plug-in's every parameter name and a device's banks, a clip's automation at a time, a clip's time in samples and seconds.
    "live_device_read",
    "live_automation_read",
    "live_clip_time_convert",
];
const MAX_RESULT_BYTES: usize = 64 * 1024;
const SMALL_PAGE: u64 = 100;
const MAX_HOST_RESULT_BYTES: usize = MAX_BRIDGE_MESSAGE_BYTES;
const MAX_CATALOG_BYTES: usize = 1024 * 1024;

type Pending = Shared<LocalBoxFuture<'static, Result<(), RuntimeError>>>;

#[derive(Default)]
struct State {
    catalog: IndexMap<String, Tool>,
    valid: bool,
    closed: bool,
    small_pages: bool,
    invalidation: u64,
    signature: String,
    revision: u64,
    unlisten: Vec<Box<dyn Fn()>>,
    closing: Option<Pending>,
    reading: Option<Pending>,
}

/// Host-owned authorization boundary; model instructions and annotations confer no authority.
#[derive(Clone)]
pub struct AllowedTools {
    endpoint: Rc<dyn McpEndpoint>,
    host_tools: Rc<HashSet<String>>,
    state: Rc<RefCell<State>>,
}

#[derive(Default, Clone, Copy)]
pub struct CallOptions {
    pub host: bool,
}

impl AllowedTools {
    /// `host_tools` are called only by Kumi itself (behind its change tools), never listed for the model.
    pub fn new(endpoint: Rc<dyn McpEndpoint>, host_tools: HashSet<String>) -> Self {
        let state = Rc::new(RefCell::new(State::default()));
        let weak = Rc::downgrade(&state);
        let invalidate: Rc<dyn Fn()> = Rc::new(move || {
            if let Some(state) = weak.upgrade() {
                let mut state = state.borrow_mut();
                state.valid = false;
                state.catalog.clear();
                state.invalidation += 1;
            }
        });
        let listeners = vec![endpoint.on_catalog_changed(invalidate.clone()), endpoint.on_disconnect(invalidate)];
        state.borrow_mut().unlisten = listeners;
        Self { endpoint, host_tools: Rc::new(host_tools), state }
    }

    pub fn generation(&self) -> u64 {
        self.state.borrow().revision
    }
    pub fn is_valid(&self) -> bool {
        let state = self.state.borrow();
        state.valid && !state.closed
    }
    pub fn list(&self) -> Vec<Tool> {
        if !self.is_valid() {
            return Vec::new();
        }
        self.state.borrow().catalog.values().filter(|tool| MODEL_TOOLS.contains(&tool.name.as_str())).cloned().collect()
    }
    pub fn has(&self, name: &str) -> bool {
        self.is_valid() && self.allowed(name) && self.state.borrow().catalog.contains_key(name)
    }
    pub fn tool(&self, name: &str) -> Option<Tool> {
        if self.is_valid() {
            self.state.borrow().catalog.get(name).cloned()
        } else {
            None
        }
    }
    fn allowed(&self, name: &str) -> bool {
        MODEL_TOOLS.contains(&name) || self.host_tools.contains(name)
    }

    /// Concurrent callers share one enumeration; a change while reading starts it over, a few times.
    pub fn refresh(&self, signal: Signal) -> LocalBoxFuture<'static, Result<(), RuntimeError>> {
        if let Err(error) = signal.check() {
            return futures::future::ready(Err(error.into())).boxed_local();
        }
        if self.state.borrow().closed {
            return futures::future::ready(Err(RuntimeError::plain("MCP catalog is closed"))).boxed_local();
        }
        let pending = {
            let mut state = self.state.borrow_mut();
            state
                .reading
                .get_or_insert_with(|| {
                    let this = self.clone();
                    let signal = signal.clone();
                    let (send, receive) = oneshot::channel();
                    tokio::task::spawn_local(async move {
                        let mut attempt = 0;
                        let result = loop {
                            match this.enumerate(signal.clone()).await {
                                Err(error)
                                    if attempt < 3
                                        && !this.state.borrow().closed
                                        && !signal.is_cancelled()
                                        && error.to_string().contains("changed during enumeration") =>
                                {
                                    attempt += 1;
                                }
                                result => break result,
                            }
                        };
                        this.state.borrow_mut().reading = None;
                        let _ = send.send(result);
                    });
                    async move { receive.await.unwrap_or_else(|_| Err(RuntimeError::plain("MCP catalog reading stopped"))) }
                        .boxed_local()
                        .shared()
                })
                .clone()
        };
        async move {
            tokio::select! {
                biased;
                _ = signal.cancelled() => Err(RuntimeError::Aborted),
                result = pending => result,
            }
        }
        .boxed_local()
    }

    async fn enumerate(&self, signal: Signal) -> Result<(), RuntimeError> {
        signal.check()?;
        let invalidation = {
            let mut state = self.state.borrow_mut();
            if state.closed {
                return Err(RuntimeError::plain("MCP catalog is closed"));
            }
            state.valid = false;
            state.catalog.clear();
            state.invalidation += 1;
            state.invalidation
        };
        let mut names = HashSet::new();
        let mut cursors = HashSet::new();
        let mut next = IndexMap::new();
        let mut cursor: Option<String> = None;
        let mut bytes = 0;
        let mut pages = 0;
        loop {
            pages += 1;
            if pages > 64 {
                return Err(RuntimeError::plain("MCP catalog has too many pages"));
            }
            let page = self.endpoint.list(cursor.as_deref(), signal.clone()).await?;
            signal.check()?;
            {
                let state = self.state.borrow();
                if state.closed || invalidation != state.invalidation {
                    return Err(RuntimeError::plain("MCP catalog changed during enumeration; refresh again"));
                }
            }
            bytes += stringify(&serde_json::to_value(&page).expect("catalog serializes")).len();
            if bytes > MAX_CATALOG_BYTES || names.len() + page.tools.len() > 512 {
                return Err(RuntimeError::plain("MCP catalog exceeds the bounded size"));
            }
            for tool in page.tools {
                if !names.insert(tool.name.clone()) {
                    return Err(RuntimeError::plain("MCP catalog contains a duplicate tool name"));
                }
                if self.allowed(&tool.name) {
                    next.insert(tool.name.clone(), tool);
                }
            }
            cursor = page.next_cursor;
            let Some(cursor) = &cursor else { break };
            if cursor.is_empty() || utf16_len(cursor) > 4096 || !cursors.insert(cursor.clone()) {
                return Err(RuntimeError::plain("MCP catalog cursor repeated or invalid"));
            }
        }
        // Keep the signature stable when a catalog advertises the same tools in another order.
        let mut sorted: Vec<_> = next.values().collect();
        sorted.sort_by(|left, right| left.name.cmp(&right.name));
        let signature = stringify(&serde_json::to_value(sorted).expect("catalog serializes"));
        let mut state = self.state.borrow_mut();
        if signature != state.signature {
            state.signature = signature;
            state.revision += 1;
        }
        state.catalog = next;
        state.valid = true;
        Ok(())
    }

    /// Host calls retain their outcome even when the catalog changes while Live is answering.
    pub async fn call(&self, name: &str, args: JsonObject, signal: Signal, options: CallOptions) -> Result<CallToolResult, RuntimeError> {
        signal.check()?;
        if !self.allowed(name) {
            return Err(RuntimeError::plain("Tool is not in Kumi's allowed tool list"));
        }
        if self.state.borrow().closed {
            return Err(RuntimeError::plain("MCP catalog is closed"));
        }
        if !options.host && !self.is_valid() {
            return Err(RuntimeError::plain("MCP catalog is invalid; refresh before calling tools"));
        }
        if !options.host && !self.state.borrow().catalog.contains_key(name) {
            return Err(RuntimeError::plain("Tool is not currently available or permitted"));
        }
        if stringify(&Value::Object(args.clone())).len() > if options.host { MAX_HOST_RESULT_BYTES } else { 16 * 1024 } {
            return Err(RuntimeError::plain("Tool arguments are too large; narrow the request"));
        }
        let invalidation = self.state.borrow().invalidation;
        let big = name == "live_discover" && args.get("limit").and_then(Value::as_f64).is_some_and(|limit| limit > SMALL_PAGE as f64);
        let small_args = || {
            let mut limited = args.clone();
            limited.insert("limit".into(), json!(SMALL_PAGE));
            limited
        };
        timing::live_request();
        let mut result = self
            .endpoint
            .call(name, if big && self.state.borrow().small_pages { small_args() } else { args.clone() }, signal.clone())
            .await?;
        if big && !self.state.borrow().small_pages && result.is_error == Some(true) {
            timing::live_request();
            let retried = self.endpoint.call(name, small_args(), signal.clone()).await?;
            if retried.is_error != Some(true) {
                self.state.borrow_mut().small_pages = true;
                result = retried;
            }
        }
        signal.check()?;
        if !options.host && (!self.is_valid() || invalidation != self.state.borrow().invalidation) {
            return Err(RuntimeError::plain("MCP catalog changed during the call; result discarded"));
        }
        if stringify(&serde_json::to_value(&result).expect("tool result serializes")).len()
            > if options.host { MAX_HOST_RESULT_BYTES } else { MAX_RESULT_BYTES }
        {
            return Ok(serde_json::from_value(json!({"isError":true,"content":[{"type":"text","text":"Result too large; narrow fields/parent/page instead of requesting a whole Set dump."}]})).expect("tool result"));
        }
        Ok(result)
    }

    pub fn close(&self) -> Pending {
        if let Some(closing) = self.state.borrow().closing.clone() {
            return closing;
        }
        let listeners = {
            let mut state = self.state.borrow_mut();
            state.closed = true;
            state.valid = false;
            state.catalog.clear();
            state.invalidation += 1;
            std::mem::take(&mut state.unlisten)
        };
        for remove in listeners {
            remove();
        }
        let endpoint = self.endpoint.clone();
        let (send, receive) = oneshot::channel();
        tokio::task::spawn_local(async move {
            let _ = send.send(endpoint.close().await);
        });
        let pending = async move { receive.await.unwrap_or(Ok(())) }.boxed_local().shared();
        self.state.borrow_mut().closing = Some(pending.clone());
        pending
    }
}
