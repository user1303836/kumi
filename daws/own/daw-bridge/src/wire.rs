//! The frames: what each line on the wire may be.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const PROTOCOL: &str = "kumi-daw/1";
/// The largest id or sequence number: JavaScript's and Lua's doubles keep integers exact up to here.
pub const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;
/// How often a bridge sends a heartbeat by default.
pub const HEARTBEAT_MS: u64 = 1000;
/// Heartbeats missed before the host calls a bridge busy (a dialog, a render, a stuck script): not gone, which only a
/// closed connection means.
pub const HEARTBEATS_BEFORE_BUSY: u64 = 3;
/// How long a bridge waits from accepting a connection to its welcome.
pub const HANDSHAKE_TIMEOUT_MS: u64 = 3000;
/// How long the host waits for the handshake.
pub const CLIENT_HANDSHAKE_TIMEOUT_MS: u64 = 5000;
/// The deepest a frame may nest, its own object counting as the first level. One limit for every reader and writer:
/// serde_json's own (128) and the Lua tool's JSON used to differ, so a frame one side wrote the other took for garbage
/// and closed the connection.
pub const MAX_JSON_DEPTH: usize = 64;
/// The same limit for a value that sits in a frame's field (params, a result, an event's data).
pub const MAX_PAYLOAD_DEPTH: usize = MAX_JSON_DEPTH - 1;
/// The credit a bridge that uses flow control starts with (its welcome's `creditBytes`): a floor the host's first grant
/// tops up to what its receive buffer holds, then grants back what it reads. A bridge cannot know that buffer, and a
/// fixed guess larger than a paused host's would fill the socket and block a bridge that must not block.
pub const CREDIT_FLOOR_BYTES: u64 = 64 * 1024;
/// No answer from a bridge that pages (the Renoise tool's reads) is longer than this. It may be longer than the credit
/// at that moment: the bridge sends it as the host grants.
pub const MAX_PAGE_BYTES: usize = 256 * 1024;
/// Connections a bridge holds at once, before and after they authenticate.
pub const MAX_UNAUTHENTICATED: usize = 4;
pub const MAX_AUTHENTICATED: usize = 4;

/// One line on the wire. Unknown types parse as [`Frame::Unknown`], which after the handshake is ignored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Frame {
    Hello {
        protocol: String,
        nonce: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client: Option<String>,
    },
    Challenge {
        nonce: String,
        proof: String,
    },
    Auth {
        proof: String,
    },
    Welcome(Box<Welcome>),
    Refused {
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        protocol: Option<String>,
    },
    Request(Request),
    Response(Response),
    Event(Event),
    Heartbeat {
        ms: u64,
    },
    /// Host to bridge: the host has read this many more bytes, so the bridge may send that many more (flow control; see
    /// [`CREDIT_FLOOR_BYTES`]). Only a bridge that names `creditBytes` in its welcome sends within credit.
    Credit {
        bytes: u64,
    },
    Bye {
        reason: String,
    },
    #[serde(other)]
    Unknown,
}

/// Why a bridge refused a connection before it authenticated.
pub mod refusal {
    /// The hello named another protocol; the refusal names the bridge's.
    pub const PROTOCOL: &str = "protocol";
    /// The host's proof was wrong.
    pub const AUTH: &str = "auth";
    /// A line before auth was longer than 1024 bytes.
    pub const LIMIT: &str = "limit";
    /// The handshake took longer than 3 seconds.
    pub const TIMEOUT: &str = "timeout";
    /// A line before auth wasn't the frame that step expects.
    pub const HANDSHAKE: &str = "handshake";
    /// Sent at accept when the bridge already holds as many connections as it takes, before reading anything.
    pub const CONNECTIONS: &str = "connections";
}

