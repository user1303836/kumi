//! Why a request got no answer, in words a producer can act on: its address couldn't be looked up
//! (DNS), the secure connection didn't check out (TLS), a proxy stood in the way, nothing answered in
//! time, or the connection failed. Setup, sign-in, updates, the doctor and model calls say it the same way.

use std::error::Error;
use std::io::ErrorKind;
use std::sync::LazyLock;

use kumi_common::js::string::{head, trim};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::ai::error::LanguageModelError;
use crate::net::proxy;
use crate::system::Env;

/// Why a request got no answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetCause {
    /// The address couldn't be looked up: no connection, or DNS that doesn't answer.
    Dns,
    /// The secure connection didn't check out: a Wi-Fi sign-in page, a network that inspects secure
    /// connections, or a wrong clock.
    Tls,
    /// A proxy refused, wanted a sign-in, or couldn't be reached.
    Proxy,
    /// Nothing answered in time.
    Timeout,
    /// The connection was refused or dropped, or there was no way there.
    Offline,
}

/// The proxy settings Kumi's HTTP reads, in the order curl names them; each upper-case spelling wins
/// over its lower-case one.
const PROXY_VARIABLES: [&str; 4] = ["HTTPS_PROXY", "ALL_PROXY", "HTTP_PROXY", "NO_PROXY"];

/// An address in an error's words ("for url (https://…)"), which says nothing of what went wrong.
static ADDRESS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\(?\b(?:https?|wss?)://[^\s)]*\)?").expect("regex"));
static OS_ERROR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"os error (\d+)").expect("regex"));
/// The words each cause shows in, as Rust's HTTP, TLS and the three systems word them, most telling first.
static WORDS: LazyLock<[(NetCause, Regex); 6]> = LazyLock::new(|| {
    let words = |pattern: &str| Regex::new(&format!("(?i){pattern}")).expect("regex");
    [
        (NetCause::Proxy, words(r"tunnel error|\bproxy\b")),
        // A connection dropped partway, though TLS is named: rustls's "peer closed connection without sending TLS
        // close_notify". A TLS handshake cut short ("tls handshake eof") is TLS's, and stays below.
        (NetCause::Offline, words(r"close_notify|unexpected eof")),
        (
            NetCause::Tls,
            words(r"certificate|received fatal alert|received corrupt message|peer is incompatible|peer misbehaved|\btls\b|\bssl\b"),
        ),
        (
            NetCause::Dns,
            words(
                r"dns error|failed to lookup address|nodename nor servname|name or service not known|no such host is known|(?:host)?name resolution|no address associated with hostname|could not resolve host|\benotfound\b|\beai_again\b",
            ),
        ),
        (NetCause::Timeout, words(r"timed out|timeout|deadline has elapsed|did not properly respond|\betimedout\b")),
        (
            NetCause::Offline,
            words(
                r"connection (?:was )?(?:refused|reset|aborted)|actively refused|forcibly closed|broken pipe|\bunreachable\b|no route to host|network is down|connection closed before message completed|end of file before message|\beconn(?:refused|reset|aborted)\b|\be(?:host|net)unreach\b|\bepipe\b",
            ),
        ),
    ]
});

impl NetCause {
    /// What a failed request's error and everything that caused it say went wrong; None when it isn't
    /// the network (a bad address, or an answer that was a refusal).
    pub fn of(error: &(dyn Error + 'static)) -> Option<Self> {
        if let Some(cause) = Self::of_text(&chain(error)) {
            return Some(cause);
        }
        let mut source = Some(error);
        while let Some(current) = source {
            if let Some(cause) = current.downcast_ref::<std::io::Error>().and_then(|io| of_kind(io.kind())) {
                return Some(cause);
            }
            source = current.source();
        }
        None
    }

    /// The same from an error's words alone: a `chain`, or what a failure kept of it.
    pub fn of_text(text: &str) -> Option<Self> {
        let text = ADDRESS.replace_all(text, "");
        if let Some((cause, _)) = WORDS.iter().find(|(_, words)| words.is_match(&text)) {
            return Some(*cause);
        }
        OS_ERROR.captures_iter(&text).filter_map(|found| found[1].parse::<i32>().ok()).find_map(|code| match code {
            // Windows' name lookups: host not found, try again, no recovery, no data.
            11001..=11004 => Some(Self::Dns),
            code => of_kind(std::io::Error::from_raw_os_error(code).kind()),
        })
    }

    /// An HTTP answer that comes from the way there rather than the server: a proxy asking for a sign-in.
    pub fn of_status(status: u16) -> Option<Self> {
        (status == 407).then_some(Self::Proxy)
    }

    /// Of a failed call through Kumi's HTTP (`ai::http`): no answer, or one that broke off.
    pub fn of_call(error: &LanguageModelError) -> Option<Self> {
        match error {
            LanguageModelError::ApiCall(call) => match call.status_code {
                // A body that broke off keeps its 200.
                None | Some(200..=299) => Self::of_text(&format!("{} {}", call.message, call.cause.as_deref().unwrap_or(""))),
                Some(status) => Self::of_status(status),
            },
            LanguageModelError::Other(message) => Self::of_text(message),
            _ => None,
        }
    }

    /// A short name for it: "DNS", "TLS", "proxy", "timeout", "offline".
    pub fn name(self) -> &'static str {
        match self {
            Self::Dns => "DNS",
            Self::Tls => "TLS",
            Self::Proxy => "proxy",
            Self::Timeout => "timeout",
            Self::Offline => "offline",
        }
    }

