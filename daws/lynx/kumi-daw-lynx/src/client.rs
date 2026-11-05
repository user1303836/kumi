//! A fresh authenticated loopback connection per call. Writes are never retried.
use kumi_common::abort::{Signal, SignalExt};
use kumi_types::{errors::RuntimeError, system::Env};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    io::Read,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
};

const MAX_CONFIG: u64 = 16 * 1024;
pub const MAX_MESSAGE: usize = 4 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
struct Discovery {
    protocol: String,
    host: String,
    port: u16,
    token: String,
    pid: u32,
}

pub fn config_path(env: &Env) -> PathBuf {
    env.get("KUMI_LYNX_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(kumi_daw::io::kumi_home(env)).join("bridges/lynx/connection.json"))
}

fn read_discovery(path: &Path) -> Result<Discovery, RuntimeError> {
    if !path.is_absolute() {
        return Err(RuntimeError::plain("LYNX discovery path must be absolute"));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file =
        options.open(path).map_err(|_| RuntimeError::plain("LYNX control discovery is unavailable; start a native-control LYNX build"))?;
    let meta = file.metadata().map_err(|_| RuntimeError::plain("Cannot inspect LYNX discovery"))?;
    if !meta.is_file() || meta.len() > MAX_CONFIG {
        return Err(RuntimeError::plain("Invalid LYNX discovery file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions.
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            return Err(RuntimeError::plain("LYNX discovery must belong to this user and have mode 0600"));
        }
    }
    let mut bytes = Vec::new();
    std::io::Read::take(file, MAX_CONFIG + 1).read_to_end(&mut bytes).map_err(|_| RuntimeError::plain("Cannot read LYNX discovery"))?;
    if bytes.len() > MAX_CONFIG as usize {
        return Err(RuntimeError::plain("LYNX discovery is too large"));
    }
    let config: Discovery = serde_json::from_slice(&bytes).map_err(|_| RuntimeError::plain("Invalid LYNX discovery JSON"))?;
    let address: IpAddr = config.host.parse().map_err(|_| RuntimeError::plain("LYNX host must be a numeric loopback address"))?;
    if config.protocol != "lynx-kumi/1"
        || !address.is_loopback()
        || config.port == 0
        || config.pid == 0
        || !(32..=1024).contains(&config.token.len())
    {
        return Err(RuntimeError::plain("Invalid LYNX control protocol, address or credentials"));
    }
    Ok(config)
}

#[derive(Clone)]
pub struct Client {
    pub path: PathBuf,
}

impl Client {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
    pub async fn call(&self, op: &str, args: Value, signal: &Signal) -> Result<Value, RuntimeError> {
        signal.check()?;
        let config = read_discovery(&self.path)?;
        let ip: IpAddr = config.host.parse().map_err(|_| RuntimeError::plain("Invalid LYNX address"))?;
        let mut request = serde_json::to_vec(&json!({"token": config.token, "op": op, "args": args}))
            .map_err(|_| RuntimeError::plain("Invalid LYNX request"))?;
        if request.len() > MAX_MESSAGE {
            return Err(RuntimeError::plain("LYNX request is too large"));
        }
        request.push(b'\n');
        let exchange = async {
            let mut stream = TcpStream::connect(SocketAddr::new(ip, config.port))
                .await
                .map_err(|_| RuntimeError::plain("LYNX control connection failed"))?;
            stream
                .write_all(&request)
                .await
                .map_err(|_| RuntimeError::plain("LYNX request failed; a change may have run. Read the state before retrying"))?;
            let mut line = Vec::new();
            BufReader::new(stream)
                .take(MAX_MESSAGE as u64 + 1)
                .read_until(b'\n', &mut line)
                .await
                .map_err(|_| RuntimeError::plain("LYNX response failed; a change may have run. Read the state before retrying"))?;
            if line.len() > MAX_MESSAGE || line.last() != Some(&b'\n') {
                return Err(RuntimeError::plain("LYNX response exceeded its bound or was incomplete"));
            }
            let result: Value = serde_json::from_slice(&line).map_err(|_| RuntimeError::plain("LYNX returned invalid JSON"))?;
            match result.get("ok").and_then(Value::as_bool) {
                Some(true) => Ok(result.get("result").cloned().unwrap_or(Value::Null)),
                Some(false) => {
                    let error = result.get("error").and_then(Value::as_str).unwrap_or("operation refused");
                    let safe = error.replace(&config.token, "[redacted]");
                    Err(RuntimeError::plain(format!("LYNX: {}", safe.chars().take(1000).filter(|c| !c.is_control()).collect::<String>())))
                }
                None => Err(RuntimeError::plain("LYNX returned an invalid response envelope")),
            }
        };
        let timeout = if matches!(op, "plugin.parameters_set" | "effect.load" | "history.undo" | "history.redo") {
            Duration::from_secs(65)
        } else {
            TIMEOUT
        };
        tokio::select! {
            result = tokio::time::timeout(timeout, exchange) => result.map_err(|_| RuntimeError::plain("LYNX did not answer in time; a change may have run. Read the state before retrying"))?,
            _ = signal.cancelled() => Err(RuntimeError::plain("LYNX call cancelled; a change already sent may have run. Read the state before retrying")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, Write};
    fn config(host: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "{}", json!({"protocol":"lynx-kumi/1","host":host,"port":12345,"token":"a".repeat(64),"pid":1})).unwrap();
        f
    }
    #[test]
    fn discovery_rejects_remote_hosts_and_unprotected_credentials() {
        for host in ["192.0.2.1", "localhost", "127.0.0.1.example.com"] {
            assert!(read_discovery(config(host).path()).is_err());
        }
        let f = config("127.0.0.1");
        assert!(read_discovery(f.path()).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(read_discovery(f.path()).is_err());
        }
    }
    #[tokio::test]
    async fn sends_authenticated_mutation_once_and_reports_refusal() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut f = config("127.0.0.1");
        f.as_file_mut().set_len(0).unwrap();
        f.rewind().unwrap();
        write!(
            f,
            "{}",
            json!({"protocol":"lynx-kumi/1","host":"127.0.0.1","port":listener.local_addr().unwrap().port(),"token":"a".repeat(64),"pid":1})
        )
        .unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let req: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(req["op"], "tempo.set");
            assert_eq!(req["args"]["bpm"], 125);
            assert_eq!(req["token"], "a".repeat(64));
            reader.get_mut().write_all(b"{\"ok\":false,\"error\":\"tempo refused\"}\n").await.unwrap();
        });
        let error = Client::new(f.path().into()).call("tempo.set", json!({"bpm":125}), &Signal::new()).await.err().unwrap();
        assert!(error.to_string().contains("tempo refused"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn response_is_bounded_before_json_parsing() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut f = config("127.0.0.1");
        f.as_file_mut().set_len(0).unwrap();
        f.rewind().unwrap();
        write!(
            f,
            "{}",
            json!({"protocol":"lynx-kumi/1","host":"127.0.0.1","port":listener.local_addr().unwrap().port(),"token":"a".repeat(64),"pid":1})
        )
        .unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut request = String::new();
            reader.read_line(&mut request).await.unwrap();
            let _ = reader.get_mut().write_all(&vec![b'x'; MAX_MESSAGE + 1]).await;
        });
        let error = Client::new(f.path().into()).call("session.inspect", json!({}), &Signal::new()).await.err().unwrap();
        assert!(error.to_string().contains("bound"));
        server.await.unwrap();
    }
}
