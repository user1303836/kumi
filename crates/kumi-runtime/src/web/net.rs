//! Kumi's way onto the web: http and https only, and only to public addresses. A name is checked
//! before anything is sent, and again on every address it resolves to as the connection is made, so
//! a page can't point Kumi at this computer or a private network, not by a redirect either. Bodies
//! are capped, and what comes back is untrusted: the tools that read it say so.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::rc::Rc;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use async_trait::async_trait;
use encoding_rs::{Encoding, UTF_8};
use futures::future::BoxFuture;
use futures::StreamExt;
use kumi_common::abort::{Aborted, Signal};
use kumi_common::js::{number, string};
use kumi_common::time::now_ms;
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, ACCEPT_ENCODING, ACCEPT_LANGUAGE, CONTENT_TYPE, LOCATION, USER_AGENT};
use serde_json::Value;
use url::Url;

use crate::core::errors::RuntimeError;
use crate::version::KUMI_VERSION;

/// What kind of trouble a web error is, for trying elsewhere: none of it is shown as is.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WebTrouble {
    /// The service has had too many requests from here (or used up what it gives free).
    pub busy: bool,
    /// How long it asked Kumi to wait, when it said.
    pub retry_after_ms: Option<f64>,
    /// Nothing answered: no connection, no name, no answer in time.
    pub unreachable: bool,
}

impl WebTrouble {
    pub const BUSY: WebTrouble = WebTrouble { busy: true, retry_after_ms: None, unreachable: false };
    pub const UNREACHABLE: WebTrouble = WebTrouble { busy: false, retry_after_ms: None, unreachable: true };
}

/// Something Kumi couldn't read, said so the model and the producer can act on it.
#[derive(Debug, Clone, PartialEq)]
pub struct WebError {
    pub message: String,
    pub status: Option<u16>,
    pub trouble: WebTrouble,
}

impl WebError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), status: None, trouble: WebTrouble::default() }
    }

    pub fn with_status(message: impl Into<String>, status: u16) -> Self {
        Self { message: message.into(), status: Some(status), trouble: WebTrouble::default() }
    }

    pub fn with_trouble(message: impl Into<String>, status: Option<u16>, trouble: WebTrouble) -> Self {
        Self { message: message.into(), status, trouble }
    }
}

impl fmt::Display for WebError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for WebError {}

/// What a web function throws: a `WebError`, or the abort `signal.throwIfAborted()` throws, which
/// isn't the page's failure (`error instanceof WebError` is false for it).
#[derive(Debug, Clone, PartialEq)]
pub enum WebFailure {
    Web(WebError),
    Aborted,
}

impl WebFailure {
    /// `error.message`.
    pub fn message(&self) -> String {
        match self {
            Self::Web(error) => error.message.clone(),
            Self::Aborted => Aborted.to_string(),
        }
    }

    /// `error instanceof WebError ? error : undefined`.
    pub fn web(&self) -> Option<&WebError> {
        match self {
            Self::Web(error) => Some(error),
            Self::Aborted => None,
        }
    }
}

impl fmt::Display for WebFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for WebFailure {}

impl From<WebError> for WebFailure {
    fn from(error: WebError) -> Self {
        Self::Web(error)
    }
}

impl From<Aborted> for WebFailure {
    fn from(_: Aborted) -> Self {
        Self::Aborted
    }
}

impl From<WebFailure> for RuntimeError {
    fn from(failure: WebFailure) -> Self {
        match failure {
            WebFailure::Web(error) => RuntimeError::Plain(error.message),
            WebFailure::Aborted => RuntimeError::Aborted,
        }
    }
}

/// How a service says it has had too many requests: by status, or in words.
static BUSY_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)rate.?limit|too many requests|quota|slow down|out of credits|(?-u:\b)credits(?-u:\b)|(?-u:\b)429(?-u:\b)")
        .expect("regex")
});

/// `Date.parse(text)` for the dates a Retry-After carries: an HTTP date (RFC 2822), an ISO date-time, or a date.
fn date_parse(text: &str) -> Option<f64> {
    let text = string::trim(text);
    if let Ok(date) = chrono::DateTime::parse_from_rfc2822(text) {
        return Some(date.timestamp_millis() as f64);
    }
    if let Ok(date) = chrono::DateTime::parse_from_rfc3339(text) {
        return Some(date.timestamp_millis() as f64);
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis() as f64);
    }
    None
}

