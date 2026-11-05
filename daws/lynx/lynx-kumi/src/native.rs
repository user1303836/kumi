//! A new authenticated connection for each native request; a write is never retried.
use crate::{Error, Result};
use daw_bridge::{auth, files::process_alive, wire::code};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    io::Read,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    time::Duration,
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
};

pub const PROTOCOL: &str = "lynx-kumi/2";
pub const MAX_MESSAGE: usize = 1024 * 1024;
const MAX_CONFIG: u64 = 16 * 1024;

#[derive(Deserialize)]
pub struct Discovery {
    pub protocol: String,
    pub host: String,
    pub port: u16,
    token: String,
    pub pid: u32,
    #[serde(rename = "appVersion", default)]
    pub app_version: String,
}

/// No symlink following, no shared credentials, no hostname lookup and no legacy fallback.
pub fn discovery(path: &Path) -> Result<Discovery> {
    if !path.is_absolute() {
        return Err(Error::bad("LYNX discovery path must be absolute"));
    }
    if !cfg!(unix) {
        return Err(Error::bad("This experimental native LYNX endpoint is available only on POSIX builds"));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|_| Error::new(code::FAILED, "LYNX native discovery is unavailable; start a compatible opt-in control build"))?;
    let meta = file.metadata().map_err(|_| Error::bad("Cannot inspect LYNX discovery"))?;
    if !meta.is_file() || meta.len() > MAX_CONFIG {
        return Err(Error::bad("Invalid LYNX discovery file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions.
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            return Err(Error::bad("LYNX discovery must belong to this user and be owner-only"));
        }
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG + 1).read_to_end(&mut bytes).map_err(|_| Error::bad("Cannot read LYNX discovery"))?;
    if bytes.len() > MAX_CONFIG as usize {
        return Err(Error::bad("LYNX discovery exceeds its bound"));
    }
    let found: Discovery = serde_json::from_slice(&bytes).map_err(|_| Error::bad("Invalid LYNX discovery JSON"))?;
    let ip: IpAddr = found.host.parse().map_err(|_| Error::bad("LYNX host must be a numeric loopback address"))?;
    if found.protocol != PROTOCOL
        || !daw_bridge::loopback::peer_allowed(&SocketAddr::new(ip, found.port))
        || found.port == 0
        || found.pid == 0
        || found.token.len() != 64
        || !found.token.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(Error::bad("Incompatible LYNX native protocol, address or credentials; authenticated protocol 2 is required"));
    }
    if !process_alive(found.pid) {
        return Err(Error::new(code::FAILED, "LYNX discovery belongs to a process that has ended"));
    }
    Ok(found)
}

#[derive(Clone)]
pub struct Client {
    pub path: PathBuf,
    expected_pid: Option<u32>,
}
impl Client {
    pub fn new(path: PathBuf) -> Self {
        Self { path, expected_pid: None }
    }
    pub fn for_pid(path: PathBuf, pid: u32) -> Self {
        Self { path, expected_pid: Some(pid) }
    }

    pub async fn call(&self, operation: &str, args: Value) -> Result<Value> {
        let found = discovery(&self.path)?;
        if self.expected_pid.is_some_and(|pid| pid != found.pid) {
            return Err(Error::new(code::STALE, "The native LYNX process changed; reconnect to its new bridge"));
        }
        let timeout = if matches!(
            operation,
            "channel.create" | "instrument.replace" | "plugin.parameters_set" | "effect.load" | "history.undo" | "history.redo"
        ) {
            Duration::from_secs(65)
        } else {
            Duration::from_secs(10)
        };
        tokio::time::timeout(timeout, self.exchange(&found, operation, args)).await.map_err(|_| {
            Error::new(code::UNCERTAIN, "LYNX did not answer in time; a sent change may have run. Read the state before retrying")
        })?
    }

