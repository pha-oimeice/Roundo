//! TLS transport primitives shared by the Roundo client and server.

use crate::service::{CertificatePolicy, NetworkError};
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{
    ClientConfig, DigitallySignedStruct, Error, RootCertStore, ServerConfig, SignatureScheme,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::fs;
use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, RwLock};
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tokio_rustls::{TlsAcceptor, TlsConnector};

pub type ClientTlsStream = tokio_rustls::client::TlsStream<TcpStream>;
pub type ServerTlsStream = tokio_rustls::server::TlsStream<TcpStream>;

pub(crate) struct PreparedServerTls {
    pub config: Arc<ServerConfig>,
    pub certificate_pem: String,
}

pub(crate) fn prepare_server_tls(
    certificate_directory: &Path,
    subject_alt_names: &[String],
    generate_self_signed_certificate: bool,
) -> Result<PreparedServerTls, NetworkError> {
    fs::create_dir_all(certificate_directory).map_err(NetworkError::from_display)?;
    let certificate_path = certificate_directory.join("server_cert.pem");
    let private_key_path = certificate_directory.join("server_key.pem");

    if !(certificate_path.is_file() && private_key_path.is_file()) {
        if !generate_self_signed_certificate {
            return Err(NetworkError::new(format!(
                "TLS certificate or key is missing in {}",
                certificate_directory.display()
            )));
        }
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(subject_alt_names.to_vec())
                .map_err(NetworkError::from_display)?;
        fs::write(&certificate_path, cert.pem()).map_err(NetworkError::from_display)?;
        fs::write(&private_key_path, signing_key.serialize_pem())
            .map_err(NetworkError::from_display)?;
    }

    let certificate_pem =
        fs::read_to_string(&certificate_path).map_err(NetworkError::from_display)?;
    let certificates = CertificateDer::pem_file_iter(&certificate_path)
        .map_err(NetworkError::from_display)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(NetworkError::from_display)?;
    let private_key =
        PrivateKeyDer::from_pem_file(&private_key_path).map_err(NetworkError::from_display)?;
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certificates, private_key)
        .map_err(NetworkError::from_display)?;

    Ok(PreparedServerTls {
        config: Arc::new(config),
        certificate_pem,
    })
}

pub(crate) fn create_client_tls_config(
    policy: &CertificatePolicy,
) -> Result<Arc<ClientConfig>, NetworkError> {
    let config = match policy {
        CertificatePolicy::SystemRoots => ClientConfig::builder()
            .with_root_certificates(system_roots())
            .with_no_client_auth(),
        CertificatePolicy::TrustOnFirstUse => ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(TrustOnFirstUseVerifier::new()))
            .with_no_client_auth(),
        CertificatePolicy::Insecure => ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(InsecureVerifier))
            .with_no_client_auth(),
    };
    Ok(Arc::new(config))
}

fn system_roots() -> RootCertStore {
    let mut roots = RootCertStore::empty();
    roots.roots = Vec::from(webpki_roots::TLS_SERVER_ROOTS);
    roots
}

#[derive(Debug)]
struct TrustOnFirstUseVerifier {
    roots: Arc<RootCertStore>,
    fingerprints: Arc<RwLock<HashMap<String, [u8; 32]>>>,
}

