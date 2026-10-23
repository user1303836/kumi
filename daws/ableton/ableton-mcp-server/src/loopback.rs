use std::cell::{Cell, RefCell};
use std::rc::Rc;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use kumi_common::{js::string::utf16_len, time::now_ms};
use serde_json::{json, Value};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::live::{LiveAdapter, LiveError, LiveEvent, LiveInvocation, LiveListener, LiveRef, LiveSnapshot, LiveStatus, Unsubscribe};
use crate::registry::{
    canonical_json, validate_live_operation_request, validate_live_operation_result, CanonicalError, WIRE_CANONICAL_LIMITS,
};

pub const LOOPBACK_PROTOCOL_VERSION: &str = "ableton-loopback/v1";
const MAX_NONCE_LENGTH: usize = 256;
const MAX_WIRE_BYTES: usize = 256 * 1_048_576;
pub type LoopbackRequest = Value;
pub type RemoteBridgeRequest = Value;
pub type LoopbackResponse = Value;
pub type LoopbackExchange = Rc<dyn Fn(LoopbackRequest) -> LoopbackResponse>;

fn bounded_canonical(value: &Value) -> Result<String, LiveError> {
    let encoded = canonical_json(value, &WIRE_CANONICAL_LIMITS).map_err(|error| {
        LiveError::type_error(match error {
            CanonicalError::TooDeep => "wire payload is too deeply nested",
            CanonicalError::StringTooLarge => "wire string is too large",
            CanonicalError::ArrayTooLarge => "wire array is too large",
            CanonicalError::ObjectTooLarge => "wire object is too large",
        })
    })?;
    if encoded.len() > MAX_WIRE_BYTES {
        return Err(LiveError::type_error("wire payload is too large"));
    }
    Ok(encoded)
}
fn sign(secret: &str, text: &str) -> String {
    let mut hmac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts every key length");
    hmac.update(text.as_bytes());
    URL_SAFE_NO_PAD.encode(hmac.finalize().into_bytes())
}
fn verify(secret: &str, text: &str, mac: &str) -> bool {
    sign(secret, text).as_bytes().ct_eq(mac.as_bytes()).into()
}
fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn safe_integer(value: &Value) -> Option<f64> {
    value.as_f64().filter(|v| kumi_common::js::number::is_safe_integer(*v))
}
fn signed(secret: &str, mut unsigned: Value) -> Result<Value, LiveError> {
    let mac = sign(secret, &bounded_canonical(&unsigned)?);
    unsigned["mac"] = mac.into();
    Ok(unsigned)
}
fn response(secret: &str, id: &str, ok: bool, error: Option<&str>, result: Option<Value>) -> Value {
    let mut unsigned = json!({"version":LOOPBACK_PROTOCOL_VERSION,"id":if valid_id(id) {id} else {"invalid"},"ok":ok,"bridgeEpoch":"in-process","connectionChallenge":"in-process"});
    if let Some(result) = result {
        unsigned["result"] = result;
    }
    if let Some(error) = error {
        unsigned["error"] = error.into();
    }
    signed(secret, unsigned.clone()).unwrap_or_else(|_| signed(secret, json!({"version":LOOPBACK_PROTOCOL_VERSION,"id":unsigned["id"],"ok":false,"bridgeEpoch":"in-process","connectionChallenge":"in-process","error":"response exceeds wire limits"})).expect("bounded failure response"))
}

