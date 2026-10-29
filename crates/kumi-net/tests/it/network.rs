//! Kumi's HTTP behind a network that inspects secure connections, a SOCKS proxy, and a proxy for everything but this
//! computer, beside reqwest as Kumi made its clients before (the certificates it carries, the variables' proxies).
//!
//! The ignored tests are the Network workflow's, on each system: the authority it installs in the system's own trust
//! store, and the proxy it sets in the system's settings (or HTTPS_PROXY, on Linux).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{Datelike, Utc};
use kumi_net::{self as net, proxy::Settings};
use rcgen::{BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};

/// A name that never resolves (RFC 6761): it's reached through a proxy, or not at all.
const UNRESOLVED: &str = "kumi-network.test";

/// Certificates valid from yesterday for `days`, as the systems accept them (macOS refuses a server's that lasts years).
fn valid_for(params: &mut CertificateParams, days: i64) {
    let day = |from_today: i64| {
        let date = (Utc::now() + chrono::Duration::days(from_today)).date_naive();
        rcgen::date_time_ymd(date.year(), date.month() as u8, date.day() as u8)
    };
    params.not_before = day(-1);
    params.not_after = day(days);
}

fn authority_params() -> CertificateParams {
    let mut params = CertificateParams::default();
    params.distinguished_name.push(DnType::CommonName, "Kumi network test authority");
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    valid_for(&mut params, 30);
    params
}

/// A certificate authority of its own, as an inspecting network has: its certificate (PEM) and the issuer of its
/// servers' certificates.
fn authority() -> (String, Issuer<'static, KeyPair>) {
    let key = KeyPair::generate().unwrap();
    let params = authority_params();
    let pem = params.self_signed(&key).unwrap().pem();
    (pem, Issuer::new(params, key))
}

/// The authority the workflow made and installed, from its folder.
fn installed_authority() -> Issuer<'static, KeyPair> {
    let key = std::fs::read_to_string(authority_folder().join("authority.key")).unwrap();
    Issuer::new(authority_params(), KeyPair::from_pem(&key).unwrap())
}

fn authority_folder() -> PathBuf {
    PathBuf::from(std::env::var("KUMI_TEST_AUTHORITY").expect("KUMI_TEST_AUTHORITY names the authority's folder"))
}

/// A server's TLS, with a certificate for `host` from `issuer`.
fn server_tls(host: &str, issuer: &Issuer<'_, KeyPair>) -> Arc<rustls::ServerConfig> {
    server_tls_with(host, issuer, Vec::new())
}

/// [`server_tls`], with more extensions in the certificate.
fn server_tls_with(host: &str, issuer: &Issuer<'_, KeyPair>, extensions: Vec<rcgen::CustomExtension>) -> Arc<rustls::ServerConfig> {
    let mut params = CertificateParams::new(vec![host.to_string()]).unwrap();
    params.distinguished_name.push(DnType::CommonName, host);
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.custom_extensions = extensions;
    valid_for(&mut params, 7);
    let key = KeyPair::generate().unwrap();
    let certificate = params.signed_by(&key, issuer).unwrap();
    let chain = vec![certificate.der().clone()];
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    Arc::new(
        rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(rustls::ALL_VERSIONS)
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .unwrap(),
    )
}

/// A request's head, read off `stream`.
fn head(stream: &mut impl Read) -> Option<String> {
    let mut head = Vec::new();
    let mut byte = [0u8];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).ok()? == 0 {
            return None;
        }
        head.push(byte[0]);
    }
    Some(String::from_utf8_lossy(&head).into_owned())
}

fn answer(stream: &mut impl Write, body: &str) {
    let _ = write!(stream, "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
    let _ = stream.flush();
}

/// Serves each connection to `listener` on a thread of its own.
fn serve(listener: TcpListener, handle: impl Fn(TcpStream) + Send + Sync + 'static) {
    let handle = Arc::new(handle);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let handle = Arc::clone(&handle);
            std::thread::spawn(move || handle(stream));
        }
    });
}