    async fn exchange(&self, found: &Discovery, operation: &str, args: Value) -> Result<Value> {
        let ip = found.host.parse().map_err(|_| Error::bad("Invalid LYNX address"))?;
        let stream = TcpStream::connect(SocketAddr::new(ip, found.port))
            .await
            .map_err(|_| Error::new(code::FAILED, "LYNX native control connection failed"))?;
        let mut stream = BufReader::new(stream);
        let client_nonce = auth::random_hex16();
        send(&mut stream, &json!({"type":"hello","protocol":PROTOCOL,"nonce":client_nonce})).await?;
        let challenge = line(&mut stream, 1024, 8).await?;
        let server_nonce = challenge["nonce"]
            .as_str()
            .filter(|n| auth::valid_nonce(n))
            .ok_or_else(|| Error::new(code::FAILED, "LYNX did not supply a valid server challenge"))?;
        let received = challenge["proof"].as_str().unwrap_or("");
        let expected = proof(&found.token, "server", &client_nonce, server_nonce);
        if challenge["type"] != "challenge"
            || challenge["protocol"] != PROTOCOL
            || received.len() != expected.len()
            || !bool::from(expected.as_bytes().ct_eq(received.as_bytes()))
        {
            return Err(Error::new(code::FAILED, "LYNX server proof failed; no client proof or request was sent"));
        }
        send(&mut stream, &json!({"type":"auth","proof":proof(&found.token,"client",&client_nonce,server_nonce)})).await?;
        let ready = line(&mut stream, 1024, 8).await?;
        if ready["type"] != "ready" || ready["protocol"] != PROTOCOL {
            return Err(Error::new(code::FAILED, "LYNX native authentication was refused"));
        }
        let request = json!({"op":operation,"args":args});
        if request.to_string().len() > MAX_MESSAGE {
            return Err(Error::bad("LYNX native request exceeds 1 MiB"));
        }
        send(&mut stream, &request)
            .await
            .map_err(|_| Error::new(code::UNCERTAIN, "LYNX request failed; a change may have run. Inspect before retrying"))?;
        let result = line(&mut stream, MAX_MESSAGE, 64).await.map_err(|_| {
            Error::new(code::UNCERTAIN, "LYNX response was lost or invalid; a change may have run. Inspect before retrying")
        })?;
        match result["ok"].as_bool() {
            Some(true) => Ok(result.get("result").cloned().unwrap_or(Value::Null)),
            Some(false) => {
                let message = result["error"].as_str().unwrap_or("operation refused").replace(&found.token, "[redacted]");
                Err(Error::new(
                    code::FAILED,
                    format!("LYNX: {}", message.chars().filter(|c| !c.is_control()).take(1000).collect::<String>()),
                ))
            }
            None => Err(Error::new(code::UNCERTAIN, "Invalid LYNX response envelope; inspect before retrying")),
        }
    }
}

pub fn proof(token: &str, role: &str, client: &str, server: &str) -> String {
    auth::hmac_sha256_hex(token.as_bytes(), format!("{PROTOCOL}\n{role}\n{client}\n{server}").as_bytes())
}

async fn send(stream: &mut BufReader<TcpStream>, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| Error::bad("Cannot encode native request"))?;
    bytes.push(b'\n');
    stream.get_mut().write_all(&bytes).await.map_err(|_| Error::new(code::FAILED, "Native connection write failed"))
}

async fn line(stream: &mut BufReader<TcpStream>, limit: usize, depth: usize) -> Result<Value> {
    let mut bytes = Vec::new();
    stream
        .take(limit as u64 + 2)
        .read_until(b'\n', &mut bytes)
        .await
        .map_err(|_| Error::new(code::FAILED, "Native connection read failed"))?;
    if bytes.len() > limit + 1 || bytes.last() != Some(&b'\n') {
        return Err(Error::bad("Native response exceeded its line bound"));
    }
    depth_bound(&bytes, depth)?;
    serde_json::from_slice(&bytes).map_err(|_| Error::bad("Invalid native response JSON"))
}

/// Reject nesting before serde allocates or descends. Escapes inside strings are not structure.
fn depth_bound(bytes: &[u8], maximum: usize) -> Result<()> {
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for &b in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                quoted = false;
            }
        } else if b == b'"' {
            quoted = true;
        } else if b == b'{' || b == b'[' {
            depth += 1;
            if depth > maximum {
                return Err(Error::bad("Native JSON nesting exceeded its bound"));
            }
        } else if b == b'}' || b == b']' {
            depth = depth.saturating_sub(1);
        }
    }
    Ok(())
}