/// How long a service asked to be left alone: its Retry-After (seconds or a date), or a field in its answer.
pub fn retry_after(headers: &BTreeMap<String, String>, body: Option<&Value>, now: Option<f64>) -> Option<f64> {
    let now = now.unwrap_or_else(|| now_ms() as f64);
    if let Some(value) = headers.get("retry-after").filter(|value| !value.is_empty()) {
        let trimmed = string::trim(value);
        if !trimmed.is_empty() && trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
            return Some(trimmed.parse::<f64>().unwrap_or(f64::INFINITY) * 1000.0);
        }
        if let Some(at) = date_parse(value) {
            return Some((at - now).max(0.0));
        }
    }
    let field = body
        .and_then(Value::as_object)
        .and_then(|body| body.get("retry_after_seconds").filter(|v| !v.is_null()).or_else(|| body.get("retry_after")));
    match field.and_then(Value::as_f64) {
        Some(seconds) if seconds.is_finite() && seconds >= 0.0 => Some(seconds * 1000.0),
        _ => None,
    }
}

/// A service's answer that isn't what was asked for, as a WebError that says whether it's busy.
pub fn service_trouble(service: &str, response: &WebResponse) -> WebError {
    let text = String::from_utf8_lossy(&response.body[..response.body.len().min(4096)]).into_owned();
    let body: Option<Value> = serde_json::from_str(&text).ok();
    if response.status == 429 || (response.status >= 400 && BUSY_WORDS.is_match(&text)) {
        let wait = retry_after(&response.headers, body.as_ref(), None);
        return WebError::with_trouble(
            format!("{service} has had too many requests from here for now."),
            Some(response.status),
            WebTrouble { busy: true, retry_after_ms: wait, unreachable: false },
        );
    }
    WebError::with_trouble(
        format!("{service} answered {}{}.", response.status, if response.status >= 500 { " (its server had trouble)" } else { "" }),
        Some(response.status),
        if response.status >= 500 { WebTrouble::UNREACHABLE } else { WebTrouble::default() },
    )
}

/// Whether a service's own words say it has had too many requests.
pub fn busy_words(text: &str) -> bool {
    BUSY_WORDS.is_match(text)
}

/// Sites answer a browser; Kumi says who it is too.
pub static WEB_USER_AGENT: LazyLock<String> = LazyLock::new(|| {
    format!("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36 Kumi/{KUMI_VERSION}")
});

const MAX_REDIRECTS: u32 = 5;
const TIMEOUT_MS: u64 = 20_000;
pub const MAX_BYTES: usize = 5 * 1024 * 1024;

const PRIVATE_V4: [(Ipv4Addr, u32); 15] = [
    (Ipv4Addr::new(0, 0, 0, 0), 8),
    (Ipv4Addr::new(10, 0, 0, 0), 8),
    (Ipv4Addr::new(100, 64, 0, 0), 10),
    (Ipv4Addr::new(127, 0, 0, 0), 8),
    (Ipv4Addr::new(169, 254, 0, 0), 16),
    (Ipv4Addr::new(172, 16, 0, 0), 12),
    (Ipv4Addr::new(192, 0, 0, 0), 24),
    (Ipv4Addr::new(192, 0, 2, 0), 24),
    (Ipv4Addr::new(192, 88, 99, 0), 24),
    (Ipv4Addr::new(192, 168, 0, 0), 16),
    (Ipv4Addr::new(198, 18, 0, 0), 15),
    (Ipv4Addr::new(198, 51, 100, 0), 24),
    (Ipv4Addr::new(203, 0, 113, 0), 24),
    (Ipv4Addr::new(224, 0, 0, 0), 4),
    (Ipv4Addr::new(240, 0, 0, 0), 4),
];
// ::/96 holds :: and ::1; an IPv4 address written as IPv6 (::ffff:10.0.0.1) is checked as IPv4.
const PRIVATE_V6: [(Ipv6Addr, u32); 10] = [
    (Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0), 96),
    (Ipv6Addr::new(0x100, 0, 0, 0, 0, 0, 0, 0), 64),
    (Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 32),
    (Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0), 32),
    (Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0), 16),
    (Ipv6Addr::new(0x64, 0xff9b, 1, 0, 0, 0, 0, 0), 48),
    (Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0), 7),
    (Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0), 10),
    (Ipv6Addr::new(0xfec0, 0, 0, 0, 0, 0, 0, 0), 10),
    (Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0), 8),
];