/// A proxy that inspects secure connections: it answers CONNECT, then presents a certificate for the asked-for host
/// from `issuer` and answers the request itself, "inspected <host>". The hosts it was asked for are in the list.
fn inspecting_proxy(listener: TcpListener, issuer: Issuer<'static, KeyPair>) -> Arc<Mutex<Vec<String>>> {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let (issuer, seen) = (Arc::new(issuer), Arc::clone(&asked));
    serve(listener, move |mut stream| {
        let Some(request) = head(&mut stream) else { return };
        let target = request.split_whitespace().nth(1).unwrap_or("").to_string();
        let host = target.rsplit_once(':').map_or(target.as_str(), |(host, _)| host).to_string();
        seen.lock().unwrap().push(host.clone());
        let _ = stream.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n");
        let connection = rustls::ServerConnection::new(server_tls(&host, &issuer)).unwrap();
        let mut tls = rustls::StreamOwned::new(connection, stream);
        if head(&mut tls).is_some() {
            answer(&mut tls, &format!("inspected {host}"));
        }
    });
    asked
}

/// A plain local server that answers "direct".
fn local_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    serve(listener, |mut stream| {
        if head(&mut stream).is_some() {
            answer(&mut stream, "direct");
        }
    });
    address
}

/// A port nothing listens on: a proxy that's gone.
fn closed_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// An error with everything that caused it.
fn chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut said = vec![error.to_string()];
    let mut source = error.source();
    while let Some(cause) = source {
        said.push(cause.to_string());
        source = cause.source();
    }
    said.join(": ")
}

async fn get(client: reqwest::ClientBuilder, url: &str) -> Result<String, String> {
    let client = client.timeout(Duration::from_secs(20)).build().unwrap();
    match client.get(url).send().await {
        Ok(response) => Ok(response.text().await.unwrap_or_default()),
        Err(error) => Err(chain(&error)),
    }
}

/// Runs the ignored test `name` of this file in a process of its own, with `env` besides this one's (less the
/// proxy and certificate variables), and says how it went.
fn child(name: &str, env: &[(&str, String)]) -> std::process::Output {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", &format!("network::{name}"), "--ignored", "--nocapture", "--test-threads=1"]);
    for variable in
        ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy", "NO_PROXY", "no_proxy", "SSL_CERT_FILE"]
    {
        command.env_remove(variable);
    }
    command.envs(env.iter().map(|(name, value)| (*name, value.as_str())));
    command.output().unwrap()
}

#[test]
fn an_inspecting_network_is_trusted_once_its_certificate_is_and_only_then() {
    let (pem, issuer) = authority();
    let folder = tempfile::tempdir().unwrap();
    let certificate = folder.path().join("inspecting-network.pem");
    std::fs::write(&certificate, pem).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    let asked = inspecting_proxy(listener, issuer);
    for (trusted, extra) in [(true, Some(&certificate)), (false, None)] {
        let mut env = vec![("HTTPS_PROXY", proxy.clone()), ("KUMI_TEST_TRUSTED", trusted.to_string())];
        env.extend(extra.map(|path| ("SSL_CERT_FILE", path.display().to_string())));
        let ran = child("through_an_inspecting_proxy", &env);
        let said = format!("{}{}", String::from_utf8_lossy(&ran.stdout), String::from_utf8_lossy(&ran.stderr));
        assert!(ran.status.success() && said.contains("1 passed"), "trusted: {trusted}\n{said}");
    }
    assert!(asked.lock().unwrap().iter().all(|host| host == UNRESOLVED), "{asked:?}");
}

