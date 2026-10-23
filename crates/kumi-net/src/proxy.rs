//! Which proxy a request of Kumi's goes through, if any:
//! - HTTPS_PROXY, HTTP_PROXY or ALL_PROXY (each upper case first, as before), with NO_PROXY's exceptions; a socks5://
//!   proxy too.
//! - Without them, the system's own settings, with their exceptions: macOS's network proxies and Windows' Internet
//!   settings (Linux has only the variables). Auto-config files and discovery aren't read: `auto_config` says when the
//!   system uses one, so an error can say to set HTTPS_PROXY.
//! - Never for this computer itself (localhost, 127.0.0.0/8, ::1). With the system's settings, nor for a private
//!   network's address or a .local name, which Kumi reached directly before. A model server the producer names goes by
//!   the variables alone ([`environment`]), as before: it's on their computer or network, by address or by name.
//! - A system's proxy in the way is passed by: one that wants a sign-in Kumi can't give (NTLM, Kerberos, a password the
//!   system keeps), that isn't answering (a crashed ClashX, an office's proxy set on a laptop at home), or that shows a
//!   certificate this computer doesn't trust (one that inspects secure connections, its authority not installed).
//!   After its 407, its refusal, its name not found or its certificate refused, requests go directly while the system
//!   names it, as they did before Kumi read the settings ([`crate::send`]).

use std::net::IpAddr;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use url::{Host, Url};

use kumi_types::system::{process_env, Env};

/// How long the system's settings are kept before they're read again: a laptop moves between networks.
const FRESH: Duration = Duration::from_secs(30);

/// Where proxy settings come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// HTTPS_PROXY, HTTP_PROXY or ALL_PROXY.
    Environment,
    /// The system's settings.
    System,
}

/// The proxies Kumi's requests go through, and where they don't.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// For http:// addresses.
    pub http: Option<Url>,
    /// For https:// addresses.
    pub https: Option<Url>,
    pub source: Source,
    /// The system finds its proxy with an auto-config file or by discovery, which Kumi doesn't read.
    pub auto_config: bool,
    bypass: Bypass,
}

impl Settings {
    /// No proxy for any address.
    pub fn none() -> Self {
        Self { http: None, https: None, source: Source::Environment, auto_config: false, bypass: Bypass::default() }
    }

    /// The environment's: None when its variables name no proxy, so the system's settings say.
    pub fn from_env(env: &Env) -> Option<Self> {
        // Run as a CGI script, HTTP_PROXY could come from a request's own Proxy header: no proxy, as reqwest had it.
        if env.contains_key("REQUEST_METHOD") {
            return Some(Self::none());
        }
        let all = variable(env, "ALL_PROXY");
        let http = variable(env, "HTTP_PROXY").or(all);
        let https = variable(env, "HTTPS_PROXY").or(all);
        if http.is_none() && https.is_none() {
            return None;
        }
        Some(Self {
            http: http.and_then(proxy_address),
            https: https.and_then(proxy_address),
            source: Source::Environment,
            auto_config: false,
            bypass: Bypass::list(variable(env, "NO_PROXY").unwrap_or(""), ','),
        })
    }

    /// The system's, as read, with NO_PROXY's exceptions besides its own.
    fn from_system(found: Found, no_proxy: &str) -> Self {
        let socks = found.socks.as_deref().and_then(proxy_address);
        let mut bypass = Bypass::list(no_proxy, ',');
        found.exceptions.iter().for_each(|exception| bypass.add(exception));
        bypass.simple |= found.simple;
        Self {
            http: found.http.as_deref().and_then(proxy_address).or_else(|| socks.clone()),
            https: found.https.as_deref().and_then(proxy_address).or(socks),
            source: Source::System,
            auto_config: found.auto_config,
            bypass,
        }
    }

    /// The system's settings with `proxy` for every address and nothing excepted, or none and an auto-config file.
    #[cfg(any(test, feature = "test-support"))]
    pub fn system_for_tests(proxy: Option<&str>, auto_config: bool) -> Self {
        let proxy = proxy.map(str::to_string);
        Self::from_system(Found { http: proxy.clone(), https: proxy, auto_config, ..Found::default() }, "")
    }