fn in_v4(address: Ipv4Addr, net: Ipv4Addr, prefix: u32) -> bool {
    (u32::from(address) ^ u32::from(net)).checked_shr(32 - prefix).unwrap_or(0) == 0
}

fn in_v6(address: Ipv6Addr, net: Ipv6Addr, prefix: u32) -> bool {
    (u128::from(address) ^ u128::from(net)).checked_shr(128 - prefix).unwrap_or(0) == 0
}

/// `PRIVATE.check(address, family)`: Node's BlockList checks an IPv4-mapped IPv6 address against the
/// IPv4 rules too, and an IPv4 address, as its mapped form, against the IPv6 rules.
fn blocked(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            PRIVATE_V4.iter().any(|&(net, prefix)| in_v4(v4, net, prefix))
                || PRIVATE_V6.iter().any(|&(net, prefix)| in_v6(v4.to_ipv6_mapped(), net, prefix))
        }
        IpAddr::V6(v6) => {
            PRIVATE_V6.iter().any(|&(net, prefix)| in_v6(v6, net, prefix))
                || v6.to_ipv4_mapped().is_some_and(|v4| PRIVATE_V4.iter().any(|&(net, prefix)| in_v4(v4, net, prefix)))
        }
    }
}

/// `address.replace(/^\[|\]$/g, "")`.
fn unbracketed(address: &str) -> &str {
    let address = address.strip_prefix('[').unwrap_or(address);
    address.strip_suffix(']').unwrap_or(address)
}

/// `address.replace(/%.*$/, "")`: the address without its zone.
fn unzoned(address: &str) -> &str {
    address.split('%').next().unwrap_or(address)
}

/// `isIP(text)`: 4, 6 or 0.
fn is_ip(text: &str) -> u8 {
    if text.parse::<Ipv4Addr>().is_ok() {
        4
    } else if text.parse::<Ipv6Addr>().is_ok() {
        6
    } else {
        0
    }
}

/// JavaScript's ToInt32, for `>>` and `&` on a parsed number (NaN is 0).
fn to_int32(value: f64) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    (value.trunc().rem_euclid(4_294_967_296.0) as u32) as i32
}

/// Whether an IP address is this computer, a private network, or otherwise not somewhere public.
pub fn private_address(address: &str) -> bool {
    let bare = unzoned(unbracketed(address)).to_lowercase();
    let family = is_ip(&bare);
    if family == 4 {
        return blocked(IpAddr::V4(bare.parse().expect("an IPv4 address")));
    }
    if family != 6 {
        return true;
    }
    // NAT64 carries an IPv4 address in its last 32 bits: that address decides.
    if let Some(nat64) = bare.strip_prefix("64:ff9b::").filter(|rest| !rest.is_empty()) {
        if nat64.contains('.') {
            return private_address(nat64);
        }
        let words: Vec<f64> = nat64
            .split(':')
            .map(|word| u32::from_str_radix(if word.is_empty() { "0" } else { word }, 16).map(f64::from).unwrap_or(f64::NAN))
            .collect();
        let (high, low) =
            if words.len() >= 2 { (words[words.len() - 2], words[words.len() - 1]) } else { (0.0, words.first().copied().unwrap_or(0.0)) };
        let (high, low) = (to_int32(high), to_int32(low));
        return private_address(&format!("{}.{}.{}.{}", high >> 8, high & 255, low >> 8, low & 255));
    }
    blocked(IpAddr::V6(bare.parse().expect("an IPv6 address")))
}