/// In a process of its own (see above): through HTTPS_PROXY's inspecting proxy, with SSL_CERT_FILE naming its
/// certificate when KUMI_TEST_TRUSTED is true.
#[tokio::test]
#[ignore = "run by an_inspecting_network_is_trusted_once_its_certificate_is_and_only_then"]
async fn through_an_inspecting_proxy() {
    let trusted = std::env::var("KUMI_TEST_TRUSTED").unwrap() == "true";
    let url = format!("https://{UNRESOLVED}/v1/models");
    let kumi = get(net::client(), &url).await;
    if trusted {
        assert_eq!(kumi.as_deref(), Ok(format!("inspected {UNRESOLVED}").as_str()));
    } else {
        let refused = kumi.unwrap_err();
        assert!(refused.contains("invalid peer certificate: UnknownIssuer"), "{refused}");
    }
    // As Kumi made its clients before: the same proxy, but only the certificates it carries.
    let before = get(reqwest::Client::builder(), &url).await.unwrap_err();
    assert!(before.contains("invalid peer certificate: UnknownIssuer"), "{before}");
}

#[tokio::test]
async fn this_computer_is_reached_directly_and_socks_proxies_work() {
    let local = local_server();
    let gone = closed_port();
    let everything = Arc::new(Settings::from_env(&[("ALL_PROXY".to_string(), format!("http://127.0.0.1:{gone}"))].into()).unwrap());
    for address in [format!("http://{local}/api/tags"), format!("http://localhost:{}/v1/models", local.rsplit(':').next().unwrap())] {
        let settings = Arc::clone(&everything);
        assert_eq!(get(net::client_through(move || Arc::clone(&settings)), &address).await.as_deref(), Ok("direct"), "{address}");
    }
    // Before, a local model server was asked through ALL_PROXY's proxy, which can't reach this computer.
    let all = reqwest::Proxy::all(format!("http://127.0.0.1:{gone}")).unwrap();
    assert!(get(reqwest::Client::builder().proxy(all), &format!("http://{local}/")).await.is_err());

    // A SOCKS5 proxy that relays to the local server whatever name it's asked for.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let socks =
        Arc::new(Settings::from_env(&[("all_proxy".to_string(), format!("socks5h://{}", listener.local_addr().unwrap()))].into()).unwrap());
    let asked = Arc::new(Mutex::new(Vec::new()));
    let (seen, relay_to) = (Arc::clone(&asked), local.clone());
    serve(listener, move |mut stream| {
        let mut greeting = [0u8; 2];
        stream.read_exact(&mut greeting).unwrap();
        let mut methods = vec![0u8; greeting[1] as usize];
        stream.read_exact(&mut methods).unwrap();
        stream.write_all(&[5, 0]).unwrap();
        let mut request = [0u8; 5];
        stream.read_exact(&mut request).unwrap();
        let mut name = vec![0u8; request[4] as usize + 2];
        stream.read_exact(&mut name).unwrap();
        seen.lock().unwrap().push(String::from_utf8_lossy(&name[..name.len() - 2]).into_owned());
        stream.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
        let mut relay = TcpStream::connect(&relay_to).unwrap();
        let mut back = relay.try_clone().unwrap();
        let mut client = stream.try_clone().unwrap();
        std::thread::spawn(move || std::io::copy(&mut back, &mut client));
        let _ = std::io::copy(&mut stream, &mut relay);
    });
    let through = get(net::client_through(move || Arc::clone(&socks)), "http://kumi-socks.test/").await;
    assert_eq!(through.as_deref(), Ok("direct"));
    assert_eq!(*asked.lock().unwrap(), ["kumi-socks.test"], "the proxy looked the name up");
}

/// The access method an authority information access names a certificate's issuer by (caIssuers), as DER.
const CA_ISSUERS: [u8; 10] = [0x06, 0x08, 0x2B, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x02];

