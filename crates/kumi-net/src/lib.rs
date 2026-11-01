//! How Kumi's HTTP clients are made, in one place, so they all trust certificates and go through proxies alike
//! (sign-in, updates, model calls, downloads). Each client begins at [`client`] and adds its own timeouts, redirects
//! and user agent. [`model_servers`] is for a model server the producer names, [`direct`] for the web tool's guarded
//! lookups, which a proxy would get around.

pub mod cause;
pub mod error;
pub mod proxy;
mod trust;
pub mod web;

use std::sync::Arc;

/// The beginning of every HTTP client Kumi makes: its trust (the certificates Kumi carries, then this computer's own)
/// and its proxies (the environment's or the system's, never for this computer itself).
pub fn client() -> reqwest::ClientBuilder {
    client_through(proxy::current)
}

/// [`client`] for a model server the producer names (Ollama, LM Studio, settings.json's modelServers, a listening
/// model's address): on their computer or network, by address or by a name (nas.lan, a Tailscale name), which a
/// system's proxy to the internet doesn't lead to. The variables' proxies alone, as before.
pub fn model_servers() -> reqwest::ClientBuilder {
    client_through(proxy::environment)
}

/// [`client`] with proxies from `settings` (asked as each connection is made) instead of the environment's or the
/// system's.
pub fn client_through(settings: impl Fn() -> Arc<proxy::Settings> + Send + Sync + 'static) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .use_preconfigured_tls(trust::config(false))
        .proxy(reqwest::Proxy::custom(move |url| settings().proxy_for(url)))
}

/// Sends a request of a [`client`]'s, and once more directly when the system's proxy was in the way: it wants a sign-in
/// Kumi can't give, isn't answering, or showed a certificate this computer doesn't trust. Kumi went directly then before
/// it read the system's settings, so a crashed ClashX, an office's proxy at home, an NTLM proxy or one whose authority
/// isn't installed is never worse than that. That proxy is passed by from then on, until going directly fails too.
pub async fn send(request: reqwest::RequestBuilder) -> reqwest::Result<reqwest::Response> {
    send_through(request, proxy::current()).await
}

/// [`send`], for a client that goes by `settings` ([`model_servers`]: the variables' alone, so never passed by).
pub async fn send_through(request: reqwest::RequestBuilder, settings: Arc<proxy::Settings>) -> reqwest::Result<reqwest::Response> {
    let (client, request) = request.build_split();
    let request = request?;
    let (url, again) = (request.url().clone(), request.try_clone());
    let sent = client.execute(request).await;
    let in_the_way = match &sent {
        Ok(answer) => answer.status() == reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED,
        Err(error) => proxy::in_the_way(error),
    };
    if let Some(again) = again.filter(|_| in_the_way && proxy::pass_by(&settings, &url)) {
        match client.execute(again).await {
            Ok(direct) => return Ok(direct),
            // The proxy is used again, and its trouble is what's said.
            Err(_) => proxy::stop_passing_by(),
        }
    }
    sent
}

/// A client's beginning that never goes through a proxy. The web tool looks each name up itself and connects only
/// to public addresses: a proxy would look the name up again, where Kumi can't check it. Nor does its certificate check
/// fetch anything (an issuer a certificate names could be any address): it reads the computer's list of trusted roots.
pub fn direct() -> reqwest::ClientBuilder {
    reqwest::Client::builder().use_preconfigured_tls(trust::guarded_config(false)).no_proxy()
}

/// The TLS of [`client`] for a connection that speaks HTTP/1.1 only, as a WebSocket's opening does: the same trust.
pub fn http1_tls() -> rustls::ClientConfig {
    trust::config(true)
}

