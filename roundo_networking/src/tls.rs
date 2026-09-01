//! TLS configuration for the QUIC transport.

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
use std::fs;
use std::path::Path;
use std::sync::{Arc, RwLock};

pub(crate) struct PreparedServerTls {
    pub config: Arc<ServerConfig>,
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

    let certificates = CertificateDer::pem_file_iter(&certificate_path)
        .map_err(NetworkError::from_display)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(NetworkError::from_display)?;
    let private_key =
        PrivateKeyDer::from_pem_file(&private_key_path).map_err(NetworkError::from_display)?;
    let config = ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(NetworkError::from_display)?
    .with_no_client_auth()
    .with_single_cert(certificates, private_key)
    .map_err(NetworkError::from_display)?;

    Ok(PreparedServerTls {
        config: Arc::new(config),
    })
}

pub(crate) fn create_client_tls_config(
    policy: &CertificatePolicy,
) -> Result<Arc<ClientConfig>, NetworkError> {
    let builder = ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(NetworkError::from_display)?;
    let config = match policy {
        CertificatePolicy::SystemRoots => builder
            .with_root_certificates(system_roots())
            .with_no_client_auth(),
        CertificatePolicy::TrustOnFirstUse => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(TrustOnFirstUseVerifier::new()))
            .with_no_client_auth(),
        CertificatePolicy::Insecure => builder
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
        let verifier = WebPkiServerVerifier::builder_with_provider(
            Arc::clone(&self.roots),
            Arc::new(rustls::crypto::aws_lc_rs::default_provider()),
        )
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