/// Authority information access: where a certificate's issuer can be fetched, as one names it (caIssuers).
fn issuer_at(url: &str) -> rcgen::CustomExtension {
    let location = [&[0x86, url.len() as u8][..], url.as_bytes()].concat();
    let description = [&[0x30, (CA_ISSUERS.len() + location.len()) as u8][..], &CA_ISSUERS, &location].concat();
    let content = [&[0x30, description.len() as u8][..], &description].concat();
    rcgen::CustomExtension::from_oid_content(&[1, 3, 6, 1, 5, 5, 7, 1, 1], content)
}

/// The address a certificate names for its issuer, read from it as [`issuer_at`] writes it.
fn named_issuer(certificate: &[u8]) -> Option<String> {
    let at = certificate.windows(CA_ISSUERS.len()).position(|bytes| bytes == CA_ISSUERS)? + CA_ISSUERS.len();
    let (&[0x86, length], rest) = certificate.get(at..)?.split_first_chunk::<2>()? else { return None };
    String::from_utf8(rest.get(..usize::from(length))?.to_vec()).ok()
}

/// A certificate check that follows the issuer a certificate names, as the system's does on macOS: it asks that address
/// and waits for the answer, then refuses.
#[derive(Debug)]
struct FollowsIssuer(Arc<rustls::crypto::CryptoProvider>);

impl ServerCertVerifier for FollowsIssuer {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let address = named_issuer(end_entity);
        if let Some((host, path)) = address.as_deref().and_then(|address| address.strip_prefix("http://")?.split_once('/')) {
            if let Ok(mut stream) = TcpStream::connect(host) {
                let _ = write!(stream, "GET /{path} HTTP/1.1\r\nhost: {host}\r\nconnection: close\r\n\r\n");
                let _ = stream.read_to_end(&mut Vec::new());
            }
        }
        Err(rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer))
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, certificate, signature, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, certificate, signature, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// A client whose certificate check follows the issuer a certificate names ([`FollowsIssuer`]).
fn following() -> reqwest::ClientBuilder {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_protocol_versions(rustls::ALL_VERSIONS)
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(FollowsIssuer(provider)))
        .with_no_client_auth();
    reqwest::Client::builder().use_preconfigured_tls(config).no_proxy()
}

#[tokio::test]
async fn the_web_tools_certificate_check_fetches_nothing_a_certificate_names() {
    // Where a page's certificate says its issuer is: an address a page could choose (this computer, a router).
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let issuer_address = format!("http://{}/issuer.crt", listener.local_addr().unwrap());
    let fetched = Arc::new(Mutex::new(Vec::new()));
    let asked = Arc::clone(&fetched);
    serve(listener, move |mut stream| {
        if let Some(request) = head(&mut stream) {
            asked.lock().unwrap().push(request.lines().next().unwrap_or("").to_string());
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
        }
    });
    // A server whose certificate comes from an intermediate it doesn't send, and names that address for it.
    let (_, root) = authority();
    let mut intermediate = authority_params();
    intermediate.distinguished_name = rcgen::DistinguishedName::new();
    intermediate.distinguished_name.push(DnType::CommonName, "Kumi network test intermediate");
    let key = KeyPair::generate().unwrap();
    intermediate.signed_by(&key, &root).unwrap();
    let tls = server_tls_with("localhost", &Issuer::new(intermediate, key), vec![issuer_at(&issuer_address)]);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("https://localhost:{}/", listener.local_addr().unwrap().port());
    serve(listener, move |stream| {
        let mut tls = rustls::StreamOwned::new(rustls::ServerConnection::new(Arc::clone(&tls)).unwrap(), stream);
        if head(&mut tls).is_some() {
            answer(&mut tls, "read");
        }
    });
    let web = get(net::direct(), &url).await.unwrap_err();
    assert!(web.contains("invalid peer certificate: UnknownIssuer"), "{web}");
    std::thread::sleep(Duration::from_millis(500));
    assert!(fetched.lock().unwrap().is_empty(), "the web tool's check fetched {:?}", fetched.lock().unwrap());
    // A check that follows what a certificate names asks that address, and has its answer, before its handshake ends:
    // so the certificate names it, and the web tool's check didn't ask it. (The system's check, which Kumi's own
    // requests ask, follows it on macOS, but within trustd's own 3 s, which a busy computer can miss: #729.)
    let following = get(following(), &url).await.unwrap_err();
    assert!(following.contains("invalid peer certificate: UnknownIssuer"), "{following}");
    assert_eq!(*fetched.lock().unwrap(), ["GET /issuer.crt HTTP/1.1"]);
}