/// What the bridge says about itself once the host has proved it knows the secret.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Welcome {
    /// This connection's id: 32 lowercase hex.
    pub session: String,
    #[serde(flatten)]
    pub identity: Identity,
    pub max_frame_bytes: u64,
    pub heartbeat_ms: u64,
    #[serde(default)]
    pub methods: Vec<String>,
    #[serde(default)]
    pub events: Vec<String>,
    /// A bridge that sends only within the credit the host grants (the Renoise tool) says how many bytes it may send
    /// before the host's first grant; a host that sees this grants more as it reads. Without it the bridge sends freely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit_bytes: Option<u64>,
}

/// Which bridge, in which running DAW, on which project: the welcome's, `status`'s and the discovery file's shared
/// fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    /// `reaper` or `renoise`, kept as text so another DAW's bridge is reported, not a parse error.
    pub daw: String,
    /// The bridge's own version.
    pub bridge: String,
    /// The DAW's version as it reports it (REAPER's `GetAppVersion`, Renoise's `RENOISE_VERSION`).
    pub daw_version: String,
    /// The DAW's process id; 0 when the bridge can't learn it (Renoise's Lua on Windows), and then the host checks
    /// the bridge is alive by connecting.
    pub pid: u32,
    /// REAPER's resource folder or Renoise's preferences folder: what tells several instances apart.
    pub resource: String,
    pub project: Option<Project>,
}

/// The open project or song. Any field may be missing: an unsaved project has no path.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// Renoise's song id (32 hex in the song's tool data) or a Kumi id kept in a REAPER project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default = "empty_object")]
    pub params: Value,
    /// Unix milliseconds after which the bridge must not start the work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

/// A response's error codes.
pub mod code {
    pub const UNKNOWN_METHOD: &str = "unknown-method";
    pub const BAD_PARAMS: &str = "bad-params";
    /// The DAW is loading, saving, rendering or showing a dialog: nothing changed.
    pub const BUSY: &str = "busy";
    /// The deadline passed before the work started: nothing changed.
    pub const DEADLINE: &str = "deadline";
    /// The work started and did not finish in time: it may or may not have happened.
    pub const UNCERTAIN: &str = "uncertain";
    pub const FAILED: &str = "failed";
    /// The refs or the cursor are from before the project in the tab changed: read again.
    pub const STALE: &str = "stale";
    /// The list under a cursor changed between two pages, so the page after it can't be told: read again.
    pub const CHANGED: &str = "changed";
    pub const INTERNAL: &str = "internal";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// From 1 on each connection, one more each time.
    pub seq: u64,
    pub name: String,
    #[serde(default)]
    pub data: Value,
}

/// The events every bridge sends; a DAW's own are named `reaper.…` or `renoise.…`.
pub mod event {
    /// The project opened, switched, closed or was saved under another name: data is the project or null.
    pub const PROJECT: &str = "project";
    pub const TRANSPORT: &str = "transport";
    pub const TRACKS: &str = "tracks";
    pub const SELECTION: &str = "selection";
    /// The producer undid or redid Kumi's changes in the DAW itself: data is `{descs, how: undo|redo}`.
    pub const UNDONE_IN_DAW: &str = "undone_in_daw";
}

/// What a line could not be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    NotJson(String),
    /// Nested deeper than [`MAX_JSON_DEPTH`].
    TooDeep,
    NotObject,
    Invalid(String),
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::NotJson(why) => write!(f, "not JSON: {why}"),
            WireError::TooDeep => write!(f, "nested deeper than {MAX_JSON_DEPTH} levels"),
            WireError::NotObject => f.write_str("not a JSON object"),
            WireError::Invalid(why) => write!(f, "invalid frame: {why}"),
        }
    }
}

impl std::error::Error for WireError {}

fn empty_object() -> Value {
    Value::Object(Map::new())
}

/// Whether `value` nests at most `levels` deep: a scalar is 0, an array or object is one more than its deepest member.
/// Looks with a stack of its own, so a value built in code can't overflow the thread's.
pub fn nests_within(value: &Value, levels: usize) -> bool {
    let mut pending = vec![(value, 0usize)];
    while let Some((value, depth)) = pending.pop() {
        match value {
            Value::Array(items) => pending.extend(items.iter().map(|item| (item, depth + 1))),
            Value::Object(members) => pending.extend(members.values().map(|member| (member, depth + 1))),
            _ => continue,
        }
        if depth + 1 > levels {
            return false;
        }
    }
    true
}