    /// What went wrong, to follow "couldn't reach …:".
    pub fn words(self) -> &'static str {
        match self {
            Self::Dns => "its address couldn't be looked up (DNS)",
            Self::Tls => "the secure connection didn't check out (TLS)",
            Self::Proxy => "the proxy didn't let Kumi through",
            Self::Timeout => "nothing answered in time",
            Self::Offline => "the connection failed",
        }
    }

    /// What to do about it, as a sentence.
    pub fn fix(self) -> &'static str {
        match self {
            Self::Dns => "Check that this computer is online; if it is, its DNS isn't answering, so try another network.",
            Self::Tls => {
                "A Wi-Fi sign-in page, a wrong clock, or a school or office network that inspects secure connections does this: sign in to the Wi-Fi, check the clock, or have the network's certificate installed on this computer (or name its file in SSL_CERT_FILE)."
            }
            Self::Proxy => "Check the proxy setting, or try without the proxy.",
            Self::Timeout => "Check the connection, then try again.",
            Self::Offline => "Check that this computer is online, or try another network.",
        }
    }
}

/// The kinds of io error that say what happened to a connection.
fn of_kind(kind: ErrorKind) -> Option<NetCause> {
    match kind {
        ErrorKind::TimedOut => Some(NetCause::Timeout),
        ErrorKind::ConnectionRefused
        | ErrorKind::ConnectionReset
        | ErrorKind::ConnectionAborted
        | ErrorKind::NotConnected
        | ErrorKind::BrokenPipe
        | ErrorKind::UnexpectedEof
        | ErrorKind::HostUnreachable
        | ErrorKind::NetworkUnreachable
        | ErrorKind::NetworkDown => Some(NetCause::Offline),
        _ => None,
    }
}

/// An error with what caused it, outermost first: "error sending request: client error (Connect): dns error: …".
pub fn chain(error: &(dyn Error + 'static)) -> String {
    let mut said: Vec<String> = Vec::new();
    let mut source = Some(error);
    while let Some(current) = source {
        let words = current.to_string();
        // An error often repeats its cause's words in its own.
        if !words.is_empty() && !said.last().is_some_and(|last| last.contains(&words)) {
            said.push(words);
        }
        source = current.source();
    }
    said.join(": ")
}

/// Why `host` gave no answer and what to do about it, in a sentence or two, then the proxy settings Kumi
/// went through (or where one is set).
pub fn unreachable(host: &str, cause: Option<NetCause>, env: &Env) -> String {
    unreachable_with(host, cause, env, &proxy::current())
}

/// [`unreachable`], with the system's proxy settings as `system` has them.
pub fn unreachable_with(host: &str, cause: Option<NetCause>, env: &Env, system: &proxy::Settings) -> String {
    told(host, at(host).as_ref(), cause, false, env, system)
}

/// Why a call to `host`, at `url`, got no answer, from what Kumi's HTTP kept of it: as [`unreachable`], with a proxy
/// that isn't answering told apart from one that turned Kumi away. None when it wasn't the network.
pub fn unreachable_call(host: &str, url: &str, error: &LanguageModelError, env: &Env, system: &proxy::Settings) -> Option<String> {
    let cause = NetCause::of_call(error)?;
    Some(told(host, url::Url::parse(url).ok().as_ref(), Some(cause), proxy_down(error), env, system))
}

/// A call's proxy that isn't answering (its address refused, its name not found), not one that turned Kumi away.
pub fn proxy_down(error: &LanguageModelError) -> bool {
    NetCause::of_call(error) == Some(NetCause::Proxy) && proxy::not_answering(&said(error))
}

/// What refused a call on the way, in a few words to follow "couldn't be reached:" while Kumi waits it out: a
/// certificate (the system proxy's own, when the call went through it) or a proxy (one that isn't answering told
/// apart). None for any other cause.
pub fn refused_words(error: &LanguageModelError, env: &Env, system: &proxy::Settings) -> Option<&'static str> {
    match NetCause::of_call(error)? {
        NetCause::Proxy if proxy_down(error) => Some(NOT_ANSWERING),
        NetCause::Tls if system_proxy_for(env, system, called(error).as_ref()).is_some() => Some(PROXY_CERTIFICATE),
        cause @ (NetCause::Tls | NetCause::Proxy) => Some(cause.words()),
        _ => None,
    }
}