#[test]
#[ignore = "the Network workflow's: makes the authority it installs"]
fn make_test_authority() {
    let folder = authority_folder();
    std::fs::create_dir_all(&folder).unwrap();
    let key = KeyPair::generate().unwrap();
    std::fs::write(folder.join("authority.pem"), authority_params().self_signed(&key).unwrap().pem()).unwrap();
    std::fs::write(folder.join("authority.key"), key.serialize_pem()).unwrap();
}

#[tokio::test]
#[ignore = "the Network workflow's: needs the authority installed in the system's trust store"]
async fn the_systems_trust_store_is_trusted() {
    let issuer = installed_authority();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let tls = server_tls("localhost", &issuer);
    serve(listener, move |stream| {
        let mut tls = rustls::StreamOwned::new(rustls::ServerConnection::new(Arc::clone(&tls)).unwrap(), stream);
        if head(&mut tls).is_some() {
            answer(&mut tls, "trusted by the system");
        }
    });
    let url = format!("https://localhost:{port}/");
    assert_eq!(get(net::client(), &url).await.as_deref(), Ok("trusted by the system"));
    // The web tool, by the system's list of trusted roots.
    assert_eq!(get(net::direct(), &url).await.as_deref(), Ok("trusted by the system"));
    let before = get(reqwest::Client::builder(), &url).await.unwrap_err();
    assert!(before.contains("invalid peer certificate: UnknownIssuer"), "{before}");
    println!("Kumi: trusted by the system\nbefore: {before}");
}

#[tokio::test]
#[ignore = "the Network workflow's: needs the system's proxy (HTTPS_PROXY on Linux) at KUMI_TEST_PROXY_PORT"]
async fn the_systems_proxy_is_followed_with_its_exceptions() {
    let port: u16 = std::env::var("KUMI_TEST_PROXY_PORT").unwrap().parse().unwrap();
    let asked = inspecting_proxy(TcpListener::bind(("127.0.0.1", port)).unwrap(), installed_authority());
    let kumi = get(net::client(), &format!("https://{UNRESOLVED}/")).await;
    assert_eq!(kumi.as_deref(), Ok(format!("inspected {UNRESOLVED}").as_str()));
    // An exception in the system's settings (NO_PROXY on Linux) goes directly, and a name that never resolves fails.
    let exception = get(net::client(), &format!("https://bypass.{UNRESOLVED}/")).await.unwrap_err();
    assert!(!exception.contains("inspected"), "{exception}");
    // This computer itself, directly.
    let local = local_server();
    assert_eq!(get(net::client(), &format!("http://{local}/")).await.as_deref(), Ok("direct"));
    assert_eq!(*asked.lock().unwrap(), [UNRESOLVED]);
    // A model server the producer names (nas.lan, a Tailscale name) goes by the variables alone, never the system's proxy.
    let model_server = format!("http://studio-gpu.lan:{}/api/version", local.rsplit(':').next().unwrap());
    let named = net::model_servers().resolve("studio-gpu.lan", local.parse().unwrap());
    assert_eq!(get(named, &model_server).await.as_deref(), Ok("direct"));
    assert_eq!(*asked.lock().unwrap(), [UNRESOLVED], "the model server went through the proxy");
    let before = get(reqwest::Client::builder(), &format!("https://{UNRESOLVED}/")).await.unwrap_err();
    println!("Kumi: inspected {UNRESOLVED} through the system's proxy\nan exception: {exception}\nbefore: {before}");
}