impl Frame {
    /// One line read from the wire (without its newline).
    pub fn parse(line: &str) -> Result<Frame, WireError> {
        let value: Value = serde_json::from_str(line).map_err(|error| {
            let why = error.to_string();
            // serde's own limit, which comes first for a line nested past 128 levels.
            if why.starts_with("recursion limit exceeded") {
                WireError::TooDeep
            } else {
                WireError::NotJson(why)
            }
        })?;
        if !nests_within(&value, MAX_JSON_DEPTH) {
            return Err(WireError::TooDeep);
        }
        if !value.is_object() {
            return Err(WireError::NotObject);
        }
        if !value["type"].is_string() {
            return Err(WireError::Invalid("no type".into()));
        }
        let frame: Frame = serde_json::from_value(value).map_err(|error| WireError::Invalid(error.to_string()))?;
        frame.check()?;
        Ok(frame)
    }

    fn check(&self) -> Result<(), WireError> {
        let safe = |name: &str, n: u64, zero: bool| {
            if n > MAX_SAFE_INTEGER || (!zero && n == 0) {
                Err(WireError::Invalid(format!("{name} out of range")))
            } else {
                Ok(())
            }
        };
        match self {
            Frame::Request(request) => {
                safe("id", request.id, false)?;
                if !request.params.is_object() {
                    return Err(WireError::Invalid("params must be an object".into()));
                }
            }
            Frame::Response(response) => {
                safe("id", response.id, false)?;
                if response.ok != response.error.is_none() {
                    return Err(WireError::Invalid("a response carries an error exactly when it isn't ok".into()));
                }
            }
            Frame::Event(event) => safe("seq", event.seq, false)?,
            Frame::Heartbeat { ms } => safe("ms", *ms, true)?,
            Frame::Credit { bytes } => safe("bytes", *bytes, false)?,
            _ => {}
        }
        Ok(())
    }

