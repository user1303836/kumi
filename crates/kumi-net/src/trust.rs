//! Whose certificates Kumi trusts. First the certificate authorities it carries (webpki-roots), as before; when those
//! don't vouch for a server, this computer's own trust store, with SSL_CERT_FILE's certificates besides. A school or
//! office network that inspects secure connections works once its certificate is installed on the computer, as the
//! browser and the installer need. The second word is asked only when the carried authorities refuse, so a connection
//! that worked before is checked as before. It comes two ways:
//! - [`config`], for Kumi's own requests: the system's check itself (rustls-platform-verifier), which can fetch an
//!   issuer a certificate names, so it's bounded and its verdict kept per certificate.
//! - [`guarded_config`], for the web tool's: the computer's list of trusted roots (rustls-native-certs), checked by
//!   webpki, which fetches nothing, so a page's certificate can't send Kumi to an address of its choosing.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, DigitallySignedStruct, Error, RootCertStore, SignatureScheme};
use sha2::{Digest, Sha256};

/// How long the system's check may hold a connection (and Kumi's one thread) before it's refused for now.
const BOUND: Duration = Duration::from_secs(3);
/// How long the system's verdict on a certificate is kept: a yes, and a no.
const KEPT: Duration = Duration::from_secs(600);
const KEPT_NO: Duration = Duration::from_secs(30);

/// A second word on a certificate the carried authorities refuse: None when there's none to ask (a Linux without a
/// certificate bundle).
type Second = Option<Arc<dyn ServerCertVerifier>>;

/// Kumi's own requests' TLS: the carried authorities, then the system's check. TLS 1.2 and 1.3, and HTTP/2 offered
/// unless `http1`, as reqwest's own had it. Every client shares one trust, made when it's first needed.
pub(super) fn config(http1: bool) -> rustls::ClientConfig {
    static CONFIG: LazyLock<rustls::ClientConfig> = LazyLock::new(|| configured(Arc::new(Trust::new(provider(), asked))));
    speaking(CONFIG.clone(), http1)
}

/// The web tool's TLS: the carried authorities, then the computer's list of trusted roots, nothing fetched.
pub(super) fn guarded_config(http1: bool) -> rustls::ClientConfig {
    static CONFIG: LazyLock<rustls::ClientConfig> = LazyLock::new(|| configured(Arc::new(Trust::new(provider(), listed))));
    speaking(CONFIG.clone(), http1)
}

fn speaking(mut config: rustls::ClientConfig, http1: bool) -> rustls::ClientConfig {
    if http1 {
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
    }
    config
}

fn configured(trust: Arc<Trust>) -> rustls::ClientConfig {
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::clone(&trust.provider))
        .with_protocol_versions(rustls::ALL_VERSIONS)
        .expect("TLS 1.2 and 1.3")
        .dangerous()
        .with_custom_certificate_verifier(trust)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    config
}

/// A client's TLS that trusts `roots` alone (for tests).
#[cfg(test)]
pub(super) fn trusting(roots: RootCertStore) -> rustls::ClientConfig {
    configured(Arc::new(Trust::with(roots, OnceLock::from(None), |_| None, provider())))
}

/// The cryptography reqwest's TLS used: one the program installed, else ring.
fn provider() -> Arc<CryptoProvider> {
    CryptoProvider::get_default().cloned().unwrap_or_else(|| Arc::new(rustls::crypto::ring::default_provider()))
}

/// The carried authorities, then a second word.
#[derive(Debug)]
struct Trust {
    carried: Arc<WebPkiServerVerifier>,
    second: OnceLock<Second>,
    /// What makes the second word, when it's first needed.
    make: fn(&Arc<CryptoProvider>) -> Second,
    provider: Arc<CryptoProvider>,
}

impl Trust {
    fn new(provider: Arc<CryptoProvider>, make: fn(&Arc<CryptoProvider>) -> Second) -> Self {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Self::with(roots, OnceLock::new(), make, provider)
    }

    /// With other carried authorities, and the second word already decided (for tests).
    fn with(
        carried: RootCertStore,
        second: OnceLock<Second>,
        make: fn(&Arc<CryptoProvider>) -> Second,
        provider: Arc<CryptoProvider>,
    ) -> Self {
        let carried =
            WebPkiServerVerifier::builder_with_provider(Arc::new(carried), Arc::clone(&provider)).build().expect("the carried authorities");
        Self { carried, second, make, provider }
    }

