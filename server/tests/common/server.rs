//! The server, started against a test database on free local ports, with a certificate that
//! [`connect`] trusts. The Host Agent's tests use this module too.

use std::{
    net::SocketAddr,
    path::Path,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use rustls::{ClientConfig, RootCertStore, crypto::ring, pki_types::CertificateDer};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
};
use tokio_rustls::{TlsConnector, client::TlsStream};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{self, client::IntoClientRequest, http::header},
};
use twoviewer_server::{
    app::{self, RunError},
    config::{Config, DatabaseUrl, Secrets},
};

use super::LOCK_TIMEOUT;

/// A test certificate authority, and a certificate for `localhost` that it signed.
struct Pki {
    authority: CertificateDer<'static>,
    certificate_pem: String,
    private_key_pem: String,
}

fn pki() -> &'static Pki {
    static PKI: OnceLock<Pki> = OnceLock::new();
    PKI.get_or_init(|| {
        let mut authority = CertificateParams::new(Vec::<String>::new()).unwrap();
        authority.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let authority_key = KeyPair::generate().unwrap();
        let authority_certificate = authority.self_signed(&authority_key).unwrap();
        let issuer = Issuer::new(authority, authority_key);

        let key = KeyPair::generate().unwrap();
        let certificate = CertificateParams::new(vec!["localhost".to_owned()])
            .unwrap()
            .signed_by(&key, &issuer)
            .unwrap();
        Pki {
            authority: authority_certificate.der().clone(),
            certificate_pem: certificate.pem(),
            private_key_pem: key.serialize_pem(),
        }
    })
}

/// TLS settings that trust the test certificate authority, and only it.
pub fn client_tls() -> Arc<ClientConfig> {
    let mut roots = RootCertStore::empty();
    roots.add(pki().authority.clone()).unwrap();
    Arc::new(
        ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

/// A local address that was free a moment ago.
pub fn free_address() -> SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

/// A server started with [`start`].
pub struct Server {
    pub https: SocketAddr,
    pub monitoring: SocketAddr,
    stop: oneshot::Sender<()>,
    pub task: JoinHandle<Result<(), RunError>>,
    /// Holds the certificate files.
    _tls: TempDir,
}

/// Starts the server against `url`, with the settings that `configure` changes.
pub async fn start(url: &str, configure: impl FnOnce(&mut Config)) -> Server {
    let tls = TempDir::new().unwrap();
    let certificate_file = tls.path().join("tls.crt");
    let private_key_file = tls.path().join("tls.key");
    write(&certificate_file, &pki().certificate_pem);
    write(&private_key_file, &pki().private_key_pem);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let monitoring = listener.local_addr().unwrap();
    let mut config = Config::default();
    config.http.listen = free_address();
    config.http.redirect_listen = "127.0.0.1:0".parse().unwrap();
    config.tls.certificate_file = Some(certificate_file);
    config.tls.private_key_file = Some(private_key_file);
    config.monitoring.readiness_timeout = Duration::from_millis(500);
    config.database.migration_lock_timeout = LOCK_TIMEOUT;
    configure(&mut config);
    let https = config.http.listen;
    let secrets = Secrets {
        database_url: DatabaseUrl::new(url),
    };
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(app::run(config, secrets, listener, async {
        let _ = stopped.await;
    }));
    Server {
        https,
        monitoring,
        stop,
        task,
        _tls: tls,
    }
}

fn write(path: &Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
}

impl Server {
    /// The status and body of `GET path` on the monitoring listener.
    pub async fn get(&self, path: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(self.monitoring).await.unwrap();
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        let status = head.split(' ').nth(1).unwrap().parse().unwrap();
        (status, body.to_owned())
    }

    /// Waits until `GET path` answers `status`, for at most 30 seconds.
    pub async fn wait_for(&self, path: &str, status: u16) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while self.get(path).await.0 != status {
            assert!(Instant::now() < deadline, "{path} never answered {status}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The value of the metric line that starts with `name `.
    pub async fn metric(&self, name: &str) -> f64 {
        let (_, metrics) = self.get("/metrics").await;
        metrics
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{name} ")))
            .unwrap_or_else(|| panic!("no {name} in {metrics}"))
            .parse()
            .unwrap()
    }

    /// Waits until the metric `name` reads `value`, for at most 30 seconds.
    pub async fn wait_for_metric(&self, name: &str, value: f64) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while self.metric(name).await != value {
            assert!(Instant::now() < deadline, "{name} never read {value}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The origin of the pages this server serves.
    pub fn origin(&self) -> String {
        format!("https://localhost:{}", self.https.port())
    }

    /// Requests shutdown without waiting for it.
    pub fn stop(self) -> JoinHandle<Result<(), RunError>> {
        let _ = self.stop.send(());
        self.task
    }

    /// Requests shutdown and returns the result of the run.
    pub async fn shut_down(self) -> Result<(), RunError> {
        tokio::time::timeout(Duration::from_secs(10), self.stop())
            .await
            .expect("the server stops promptly")
            .unwrap()
    }
}

pub type Socket = WebSocketStream<TlsStream<TcpStream>>;

/// Opens a WebSocket to `path` on `server`, sending `origin` if there is one.
pub async fn connect(
    server: &Server,
    path: &str,
    origin: Option<&str>,
) -> Result<Socket, tungstenite::Error> {
    let tcp = TcpStream::connect(server.https).await?;
    let tls = TlsConnector::from(client_tls())
        .connect("localhost".try_into().unwrap(), tcp)
        .await?;
    let mut request =
        format!("wss://localhost:{}{path}", server.https.port()).into_client_request()?;
    if let Some(origin) = origin {
        request
            .headers_mut()
            .insert(header::ORIGIN, origin.parse().unwrap());
    }
    let (socket, _) = tokio_tungstenite::client_async(request, tls).await?;
    Ok(socket)
}