    /// The proxy for a request to `url`, or None to go directly.
    pub fn proxy_for(&self, url: &Url) -> Option<Url> {
        let proxy = self.named_for(url)?;
        let passed = self.source == Source::System && passed_by().as_ref() == Some(&proxy);
        (!passed).then_some(proxy)
    }

    /// The proxy the settings name for `url`, passed by or not.
    fn named_for(&self, url: &Url) -> Option<Url> {
        let host = url.host()?;
        if this_computer(&host) || (self.source == Source::System && this_network(&host)) || self.bypass.covers(&host) {
            return None;
        }
        match url.scheme() {
            "https" | "wss" => self.https.clone(),
            "http" | "ws" => self.http.clone(),
            _ => None,
        }
    }
}

/// The system's proxy that was in the way: passed by while the system names it.
static PASSED_BY: Mutex<Option<Url>> = Mutex::new(None);

/// Tests that pass a proxy by take turns: the one passed by is the whole process's.
#[cfg(any(test, feature = "test-support"))]
pub static PASSING_BY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn passed_by() -> std::sync::MutexGuard<'static, Option<Url>> {
    PASSED_BY.lock().unwrap_or_else(PoisonError::into_inner)
}

/// After the proxy `settings` name for `url` was in the way ([`in_the_way`]): when it's the system's, it's passed by
/// from now on (another request may have passed it by already), and true says to try once directly. The variables'
/// proxies are the producer's own choice, to sign in to (HTTPS_PROXY=http://user:pass@host:port) or start, so they're
/// kept.
pub fn pass_by(settings: &Settings, url: &Url) -> bool {
    let Some(proxy) = settings.named_for(url).filter(|_| settings.source == Source::System) else { return false };
    *passed_by() = Some(proxy);
    true
}

/// Whether a failed request's proxy, rather than the server, was in the way: it wants a sign-in Kumi can't give, isn't
/// answering (its address refused, its name not found), or showed a certificate this computer doesn't trust. A timeout
/// says none of these: the network may just be slow. What reached Kumi directly says nothing of a proxy, so
/// [`pass_by`] decides from the settings whether there was one.
pub fn in_the_way(error: &reqwest::Error) -> bool {
    let chain = crate::cause::chain(error);
    !chain.contains("timed out") && (chain.contains("proxy authorization required") || not_answering(&chain) || certificate_refused(&chain))
}

/// A certificate the check refused (an unknown issuer, say), as rustls words it, rather than any other TLS failure: an
/// alert, a protocol neither side speaks, or the system's check running out of time.
pub fn certificate_refused(chain: &str) -> bool {
    chain.contains("invalid peer certificate")
}

/// A proxy that isn't answering, as what a request kept of the failure says: its address refused, or its name not
/// found.
pub fn not_answering(chain: &str) -> bool {
    static WORDS: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)connection refused|actively refused|dns error|failed to lookup address|no such host is known|name or service not known|nodename nor servname")
            .expect("regex")
    });
    WORDS.is_match(chain)
}

/// Going directly failed too: the system's proxy is used again, and its 407 is what's said.
pub fn stop_passing_by() {
    *passed_by() = None;
}

/// The environment's proxy settings, read once.
struct Environment {
    proxies: Option<Arc<Settings>>,
    no_proxy: String,
}

static ENVIRONMENT: LazyLock<Environment> = LazyLock::new(|| {
    let env = process_env();
    Environment { proxies: Settings::from_env(&env).map(Arc::new), no_proxy: variable(&env, "NO_PROXY").unwrap_or("").to_string() }
});

/// The variables' settings alone, for a model server the producer names (Ollama, LM Studio, settings.json's
/// modelServers, a listening model's address): none when they name no proxy.
pub fn environment() -> Arc<Settings> {
    static NONE: LazyLock<Arc<Settings>> = LazyLock::new(|| Arc::new(Settings::none()));
    Arc::clone(ENVIRONMENT.proxies.as_ref().unwrap_or(&NONE))
}

/// The settings in force: the environment's (read once), or else the system's (read again after a while).
pub fn current() -> Arc<Settings> {
    static SYSTEM: Mutex<Option<(Instant, Arc<Settings>)>> = Mutex::new(None);
    if let Some(proxies) = &ENVIRONMENT.proxies {
        return Arc::clone(proxies);
    }
    let mut held = SYSTEM.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((read, settings)) = &*held {
        if read.elapsed() < FRESH {
            return Arc::clone(settings);
        }
    }
    let settings = Arc::new(Settings::from_system(read_system(), &ENVIRONMENT.no_proxy));
    *held = Some((Instant::now(), Arc::clone(&settings)));
    settings
}