/// A proxy that isn't answering.
const NOT_ANSWERING: &str = "the proxy isn't answering";
/// A certificate refused through the system's proxy, though going directly was tried as well: the proxy's own, from a
/// network that inspects secure connections without its authority installed here.
const PROXY_CERTIFICATE: &str = "the proxy's certificate isn't trusted on this computer";

/// The cause, its fix, then the proxy settings in force for `url`. A proxy in the way says how to get past it: started
/// again when it isn't answering; its certificate installed when this computer doesn't trust it; with its password in
/// the variable when it turned Kumi away; or not at all.
fn told(host: &str, url: Option<&url::Url>, cause: Option<NetCause>, down: bool, env: &Env, system: &proxy::Settings) -> String {
    let what = match cause {
        Some(NetCause::Proxy) if down => format!(
            "Kumi couldn't reach {host}: {NOT_ANSWERING}. Start it again, or take it out of the settings; NO_PROXY=* connects directly."
        ),
        Some(NetCause::Tls) if system_proxy_for(env, system, url).is_some() => {
            format!("Kumi couldn't reach {host}: {PROXY_CERTIFICATE}. Install it, or NO_PROXY=* connects directly.")
        }
        Some(cause) => format!("Kumi couldn't reach {host}: {}. {}", cause.words(), cause.fix()),
        None => format!("Kumi couldn't reach {host}. Check the connection, then try again."),
    };
    let around = if cause == Some(NetCause::Proxy) && !down {
        " A proxy that wants a password takes HTTPS_PROXY=http://user:pass@host:port, and NO_PROXY=* connects directly."
    } else {
        ""
    };
    format!("{what} {}{around}", proxy_words_for(env, system, url))
}

/// What a failed call said, as [`NetCause::of_call`] reads it.
fn said(error: &LanguageModelError) -> String {
    match error {
        LanguageModelError::ApiCall(call) => format!("{} {}", call.message, call.cause.as_deref().unwrap_or("")),
        LanguageModelError::Other(message) => message.clone(),
        _ => String::new(),
    }
}

/// The address a failed call went to, when it kept one.
fn called(error: &LanguageModelError) -> Option<url::Url> {
    match error {
        LanguageModelError::ApiCall(call) => url::Url::parse(&call.url).ok(),
        _ => None,
    }
}

/// A host's own address, when it's a name with a dot (auth.openai.com), not a provider's (OpenAI).
fn at(host: &str) -> Option<url::Url> {
    (host.contains('.') && !host.contains(char::is_whitespace)).then(|| url::Url::parse(&format!("https://{host}/")).ok()).flatten()
}

/// The proxy settings Kumi goes through, in a sentence; with none set, where to set one.
pub fn proxy_words(env: &Env) -> String {
    proxy_words_with(env, &proxy::current())
}

/// [`proxy_words`], with the system's proxy settings as `system` has them: without the variables, Kumi goes through
/// the system's proxy, and says when the system finds one with an auto-config file, which Kumi doesn't read.
pub fn proxy_words_with(env: &Env, system: &proxy::Settings) -> String {
    proxy_words_for(env, system, None)
}