/// Whether a host name is somewhere public by its name alone (an IP address, by `allow`).
pub fn public_host(hostname: &str, allow: Option<&dyn Fn(&str) -> bool>) -> bool {
    let lowered = hostname.to_lowercase();
    let host = unbracketed(&lowered);
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty() {
        return false;
    }
    if is_ip(unzoned(host)) != 0 {
        return match allow {
            Some(allow) => allow(host),
            None => !private_address(host),
        };
    }
    if host == "localhost"
        || [".localhost", ".local", ".internal", ".lan", ".home.arpa"].iter().any(|suffix| host.ends_with(suffix))
        || !host.contains('.')
    {
        return false;
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Method {
    #[default]
    Get,
    Post,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

#[derive(Clone, Default)]
pub struct WebRequest {
    pub method: Method,
    pub headers: BTreeMap<String, String>,
    pub body: Option<String>,
    pub signal: Option<Signal>,
    /// The most read; the rest isn't (truncated).
    pub max_bytes: Option<usize>,
    pub timeout_ms: Option<u64>,
    /// Whether to read a body of this type at all: a PDF, say, is handed on by its address.
    pub wants: Option<Rc<dyn Fn(&str) -> bool>>,
}

impl fmt::Debug for WebRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebRequest")
            .field("method", &self.method)
            .field("headers", &self.headers)
            .field("body", &self.body)
            .field("max_bytes", &self.max_bytes)
            .field("timeout_ms", &self.timeout_ms)
            .field("wants", &self.wants.as_ref().map(|_| "fn"))
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebResponse {
    /// Where it came from in the end, after redirects.
    pub url: String,
    pub status: u16,
    /// Header names in lower case; repeated ones joined with ", " (a first-wins few, as Node keeps them).
    pub headers: BTreeMap<String, String>,
    /// Its media type, lower case, without parameters ("text/html").
    pub content_type: String,
    /// The charset its Content-Type names, if any.
    pub charset: Option<String>,
    pub body: Vec<u8>,
    /// The body was longer than maxBytes.
    pub truncated: bool,
    /// `wants` turned the body down, so none was read.
    pub skipped: bool,
}

#[async_trait(?Send)]
pub trait WebClient {
    async fn fetch(&self, url: &str, request: WebRequest) -> Result<WebResponse, WebFailure>;
}

/// Which addresses Kumi may connect to: public ones only, except in tests.
pub type Allow = Arc<dyn Fn(&str) -> bool + Send + Sync>;
/// How names become addresses (the system's, by default): all of a name's addresses, any family.
pub type Lookup = Arc<dyn Fn(String) -> BoxFuture<'static, std::io::Result<Vec<IpAddr>>> + Send + Sync>;

#[derive(Clone, Default)]
pub struct WebClientOptions {
    /// How names become addresses (the system's, by default).
    pub lookup: Option<Lookup>,
    /// Which addresses Kumi may connect to: public ones only, except in tests.
    pub allow: Option<Allow>,
}

/// A key's random part: 16 or more letters and digits in a row, with both in it (a slug's words aren't).
const RANDOM: &str = "(?=[A-Za-z_]*[0-9])(?=[0-9_]*[A-Za-z])[A-Za-z0-9_]{16,}";
/// Keys and tokens as services issue them, each with a digit in it as real ones have. A key starts
/// the address's part it's in: "casio-sk-1-sampler" or "boss-fc-300" is a page.
static KEY_SHAPES: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    let shapes = [
        format!("sk-(?:[A-Za-z0-9]+-){{0,3}}{RANDOM}"),
        "(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{10,}".to_string(),
        "gh[pousr]_[A-Za-z0-9]{20,}".to_string(),
        "github_pat_[A-Za-z0-9_]{20,}".to_string(),
        "glpat-[A-Za-z0-9_-]{20,}".to_string(),
        "xox[abprs]-[A-Za-z0-9-]{10,}".to_string(),
        "xapp-[0-9]+-[A-Za-z0-9-]{10,}".to_string(),
        "AIza[A-Za-z0-9_-]{30,}".to_string(),
        "AKIA[A-Z0-9]{16}".to_string(),
        "ya29\\.[A-Za-z0-9_-]{20,}".to_string(),
        "(?:hf|r8|npm|gsk|exa|fal)_[A-Za-z0-9]{20,}".to_string(),
        format!("(?:tvly|pplx|pypi|fc)-(?:[A-Za-z0-9]+-){{0,2}}{RANDOM}"),
        "SG\\.[A-Za-z0-9_-]{16,}\\.[A-Za-z0-9_-]{16,}".to_string(),
        "eyJ[A-Za-z0-9_-]{10,}\\.eyJ[A-Za-z0-9_-]{10,}\\.[A-Za-z0-9_-]{10,}".to_string(),
    ];
    fancy_regex::Regex::new(&format!("(?:^|[^A-Za-z0-9_.-])(?=[A-Za-z0-9._-]*[0-9])(?:{})", shapes.join("|"))).expect("regex")
});
/// A key passed by name in a query: ?api_key=…, &access_token=….
static NAMED_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[?&#;](?:api[_-]?key|apikey|access[_-]?token|auth[_-]?token|private[_-]?token|client[_-]?secret|secret|password|passwd)=[^&#]{8,}")
        .expect("regex")
});