/// A proxy variable by its upper-case name, else its lower-case one (on Windows, in any case); one set but empty counts
/// as unset.
fn variable<'a>(env: &'a Env, name: &str) -> Option<&'a str> {
    let set = |value: &'a String| Some(value.trim()).filter(|value| !value.is_empty());
    let lower = name.to_lowercase();
    env.get(name)
        .and_then(set)
        .or_else(|| env.get(&lower).and_then(set))
        .or_else(|| env.iter().filter(|(key, _)| cfg!(windows) && key.eq_ignore_ascii_case(name)).find_map(|(_, value)| set(value)))
}

/// A proxy's address as a variable or the system gives it ("proxy.corp:8080" is http://), with a scheme reqwest uses.
fn proxy_address(value: &str) -> Option<Url> {
    let value = value.trim();
    let url = if value.contains("://") { Url::parse(value) } else { Url::parse(&format!("http://{value}")) }.ok()?;
    (matches!(url.scheme(), "http" | "https" | "socks4" | "socks4a" | "socks5" | "socks5h") && url.host().is_some()).then_some(url)
}

/// This computer itself: never through a proxy.
fn this_computer(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => {
            let name = name.trim_end_matches('.');
            name == "localhost" || name.ends_with(".localhost")
        }
        Host::Ipv4(address) => address.is_loopback() || address.is_unspecified(),
        Host::Ipv6(address) => {
            address.is_loopback() || address.is_unspecified() || address.to_ipv4_mapped().is_some_and(|mapped| mapped.is_loopback())
        }
    }
}

/// Where a system's proxy to the internet doesn't lead: a private network's address, or a name on this network alone.
fn this_network(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => {
            let name = name.trim_end_matches('.');
            name == "local" || name.ends_with(".local")
        }
        Host::Ipv4(address) => crate::web::private_address(&address.to_string()),
        Host::Ipv6(address) => crate::web::private_address(&address.to_string()),
    }
}

/// Where no proxy is used: NO_PROXY's list, or the system's exceptions. An entry takes in the names under it.
#[derive(Debug, Clone, Default, PartialEq)]
struct Bypass {
    /// "*": everywhere.
    all: bool,
    /// Names without a dot ("intranet"): Windows' <local>, macOS's "Exclude simple hostnames".
    simple: bool,
    /// Names, lower case ("corp.example" takes in "wiki.corp.example").
    names: Vec<String>,
    /// Names with a * inside ("*.corp.*", "intra*").
    patterns: Vec<String>,
    networks: Vec<Network>,
}

impl Bypass {
    fn list(entries: &str, separator: char) -> Self {
        let mut bypass = Self::default();
        entries.split(separator).for_each(|entry| bypass.add(entry));
        bypass
    }

    fn add(&mut self, entry: &str) {
        let entry = entry.trim().to_ascii_lowercase();
        let entry = entry.trim_end_matches('.');
        if entry == "*" {
            self.all = true;
        } else if entry == "<local>" {
            self.simple = true;
        } else if let Some(network) = Network::parse(entry) {
            self.networks.push(network);
        } else {
            // A port says nothing here: "wiki.corp:8080" is wiki.corp.
            let name = entry.rsplit_once(':').filter(|(_, port)| port.parse::<u16>().is_ok()).map_or(entry, |(name, _)| name);
            let name = name.strip_prefix("*.").or_else(|| name.strip_prefix('.')).unwrap_or(name);
            if name.contains('*') {
                self.patterns.push(name.to_string());
            } else if !name.is_empty() {
                self.names.push(name.to_string());
            }
        }
    }