/// [`proxy_words_with`] for a request to `url`: the system's proxy is named only when it's the one that request went
/// through (not for an exception, a local or private address, NO_PROXY=*, or one passed by).
fn proxy_words_for(env: &Env, system: &proxy::Settings, url: Option<&url::Url>) -> String {
    if proxy::Settings::from_env(env).is_some() {
        return format!("Kumi's proxy settings: {}.", proxy_variables(env).join(", "));
    }
    match system_proxy_for(env, system, url) {
        Some(through) => {
            let port = through.port().map(|port| format!(":{port}")).unwrap_or_default();
            format!("Kumi went through this computer's proxy, {}://{}{port}.", through.scheme(), through.host_str().unwrap_or(""))
        }
        None if system.source == proxy::Source::System && system.auto_config => AUTO_CONFIG.into(),
        None => "Behind a proxy? Set HTTPS_PROXY to it.".into(),
    }
}

/// The system's proxy a request to `url` went through: none when a variable names a proxy, or for an exception, a local
/// or private address, NO_PROXY=* or a proxy passed by. Without an address, one on the internet that no exception
/// names.
fn system_proxy_for(env: &Env, system: &proxy::Settings, url: Option<&url::Url>) -> Option<url::Url> {
    // NO_PROXY alone (Docker Desktop and many shells set it) names no proxy: the system's is the one in use.
    if proxy::Settings::from_env(env).is_some() || system.source != proxy::Source::System {
        return None;
    }
    let somewhere = url::Url::parse("https://kumi.invalid/").expect("an address");
    system.proxy_for(url.unwrap_or(&somewhere))
}

const AUTO_CONFIG: &str =
    "This computer finds its proxy with an auto-config (PAC) file, which Kumi doesn't read: set HTTPS_PROXY to the proxy's address.";

/// When this computer finds its proxy with an auto-config file and no variable names one, the sentence that says so:
/// a network that lets nothing out directly needs HTTPS_PROXY set.
pub fn auto_config_words(env: &Env, system: &proxy::Settings) -> Option<&'static str> {
    (proxy::Settings::from_env(env).is_none() && system.source == proxy::Source::System && system.auto_config).then_some(AUTO_CONFIG)
}

/// The proxy variables set in `env`, as NAME=value with any user name and password left out.
pub fn proxy_variables(env: &Env) -> Vec<String> {
    PROXY_VARIABLES
        .iter()
        .filter_map(|upper| {
            [upper.to_string(), upper.to_lowercase()]
                .into_iter()
                .find_map(|name| env.get(&name).map(|value| trim(value)).filter(|value| !value.is_empty()).map(|value| (name, value)))
                .map(|(name, value)| format!("{name}={}", without_credentials(value)))
        })
        .collect()
}