/// `decodeURIComponent(text)`: None where it would throw (a percent sign without two hex digits, or bytes that aren't UTF-8).
pub(crate) fn decode_uri_component(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = bytes.get(at + 1..at + 3)?;
            let byte = u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
            out.push(byte);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Whether an address carries what looks like a key or token, as written or percent-decoded.
pub fn carries_key(address: &str) -> bool {
    let decoded = decode_uri_component(address).unwrap_or_else(|| address.to_string());
    [address, decoded.as_str()].iter().any(|form| KEY_SHAPES.is_match(form).unwrap_or(false) || NAMED_KEY.is_match(form))
}

/// An address Kumi may read: http(s), no credentials in it, and public by name.
pub fn checked_url(value: &str, allow: Option<&dyn Fn(&str) -> bool>) -> Result<Url, WebError> {
    let url = Url::parse(value).map_err(|_| WebError::new(format!("That isn't a web address: {}", string::head(value, 200))))?;
    if url.scheme() != "https" && url.scheme() != "http" {
        return Err(WebError::new(format!("Kumi reads http and https addresses, not {}.", url.scheme())));
    }
    if !url.username().is_empty() || url.password().is_some_and(|password| !password.is_empty()) {
        return Err(WebError::new("Kumi doesn't send names or passwords inside an address."));
    }
    let hostname = url.host_str().unwrap_or("");
    if !public_host(hostname, allow) {
        return Err(WebError::new(format!("Kumi reads only public web addresses, not this computer or a private network ({hostname}).")));
    }
    Ok(url)
}

/// A name the guarded lookup refused (the TypeScript's `EKUMIPRIVATE`).
#[derive(Debug)]
struct PrivateName(String);

impl fmt::Display for PrivateName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PrivateName {}

/// A name the system couldn't resolve (`ENOTFOUND`, `EAI_AGAIN`).
#[derive(Debug)]
struct LookupFailed(std::io::Error);

impl fmt::Display for LookupFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for LookupFailed {}

/// The system's lookup, refusing a name any of whose addresses isn't public.
struct GuardedLookup {
    resolve: Lookup,
    allow: Allow,
}

impl reqwest::dns::Resolve for GuardedLookup {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let hostname = name.as_str().to_string();
        let resolve = Arc::clone(&self.resolve);
        let allow = Arc::clone(&self.allow);
        Box::pin(async move {
            let list = match (resolve)(hostname.clone()).await {
                Ok(list) => list,
                Err(error) => return Err(Box::new(LookupFailed(error)) as Box<dyn std::error::Error + Send + Sync>),
            };
            if list.is_empty() || list.iter().any(|address| !allow(&address.to_string())) {
                return Err(Box::new(PrivateName(format!(
                    "Kumi reads only public web addresses, and {hostname} is on this computer or a private network."
                ))) as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(list.into_iter().map(|address| SocketAddr::new(address, 0))) as reqwest::dns::Addrs)
        })
    }
}

fn status_word(status: u16) -> Option<&'static str> {
    Some(match status {
        401 => "it needs a sign-in",
        403 => "it refused Kumi",
        404 => "there's nothing at that address",
        410 => "it's gone",
        429 => "it's had too many requests; try again in a while",
        451 => "it's unavailable here",
        _ => return None,
    })
}