/// [`direct`], speaking HTTP/1.1 only.
pub fn direct_http1() -> reqwest::ClientBuilder {
    reqwest::Client::builder().use_preconfigured_tls(trust::guarded_config(true)).http1_only().no_proxy()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use proxy::Settings;
    use rcgen::{BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose};
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A server on this computer that answers each connection with `answer`, counting them; TLS with `tls`.
    fn server(answer: &'static [u8], tls: Option<Arc<rustls::ServerConfig>>) -> (SocketAddr, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&count);
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                counted.fetch_add(1, Ordering::SeqCst);
                let tls = tls.clone();
                std::thread::spawn(move || {
                    let mut request = [0u8; 4096];
                    match tls {
                        Some(tls) => {
                            let mut stream = rustls::StreamOwned::new(rustls::ServerConnection::new(tls).unwrap(), stream);
                            if stream.read(&mut request).is_ok() {
                                let _ = stream.write_all(answer);
                                let _ = stream.flush();
                            }
                        }
                        None => {
                            let _ = stream.read(&mut request);
                            let _ = stream.write_all(answer);
                        }
                    }
                });
            }
        });
        (address, count)
    }

    /// A proxy that inspects secure connections: it takes each CONNECT and answers in the tunnel itself, with a
    /// certificate of its own (`tls`), counting the connections.
    fn inspecting(tls: Arc<rustls::ServerConfig>) -> (SocketAddr, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&count);
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                counted.fetch_add(1, Ordering::SeqCst);
                let tls = Arc::clone(&tls);
                std::thread::spawn(move || {
                    let mut connect = [0u8; 4096];
                    if stream.read(&mut connect).is_err() || stream.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").is_err() {
                        return;
                    }
                    let mut stream = rustls::StreamOwned::new(rustls::ServerConnection::new(tls).unwrap(), stream);
                    let mut request = [0u8; 4096];
                    if stream.read(&mut request).is_ok() {
                        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 9\r\nconnection: close\r\n\r\ninspected");
                        let _ = stream.flush();
                    }
                });
            }
        });
        (address, count)
    }

    /// An authority by its `name`, and a server's TLS for `host` from it.
    fn tls_for(name: &str, host: &str) -> (rustls::RootCertStore, Arc<rustls::ServerConfig>) {
        let mut authority = CertificateParams::default();
        authority.distinguished_name.push(DnType::CommonName, name);
        authority.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        authority.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        let key = KeyPair::generate().unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(authority.self_signed(&key).unwrap().der().clone()).unwrap();
        let issuer = Issuer::new(authority, key);
        let mut leaf = CertificateParams::new(vec![host.to_string()]).unwrap();
        leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let key = KeyPair::generate().unwrap();
        let chain = vec![leaf.signed_by(&key, &issuer).unwrap().der().clone()];
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into());
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(rustls::ALL_VERSIONS)
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .unwrap();
        (roots, Arc::new(config))
    }

    /// A client trusting `roots`, through `settings`' proxies, with `name` looked up as `at`.
    fn through(settings: Settings, roots: rustls::RootCertStore, name: &str, at: SocketAddr) -> (reqwest::Client, Arc<Settings>) {
        let settings = Arc::new(settings);
        let asked = Arc::clone(&settings);
        let client = reqwest::Client::builder()
            .use_preconfigured_tls(trust::trusting(roots))
            .proxy(reqwest::Proxy::custom(move |url| asked.proxy_for(url)))
            .resolve(name, at)
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();
        (client, settings)
    }

    async fn text(sent: reqwest::Result<reqwest::Response>) -> Result<(u16, String), String> {
        match sent {
            Ok(answer) => Ok((answer.status().as_u16(), answer.text().await.unwrap_or_default())),
            Err(error) => Err(crate::cause::chain(&error)),
        }
    }

    #[tokio::test]
    async fn a_system_proxy_in_the_way_is_passed_by_once_directly_as_kumi_went_before() {
        let _turn = proxy::PASSING_BY.lock().await;
        let (roots, tls) = tls_for("A public authority", "kumi-send.test");
        let (target, _) = server(b"HTTP/1.1 200 OK\r\ncontent-length: 6\r\nconnection: close\r\n\r\ndirect", Some(tls));
        let url = format!("https://kumi-send.test:{}/", target.port());
        let wants = b"HTTP/1.1 407 Proxy Authentication Required\r\nproxy-authenticate: NTLM\r\n\r\n";
        let (signing_in, asked) = server(wants, None);
        // A network that inspects secure connections, its own authority not installed here.
        let (_, its_own) = tls_for("An office's inspecting proxy", "kumi-send.test");
        let (inspects, inspected) = inspecting(its_own);
        let refused = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        // The system names one proxy at a time: each case starts with none passed by.
        for (proxy, asked, why) in [
            (format!("http://{signing_in}"), Some(&asked), "an NTLM sign-in (407 to CONNECT)"),
            (format!("http://{inspects}"), Some(&inspected), "a certificate this computer doesn't trust"),
            (format!("http://{refused}"), None, "a proxy that isn't running"),
            ("http://kumi-proxy.invalid:3128".to_string(), None, "a proxy whose name isn't found"),
        ] {
            proxy::stop_passing_by();
            let (client, settings) = through(Settings::system_for_tests(Some(&proxy), false), roots.clone(), "kumi-send.test", target);
            assert_eq!(text(send_through(client.get(&url), settings).await).await, Ok((200, "direct".into())), "{why}");
            // Passed by from then on: the next request goes directly at once.
            let before = asked.map(|asked| asked.load(Ordering::SeqCst));
            let (client, settings) = through(Settings::system_for_tests(Some(&proxy), false), roots.clone(), "kumi-send.test", target);
            assert_eq!(text(send_through(client.get(&url), settings).await).await, Ok((200, "direct".into())), "{why}");
            assert_eq!(asked.map(|asked| asked.load(Ordering::SeqCst)), before, "{why}: passed by");
        }
        proxy::stop_passing_by();
        // A variable's proxy is the producer's own: its 407, its refusal and its certificate stand.
        for (proxy, says) in [
            (format!("http://{signing_in}"), "proxy authorization required"),
            (format!("http://{refused}"), "refused"),
            (format!("http://{inspects}"), "invalid peer certificate"),
        ] {
            let env = kumi_types::system::Env::from([("HTTPS_PROXY".to_string(), proxy.clone())]);
            let (client, settings) = through(Settings::from_env(&env).unwrap(), roots.clone(), "kumi-send.test", target);
            let said = text(send_through(client.get(&url), settings).await).await.unwrap_err();
            assert!(said.contains(says), "{proxy}: {said}");
        }
        // Going directly fails too: the proxy's trouble is what's said, and the proxy is asked again next time.
        let gone = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        for (proxy, asked, says) in
            [(signing_in, &asked, "proxy authorization required"), (inspects, &inspected, "invalid peer certificate: UnknownIssuer")]
        {
            let system = || Settings::system_for_tests(Some(&format!("http://{proxy}")), false);
            let (client, settings) = through(system(), roots.clone(), "kumi-send.test", gone);
            let nowhere = format!("https://kumi-send.test:{}/", gone.port());
            let said = text(send_through(client.get(nowhere), settings).await).await.unwrap_err();
            assert!(said.contains(says), "{said}");
            let before = asked.load(Ordering::SeqCst);
            let (client, settings) = through(system(), roots.clone(), "kumi-send.test", target);
            assert_eq!(text(send_through(client.get(&url), settings).await).await, Ok((200, "direct".into())));
            assert_eq!(asked.load(Ordering::SeqCst), before + 1, "{says}: asked again, then passed by");
            proxy::stop_passing_by();
        }
    }

    #[tokio::test]
    async fn a_slow_proxy_is_waited_on_not_passed_by() {
        // A proxy that takes the connection and never answers: maybe only a slow network, so no second try.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let silent = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let held: Vec<_> = listener.incoming().flatten().collect();
            drop(held);
        });
        let (roots, tls) = tls_for("A public authority", "kumi-slow.test");
        let (target, reached) = server(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n", Some(tls));
        let settings = Arc::new(Settings::system_for_tests(Some(&format!("http://{silent}")), false));
        let asked = Arc::clone(&settings);
        let client = reqwest::Client::builder()
            .use_preconfigured_tls(trust::trusting(roots))
            .proxy(reqwest::Proxy::custom(move |url| asked.proxy_for(url)))
            .resolve("kumi-slow.test", target)
            .timeout(std::time::Duration::from_millis(400))
            .build()
            .unwrap();
        let said = text(send_through(client.get(format!("https://kumi-slow.test:{}/", target.port())), settings).await).await;
        assert!(said.unwrap_err().contains("timed out"));
        assert_eq!(reached.load(Ordering::SeqCst), 0, "no direct try");
    }

    /// Every client is made here: one made elsewhere would trust and proxy its own way. (Tests may make their own.)
    #[test]
    fn every_client_begins_here() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        // Every crate of the workspace, Kumi's own and each DAW's, but this one.
        let root = kumi_testkit::workspace();
        let mut sources = vec![root.join("crates"), root.join("daws")];
        let mut elsewhere = Vec::new();
        while let Some(path) = sources.pop() {
            if path.is_dir() {
                let skipped = |name: &std::ffi::OsStr| ["tests", "target", "node_modules"].iter().any(|skip| name == *skip);
                if path != here.join("src") && path.file_name().is_some_and(|name| !skipped(name)) {
                    sources.extend(std::fs::read_dir(&path).unwrap().map(|entry| entry.unwrap().path()));
                }
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                let code = text.split("#[cfg(test)]").next().unwrap_or("");
                for made in ["Client::builder()", "Client::new()", "ClientBuilder::new()", "reqwest::get(", "reqwest::blocking"] {
                    if code.contains(made) {
                        elsewhere.push(format!("{}: {made}", path.strip_prefix(&root).unwrap().display()));
                    }
                }
            }
        }
        assert!(elsewhere.is_empty(), "begin these at kumi_net::client (or direct): {elsewhere:#?}");
    }
}