    /// The frame as one line, newline included.
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = serde_json::to_vec(self).expect("frames serialize");
        bytes.push(b'\n');
        bytes
    }

    /// A successful response; a result nested past [`MAX_PAYLOAD_DEPTH`] can't be sent, so the response says so instead.
    pub fn ok(id: u64, result: Value) -> Frame {
        if !nests_within(&result, MAX_PAYLOAD_DEPTH) {
            return Frame::error(id, code::INTERNAL, format!("could not encode the result: nested deeper than {MAX_PAYLOAD_DEPTH} levels"));
        }
        Frame::Response(Response { id, ok: true, result: Some(result), error: None })
    }

    pub fn error(id: u64, code: &str, message: impl Into<String>) -> Frame {
        Frame::Response(Response { id, ok: false, result: None, error: Some(ErrorBody { code: code.into(), message: message.into() }) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn frames_round_trip_and_unknown_types_are_kept_apart() {
        let request = Frame::Request(Request { id: 7, method: "status".into(), params: json!({}), deadline_ms: Some(5) });
        let line = String::from_utf8(request.encode()).unwrap();
        assert_eq!(line, "{\"type\":\"request\",\"id\":7,\"method\":\"status\",\"params\":{},\"deadlineMs\":5}\n");
        assert_eq!(Frame::parse(line.trim_end()).unwrap(), request);
        assert_eq!(
            Frame::parse(r#"{"type":"request","id":1,"method":"ping"}"#).unwrap(),
            Frame::Request(Request { id: 1, method: "ping".into(), params: json!({}), deadline_ms: None })
        );
        assert_eq!(Frame::parse(r#"{"type":"later","x":1}"#).unwrap(), Frame::Unknown);
        assert_eq!(Frame::parse(r#"{"type":"heartbeat","ms":0,"extra":true}"#).unwrap(), Frame::Heartbeat { ms: 0 });
    }

    #[test]
    fn malformed_frames_are_refused() {
        assert!(matches!(Frame::parse("[1]"), Err(WireError::NotObject)));
        assert!(matches!(Frame::parse("{"), Err(WireError::NotJson(_))));
        assert!(matches!(Frame::parse(r#"{"id":1}"#), Err(WireError::Invalid(_))));
        assert!(Frame::parse(r#"{"type":"request","id":0,"method":"ping"}"#).is_err());
        assert!(Frame::parse(r#"{"type":"request","id":9007199254740992,"method":"ping"}"#).is_err());
        assert!(Frame::parse(r#"{"type":"request","id":1.5,"method":"ping"}"#).is_err());
        assert!(Frame::parse(r#"{"type":"request","id":1,"method":"ping","params":[]}"#).is_err());
        assert!(Frame::parse(r#"{"type":"response","id":1,"ok":false}"#).is_err());
        assert!(Frame::parse(r#"{"type":"event","seq":0,"name":"project"}"#).is_err());
    }

    fn nested(levels: usize) -> Value {
        (1..levels).fold(json!([]), |inner, _| json!([inner]))
    }

    #[test]
    fn nesting_is_limited_to_one_depth_everywhere() {
        assert!(nests_within(&json!(1), 0));
        assert!(nests_within(&json!({"a": [1, {"b": []}]}), 4));
        assert!(!nests_within(&json!({"a": [1, {"b": []}]}), 3));
        assert!(nests_within(&nested(MAX_PAYLOAD_DEPTH), MAX_PAYLOAD_DEPTH));
        assert!(!nests_within(&nested(MAX_PAYLOAD_DEPTH + 1), MAX_PAYLOAD_DEPTH));
        // A result at the limit is sent; one level more is answered with an error, not dropped or sent.
        assert!(matches!(Frame::ok(1, nested(MAX_PAYLOAD_DEPTH)), Frame::Response(Response { ok: true, .. })));
        let Frame::Response(refused) = Frame::ok(1, nested(MAX_PAYLOAD_DEPTH + 1)) else { panic!() };
        assert_eq!((refused.ok, refused.error.map(|e| e.code)), (false, Some(code::INTERNAL.into())));
        // The line a frame becomes parses back at the limit and not past it, up to and past serde's own limit.
        let line = |levels: usize| {
            format!("{{\"type\":\"event\",\"seq\":1,\"name\":\"x\",\"data\":{}{}}}", "[".repeat(levels), "]".repeat(levels))
        };
        assert!(Frame::parse(&line(MAX_PAYLOAD_DEPTH)).is_ok());
        for levels in [MAX_PAYLOAD_DEPTH + 1, 100, 127, 128, 129, 5000] {
            assert_eq!(Frame::parse(&line(levels)), Err(WireError::TooDeep), "{levels} levels");
        }
        // Brackets alone, with nothing closing them, are too deep too, and not a panic or a stack overflow.
        assert_eq!(Frame::parse(&format!("{{\"type\":{}", "[".repeat(100_000))), Err(WireError::TooDeep));
    }

    #[test]
    fn welcome_flattens_the_identity() {
        let welcome = Frame::Welcome(Box::new(Welcome {
            session: "0".repeat(32),
            identity: Identity {
                daw: "renoise".into(),
                bridge: "0.1.0".into(),
                daw_version: "3.5.4".into(),
                pid: 42,
                resource: "/prefs/V3.5.4".into(),
                project: Some(Project { id: Some("a".repeat(32)), name: Some("x.xrns".into()), ..Project::default() }),
            },
            max_frame_bytes: 64,
            heartbeat_ms: 1000,
            methods: vec!["status".into()],
            events: vec![],
            credit_bytes: None,
        }));
        let value: Value = serde_json::from_slice(&welcome.encode()).unwrap();
        assert_eq!(value["daw"], "renoise");
        assert_eq!(value["dawVersion"], "3.5.4");
        assert_eq!(value["project"], json!({"id":"a".repeat(32),"name":"x.xrns"}));
        assert_eq!(Frame::parse(&value.to_string()).unwrap(), welcome);
    }
}