/// The system's language, as `Intl.DateTimeFormat().resolvedOptions().locale` named it (from LC_ALL,
/// LC_MESSAGES or LANG; "en-US" when none says).
fn locale() -> String {
    let tag = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .map(|value| value.split(['.', '@']).next().unwrap_or("").replace('_', "-"))
        .find(|value| !value.is_empty() && value != "C" && value != "POSIX")
        .unwrap_or_else(|| "en-US".to_string());
    let base = tag.split('-').next().unwrap_or("");
    if !base.is_empty() && base != tag {
        format!("{tag},{base};q=0.9,*;q=0.5")
    } else {
        format!("{tag},*;q=0.5")
    }
}

/// Why one hop failed, before it's told in Kumi's words.
enum Hop {
    Web(WebError),
    Transport(reqwest::Error),
    Other(String),
}

struct NetClient {
    http: reqwest::Client,
    allow: Allow,
    language: String,
}

static CHARSET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)charset\s*=\s*"?([A-Za-z0-9_.:-]+)"#).expect("regex"));

/// Header values as Node gives them: each byte a character.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&byte| byte as char).collect()
}

/// Node discards repeats of these headers (the first wins); the rest it joins with ", ".
const FIRST_WINS: [&str; 19] = [
    "age",
    "authorization",
    "content-length",
    "content-type",
    "etag",
    "expires",
    "from",
    "host",
    "if-modified-since",
    "if-unmodified-since",
    "last-modified",
    "location",
    "max-forwards",
    "proxy-authorization",
    "referer",
    "retry-after",
    "server",
    "user-agent",
    "set-cookie",
];

fn headers_map(headers: &HeaderMap) -> BTreeMap<String, String> {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for (name, value) in headers {
        let name = name.as_str();
        let value = latin1(value.as_bytes());
        match map.get_mut(name) {
            None => {
                map.insert(name.to_string(), value);
            }
            Some(kept) if !FIRST_WINS.contains(&name) => {
                kept.push_str(", ");
                kept.push_str(&value);
            }
            Some(_) => {}
        }
    }
    map
}