    fn covers(&self, host: &Host<&str>) -> bool {
        let text = match host {
            Host::Domain(name) => name.trim_end_matches('.').to_ascii_lowercase(),
            Host::Ipv4(address) => address.to_string(),
            Host::Ipv6(address) => address.to_string(),
        };
        let address = match host {
            Host::Domain(_) => None,
            Host::Ipv4(address) => Some(IpAddr::V4(*address)),
            Host::Ipv6(address) => Some(IpAddr::V6(*address)),
        };
        self.all
            || (self.simple && address.is_none() && !text.contains('.'))
            || address.is_some_and(|address| self.networks.iter().any(|network| network.contains(address)))
            || self.names.iter().any(|name| text == *name || text.strip_suffix(name.as_str()).is_some_and(|under| under.ends_with('.')))
            || self.patterns.iter().any(|pattern| wildcard(pattern, &text))
    }
}

/// An address, or a network of them.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Network {
    address: IpAddr,
    bits: u32,
}

impl Network {
    /// "10.0.0.0/8", "::1" or "[::1]", macOS's short "169.254/16", and Windows' "10.*" or "192.168.*.*".
    fn parse(entry: &str) -> Option<Self> {
        let entry = entry.strip_prefix('[').and_then(|inner| inner.split_once(']')).map_or(entry, |(inner, _)| inner);
        let width = |address: &IpAddr| if address.is_ipv4() { 32 } else { 128 };
        if let Some((address, bits)) = entry.split_once('/') {
            let address = short_ipv4(address).or_else(|| address.parse().ok())?;
            let bits: u32 = bits.parse().ok()?;
            return (bits <= width(&address)).then_some(Self { address, bits });
        }
        if let Ok(address) = entry.parse::<IpAddr>() {
            return Some(Self { address, bits: width(&address) });
        }
        let parts: Vec<&str> = entry.split('.').collect();
        let known = parts.iter().position(|part| *part == "*")?;
        if known == 0 || parts.len() > 4 || parts[known..].iter().any(|part| *part != "*") {
            return None;
        }
        let mut octets = [0u8; 4];
        for (octet, part) in octets.iter_mut().zip(&parts[..known]) {
            *octet = part.parse().ok()?;
        }
        Some(Self { address: IpAddr::from(octets), bits: known as u32 * 8 })
    }

    fn contains(&self, address: IpAddr) -> bool {
        let (network, address, width) = match (self.address, address) {
            (IpAddr::V4(network), IpAddr::V4(address)) => (u32::from(network) as u128, u32::from(address) as u128, 32),
            (IpAddr::V6(network), IpAddr::V6(address)) => (u128::from(network), u128::from(address), 128),
            _ => return false,
        };
        (network ^ address).checked_shr(width - self.bits).unwrap_or(0) == 0
    }
}

/// macOS writes a network's address short: 169.254 for 169.254.0.0.
fn short_ipv4(address: &str) -> Option<IpAddr> {
    let parts = address.split('.').count();
    if !(1..4).contains(&parts) || !address.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    format!("{address}{}", ".0".repeat(4 - parts)).parse().ok()
}

