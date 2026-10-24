//! TLS for the HTTPS listener (IF-08): TLS 1.2 and 1.3 only, with a certificate that is reloaded
//! when its files change.
//!
//! rustls implements no protocol version below TLS 1.2. Handshakes run concurrently, each limited
//! by `tls.handshake_timeout`, so a slow or silent client cannot hold up the others.

use std::{
    io,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};

use axum::serve::Listener;
use rustls::{
    ServerConfig,
    crypto::{CryptoProvider, ring},
    pki_types::{
        CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer,
        pem::{self, PemObject},
    },
    server::{ClientHello, ResolvesServerCert},
    sign::CertifiedKey,
    version::{TLS12, TLS13},
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc,
    task::JoinHandle,
    time::MissedTickBehavior,
};
use tokio_rustls::{TlsAcceptor, server::TlsStream};
use tracing::{debug, info, warn};

use crate::config;

/// The names the self-signed development certificate is valid for.
const SELF_SIGNED_NAMES: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("cannot read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid PEM in {}: {source}", path.display())]
    Pem {
        path: PathBuf,
        #[source]
        source: pem::Error,
    },
    #[error("invalid TLS certificate or private key: {0}")]
    Key(#[from] rustls::Error),
    #[error("cannot generate a self-signed certificate: {0}")]
    SelfSigned(#[from] rcgen::Error),
}

/// The certificate the HTTPS listener presents. A renewed certificate replaces it while the
/// listener runs.
#[derive(Debug)]
pub struct Certificate {
    current: RwLock<Arc<CertifiedKey>>,
    /// Where the certificate comes from, unless it is self-signed.
    files: Option<Files>,
}

#[derive(Debug)]
struct Files {
    certificate: PathBuf,
    private_key: PathBuf,
}

impl Certificate {
    /// Loads the certificate from the files that `config` names, or generates a self-signed one
    /// if it says so.
    pub async fn load(config: &config::Tls) -> Result<Self, TlsError> {
        let (key, files) = match (&config.certificate_file, &config.private_key_file) {
            (Some(certificate), Some(private_key)) => {
                let files = Files {
                    certificate: certificate.clone(),
                    private_key: private_key.clone(),
                };
                (files.load().await?, Some(files))
            }
            // The configuration has been validated: without files, it asks for self-signed.
            _ => {
                warn!(
                    "serving a self-signed certificate for localhost, which browsers and Host \
                     Agents do not trust: for development only"
                );
                (self_signed()?, None)
            }
        };
        Ok(Self {
            current: RwLock::new(Arc::new(key)),
            files,
        })
    }

    /// Checks the files every `interval` and switches to a renewed certificate. Runs until it is
    /// dropped. A self-signed certificate is never reloaded.
    pub async fn reload(&self, interval: Duration) {
        if self.files.is_none() {
            return;
        }
        let mut ticks = tokio::time::interval(interval);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // The first tick completes at once, and the files were loaded at startup.
        ticks.tick().await;
        loop {
            ticks.tick().await;
            match self.refresh().await {
                Ok(true) => info!("TLS certificate reloaded"),
                Ok(false) => {}
                // For example, the new certificate has been written but its key not yet: the
                // next tick tries again.
                Err(err) => warn!(
                    error = %err,
                    "cannot reload the TLS certificate; keeping the current one"
                ),
            }
        }
    }

    /// Reloads the files, and reports whether they hold a different certificate than before.
    async fn refresh(&self) -> Result<bool, TlsError> {
        let Some(files) = &self.files else {
            return Ok(false);
        };
        let key = files.load().await?;
        if key.cert == self.current().cert {
            return Ok(false);
        }
        *self.current.write().expect("never poisoned") = Arc::new(key);
        Ok(true)
    }

    fn current(&self) -> Arc<CertifiedKey> {
        // Writers only swap the `Arc`, so they cannot panic while holding the lock.
        self.current.read().expect("never poisoned").clone()
    }
}

impl ResolvesServerCert for Certificate {
    fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.current())
    }
}

impl Files {
    async fn load(&self) -> Result<CertifiedKey, TlsError> {
        let chain = CertificateDer::pem_slice_iter(&read(&self.certificate).await?)
            .collect::<Result<Vec<_>, _>>()
            .and_then(|chain| {
                if chain.is_empty() {
                    Err(pem::Error::NoItemsFound)
                } else {
                    Ok(chain)
                }
            })
            .map_err(|source| TlsError::Pem {
                path: self.certificate.clone(),
                source,
            })?;
        let key =
            PrivateKeyDer::from_pem_slice(&read(&self.private_key).await?).map_err(|source| {
                TlsError::Pem {
                    path: self.private_key.clone(),
                    source,
                }
            })?;
        // Also checks that the key belongs to the certificate.
        Ok(CertifiedKey::from_der(chain, key, &provider())?)
    }
}