    fn second(&self) -> Option<&Arc<dyn ServerCertVerifier>> {
        self.second.get_or_init(|| (self.make)(&self.provider)).as_ref()
    }
}

/// The system's check, with SSL_CERT_FILE's certificates besides when it names a file of them; bounded and kept.
fn asked(provider: &Arc<CryptoProvider>) -> Second {
    use rustls_platform_verifier::Verifier;
    let extra = extra_certificates();
    // Certificates the system can't take are left out rather than losing its trust store with them.
    let verifier = (!extra.is_empty())
        .then(|| Verifier::new_with_extra_roots(extra, Arc::clone(provider)).ok())
        .flatten()
        .or_else(|| Verifier::new(Arc::clone(provider)).ok())?;
    Some(Arc::new(Kept::new(Arc::new(verifier), BOUND)))
}

/// The computer's list of trusted roots with SSL_CERT_FILE's, checked by webpki. As OpenSSL reads them,
/// SSL_CERT_FILE or SSL_CERT_DIR stand in for the list when set.
fn listed(provider: &Arc<CryptoProvider>) -> Second {
    let mut roots = RootCertStore::empty();
    roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
    roots.add_parsable_certificates(extra_certificates());
    if roots.is_empty() {
        return None;
    }
    let verifier = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), Arc::clone(provider)).build().ok()?;
    Some(verifier)
}

fn extra_certificates() -> Vec<CertificateDer<'static>> {
    std::env::var_os("SSL_CERT_FILE").map(|path| certificates(Path::new(&path))).unwrap_or_default()
}

/// The certificates in a file: PEM, or one in DER, as Windows exports it.
fn certificates(path: &Path) -> Vec<CertificateDer<'static>> {
    use rustls::pki_types::pem::PemObject;
    let pem: Vec<_> = CertificateDer::pem_file_iter(path).map(|found| found.filter_map(Result::ok).collect()).unwrap_or_default();
    if !pem.is_empty() {
        return pem;
    }
    std::fs::read(path).ok().filter(|der| der.first() == Some(&0x30)).map(|der| vec![CertificateDer::from(der)]).unwrap_or_default()
}

/// The system's check, run beside Kumi's one thread for at most `bound`: it can fetch an issuer a certificate names,
/// which an address that never answers holds for seconds. One check runs per certificate and name, however many
/// connections ask meanwhile, each waiting out only what's left of its bound; its verdict is kept, so a slow check
/// holds Kumi once rather than on every connection, and one that outlasts the bound is ready for the next try.
#[derive(Debug)]
struct Kept {
    system: Arc<dyn ServerCertVerifier>,
    bound: Duration,
    verdicts: Arc<Mutex<HashMap<[u8; 32], Verdict>>>,
}

/// A certificate's verdict: still being checked (since when), or given (when).
#[derive(Debug, Clone)]
enum Verdict {
    Checking(Instant, Arc<Pending>),
    Given(Instant, Result<(), Error>),
}

/// A check under way: its verdict once it's given.
#[derive(Debug, Default)]
struct Pending {
    verdict: Mutex<Option<Result<(), Error>>>,
    given: std::sync::Condvar,
}

/// How long a check that never ends is waited for before another may start.
const STUCK: Duration = Duration::from_secs(60);

impl Kept {
    fn new(system: Arc<dyn ServerCertVerifier>, bound: Duration) -> Self {
        Self { system, bound, verdicts: Arc::default() }
    }
}

/// The system's check outlasted [`BOUND`]: a timeout, not a refusal (the certificate may well be installed).
fn slow() -> Error {
    Error::General(format!("this computer's check of the server timed out after {} s", BOUND.as_secs()))
}

/// A certificate's chain and the name it's for, as a key.
fn fingerprint(end_entity: &CertificateDer<'_>, intermediates: &[CertificateDer<'_>], server_name: &ServerName<'_>) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(server_name.to_str().as_bytes());
    for certificate in std::iter::once(end_entity).chain(intermediates) {
        hash.update((certificate.len() as u64).to_be_bytes());
        hash.update(certificate.as_ref());
    }
    hash.finalize().into()
}