/// Whether `text` fits `pattern`, where each * stands for any run of characters.
fn wildcard(pattern: &str, text: &str) -> bool {
    let (pattern, text) = (pattern.as_bytes(), text.as_bytes());
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == b'*' {
            star = Some((p, t));
            p += 1;
        } else if p < pattern.len() && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if let Some((at, from)) = star {
            p = at + 1;
            t = from + 1;
            star = Some((at, from + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&byte| byte == b'*')
}

/// What a system's proxy settings say, as read.
#[derive(Debug, Default)]
struct Found {
    http: Option<String>,
    https: Option<String>,
    socks: Option<String>,
    exceptions: Vec<String>,
    simple: bool,
    auto_config: bool,
}

/// macOS's proxies for the network in use, as System Settings › Network › Proxies has them.
#[cfg(target_os = "macos")]
fn read_system() -> Found {
    use std::ffi::{c_char, c_void, CStr};
    #[link(name = "SystemConfiguration", kind = "framework")]
    unsafe extern "C" {
        fn SCDynamicStoreCopyProxies(store: *const c_void) -> *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithBytes(allocator: *const c_void, bytes: *const u8, length: isize, encoding: u32, external: u8)
            -> *const c_void;
        fn CFDictionaryGetValue(dictionary: *const c_void, key: *const c_void) -> *const c_void;
        fn CFGetTypeID(object: *const c_void) -> usize;
        fn CFNumberGetTypeID() -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFArrayGetTypeID() -> usize;
        fn CFNumberGetValue(number: *const c_void, kind: isize, value: *mut c_void) -> u8;
        fn CFStringGetLength(string: *const c_void) -> isize;
        fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
        fn CFStringGetCString(string: *const c_void, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
        fn CFArrayGetCount(array: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
        fn CFRelease(object: *const c_void);
    }
    // CoreFoundation's Boolean is an unsigned char: read as a byte, not a Rust bool.
    const UTF8: u32 = 0x0800_0100;
    const SINT64: isize = 4;
    // SAFETY: each value is read only after its type is checked; the copied dictionary and the keys made here are
    // the only objects owned, and each is released once.
    unsafe {
        let proxies = SCDynamicStoreCopyProxies(std::ptr::null());
        if proxies.is_null() {
            return Found::default();
        }
        let value = |name: &str| {
            let key = CFStringCreateWithBytes(std::ptr::null(), name.as_ptr(), name.len() as isize, UTF8, 0);
            if key.is_null() {
                return std::ptr::null();
            }
            let value = CFDictionaryGetValue(proxies, key);
            CFRelease(key);
            value
        };
        let number = |name: &str| {
            let (found, mut read) = (value(name), 0i64);
            (!found.is_null()
                && CFGetTypeID(found) == CFNumberGetTypeID()
                && CFNumberGetValue(found, SINT64, (&mut read as *mut i64).cast()) != 0)
                .then_some(read)
        };
        let text = |string: *const c_void| {
            if string.is_null() || CFGetTypeID(string) != CFStringGetTypeID() {
                return None;
            }
            let size = CFStringGetMaximumSizeForEncoding(CFStringGetLength(string), UTF8) + 1;
            let mut buffer = vec![0 as c_char; size.max(1) as usize];
            (CFStringGetCString(string, buffer.as_mut_ptr(), size, UTF8) != 0)
                .then(|| CStr::from_ptr(buffer.as_ptr()).to_string_lossy().into_owned())
        };
        let server = |kind: &str, scheme: &str| {
            let host = (number(&format!("{kind}Enable")) == Some(1)).then(|| text(value(&format!("{kind}Proxy")))).flatten()?;
            let host = host.trim();
            (!host.is_empty()).then(|| match number(&format!("{kind}Port")) {
                Some(port) if port > 0 => format!("{scheme}://{host}:{port}"),
                _ => format!("{scheme}://{host}"),
            })
        };
        let list = value("ExceptionsList");
        let exceptions = if !list.is_null() && CFGetTypeID(list) == CFArrayGetTypeID() {
            (0..CFArrayGetCount(list)).filter_map(|index| text(CFArrayGetValueAtIndex(list, index))).collect()
        } else {
            Vec::new()
        };
        let found = Found {
            http: server("HTTP", "http"),
            // The secure web proxy is asked to connect, over plain http, as any proxy for https addresses is.
            https: server("HTTPS", "http"),
            socks: server("SOCKS", "socks5h"),
            exceptions,
            simple: number("ExcludeSimpleHostnames") == Some(1),
            auto_config: number("ProxyAutoConfigEnable") == Some(1) || number("ProxyAutoDiscoveryEnable") == Some(1),
        };
        CFRelease(proxies);
        found
    }
}

/// Windows' proxy, as Settings › Network & internet › Proxy keeps it for this user.
#[cfg(windows)]
fn read_system() -> Found {
    use std::ffi::c_void;
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegGetValueW(
            key: isize,
            subkey: *const u16,
            value: *const u16,
            flags: u32,
            kind: *mut u32,
            data: *mut c_void,
            size: *mut u32,
        ) -> i32;
    }
    const HKEY_CURRENT_USER: isize = 0x8000_0001_u32 as i32 as isize;
    const RRF_RT_REG_SZ: u32 = 0x2;
    const RRF_RT_REG_DWORD: u32 = 0x10;
    let wide = |text: &str| text.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let settings = wide(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings");
    let number = |name: &str| {
        let name = wide(name);
        let (mut read, mut size) = (0u32, std::mem::size_of::<u32>() as u32);
        // SAFETY: both names end in NUL, and `read` has the room `size` says for the DWORD asked for.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                settings.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut read as *mut u32).cast(),
                &mut size,
            )
        };
        (status == 0).then_some(read)
    };
    let text = |name: &str| {
        let name = wide(name);
        let mut size = 0u32;
        // SAFETY: without a buffer, the call only says the size the value needs, in bytes.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                settings.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if status != 0 {
            return None;
        }
        let mut buffer = vec![0u16; size as usize / 2 + 1];
        size = (buffer.len() * 2) as u32;
        // SAFETY: the buffer has the `size` bytes of room it's said to have.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                settings.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        let end = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
        (status == 0).then(|| String::from_utf16_lossy(&buffer[..end]))
    };
    let mut found = Found { auto_config: text("AutoConfigURL").is_some_and(|address| !address.trim().is_empty()), ..Found::default() };
    if number("ProxyEnable") == Some(1) {
        if let Some(servers) = text("ProxyServer") {
            (found.http, found.https, found.socks) = windows_servers(&servers);
        }
        found.exceptions = text("ProxyOverride").map(|exceptions| exceptions.split(';').map(str::to_string).collect()).unwrap_or_default();
    }
    found
}

/// Linux keeps no proxy setting of its own for programs to read: the variables are all there is.
#[cfg(not(any(target_os = "macos", windows)))]
fn read_system() -> Found {
    Found::default()
}

/// Windows' ProxyServer: "proxy:8080" for every protocol, or "http=proxy:8080;https=proxy:8443;socks=proxy:1080".
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn windows_servers(servers: &str) -> (Option<String>, Option<String>, Option<String>) {
    let address = |scheme: &str, address: &str| {
        let address = address.trim();
        (!address.is_empty()).then(|| if address.contains("://") { address.to_string() } else { format!("{scheme}://{address}") })
    };
    if !servers.contains('=') {
        let every = address("http", servers.trim_end_matches(';'));
        return (every.clone(), every, None);
    }
    let (mut http, mut https, mut socks) = (None, None, None);
    for part in servers.split(';') {
        let Some((protocol, at)) = part.split_once('=') else { continue };
        match protocol.trim().to_ascii_lowercase().as_str() {
            "http" => http = address("http", at),
            "https" => https = address("http", at),
            // Windows speaks SOCKS 4 to it, and the proxy looks names up.
            "socks" => socks = address("socks4a", at),
            _ => {}
        }
    }
    (http, https, socks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Env {
        pairs.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect()
    }

    fn through(settings: &Settings, address: &str) -> Option<String> {
        settings.proxy_for(&Url::parse(address).unwrap()).map(|proxy| proxy.to_string())
    }

    #[test]
    fn the_variables_name_a_proxy_for_each_scheme_upper_case_first() {
        assert_eq!(Settings::from_env(&env(&[])), None, "none set: the system's settings say");
        assert_eq!(Settings::from_env(&env(&[("HTTPS_PROXY", "  "), ("NO_PROXY", "corp")])), None, "an empty one is unset");
        let lower = Settings::from_env(&env(&[("HTTPS_PROXY", ""), ("https_proxy", "http://lower:2")])).unwrap();
        assert_eq!(through(&lower, "https://github.com/"), Some("http://lower:2/".into()), "an empty upper case one gives way");
        let set =
            Settings::from_env(&env(&[("HTTPS_PROXY", "http://upper:1"), ("https_proxy", "http://lower:2"), ("http_proxy", "cache:3128")]))
                .unwrap();
        assert_eq!(set.source, Source::Environment);
        assert_eq!(through(&set, "https://api.openai.com/v1"), Some("http://upper:1/".into()));
        assert_eq!(through(&set, "http://example.com/"), Some("http://cache:3128/".into()), "no scheme: http://");
        let all = Settings::from_env(&env(&[("all_proxy", "socks5h://127.0.0.1:7890"), ("HTTP_PROXY", "http://web:80")])).unwrap();
        assert_eq!(through(&all, "https://github.com/"), Some("socks5h://127.0.0.1:7890".into()), "ALL_PROXY when there's no HTTPS_PROXY");
        assert_eq!(through(&all, "http://github.com/"), Some("http://web/".into()));
        let unusable = Settings::from_env(&env(&[("HTTPS_PROXY", "ftp://files:21")])).unwrap();
        assert_eq!(through(&unusable, "https://github.com/"), None, "a scheme Kumi can't use: directly, as before");
        let cgi = Settings::from_env(&env(&[("REQUEST_METHOD", "GET"), ("HTTP_PROXY", "http://injected:80")])).unwrap();
        assert_eq!(through(&cgi, "http://example.com/"), None);
    }

    #[test]
    fn no_proxy_takes_in_names_under_it_addresses_networks_and_everything() {
        let set = Settings::from_env(&env(&[
            ("HTTPS_PROXY", "http://proxy:8080"),
            ("NO_PROXY", "corp.example, .internal,*.lan, wiki.team:8443, 10.0.0.0/8, 192.168.1.7, [fd00::1], intra*"),
        ]))
        .unwrap();
        for direct in [
            "https://corp.example/",
            "https://wiki.corp.example/",
            "https://WIKI.Corp.Example./",
            "https://internal/",
            "https://a.b.internal/",
            "https://nas.lan/",
            "https://wiki.team/",
            "https://10.20.30.40/",
            "https://192.168.1.7/",
            "https://[fd00::1]/",
            "https://intranet-files/",
        ] {
            assert_eq!(through(&set, direct), None, "{direct}");
        }
        for proxied in
            ["https://notcorp.example/", "https://corp.example.com/", "https://192.168.1.8/", "https://11.0.0.1/", "https://plan/"]
        {
            assert_eq!(through(&set, proxied), Some("http://proxy:8080/".into()), "{proxied}");
        }
        let everything = Settings::from_env(&env(&[("HTTPS_PROXY", "http://proxy:8080"), ("no_proxy", "*")])).unwrap();
        assert_eq!(through(&everything, "https://github.com/"), None);
    }

    #[test]
    fn this_computer_is_never_reached_through_a_proxy() {
        let set = Settings::from_env(&env(&[("ALL_PROXY", "http://proxy:8080")])).unwrap();
        for local in [
            "http://127.0.0.1:11434/api/tags",
            "http://localhost:1234/v1",
            "http://LOCALHOST./",
            "http://ollama.localhost/",
            "http://127.8.9.10/",
            "http://[::1]:8080/",
            "http://[::ffff:127.0.0.1]/",
            "http://0.0.0.0:11434/",
        ] {
            assert_eq!(through(&set, local), None, "{local}");
        }
        assert_eq!(through(&set, "http://192.168.1.20:11434/"), Some("http://proxy:8080/".into()), "the variables decide the rest");
    }

    #[test]
    fn the_systems_settings_keep_its_exceptions_and_leave_private_networks_direct() {
        let found = Found {
            http: Some("http://proxy.corp:3128".into()),
            https: Some("http://proxy.corp:3129".into()),
            exceptions: vec!["*.local".into(), "169.254/16".into(), "*.corp.example".into(), "10.*".into()],
            simple: true,
            ..Found::default()
        };
        let set = Settings::from_system(found, "skip.example");
        assert_eq!(set.source, Source::System);
        assert_eq!(through(&set, "https://api.anthropic.com/v1/messages"), Some("http://proxy.corp:3129/".into()));
        assert_eq!(through(&set, "http://example.com/"), Some("http://proxy.corp:3128/".into()));
        for direct in [
            "https://wiki.corp.example/",
            "https://intranet/",
            "https://studio.local/",
            "https://skip.example/",
            "https://169.254.10.1/",
            "https://10.1.2.3/",
            // A private network's address, with no exception for it: Kumi reached it directly before.
            "http://192.168.1.20:11434/",
            "http://[fd12::5]/",
        ] {
            assert_eq!(through(&set, direct), None, "{direct}");
        }
        let socks = Settings::from_system(Found { socks: Some("socks5h://127.0.0.1:1080".into()), ..Found::default() }, "");
        assert_eq!(through(&socks, "https://github.com/"), Some("socks5h://127.0.0.1:1080".into()), "SOCKS stands in for both");
        let none = Settings::from_system(Found { auto_config: true, ..Found::default() }, "");
        assert!(none.auto_config && none.https.is_none());
        assert_eq!(through(&none, "https://github.com/"), None);
    }

    #[tokio::test]
    async fn a_proxy_passed_by_already_is_passed_by_again_and_only_the_systems() {
        let _turn = PASSING_BY.lock().await;
        stop_passing_by();
        let system = Settings::system_for_tests(Some("http://127.0.0.1:7890"), false);
        let url = Url::parse("https://api.openai.com/v1/responses").unwrap();
        assert!(pass_by(&system, &url));
        assert_eq!(system.proxy_for(&url), None, "passed by");
        // Another request that met it in the way before it was passed by goes directly too.
        assert!(pass_by(&system, &url));
        // Not for an address the proxy isn't for, nor for the variables' proxy.
        assert!(!pass_by(&system, &Url::parse("http://localhost:11434/").unwrap()));
        let env = env(&[("HTTPS_PROXY", "http://127.0.0.1:7890")]);
        assert!(!pass_by(&Settings::from_env(&env).unwrap(), &url));
        stop_passing_by();
        assert_eq!(system.proxy_for(&url), Some(Url::parse("http://127.0.0.1:7890").unwrap()));
    }

    #[test]
    fn a_proxy_in_the_way_is_one_that_refuses_or_isnt_found_not_one_thats_slow() {
        for said in [
            "client error (Connect): tunnel error: failed to create underlying connection: tcp connect error: Connection refused (os error 61)",
            "No connection could be made because the target machine actively refused it. (os error 10061)",
            "tunnel error: failed to create underlying connection: dns error: failed to lookup address information: nodename nor servname provided, or not known",
            "dns error: No such host is known. (os error 11001)",
        ] {
            assert!(not_answering(said), "{said}");
        }
        for said in ["operation timed out", "invalid peer certificate: UnknownIssuer", "connection closed before message completed"] {
            assert!(!not_answering(said), "{said}");
        }
    }

    #[test]
    fn this_systems_own_settings_read_as_settings() {
        // Whatever this machine has set: read through the system's own interface, every time, without failing.
        for _ in 0..3 {
            let found = read_system();
            let settings = Settings::from_system(found, "");
            assert_eq!(settings.source, Source::System);
        }
    }

    #[test]
    fn windows_writes_one_proxy_or_one_per_protocol_and_its_own_exceptions() {
        assert_eq!(windows_servers("proxy:8080"), (Some("http://proxy:8080".into()), Some("http://proxy:8080".into()), None));
        assert_eq!(
            windows_servers("http=web:80;https=secure:8443;ftp=files:21;socks=tunnel:1080"),
            (Some("http://web:80".into()), Some("http://secure:8443".into()), Some("socks4a://tunnel:1080".into()))
        );
        assert_eq!(windows_servers("https=http://secure:8443"), (None, Some("http://secure:8443".into()), None));
        let (http, https, socks) = windows_servers("proxy.corp:8080");
        let found = Found {
            http,
            https,
            socks,
            exceptions: "*.contoso.com;192.168.*;<local>".split(';').map(str::to_string).collect(),
            ..Found::default()
        };
        let set = Settings::from_system(found, "");
        for direct in ["https://portal.contoso.com/", "https://192.168.40.2/", "https://fileserver/"] {
            assert_eq!(through(&set, direct), None, "{direct}");
        }
        assert_eq!(through(&set, "https://contoso.org/"), Some("http://proxy.corp:8080/".into()));
    }

    #[test]
    fn networks_and_patterns_read_as_the_systems_write_them() {
        let contains = |entry: &str, address: &str| Network::parse(entry).unwrap().contains(address.parse().unwrap());
        assert!(contains("169.254/16", "169.254.3.4") && !contains("169.254/16", "169.255.0.1"));
        assert!(contains("10.*", "10.200.0.1") && contains("192.168.*.*", "192.168.9.9") && !contains("192.168.*", "192.169.0.1"));
        assert!(contains("0.0.0.0/0", "8.8.8.8") && contains("::/0", "2001:db8::1") && contains("fd00::/8", "fd12::1"));
        assert!(!contains("10.0.0.0/8", "::ffff:10.0.0.1"), "a family only matches its own");
        assert_eq!(Network::parse("10.*.1.*"), None);
        assert_eq!(Network::parse("192.168.0.0/33"), None);
        assert!(
            wildcard("*.corp.*", "a.corp.b") && wildcard("intra*", "intra") && wildcard("a*b*c", "aXbYbc") && !wildcard("intra*", "extra")
        );
    }
}