impl TrustOnFirstUseVerifier {
    fn new() -> Self {
        Self {
            roots: Arc::new(system_roots()),
            fingerprints: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl ServerCertVerifier for TrustOnFirstUseVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let verifier = WebPkiServerVerifier::builder(Arc::clone(&self.roots))
            .build()
            .map_err(|error| Error::General(error.to_string()))?;
        if let Ok(verified) =
            verifier.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
        {
            return Ok(verified);
        }

        let server_name = server_name.to_str().to_string();
        let fingerprint: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        let mut fingerprints = self
            .fingerprints
            .write()
            .map_err(|_| Error::General("certificate trust store lock poisoned".into()))?;
        match fingerprints.get(&server_name) {
            Some(trusted) if trusted == &fingerprint => Ok(ServerCertVerified::assertion()),
            Some(_) => Err(Error::General("server certificate changed".into())),
            None => {
                fingerprints.insert(server_name, fingerprint);
                Ok(ServerCertVerified::assertion())
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::aws_lc_rs::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::aws_lc_rs::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::aws_lc_rs::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[derive(Debug)]
struct InsecureVerifier;

impl ServerCertVerifier for InsecureVerifier {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::aws_lc_rs::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::aws_lc_rs::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::aws_lc_rs::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// A TCP listener configured to upgrade accepted connections to TLS.
pub struct TlsServer {
    listener: TcpListener,
    acceptor: TlsAcceptor,
}

impl TlsServer {
    pub async fn bind<A>(address: A, config: Arc<ServerConfig>) -> io::Result<Self>
    where
        A: ToSocketAddrs,
    {
        Ok(Self {
            listener: TcpListener::bind(address).await?,
            acceptor: TlsAcceptor::from(config),
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Accept a TCP connection. Call [`TlsIncoming::handshake`] in a separate
    /// task so a slow or abandoned handshake never blocks later connections.
    pub async fn accept(&self) -> io::Result<TlsIncoming> {
        let (stream, peer_addr) = self.listener.accept().await?;
        Ok(TlsIncoming {
            stream,
            peer_addr,
            acceptor: self.acceptor.clone(),
        })
    }
}

/// A connected peer that has not completed its TLS handshake yet.
pub struct TlsIncoming {
    stream: TcpStream,
    peer_addr: SocketAddr,
    acceptor: TlsAcceptor,
}

impl TlsIncoming {
    pub fn peer_addr(&self) -> SocketAddr {
        self.peer_addr
    }

    /// Upgrade this connection to TLS without treating peer disconnects as a
    /// server failure.
    pub async fn handshake(self) -> Result<ServerTlsStream, TlsHandshakeError> {
        self.acceptor
            .accept(self.stream)
            .await
            .map_err(|source| TlsHandshakeError {
                peer_addr: self.peer_addr,
                peer_disconnected: is_peer_disconnect(&source),
                source,
            })
    }
}

/// A TLS handshake failure annotated with the remote peer and its severity.
#[derive(Debug)]
pub struct TlsHandshakeError {
    peer_addr: SocketAddr,
    peer_disconnected: bool,
    source: io::Error,
}

impl TlsHandshakeError {
    pub fn peer_addr(&self) -> SocketAddr {
        self.peer_addr
    }

    /// Whether the peer closed or reset the TCP connection during the TLS handshake.
    pub fn is_peer_disconnect(&self) -> bool {
        self.peer_disconnected
    }
}

impl Display for TlsHandshakeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "TLS handshake with {} failed: {}",
            self.peer_addr, self.source
        )
    }
}

impl std::error::Error for TlsHandshakeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// Connect a client TCP stream and complete its TLS handshake.
pub async fn connect<A>(
    address: A,
    server_name: ServerName<'static>,
    config: Arc<ClientConfig>,
) -> io::Result<ClientTlsStream>
where
    A: ToSocketAddrs,
{
    let stream = TcpStream::connect(address).await?;
    TlsConnector::from(config)
        .connect(server_name, stream)
        .await
}

fn is_peer_disconnect(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
    )
}

#[cfg(test)]
mod tests {
    use super::{TlsServer, connect};
    use rcgen::generate_simple_self_signed;
    use rustls::{
        ClientConfig, RootCertStore, ServerConfig,
        pki_types::{PrivateKeyDer, ServerName},
    };
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream;

    #[tokio::test]
    async fn peer_disconnect_before_client_hello_is_a_recoverable_handshake_error() {
        let server = TlsServer::bind("127.0.0.1:0", test_server_config())
            .await
            .expect("test TLS listener should bind");
        let address = server
            .local_addr()
            .expect("test TLS listener should expose its address");

        let mut client = TcpStream::connect(address)
            .await
            .expect("test TCP client should connect");
        client
            .shutdown()
            .await
            .expect("test TCP client should close its write side");

        let error = server
            .accept()
            .await
            .expect("test TLS listener should accept the TCP connection")
            .handshake()
            .await
            .expect_err("a client that sends no ClientHello must not complete TLS");

        assert!(error.is_peer_disconnect());
    }

    #[tokio::test]
    async fn client_and_server_complete_a_tls_handshake() {
        let (server_config, client_config) = test_configs();
        let server = TlsServer::bind("127.0.0.1:0", server_config)
            .await
            .expect("test TLS listener should bind");
        let address = server
            .local_addr()
            .expect("test TLS listener should expose its address");
        let server_task = tokio::spawn(async move {
            server
                .accept()
                .await
                .expect("test TLS listener should accept")
                .handshake()
                .await
                .expect("test TLS handshake should succeed")
        });

        let client = connect(
            address,
            ServerName::try_from("localhost").expect("static server name should be valid"),
            client_config,
        )
        .await
        .expect("test TLS client handshake should succeed");
        let server_stream = server_task
            .await
            .expect("test TLS server task should not panic");

        drop(client);
        drop(server_stream);
    }

    fn test_server_config() -> Arc<ServerConfig> {
        let certificate = generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("test certificate should generate");
        let certificate_der = certificate.cert.der().clone();
        let key_der = PrivateKeyDer::Pkcs8(certificate.signing_key.serialize_der().into());
        Arc::new(
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![certificate_der], key_der)
                .expect("test TLS server config should build"),
        )
    }

    fn test_configs() -> (Arc<ServerConfig>, Arc<ClientConfig>) {
        let certificate = generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("test certificate should generate");
        let certificate_der = certificate.cert.der().clone();
        let key_der = PrivateKeyDer::Pkcs8(certificate.signing_key.serialize_der().into());
        let server_config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![certificate_der.clone()], key_der)
            .expect("test TLS server config should build");
        let mut roots = RootCertStore::empty();
        roots
            .add(certificate_der)
            .expect("test certificate should enter root store");
        let client_config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        (Arc::new(server_config), Arc::new(client_config))
    }
}