impl ServerCertVerifier for Kept {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let key = fingerprint(end_entity, intermediates, server_name);
        let (since, pending) = {
            let mut verdicts = self.verdicts.lock().unwrap_or_else(PoisonError::into_inner);
            verdicts.retain(|_, verdict| match verdict {
                Verdict::Checking(since, _) => since.elapsed() < STUCK,
                Verdict::Given(when, verdict) => when.elapsed() < if verdict.is_ok() { KEPT } else { KEPT_NO },
            });
            match verdicts.get(&key) {
                Some(Verdict::Given(_, verdict)) => return verdict.clone().map(|()| ServerCertVerified::assertion()),
                // Already being checked: wait for that check, for what's left of its bound.
                Some(Verdict::Checking(since, pending)) => (*since, Arc::clone(pending)),
                None => {
                    let pending = Arc::new(Pending::default());
                    let (system, waiting, kept) = (Arc::clone(&self.system), Arc::clone(&pending), Arc::clone(&self.verdicts));
                    let (end_entity, server_name) = (end_entity.clone().into_owned(), server_name.to_owned());
                    let intermediates: Vec<_> = intermediates.iter().map(|certificate| certificate.clone().into_owned()).collect();
                    let ocsp_response = ocsp_response.to_vec();
                    let checking = std::thread::Builder::new().name("kumi-certificate-check".into()).spawn(move || {
                        let verdict = system.verify_server_cert(&end_entity, &intermediates, &server_name, &ocsp_response, now).map(|_| ());
                        kept.lock().unwrap_or_else(PoisonError::into_inner).insert(key, Verdict::Given(Instant::now(), verdict.clone()));
                        *waiting.verdict.lock().unwrap_or_else(PoisonError::into_inner) = Some(verdict);
                        waiting.given.notify_all();
                    });
                    // No thread to check beside Kumi's: the check can't be bounded, so it's not asked.
                    if checking.is_err() {
                        return Err(slow());
                    }
                    verdicts.insert(key, Verdict::Checking(Instant::now(), Arc::clone(&pending)));
                    (Instant::now(), pending)
                }
            }
        };
        let left = self.bound.saturating_sub(since.elapsed());
        let verdict = pending.verdict.lock().unwrap_or_else(PoisonError::into_inner);
        let (verdict, _) =
            pending.given.wait_timeout_while(verdict, left, |verdict| verdict.is_none()).unwrap_or_else(PoisonError::into_inner);
        match &*verdict {
            Some(verdict) => verdict.clone().map(|()| ServerCertVerified::assertion()),
            // Still checking: a timeout for now, and the verdict is there for the next try.
            None => Err(slow()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.system.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.system.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.system.supported_verify_schemes()
    }
}

/// A refusal for something other than who issued the certificate.
fn telling(refused: &Error) -> bool {
    use CertificateError::*;
    matches!(
        refused,
        Error::InvalidCertificate(
            NotValidForName
                | NotValidForNameContext { .. }
                | Expired
                | ExpiredContext { .. }
                | NotValidYet
                | NotValidYetContext { .. }
                | Revoked
                | InvalidPurpose
                | InvalidPurposeContext { .. }
        )
    )
}

impl ServerCertVerifier for Trust {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let carried = self.carried.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now);
        // A certificate no carried authority vouches for (an office's own, say): the system's word. When it refuses
        // too, the refusal reads as the carried one, the same on every system, unless the system knew the authority
        // and found something else wrong (the name, the dates, a revocation), which says more.
        if let (Err(Error::InvalidCertificate(why)), Some(second)) = (&carried, self.second()) {
            match second.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now) {
                Ok(verified) => return Ok(verified),
                // The system's check timed out: the certificate may well be installed, so that's what's said.
                Err(refused) if refused == slow() => return Err(refused),
                Err(refused) if *why == CertificateError::UnknownIssuer && telling(&refused) => return Err(refused),
                Err(_) => {}
            }
        }
        carried
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.carried.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.carried.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.carried.supported_verify_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose};

    /// A certificate authority of its own, as a school's or an office's network has.
    fn authority(name: &str) -> (CertificateDer<'static>, Issuer<'static, KeyPair>) {
        let mut params = CertificateParams::default();
        params.distinguished_name.push(DnType::CommonName, name);
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let key = KeyPair::generate().unwrap();
        let der = params.self_signed(&key).unwrap().der().clone();
        (der, Issuer::new(params, key))
    }

    fn server(host: &str, by: &Issuer<'_, KeyPair>) -> CertificateDer<'static> {
        let mut params = CertificateParams::new(vec![host.to_string()]).unwrap();
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.signed_by(&KeyPair::generate().unwrap(), by).unwrap().der().clone()
    }

    fn roots(authorities: &[&CertificateDer<'static>]) -> RootCertStore {
        let mut store = RootCertStore::empty();
        authorities.iter().for_each(|authority| store.add((*authority).clone()).unwrap());
        store
    }

    fn unused(_: &Arc<CryptoProvider>) -> Second {
        None
    }

    /// The system, as a verifier trusting `authorities` alone.
    fn system(authorities: &[&CertificateDer<'static>]) -> OnceLock<Option<Arc<dyn ServerCertVerifier>>> {
        let verifier = WebPkiServerVerifier::builder_with_provider(Arc::new(roots(authorities)), provider()).build().unwrap();
        OnceLock::from(Some(verifier as Arc<dyn ServerCertVerifier>))
    }

    fn check(trust: &Trust, certificate: &CertificateDer<'_>, host: &str) -> Result<ServerCertVerified, Error> {
        trust.verify_server_cert(certificate, &[], &ServerName::try_from(host.to_string()).unwrap(), &[], UnixTime::now())
    }

    #[test]
    fn the_system_vouches_for_a_certificate_the_carried_authorities_dont() {
        let (public, public_issuer) = authority("A public authority");
        let (office, office_issuer) = authority("An office's inspecting proxy");
        let both = Trust::with(roots(&[&public]), system(&[&office]), unused, provider());
        assert!(check(&both, &server("api.openai.com", &public_issuer), "api.openai.com").is_ok(), "the carried ones, as before");
        assert!(check(&both, &server("api.openai.com", &office_issuer), "api.openai.com").is_ok(), "the system's");
        let before = Trust::with(roots(&[&public]), OnceLock::from(None), unused, provider());
        assert!(matches!(
            check(&before, &server("api.openai.com", &office_issuer), "api.openai.com"),
            Err(Error::InvalidCertificate(CertificateError::UnknownIssuer))
        ));
    }

    #[test]
    fn a_refusal_reads_as_the_carried_authorities_have_it() {
        let (public, public_issuer) = authority("A public authority");
        let (office, office_issuer) = authority("An office's inspecting proxy");
        let (_, stranger) = authority("Nobody's");
        let trust = Trust::with(roots(&[&public]), system(&[&office]), unused, provider());
        assert!(matches!(
            check(&trust, &server("api.openai.com", &stranger), "api.openai.com"),
            Err(Error::InvalidCertificate(CertificateError::UnknownIssuer))
        ));
        // Another server's certificate: the carried authorities' refusal, or the system's when only it knows the issuer.
        for issuer in [&public_issuer, &office_issuer] {
            let refused = check(&trust, &server("github.com", issuer), "api.openai.com");
            assert!(
                matches!(
                    refused,
                    Err(Error::InvalidCertificate(CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. }))
                ),
                "{refused:?}"
            );
        }
    }

    #[test]
    fn ssl_cert_file_reads_pem_and_der_and_nothing_else() {
        let folder = tempfile::tempdir().unwrap();
        let (one, _) = authority("One");
        let (two, _) = authority("Two");
        let pem = |der: &CertificateDer<'_>| {
            use base64::Engine;
            let body = base64::engine::general_purpose::STANDARD.encode(der.as_ref());
            format!("-----BEGIN CERTIFICATE-----\n{body}\n-----END CERTIFICATE-----\n")
        };
        let bundle = folder.path().join("bundle.pem");
        std::fs::write(&bundle, format!("# an office's\n{}{}", pem(&one), pem(&two))).unwrap();
        assert_eq!(certificates(&bundle), [one.clone(), two]);
        let der = folder.path().join("office.cer");
        std::fs::write(&der, one.as_ref()).unwrap();
        assert_eq!(certificates(&der), [one]);
        let text = folder.path().join("notes.txt");
        std::fs::write(&text, "not a certificate").unwrap();
        assert!(certificates(&text).is_empty() && certificates(&folder.path().join("missing.pem")).is_empty());
    }

    /// A system check that takes `taking` and says yes or no, counting how often it's asked.
    #[derive(Debug)]
    struct Slowly {
        taking: Duration,
        yes: bool,
        asked: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl ServerCertVerifier for Slowly {
        fn verify_server_cert(
            &self,
            _: &CertificateDer<'_>,
            _: &[CertificateDer<'_>],
            _: &ServerName<'_>,
            _: &[u8],
            _: UnixTime,
        ) -> Result<ServerCertVerified, Error> {
            std::thread::sleep(self.taking);
            self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.yes {
                Ok(ServerCertVerified::assertion())
            } else {
                Err(Error::InvalidCertificate(CertificateError::UnknownIssuer))
            }
        }
        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            Err(Error::General("not asked".into()))
        }
        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            Err(Error::General("not asked".into()))
        }
        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            Vec::new()
        }
    }

    #[test]
    fn a_slow_system_check_holds_a_connection_at_most_its_bound_and_its_verdict_is_kept() {
        let (_, issuer) = authority("An office's inspecting proxy");
        let certificate = server("api.openai.com", &issuer);
        let name = |host: &str| ServerName::try_from(host.to_string()).unwrap();
        let ask = |kept: &Kept, host: &str| {
            let started = Instant::now();
            (kept.verify_server_cert(&certificate, &[], &name(host), &[], UnixTime::now()).is_ok(), started.elapsed())
        };
        for yes in [true, false] {
            let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let slowly = Slowly { taking: Duration::from_millis(400), yes, asked: Arc::clone(&asked) };
            let kept = Kept::new(Arc::new(slowly), Duration::from_millis(100));
            // An issuer that doesn't answer: refused for now, at the bound, not when the system gives up.
            let (verified, took) = ask(&kept, "api.openai.com");
            assert!(!verified && took < Duration::from_millis(350), "{took:?}");
            // The check finishes beside Kumi, and its verdict answers the next try at once.
            std::thread::sleep(Duration::from_millis(600));
            let (verified, took) = ask(&kept, "api.openai.com");
            assert!(verified == yes && took < Duration::from_millis(50), "{yes}: {took:?}");
            assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
            // Another name is another question.
            let (_, took) = ask(&kept, "chatgpt.com");
            assert!(took >= Duration::from_millis(100), "{took:?}");
        }
    }

    #[test]
    fn connections_at_once_share_one_system_check_and_its_bound() {
        let (_, issuer) = authority("An office's inspecting proxy");
        let certificate = server("api.openai.com", &issuer);
        let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let slowly = Slowly { taking: Duration::from_millis(1500), yes: true, asked: Arc::clone(&asked) };
        let kept = Arc::new(Kept::new(Arc::new(slowly), Duration::from_millis(600)));
        // Each connection: its verdict, and how long it was held.
        let connect = || {
            let (kept, certificate) = (Arc::clone(&kept), certificate.clone());
            std::thread::spawn(move || {
                let name = ServerName::try_from("api.openai.com").unwrap();
                let started = Instant::now();
                (kept.verify_server_cert(&certificate, &[], &name, &[], UnixTime::now()).err(), started.elapsed())
            })
        };
        let first = connect();
        std::thread::sleep(Duration::from_millis(300));
        let later: Vec<_> = (0..3).map(|_| connect()).collect();
        assert_eq!(first.join().unwrap().0, Some(slow()), "a timeout for now");
        for connection in later {
            let (verdict, held) = connection.join().unwrap();
            assert_eq!(verdict, Some(slow()), "a timeout for now");
            // What's left of the first's bound (300 ms), not a bound of its own (600 ms).
            assert!(held < Duration::from_millis(500), "{held:?}");
        }
        std::thread::sleep(Duration::from_millis(1400));
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1, "one check");
        let name = ServerName::try_from("api.openai.com").unwrap();
        assert!(kept.verify_server_cert(&certificate, &[], &name, &[], UnixTime::now()).is_ok(), "its verdict, kept");
    }

    #[test]
    fn a_system_check_that_runs_out_its_time_reads_as_a_timeout_not_an_unknown_issuer() {
        let (public, _) = authority("A public authority");
        let (_, office_issuer) = authority("An office's inspecting proxy");
        let slowly = Slowly { taking: Duration::from_millis(300), yes: true, asked: Arc::default() };
        let kept: Arc<dyn ServerCertVerifier> = Arc::new(Kept::new(Arc::new(slowly), Duration::from_millis(50)));
        let trust = Trust::with(roots(&[&public]), OnceLock::from(Some(kept)), unused, provider());
        let refused = check(&trust, &server("api.openai.com", &office_issuer), "api.openai.com").err();
        assert_eq!(refused, Some(slow()));
        assert_eq!(crate::web::cause::NetCause::of_text(&refused.unwrap().to_string()), Some(crate::web::cause::NetCause::Timeout));
    }
}