async fn read(path: &Path) -> Result<Vec<u8>, TlsError> {
    tokio::fs::read(path)
        .await
        .map_err(|source| TlsError::Read {
            path: path.to_owned(),
            source,
        })
}

fn self_signed() -> Result<CertifiedKey, TlsError> {
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(SELF_SIGNED_NAMES.map(String::from))?;
    let key = PrivatePkcs8KeyDer::from(signing_key.serialize_der());
    Ok(CertifiedKey::from_der(
        vec![cert.der().clone()],
        key.into(),
        &provider(),
    )?)
}

/// ring, which sqlx uses too. It is named rather than taken from the process default, which
/// rustls cannot choose if a dependency ever enables a second provider.
fn provider() -> CryptoProvider {
    ring::default_provider()
}

/// The TLS settings of the HTTPS listener: TLS 1.3 and 1.2 only (IF-08), presenting
/// `certificate`.
pub fn server_config(certificate: Arc<Certificate>) -> Arc<ServerConfig> {
    let mut config = ServerConfig::builder_with_provider(Arc::new(provider()))
        .with_protocol_versions(&[&TLS13, &TLS12])
        .expect("ring supports TLS 1.2 and 1.3")
        .with_no_client_auth()
        .with_cert_resolver(certificate);
    // The only protocol the listener speaks, so a client cannot mistake it for another service.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(config)
}

/// The HTTPS listener for `axum::serve`: it accepts TCP connections and hands them over once
/// their TLS handshake has completed.
pub struct TlsListener {
    local_addr: SocketAddr,
    handshaken: mpsc::Receiver<(TlsStream<TcpStream>, SocketAddr)>,
    accepting: JoinHandle<()>,
}

impl TlsListener {
    pub fn new(
        listener: TcpListener,
        config: Arc<ServerConfig>,
        handshake_timeout: Duration,
    ) -> io::Result<Self> {
        let local_addr = listener.local_addr()?;
        let (handshaken_tx, handshaken) = mpsc::channel(1);
        let accepting = tokio::spawn(accept(
            listener,
            TlsAcceptor::from(config),
            handshake_timeout,
            handshaken_tx,
        ));
        Ok(Self {
            local_addr,
            handshaken,
            accepting,
        })
    }
}

/// Accepts connections on `listener` and starts a handshake for each.
async fn accept(
    mut listener: TcpListener,
    acceptor: TlsAcceptor,
    handshake_timeout: Duration,
    handshaken: mpsc::Sender<(TlsStream<TcpStream>, SocketAddr)>,
) {
    loop {
        // axum's accept, which retries errors such as running out of file descriptors.
        let (stream, peer) = Listener::accept(&mut listener).await;
        let acceptor = acceptor.clone();
        let handshaken = handshaken.clone();
        tokio::spawn(async move {
            // Failures are routine, from scanners and clients that do not trust the certificate,
            // so they are not worth more than debug logs.
            match tokio::time::timeout(handshake_timeout, acceptor.accept(stream)).await {
                Ok(Ok(stream)) => {
                    // Fails only once the listener has closed.
                    let _ = handshaken.send((stream, peer)).await;
                }
                Ok(Err(err)) => debug!(error = %err, "TLS handshake failed"),
                Err(_) => debug!("TLS handshake timed out"),
            }
        });
    }
}

impl Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        self.handshaken
            .recv()
            .await
            .expect("the accept task runs until the listener is dropped")
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(self.local_addr)
    }
}

impl Drop for TlsListener {
    fn drop(&mut self) {
        // Closes the socket, so that shutdown stops new connections.
        self.accepting.abort();
    }
}

#[cfg(test)]
mod tests {
    use axum::{Router, routing::get};
    use rustls::{ClientConfig, RootCertStore, SupportedProtocolVersion, pki_types::ServerName};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::TlsConnector;

    use super::*;

    /// A certificate for `localhost` and its private key, PEM-encoded, and the certificate in DER.
    struct TestCertificate {
        certificate: String,
        private_key: String,
        der: CertificateDer<'static>,
    }