/// A proxy setting without its user name and password: "http://user:secret@proxy:8080" as "http://proxy:8080".
/// Everything before the value's last @ goes, so a password with a /, ? or # in it never shows either; a scheme is
/// kept only when it is one (letters, digits, + - .), never part of a password.
fn without_credentials(value: &str) -> String {
    let scheme = value
        .split_once("://")
        .map(|(scheme, _)| scheme)
        .filter(|scheme| !scheme.is_empty() && scheme.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')));
    let rest = match value.rsplit_once('@') {
        Some((_, after)) => after,
        None => scheme.map_or(value, |scheme| &value[scheme.len() + 3..]),
    };
    head(&scheme.map_or_else(|| rest.to_string(), |scheme| format!("{scheme}://{rest}")), 120)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::error::ApiCallError;
    use crate::ai::http::{Fetch, FetchInit, HttpFetch};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn each_cause_is_read_from_how_the_systems_and_libraries_word_it() {
        let cases: [(&str, Option<NetCause>); 25] = [
            // reqwest's chain, as `chain` joins it, on macOS and Linux.
            ("error sending request for url (https://auth.openai.com/oauth/token): client error (Connect): dns error: failed to lookup address information: nodename nor servname provided, or not known", Some(NetCause::Dns)),
            ("dns error: failed to lookup address information: Name or service not known", Some(NetCause::Dns)),
            ("failed to lookup address information: Temporary failure in name resolution", Some(NetCause::Dns)),
            ("No such host is known. (os error 11001)", Some(NetCause::Dns)),
            ("This is usually a temporary error during hostname resolution (os error 11002)", Some(NetCause::Dns)),
            ("curl: (6) Could not resolve host: github.com", Some(NetCause::Dns)),
            ("client error (Connect): invalid peer certificate: UnknownIssuer", Some(NetCause::Tls)),
            ("received fatal alert: HandshakeFailure", Some(NetCause::Tls)),
            ("received corrupt message of type InvalidContentType", Some(NetCause::Tls)),
            ("tls handshake eof", Some(NetCause::Tls)),
            ("client error (Connect): tunnel error: proxy authorization required", Some(NetCause::Proxy)),
            ("tunnel error: failed to create underlying connection: tcp connect error: Connection refused (os error 61)", Some(NetCause::Proxy)),
            ("error sending request: operation timed out", Some(NetCause::Timeout)),
            ("A connection attempt failed because the connected party did not properly respond after a period of time (os error 10060)", Some(NetCause::Timeout)),
            ("deadline has elapsed", Some(NetCause::Timeout)),
            ("tcp connect error: Connection refused (os error 61)", Some(NetCause::Offline)),
            ("No connection could be made because the target machine actively refused it. (os error 10061)", Some(NetCause::Offline)),
            ("An existing connection was forcibly closed by the remote host. (os error 10054)", Some(NetCause::Offline)),
            ("Network is unreachable (os error 51)", Some(NetCause::Offline)),
            ("connection closed before message completed", Some(NetCause::Offline)),
            ("error decoding response body: request or response body error: error reading a body from connection: end of file before message length reached", Some(NetCause::Offline)),
            ("ECONNRESET", Some(NetCause::Offline)),
            // The address is never read as words: a host named "proxy" or "tls" says nothing.
            ("error sending request for url (https://proxy.example.com/tls/dns-error)", None),
            ("Cannot connect to API: This operation was aborted", None),
            ("The usage limit has been reached", None),
        ];
        for (text, cause) in cases {
            assert_eq!(NetCause::of_text(text), cause, "{text}");
        }
        let timed_out = std::io::Error::new(ErrorKind::TimedOut, "slow");
        assert_eq!(NetCause::of(&timed_out), Some(NetCause::Timeout), "the kind says what the words don't");
    }

    #[test]
    fn a_failed_call_is_read_from_what_kumis_http_kept_of_it() {
        let mut offline = ApiCallError::new("Cannot connect to API: error sending request for url (https://x.test/)", "u", None, None);
        offline.cause = Some("error sending request for url (https://x.test/): client error (Connect): dns error: failed to lookup address information: nodename nor servname provided, or not known".into());
        assert_eq!(NetCause::of_call(&offline.clone().into()), Some(NetCause::Dns));
        let mut broke = ApiCallError::new("Failed to process successful response", "u", None, Some(200));
        broke.cause = Some("error decoding response body: request or response body error: operation timed out".into());
        assert_eq!(NetCause::of_call(&broke.into()), Some(NetCause::Timeout));
        assert_eq!(
            NetCause::of_call(&ApiCallError::new("Proxy Authentication Required", "u", None, Some(407)).into()),
            Some(NetCause::Proxy)
        );
        assert_eq!(NetCause::of_call(&ApiCallError::new("Not Found", "u", None, Some(404)).into()), None, "an answer, not the network");
        assert_eq!(
            NetCause::of_call(&LanguageModelError::other("error decoding response body: connection reset")),
            Some(NetCause::Offline)
        );
    }

    #[test]
    fn the_words_name_the_cause_the_fix_and_the_proxy_settings_without_secrets() {
        let none = Env::new();
        let direct = proxy::Settings::system_for_tests(None, false);
        assert_eq!(
            unreachable_with("auth.openai.com", Some(NetCause::Dns), &none, &direct),
            "Kumi couldn't reach auth.openai.com: its address couldn't be looked up (DNS). Check that this computer is online; if it is, its DNS isn't answering, so try another network. Behind a proxy? Set HTTPS_PROXY to it."
        );
        assert_eq!(
            unreachable_with("github.com", None, &none, &direct),
            "Kumi couldn't reach github.com. Check the connection, then try again. Behind a proxy? Set HTTPS_PROXY to it."
        );
        let env = Env::from([
            ("https_proxy".to_string(), "http://kumi:hunter2@proxy.corp:8080".to_string()),
            ("HTTPS_PROXY".to_string(), " http://me:secret@proxy.example:3128/ ".to_string()),
            ("no_proxy".to_string(), "localhost,.corp".to_string()),
            ("ALL_PROXY".to_string(), "".to_string()),
            ("http_proxy".to_string(), "user@cache:80".to_string()),
        ]);
        assert_eq!(proxy_variables(&env), ["HTTPS_PROXY=http://proxy.example:3128/", "http_proxy=cache:80", "no_proxy=localhost,.corp"]);
        let said = unreachable("api.anthropic.com", Some(NetCause::Proxy), &env);
        assert_eq!(
            said,
            "Kumi couldn't reach api.anthropic.com: the proxy didn't let Kumi through. Check the proxy setting, or try without the proxy. Kumi's proxy settings: HTTPS_PROXY=http://proxy.example:3128/, http_proxy=cache:80, no_proxy=localhost,.corp. A proxy that wants a password takes HTTPS_PROXY=http://user:pass@host:port, and NO_PROXY=* connects directly."
        );
        assert!(!said.contains("secret") && !said.contains("hunter2"), "{said}");
        // Without the variables, the system's own proxy, or its auto-config file that Kumi doesn't read. A certificate
        // refused through it is the proxy's own (going directly was tried too).
        let system = proxy::Settings::system_for_tests(Some("http://me:secret@proxy.corp:3128"), false);
        assert_eq!(
            unreachable_with("api.openai.com", Some(NetCause::Tls), &none, &system),
            "Kumi couldn't reach api.openai.com: the proxy's certificate isn't trusted on this computer. Install it, or NO_PROXY=* connects directly. Kumi went through this computer's proxy, http://proxy.corp:3128."
        );
        assert!(unreachable_with("api.openai.com", Some(NetCause::Tls), &none, &direct).contains("SSL_CERT_FILE"));
        assert_eq!(
            proxy_words_with(&none, &proxy::Settings::system_for_tests(None, true)),
            "This computer finds its proxy with an auto-config (PAC) file, which Kumi doesn't read: set HTTPS_PROXY to the proxy's address."
        );
        // NO_PROXY alone (Docker Desktop sets it) names no proxy: the system's is still the one said.
        let no_proxy = Env::from([("NO_PROXY".to_string(), "localhost,127.0.0.1".to_string())]);
        assert_eq!(proxy_words_with(&no_proxy, &system), "Kumi went through this computer's proxy, http://proxy.corp:3128.");
        // The system's proxy in the way: its words, and the way past it.
        let said = unreachable_with("api.openai.com", Some(NetCause::Proxy), &none, &system);
        assert!(said.ends_with("http://proxy.corp:3128. A proxy that wants a password takes HTTPS_PROXY=http://user:pass@host:port, and NO_PROXY=* connects directly."), "{said}");
        for cause in [NetCause::Dns, NetCause::Tls, NetCause::Proxy, NetCause::Timeout, NetCause::Offline] {
            assert!(cause.fix().ends_with('.') && !cause.words().ends_with('.'), "{cause:?}");
            assert_eq!(serde_json::to_value(cause).unwrap(), cause.name().to_lowercase(), "{cause:?}");
        }
    }

    #[test]
    fn a_proxy_password_never_shows_whatever_it_holds() {
        for (value, shown) in [
            ("http://me:pa/ss@proxy.example:3128", "http://proxy.example:3128"),
            ("http://me:p?a#s/s@proxy.example:3128/", "http://proxy.example:3128/"),
            ("socks5h://me:x@y@proxy.example:1080", "socks5h://proxy.example:1080"),
            ("user:pa://ss@cache:80", "cache:80"),
            ("me:pa/ss@cache:80", "cache:80"),
            ("http://proxy.example:3128", "http://proxy.example:3128"),
            ("localhost,.corp", "localhost,.corp"),
        ] {
            assert_eq!(without_credentials(value), shown, "{value}");
        }
        let env = Env::from([("HTTPS_PROXY".to_string(), "http://kumi:hunter/2?#x@proxy.example:3128".to_string())]);
        let said = proxy_words(&env);
        assert_eq!(said, "Kumi's proxy settings: HTTPS_PROXY=http://proxy.example:3128.");
        assert!(!said.contains("hunter"), "{said}");
    }

    #[test]
    fn a_connection_dropped_partway_is_the_connection_even_where_tls_is_named() {
        let close_notify = "error decoding response body: request or response body error: peer closed connection without sending TLS close_notify: https://docs.rs/rustls/latest/rustls/manual/_03_howto/index.html#unexpected-eof";
        assert_eq!(NetCause::of_text(close_notify), Some(NetCause::Offline));
        assert_eq!(NetCause::of_text("error reading a body from connection: unexpected EOF"), Some(NetCause::Offline));
        // A handshake cut short is still TLS's: a Wi-Fi sign-in page or a network inspecting connections does that.
        assert_eq!(NetCause::of_text("client error (Connect): tls handshake eof"), Some(NetCause::Tls));
        assert_eq!(NetCause::Offline.words(), "the connection failed");
    }

    #[tokio::test]
    async fn a_proxy_in_the_way_is_told_by_what_it_did_and_named_only_when_it_was_gone_through() {
        let _turn = proxy::PASSING_BY.lock().await;
        proxy::stop_passing_by();
        let none = Env::new();
        let system = proxy::Settings::system_for_tests(Some("http://127.0.0.1:7890"), false);
        let call = |cause: &str| {
            let mut call = crate::ai::error::ApiCallError::new(
                "Cannot connect to API: error sending request",
                "https://api.openai.com/v1/responses",
                None,
                None,
            );
            call.cause =
                Some(format!("error sending request for url (https://api.openai.com/v1/responses): client error (Connect): {cause}"));
            LanguageModelError::from(call)
        };
        let url = "https://api.openai.com/v1/responses";
        // Not answering: start it again, or go without it; a password wouldn't help.
        let down = call("tunnel error: failed to create underlying connection: tcp connect error: Connection refused (os error 61)");
        let said = unreachable_call("OpenAI", url, &down, &none, &system).unwrap();
        assert_eq!(
            said,
            "Kumi couldn't reach OpenAI: the proxy isn't answering. Start it again, or take it out of the settings; NO_PROXY=* connects directly. Kumi went through this computer's proxy, http://127.0.0.1:7890."
        );
        // Turned Kumi away: its password, or no proxy.
        let said = unreachable_call("OpenAI", url, &call("tunnel error: proxy authorization required"), &none, &system).unwrap();
        assert!(
            said.starts_with("Kumi couldn't reach OpenAI: the proxy didn't let Kumi through.")
                && said.contains("HTTPS_PROXY=http://user:pass@host:port"),
            "{said}"
        );
        // A certificate refused through the system's proxy, and directly too: the proxy's own, to install. Through a
        // variable's proxy or none, the network's or the server's, as before.
        let untrusted = call("invalid peer certificate: UnknownIssuer");
        assert_eq!(
            unreachable_call("OpenAI", url, &untrusted, &none, &system).unwrap(),
            "Kumi couldn't reach OpenAI: the proxy's certificate isn't trusted on this computer. Install it, or NO_PROXY=* connects directly. Kumi went through this computer's proxy, http://127.0.0.1:7890."
        );
        let variables = Env::from([("HTTPS_PROXY".to_string(), "http://127.0.0.1:7890".to_string())]);
        let direct = proxy::Settings::system_for_tests(None, false);
        for (env, settings) in [(&variables, &system), (&none, &direct)] {
            let said = unreachable_call("OpenAI", url, &untrusted, env, settings).unwrap();
            assert!(said.starts_with("Kumi couldn't reach OpenAI: the secure connection didn't check out (TLS)."), "{said}");
        }
        // The few words while Kumi waits say the same.
        assert_eq!(refused_words(&untrusted, &none, &system), Some("the proxy's certificate isn't trusted on this computer"));
        assert_eq!(refused_words(&untrusted, &variables, &system), Some("the secure connection didn't check out (TLS)"));
        assert_eq!(refused_words(&down, &none, &system), Some("the proxy isn't answering"));
        assert_eq!(
            refused_words(&call("tunnel error: proxy authorization required"), &none, &system),
            Some("the proxy didn't let Kumi through")
        );
        assert_eq!(refused_words(&call("tcp connect error: Connection refused (os error 61)"), &none, &direct), None, "offline");
        // Not the network at all.
        assert_eq!(unreachable_call("OpenAI", url, &LanguageModelError::other("expected value at line 1"), &none, &system), None);
        // The system's proxy is named only for a request that went through it.
        let behind = "Behind a proxy? Set HTTPS_PROXY to it.";
        for host in ["192.168.1.20", "studio.local", "127.0.0.1"] {
            assert!(unreachable_with(host, Some(NetCause::Offline), &none, &system).ends_with(behind), "{host}");
        }
        assert!(proxy::pass_by(&system, &url::Url::parse(url).unwrap()));
        assert!(unreachable_call("OpenAI", url, &down, &none, &system).unwrap().ends_with(behind), "passed by");
        // Passed by, a certificate refused was the server's: the words are TLS's own.
        assert!(unreachable_call("OpenAI", url, &untrusted, &none, &system).unwrap().contains("(TLS)"), "passed by");
        proxy::stop_passing_by();
        // A system that finds its proxy with an auto-config file, and no variable naming one.
        let pac = proxy::Settings::system_for_tests(None, true);
        assert!(auto_config_words(&none, &pac).is_some_and(|words| words.contains("HTTPS_PROXY")));
        assert_eq!(auto_config_words(&Env::from([("HTTPS_PROXY".to_string(), "http://proxy:80".to_string())]), &pac), None);
        assert_eq!(auto_config_words(&none, &system), None);
    }

    /// A local server that does `answer` with each connection.
    async fn server(answer: &'static [u8], close: bool) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut request = vec![0; 4096];
                    let _ = socket.read(&mut request).await;
                    let _ = socket.write_all(answer).await;
                    if !close {
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    }
                });
            }
        });
        address
    }

    /// An address nothing listens on.
    async fn closed_port() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        drop(listener);
        address
    }

    async fn failed(fetch: &HttpFetch, url: &str) -> LanguageModelError {
        match fetch.fetch(url, FetchInit::default()).await {
            Ok(response) => response.text().await.expect_err("a broken body"),
            Err(error) => error,
        }
    }

    #[tokio::test]
    async fn kumis_own_http_keeps_what_went_wrong_for_each_cause() {
        // No proxy, whatever this machine has set: each cause as the network gives it.
        let direct =
            |read| HttpFetch::through(std::sync::Arc::new(|| std::sync::Arc::new(proxy::Settings::none())), Duration::from_secs(5), read);
        let fetch = direct(Duration::from_millis(300));
        // The read timeout counts from the request on, connecting and looking the name up included, and Windows tries a
        // closed port again for a second or two before it says refused: these two wait as long as they need.
        let patient = direct(Duration::from_secs(5));
        // Refused.
        let refused = failed(&patient, &format!("http://{}/", closed_port().await)).await;
        assert_eq!(NetCause::of_call(&refused), Some(NetCause::Offline), "{refused:?}");
        // A name that never resolves (RFC 6761).
        let unknown = failed(&patient, "http://kumi-net-cause.invalid/").await;
        assert_eq!(NetCause::of_call(&unknown), Some(NetCause::Dns), "{unknown:?}");
        // Something that answers HTTPS with plain words: a captive portal, say.
        let portal = server(b"HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\n\r\n", true).await;
        let tls = failed(&fetch, &format!("https://{portal}/")).await;
        assert_eq!(NetCause::of_call(&tls), Some(NetCause::Tls), "{tls:?}");
        // A server that accepts but never answers.
        let silent = server(b"", false).await;
        let slow = failed(&fetch, &format!("http://{silent}/")).await;
        assert_eq!(NetCause::of_call(&slow), Some(NetCause::Timeout), "{slow:?}");
        // A body that stops partway, then the connection goes.
        let partial = server(b"HTTP/1.1 200 OK\r\ncontent-length: 1000\r\n\r\npartial", true).await;
        let cut = failed(&fetch, &format!("http://{partial}/")).await;
        assert!(matches!(NetCause::of_call(&cut), Some(NetCause::Offline)), "{cut:?}");
        let LanguageModelError::Other(words) = &cut else { panic!("{cut:?}") };
        assert!(words.starts_with("error decoding response body"), "the first words stay as they were: {words}");
    }

    #[tokio::test]
    async fn a_proxy_that_refuses_or_wants_a_sign_in_is_the_proxy() {
        let target = "https://kumi.example.test/";
        for proxy in [server(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n", true).await, closed_port().await] {
            let client = reqwest::Client::builder().proxy(reqwest::Proxy::https(format!("http://{proxy}")).unwrap()).build().unwrap();
            let error = client.get(target).send().await.expect_err("no way through");
            assert_eq!(NetCause::of(&error), Some(NetCause::Proxy), "{}", chain(&error));
        }
    }
}