/// Authenticated, bounded loopback transport used by Remote Script/Extension adapters.
pub struct AuthenticatedLoopback {
    adapter: Rc<dyn LiveAdapter>,
    secret: String,
    emit: Rc<dyn Fn(LoopbackResponse)>,
    last_sequence: Cell<f64>,
    auth_sequence: Cell<u64>,
    unsubscribe: RefCell<Option<Unsubscribe>>,
}
impl AuthenticatedLoopback {
    pub fn new(
        adapter: Rc<dyn LiveAdapter>,
        secret: impl Into<String>,
        emit: Option<Rc<dyn Fn(LoopbackResponse)>>,
    ) -> Result<Self, LiveError> {
        let secret = secret.into();
        if utf16_len(&secret) < 32 {
            return Err(LiveError::error("loopback secret must contain at least 32 characters"));
        }
        Ok(Self {
            adapter,
            secret,
            emit: emit.unwrap_or_else(|| Rc::new(|_| {})),
            last_sequence: Cell::new(0.0),
            auth_sequence: Cell::new(0),
            unsubscribe: RefCell::new(None),
        })
    }
    pub fn handle(&self, request: &Value) -> LoopbackResponse {
        if !Self::is_request(request) {
            return response(&self.secret, "invalid", false, Some("invalid request"), None);
        }
        let id = request["id"].as_str().unwrap();
        let mut unsigned = request.clone();
        unsigned.as_object_mut().unwrap().remove("mac");
        let nonce = request["nonce"].as_str().unwrap();
        let authenticated = request["version"] == LOOPBACK_PROTOCOL_VERSION
            && request["bridgeEpoch"] == "in-process"
            && request["connectionChallenge"] == "in-process"
            && safe_integer(&request["deadlineMs"]).is_some_and(|deadline| deadline >= now_ms() as f64)
            && valid_id(id)
            && (16..=MAX_NONCE_LENGTH).contains(&utf16_len(nonce))
            && safe_integer(&request["sequence"]).is_some_and(|sequence| sequence > self.last_sequence.get())
            && bounded_canonical(&unsigned).is_ok_and(|text| verify(&self.secret, &text, request["mac"].as_str().unwrap()));
        if !authenticated {
            return response(&self.secret, id, false, Some("authentication or replay check failed"), None);
        }
        self.last_sequence.set(request["sequence"].as_f64().unwrap());
        match self.dispatch(request) {
            Ok(result) => response(&self.secret, id, true, None, result),
            Err(_) => response(&self.secret, id, false, Some("request failed"), None),
        }
    }
    fn dispatch(&self, request: &Value) -> Result<Option<Value>, LiveError> {
        let serialize = |value| Ok(Some(value));
        match request["method"].as_str().unwrap() {
            "status" => serialize(serde_json::to_value(self.adapter.status()?).unwrap()),
            "snapshot" => serialize(serde_json::to_value(self.adapter.snapshot()?).unwrap()),
            "discover" => Err(LiveError::error("in-process discovery requires the asynchronous adapter contract")),
            "get" => {
                let reference =
                    request["ref"].as_str().filter(|reference| !reference.is_empty()).ok_or_else(|| LiveError::error("ref is required"))?;
                self.adapter.get(&LiveRef::from(reference))
            }
            "invoke" => {
                let operation = request["operation"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| LiveError::error("operation and args are required"))?;
                let args = request["args"].as_object().ok_or_else(|| LiveError::error("operation and args are required"))?;
                validate_live_operation_request(operation, &Value::Object(args.clone()))?;
                let result = self.adapter.invoke(&LiveInvocation { operation: operation.into(), args: args.clone() })?;
                validate_live_operation_result(operation, &result)?;
                Ok(Some(result))
            }
            "reconnect" => serialize(serde_json::to_value(self.adapter.reconnect()?).unwrap()),
            "subscribe" => {
                self.close();
                let id = request["id"].as_str().unwrap().to_string();
                let secret = self.secret.clone();
                let emit = self.emit.clone();
                *self.unsubscribe.borrow_mut() = Some(
                    self.adapter.subscribe(Rc::new(move |event| emit(response(&secret, &id, true, None, Some(json!({"event":event}))))))?,
                );
                Ok(Some(json!({"subscribed":true})))
            }
            _ => unreachable!(),
        }
    }
    pub fn authenticate(&self, mut request: Value) -> Result<LoopbackRequest, LiveError> {
        let object = request.as_object_mut().ok_or_else(|| LiveError::type_error("unsupported wire value"))?;
        if object.get("sequence").is_none_or(Value::is_null) {
            let next = self.auth_sequence.get() + 1;
            self.auth_sequence.set(next);
            object.insert("sequence".into(), next.into());
        }
        object.insert("bridgeEpoch".into(), "in-process".into());
        object.insert("connectionChallenge".into(), "in-process".into());
        if object.get("deadlineMs").is_none_or(Value::is_null) {
            object.insert("deadlineMs".into(), (now_ms() + 5000).into());
        }
        signed(&self.secret, request)
    }
    pub fn close(&self) {
        if let Some(unsubscribe) = self.unsubscribe.borrow_mut().take() {
            unsubscribe();
        }
    }
    fn is_request(request: &Value) -> bool {
        let Some(object) = request.as_object() else {
            return false;
        };
        object.keys().all(|key| {
            [
                "version",
                "id",
                "method",
                "ref",
                "operation",
                "args",
                "nonce",
                "sequence",
                "bridgeEpoch",
                "connectionChallenge",
                "deadlineMs",
                "mac",
            ]
            .contains(&key.as_str())
        }) && ["version", "id", "method", "nonce", "bridgeEpoch", "connectionChallenge", "mac"].iter().all(|key| request[*key].is_string())
            && request["sequence"].is_number()
            && request["deadlineMs"].is_number()
            && ["status", "snapshot", "discover", "get", "invoke", "subscribe", "reconnect"]
                .contains(&request["method"].as_str().unwrap_or(""))
    }
}