    fn generate() -> TestCertificate {
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        TestCertificate {
            certificate: cert.pem(),
            private_key: signing_key.serialize_pem(),
            der: cert.der().clone(),
        }
    }

    /// Certificate files in a temporary directory.
    struct TestFiles {
        _dir: tempfile::TempDir,
        config: config::Tls,
    }

    impl TestFiles {
        fn new(certificate: &TestCertificate) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let config = config::Tls {
                certificate_file: Some(dir.path().join("tls.crt")),
                private_key_file: Some(dir.path().join("tls.key")),
                ..config::Tls::default()
            };
            let files = Self { _dir: dir, config };
            files.write(&certificate.certificate, &certificate.private_key);
            files
        }

        fn write(&self, certificate: &str, private_key: &str) {
            std::fs::write(self.config.certificate_file.as_ref().unwrap(), certificate).unwrap();
            std::fs::write(self.config.private_key_file.as_ref().unwrap(), private_key).unwrap();
        }
    }

    /// Serves `GET /` over TLS on a free local port.
    async fn serve(certificate: Arc<Certificate>, handshake_timeout: Duration) -> SocketAddr {
        let listener = TlsListener::new(
            TcpListener::bind("127.0.0.1:0").await.unwrap(),
            server_config(certificate),
            handshake_timeout,
        )
        .unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new().route("/", get(|| async { "hello" }));
        tokio::spawn(axum::serve(listener, router).into_future());
        address
    }

    /// What a client sees of a TLS connection.
    struct Connection {
        version: rustls::ProtocolVersion,
        certificate: CertificateDer<'static>,
        response: String,
    }

    /// Sends `GET /` with a client that trusts `trusted` and speaks only `version`.
    async fn fetch(
        address: SocketAddr,
        trusted: &[&CertificateDer<'static>],
        version: &'static SupportedProtocolVersion,
    ) -> io::Result<Connection> {
        let mut roots = RootCertStore::empty();
        for certificate in trusted {
            roots.add((*certificate).clone()).unwrap();
        }
        let config = ClientConfig::builder_with_provider(Arc::new(provider()))
            .with_protocol_versions(&[version])
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let tcp = TcpStream::connect(address).await?;
        let mut tls = TlsConnector::from(Arc::new(config))
            .connect(ServerName::try_from("localhost").unwrap(), tcp)
            .await?;
        tls.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await?;
        let mut response = Vec::new();
        tls.read_to_end(&mut response).await?;
        let (_, session) = tls.get_ref();
        Ok(Connection {
            version: session.protocol_version().unwrap(),
            certificate: session.peer_certificates().unwrap()[0].clone(),
            response: String::from_utf8(response).unwrap(),
        })
    }

    #[tokio::test]
    async fn tls_1_3_and_1_2_are_accepted() {
        let generated = generate();
        let files = TestFiles::new(&generated);
        let certificate = Arc::new(Certificate::load(&files.config).await.unwrap());
        let address = serve(certificate, Duration::from_secs(10)).await;

        for (version, expected) in [
            (&TLS13, rustls::ProtocolVersion::TLSv1_3),
            (&TLS12, rustls::ProtocolVersion::TLSv1_2),
        ] {
            let connection = fetch(address, &[&generated.der], version).await.unwrap();

            assert_eq!(connection.version, expected);
            assert!(
                connection.response.starts_with("HTTP/1.1 200 OK"),
                "{}",
                connection.response
            );
        }
    }

    #[tokio::test]
    async fn versions_below_tls_1_2_are_refused() {
        let certificate = Arc::new(Certificate::load(&self_signed_config()).await.unwrap());
        let address = serve(certificate, Duration::from_secs(10)).await;
        // A ClientHello that offers TLS 1.1 at most. It has cipher suites and signature
        // algorithms that rustls supports for TLS 1.2, so that only the version is unacceptable.
        let mut hello = vec![
            0x16, 0x03, 0x01, 0x00, 0x3b, // handshake record, 59 bytes
            0x01, 0x00, 0x00, 0x37, // ClientHello, 55 bytes
            0x03, 0x02, // TLS 1.1
        ];
        hello.extend([0x42; 32]); // random
        hello.extend([
            0x00, // no session ID
            0x00, 0x04, 0xc0, 0x2b, 0xc0, 0x2f, // ECDHE AES-128-GCM suites
            0x01, 0x00, // no compression
            0x00, 0x0a, // extensions, 10 bytes
            0x00, 0x0d, 0x00, 0x06, 0x00, 0x04, 0x04, 0x03, 0x04,
            0x01, // signature algorithms
        ]);
        let mut stream = TcpStream::connect(address).await.unwrap();

        stream.write_all(&hello).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();

        // A fatal protocol_version alert.
        assert_eq!(response[0], 0x15, "{response:x?}");
        assert_eq!(response[5..7], [2, 70], "{response:x?}");
    }

    #[tokio::test]
    async fn a_silent_client_neither_blocks_others_nor_stays_connected() {
        let certificate = Arc::new(Certificate::load(&self_signed_config()).await.unwrap());
        let trusted = certificate.current().cert[0].clone();
        let address = serve(certificate, Duration::from_millis(200)).await;
        let mut silent = TcpStream::connect(address).await.unwrap();

        let connection = fetch(address, &[&trusted], &TLS13).await.unwrap();
        assert!(connection.response.starts_with("HTTP/1.1 200 OK"));

        let mut buf = [0; 1];
        let read = tokio::time::timeout(Duration::from_secs(5), silent.read(&mut buf))
            .await
            .expect("the server closes the connection after the handshake timeout");
        assert!(matches!(read, Ok(0)), "{read:?}");
    }

    #[tokio::test]
    async fn renewed_files_replace_the_certificate() {
        let (first, second) = (generate(), generate());
        let files = TestFiles::new(&first);
        let certificate = Arc::new(Certificate::load(&files.config).await.unwrap());
        let address = serve(certificate.clone(), Duration::from_secs(10)).await;
        let trusted = [&first.der, &second.der];
        assert!(!certificate.refresh().await.unwrap());

        files.write(&second.certificate, &second.private_key);
        assert!(certificate.refresh().await.unwrap());

        let connection = fetch(address, &trusted, &TLS13).await.unwrap();
        assert_eq!(connection.certificate, second.der);

        // Half-written renewal: the new certificate, but still the old key.
        files.write(&first.certificate, &second.private_key);
        assert!(matches!(certificate.refresh().await, Err(TlsError::Key(_))));
        let connection = fetch(address, &trusted, &TLS13).await.unwrap();
        assert_eq!(connection.certificate, second.der);
    }

    #[tokio::test]
    async fn the_reload_task_picks_up_renewed_files() {
        let (first, second) = (generate(), generate());
        let files = TestFiles::new(&first);
        let certificate = Arc::new(Certificate::load(&files.config).await.unwrap());
        let reload = tokio::spawn({
            let certificate = certificate.clone();
            async move { certificate.reload(Duration::from_millis(10)).await }
        });

        files.write(&second.certificate, &second.private_key);

        tokio::time::timeout(Duration::from_secs(5), async {
            while certificate.current().cert[0] != second.der {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the certificate is reloaded");
        reload.abort();
    }

    #[tokio::test]
    async fn files_that_cannot_be_used_are_rejected() {
        let (first, second) = (generate(), generate());
        let files = TestFiles::new(&first);
        let missing = files
            .config
            .certificate_file
            .as_ref()
            .unwrap()
            .with_file_name("missing.crt");

        let mut config = files.config.clone();
        config.certificate_file = Some(missing.clone());
        let err = Certificate::load(&config).await.unwrap_err();
        assert!(err.to_string().contains("missing.crt"), "{err}");

        files.write("not a certificate", &first.private_key);
        let err = Certificate::load(&files.config).await.unwrap_err();
        assert!(matches!(err, TlsError::Pem { .. }), "{err}");
        assert!(err.to_string().contains("tls.crt"), "{err}");

        files.write(&first.certificate, &second.private_key);
        let err = Certificate::load(&files.config).await.unwrap_err();
        assert!(matches!(err, TlsError::Key(_)), "{err}");
    }

    #[tokio::test]
    async fn the_self_signed_certificate_is_valid_for_localhost() {
        let certificate = Arc::new(Certificate::load(&self_signed_config()).await.unwrap());
        let trusted = certificate.current().cert[0].clone();
        let address = serve(certificate, Duration::from_secs(10)).await;

        // The client checks the certificate against the name `localhost`.
        let connection = fetch(address, &[&trusted], &TLS13).await.unwrap();

        assert!(connection.response.starts_with("HTTP/1.1 200 OK"));
    }

    fn self_signed_config() -> config::Tls {
        config::Tls {
            self_signed: true,
            ..config::Tls::default()
        }
    }
}
