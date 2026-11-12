//! Host-owned certificate files; Quinnet owns loading and TLS configuration.

use crate::{NetworkError, ServerNetworkConfig};
use bevy_quinnet::{
    client::certificate::{
        CertStore, CertVerificationStatus, CertVerifierAction, CertVerifierBehaviour,
        CertificateVerificationMode, KnownHosts, TrustOnFirstUseConfig,
    },
    server::certificate::CertificateRetrievalMode,
};
use std::{collections::HashMap, fs};

pub(crate) fn client_mode(
    policy: &crate::CertificatePolicy,
    trusted: &CertStore,
) -> CertificateVerificationMode {
    use crate::CertificatePolicy;
    match policy {
        CertificatePolicy::SystemRoots => CertificateVerificationMode::SignedByCertificateAuthority,
        CertificatePolicy::Insecure => CertificateVerificationMode::SkipVerification,
        CertificatePolicy::TrustOnFirstUse => {
            CertificateVerificationMode::TrustOnFirstUse(TrustOnFirstUseConfig {
                known_hosts: KnownHosts::Store(trusted.clone()),
                verifier_behaviour: HashMap::from([
                    (
                        CertVerificationStatus::UnknownCertificate,
                        CertVerifierBehaviour::ImmediateAction(CertVerifierAction::TrustAndStore),
                    ),
                    (
                        CertVerificationStatus::TrustedCertificate,
                        CertVerifierBehaviour::ImmediateAction(CertVerifierAction::TrustOnce),
                    ),
                    (
                        CertVerificationStatus::UntrustedCertificate,
                        CertVerifierBehaviour::ImmediateAction(CertVerifierAction::AbortConnection),
                    ),
                ]),
            })
        }
    }
}

pub(crate) fn server_files(
    config: &ServerNetworkConfig,
) -> Result<CertificateRetrievalMode, NetworkError> {
    fs::create_dir_all(&config.certificate_directory).map_err(NetworkError::from_display)?;
    let certificate = config.certificate_directory.join("server_cert.pem");
    let key = config.certificate_directory.join("server_key.pem");
    if !(certificate.is_file() && key.is_file()) {
        if !config.generate_self_signed_certificate {
            return Err(NetworkError::new(
                "configured server certificate or private key is missing",
            ));
        }
        // Preserve the existing multi-SAN certificate provisioning contract;
        // Quinnet's GenerateSelfSigned accepts only one hostname.
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(config.server_alternative_names.clone())
                .map_err(NetworkError::from_display)?;
        fs::write(&certificate, cert.pem()).map_err(NetworkError::from_display)?;
        fs::write(&key, signing_key.serialize_pem()).map_err(NetworkError::from_display)?;
    }
    Ok(CertificateRetrievalMode::LoadFromFile {
        cert_file: certificate
            .to_str()
            .ok_or_else(|| NetworkError::new("certificate path is not UTF-8"))?
            .to_owned(),
        key_file: key
            .to_str()
            .ok_or_else(|| NetworkError::new("private key path is not UTF-8"))?
            .to_owned(),
    })
}