/// Client-side LiveAdapter for a trusted localhost Control Surface/Extension.
pub struct LoopbackLiveAdapter {
    secret: String,
    exchange: LoopbackExchange,
    listeners: Rc<RefCell<Vec<(u64, LiveListener)>>>,
    listener_number: Cell<u64>,
    request_number: Cell<u64>,
    event_sequence: Cell<f64>,
}
impl LoopbackLiveAdapter {
    pub fn new(secret: impl Into<String>, exchange: LoopbackExchange) -> Result<Self, LiveError> {
        let secret = secret.into();
        if utf16_len(&secret) < 32 {
            return Err(LiveError::error("loopback secret must contain at least 32 characters"));
        }
        Ok(Self {
            secret,
            exchange,
            listeners: Rc::new(RefCell::new(vec![])),
            listener_number: Cell::new(0),
            request_number: Cell::new(0),
            event_sequence: Cell::new(0.0),
        })
    }
    pub fn receive(&self, response: &LoopbackResponse) -> Result<(), LiveError> {
        let result = self.verify_response(response, None)?.unwrap_or(Value::Null);
        if !result.is_object() || !result["event"].is_object() {
            return Err(LiveError::error("invalid loopback event"));
        }
        let sequence = safe_integer(&result["event"]["sequence"])
            .filter(|s| *s > self.event_sequence.get())
            .ok_or_else(|| LiveError::error("stale loopback event"))?;
        self.event_sequence.set(sequence);
        let event: LiveEvent = serde_json::from_value(result["event"].clone()).map_err(|_| LiveError::error("invalid loopback event"))?;
        let mut last = 0;
        loop {
            let next = self.listeners.borrow().iter().find(|(id, _)| *id > last).cloned();
            let Some((id, listener)) = next else {
                break;
            };
            last = id;
            listener(&event);
        }
        Ok(())
    }
    fn request(&self, fields: Value) -> Result<Option<Value>, LiveError> {
        let next = self.request_number.get() + 1;
        self.request_number.set(next);
        let id = format!("client-{next}");
        let mut unsigned = json!({"version":LOOPBACK_PROTOCOL_VERSION,"id":id});
        unsigned.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
        unsigned["nonce"] = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 18]>()).into();
        unsigned["sequence"] = next.into();
        unsigned["bridgeEpoch"] = "in-process".into();
        unsigned["connectionChallenge"] = "in-process".into();
        unsigned["deadlineMs"] = (now_ms() + 5000).into();
        self.verify_response(&(self.exchange)(signed(&self.secret, unsigned)?), Some(&id))
    }
    fn verify_response(&self, response: &Value, expected_id: Option<&str>) -> Result<Option<Value>, LiveError> {
        if !response.is_object()
            || response["version"] != LOOPBACK_PROTOCOL_VERSION
            || response["bridgeEpoch"] != "in-process"
            || response["connectionChallenge"] != "in-process"
            || !response["id"].as_str().is_some_and(valid_id)
            || expected_id.is_some_and(|id| response["id"] != id)
            || !response["ok"].is_boolean()
            || !response["mac"].is_string()
        {
            return Err(LiveError::error("invalid loopback response"));
        }
        let mut unsigned = response.clone();
        unsigned.as_object_mut().unwrap().remove("mac");
        if !verify(&self.secret, &bounded_canonical(&unsigned)?, response["mac"].as_str().unwrap()) {
            return Err(LiveError::error("loopback response authentication failed"));
        }
        if response["ok"] == false {
            return Err(LiveError::error(response["error"].as_str().unwrap_or("loopback request failed")));
        }
        Ok(response.get("result").cloned())
    }
    fn typed<T: serde::de::DeserializeOwned>(&self, fields: Value) -> Result<T, LiveError> {
        serde_json::from_value(self.request(fields)?.unwrap_or(Value::Null)).map_err(|_| LiveError::error("invalid loopback response"))
    }
}
impl LiveAdapter for LoopbackLiveAdapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        self.typed(json!({"method":"status"}))
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        self.typed(json!({"method":"snapshot"}))
    }
    fn get(&self, reference: &LiveRef) -> Result<Option<Value>, LiveError> {
        self.request(json!({"method":"get","ref":reference}))
    }
    fn invoke(&self, invocation: &LiveInvocation) -> Result<Value, LiveError> {
        if !self.status()?.has_operation(&invocation.operation) {
            return Err(LiveError::error(format!("loopback operation is not negotiated: {}", invocation.operation)));
        }
        Ok(self.request(json!({"method":"invoke","operation":invocation.operation,"args":invocation.args}))?.unwrap_or(Value::Null))
    }
    fn subscribe(&self, listener: LiveListener) -> Result<Unsubscribe, LiveError> {
        let existing = self.listeners.borrow().iter().find(|(_, callback)| Rc::ptr_eq(callback, &listener)).map(|(id, _)| *id);
        let id = existing.unwrap_or_else(|| {
            let id = self.listener_number.get() + 1;
            self.listener_number.set(id);
            self.listeners.borrow_mut().push((id, listener));
            id
        });
        if let Err(error) = self.request(json!({"method":"subscribe"})) {
            self.listeners.borrow_mut().retain(|(key, _)| *key != id);
            return Err(error);
        }
        let listeners = self.listeners.clone();
        Ok(Box::new(move || listeners.borrow_mut().retain(|(key, _)| *key != id)))
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.typed(json!({"method":"reconnect"}))
    }
}
pub type AuthenticatedLoopbackClient = LoopbackLiveAdapter;
pub fn status_from_adapter(adapter: &dyn LiveAdapter) -> Result<LiveStatus, LiveError> {
    adapter.status()
}