impl NetClient {
    async fn once(&self, url: &Url, request: &WebRequest, method: Method, payload: Option<&str>) -> Result<reqwest::Response, Hop> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_str(&WEB_USER_AGENT).expect("a header value"));
        headers
            .insert(ACCEPT, HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,text/plain;q=0.8,*/*;q=0.7"));
        headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_str(&self.language).map_err(|error| Hop::Other(error.to_string()))?);
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip, deflate, br"));
        for (name, value) in &request.headers {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| Hop::Other(error.to_string()))?;
            let value = HeaderValue::from_str(value).map_err(|error| Hop::Other(error.to_string()))?;
            headers.insert(name, value);
        }
        let method = match method {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
        };
        let mut builder = self.http.request(method, url.clone()).headers(headers);
        if let Some(body) = payload {
            builder = builder.body(body.to_string());
        }
        builder.send().await.map_err(Hop::Transport)
    }

    async fn body(response: reqwest::Response, max_bytes: usize) -> Result<(Vec<u8>, bool), Hop> {
        let mut stream = response.bytes_stream();
        let mut body: Vec<u8> = Vec::new();
        let mut truncated = false;
        while let Some(chunk) = stream.next().await {
            let piece = chunk.map_err(Hop::Transport)?;
            if body.len() + piece.len() > max_bytes {
                let room = max_bytes - body.len();
                body.extend_from_slice(&piece[..room]);
                truncated = true;
                break;
            }
            body.extend_from_slice(&piece);
        }
        Ok((body, truncated))
    }

    /// The hops of one fetch; `current` is the address being read, for the error's words.
    async fn hops(&self, current: Rc<RefCell<Url>>, request: WebRequest, max_bytes: usize) -> Result<WebResponse, Hop> {
        let mut method = request.method;
        let mut payload = request.body.clone();
        let mut hop = 0;
        loop {
            let url = current.borrow().clone();
            let response = self.once(&url, &request, method, payload.as_deref()).await?;
            let status = response.status().as_u16();
            let location = response.headers().get(LOCATION).map(|value| latin1(value.as_bytes())).filter(|value| !value.is_empty());
            if let Some(location) = location.filter(|_| [301, 302, 303, 307, 308].contains(&status)) {
                drop(response);
                if hop >= MAX_REDIRECTS {
                    return Err(Hop::Web(WebError::new(format!("{} sent Kumi through too many redirects.", url.host_str().unwrap_or("")))));
                }
                let next = url.join(&location).map_err(|_| Hop::Other("Invalid URL".to_string()))?;
                *current.borrow_mut() = checked_url(next.as_str(), Some(&*self.allow)).map_err(Hop::Web)?;
                // As browsers do: a 303, or a 301/302 after a POST, fetches the new address plainly.
                if status == 303 || ((status == 301 || status == 302) && method == Method::Post) {
                    method = Method::Get;
                    payload = None;
                }
                hop += 1;
                continue;
            }
            let kind = response.headers().get(CONTENT_TYPE).map(|value| latin1(value.as_bytes())).unwrap_or_default();
            let content_type = string::trim(kind.split(';').next().unwrap_or("")).to_lowercase();
            let charset = CHARSET.captures(&kind).map(|found| found[1].to_string());
            let headers = headers_map(response.headers());
            if let Some(wants) = &request.wants {
                if !wants(&content_type) {
                    drop(response);
                    return Ok(WebResponse {
                        url: url.to_string(),
                        status,
                        headers,
                        content_type,
                        charset,
                        body: Vec::new(),
                        truncated: false,
                        skipped: true,
                    });
                }
            }
            let (body, truncated) = Self::body(response, max_bytes).await?;
            return Ok(WebResponse { url: url.to_string(), status, headers, content_type, charset, body, truncated, skipped: false });
        }
    }

    /// A transport failure in Kumi's words, by what the error chain says went wrong.
    fn transport_error(host: &str, error: &reqwest::Error) -> WebError {
        let mut deepest: String = error.to_string();
        let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
        while let Some(current) = source {
            if let Some(private) = current.downcast_ref::<PrivateName>() {
                return WebError::new(private.0.clone());
            }
            if current.downcast_ref::<LookupFailed>().is_some() {
                return WebError::with_trouble(
                    format!("Kumi couldn't find {host}: check the address, or the internet connection."),
                    None,
                    WebTrouble::UNREACHABLE,
                );
            }
            if let Some(io) = current.downcast_ref::<std::io::Error>() {
                let code = match io.kind() {
                    std::io::ErrorKind::ConnectionRefused => Some("ECONNREFUSED"),
                    std::io::ErrorKind::ConnectionReset => Some("ECONNRESET"),
                    std::io::ErrorKind::BrokenPipe => Some("EPIPE"),
                    _ => None,
                };
                if let Some(code) = code {
                    return WebError::with_trouble(format!("{host} wouldn't connect ({code})."), None, WebTrouble::UNREACHABLE);
                }
                if let Some(inner) = io.get_ref() {
                    if let Some(tls) = tls_code(&inner.to_string()) {
                        return WebError::new(format!("{host}'s secure connection didn't check out ({tls}), so Kumi didn't read it."));
                    }
                }
            }
            let text = current.to_string();
            if text == "connection closed before message completed" {
                return WebError::with_trouble(format!("{host} wouldn't connect (ECONNRESET)."), None, WebTrouble::UNREACHABLE);
            }
            if let Some(tls) = tls_code(&text) {
                return WebError::new(format!("{host}'s secure connection didn't check out ({tls}), so Kumi didn't read it."));
            }
            deepest = text;
            source = current.source();
        }
        WebError::new(format!("Kumi couldn't read {host}: {}.", string::head(&deepest, 160)))
    }
}

/// A TLS failure's code, as Node named them, from what the TLS library says.
fn tls_code(text: &str) -> Option<String> {
    let detail = text.strip_prefix("invalid peer certificate: ")?;
    Some(
        match detail.split(['(', ' ']).next().unwrap_or(detail) {
            "Expired" => "CERT_HAS_EXPIRED",
            "NotValidYet" => "CERT_NOT_YET_VALID",
            "NotValidForName" | "NotValidForNameContext" => "ERR_TLS_CERT_ALTNAME_INVALID",
            "UnknownIssuer" => "UNABLE_TO_GET_ISSUER_CERT_LOCALLY",
            "Revoked" => "CERT_REVOKED",
            other => return Some(format!("ERR_TLS_CERT_{}", other.to_uppercase())),
        }
        .to_string(),
    )
}

#[async_trait(?Send)]
impl WebClient for NetClient {
    async fn fetch(&self, address: &str, request: WebRequest) -> Result<WebResponse, WebFailure> {
        let timeout_ms = request.timeout_ms.unwrap_or(TIMEOUT_MS);
        let max_bytes = request.max_bytes.unwrap_or(MAX_BYTES);
        let signal = request.signal.clone();
        let url = Rc::new(RefCell::new(checked_url(address, Some(&*self.allow))?));
        let work = self.hops(Rc::clone(&url), request, max_bytes);
        tokio::pin!(work);
        let timeout = tokio::time::sleep(Duration::from_millis(timeout_ms));
        tokio::pin!(timeout);
        let stopped = async {
            match &signal {
                Some(signal) => signal.cancelled().await,
                None => std::future::pending().await,
            }
        };
        let failed = tokio::select! {
            outcome = &mut work => match outcome {
                Ok(response) => return Ok(response),
                Err(hop) => Some(hop),
            },
            _ = &mut timeout => None,
            _ = stopped => return Err(WebFailure::Aborted),
        };
        if signal.as_ref().is_some_and(Signal::is_cancelled) {
            return Err(WebFailure::Aborted);
        }
        let host = url.borrow().host_str().unwrap_or("").to_string();
        Err(WebFailure::Web(match failed {
            None => WebError::with_trouble(
                format!("{host} didn't answer within {} seconds.", number::to_string(number::round(timeout_ms as f64 / 1000.0))),
                None,
                WebTrouble::UNREACHABLE,
            ),
            Some(Hop::Web(error)) => error,
            Some(Hop::Transport(error)) => Self::transport_error(&host, &error),
            Some(Hop::Other(message)) => WebError::new(format!("Kumi couldn't read {host}: {}.", string::head(&message, 160))),
        }))
    }
}

pub fn create_web_client(options: WebClientOptions) -> Rc<dyn WebClient> {
    let allow: Allow = options.allow.unwrap_or_else(|| Arc::new(|address: &str| !private_address(address)));
    let resolve: Lookup = options.lookup.unwrap_or_else(|| {
        Arc::new(|hostname: String| {
            Box::pin(async move {
                tokio::net::lookup_host((hostname.as_str(), 0)).await.map(|found| found.map(|address| address.ip()).collect())
            })
        })
    });
    let http = reqwest::Client::builder()
        .dns_resolver(Arc::new(GuardedLookup { resolve, allow: Arc::clone(&allow) }))
        .redirect(reqwest::redirect::Policy::none())
        .gzip(true)
        .brotli(true)
        .deflate(true)
        .http1_only()
        .no_proxy()
        .build()
        .expect("a web client");
    Rc::new(NetClient { http, allow, language: locale() })
}

/// Why a status isn't a page, in a few words ("it refused Kumi").
pub fn status_words(status: u16) -> String {
    match status_word(status) {
        Some(words) => words.to_string(),
        None if status >= 500 => format!("its server had trouble ({status})"),
        None => format!("it answered {status}"),
    }
}

static META_CHARSET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)<meta[^>]+charset\s*=\s*["']?([A-Za-z0-9_.:-]+)"#).expect("regex"));

/// A body as text, by its charset or the page's own <meta charset>, UTF-8 otherwise.
pub fn decode_text(body: &[u8], charset: Option<&str>) -> String {
    let label = charset.map(str::to_string).or_else(|| {
        let head = latin1(&body[..body.len().min(2048)]);
        META_CHARSET.captures(&head).map(|found| found[1].to_string())
    });
    let encoding = label.as_deref().and_then(|label| Encoding::for_label_no_replacement(label.as_bytes())).unwrap_or(UTF_8);
    encoding.decode_with_bom_removal(body).0.into_owned()
}

/// `String(value)` for a JSON value, as a template writes it.
pub(crate) fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(n) => kumi_common::js::json::number(n),
        Value::String(text) => text.clone(),
        Value::Array(items) => {
            items.iter().map(|item| if item.is_null() { String::new() } else { js_string(item) }).collect::<Vec<_>>().join(",")
        }
        Value::Object(_) => "[object Object]".to_string(),
    }
}
