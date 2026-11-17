//! The server verifier of a pairing client: the coordinator's fixed TLS name.
//!
//! Every coordinator certificate issued since 0.12.0 carries [`super::COORDINATOR_TLS_NAME`]
//! as a DNS name, so a client verifies the server against that one name whatever address it
//! dialed. [`FixedNameVerifier`] wraps `rustls`'s own `webpki` verifier -- chain, validity and
//! signature checks are all `rustls`'s; only the name that is checked differs. Trust still
//! comes from the CA the client was built with (the CA pin).
//!
//! A certificate without the fixed name (a coordinator that was not migrated yet) is checked
//! against the name the client dialed, exactly as a plain [`super::client_config`] would.

use rustls::{
    CertificateError, DigitallySignedStruct, Error, SignatureScheme,
    client::{
        WebPkiServerVerifier,
        danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    },
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use std::sync::Arc;

/// Verifies the server against [`super::COORDINATOR_TLS_NAME`] first and the dialed name
/// second.
#[derive(Debug)]
pub(super) struct FixedNameVerifier {
    inner: Arc<WebPkiServerVerifier>,
    fixed: ServerName<'static>,
}

impl FixedNameVerifier {
    /// Builds the verifier over `roots`.
    ///
    /// # Errors
    ///
    /// A `rustls` error if `roots` is empty or the fixed name does not parse.
    pub(super) fn new(roots: rustls::RootCertStore) -> Result<Self, Error> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let inner = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider)
            .build()
            .map_err(|e| Error::General(e.to_string()))?;
        let fixed = ServerName::try_from(super::COORDINATOR_TLS_NAME)
            .map_err(|e| Error::General(format!("invalid coordinator TLS name: {e}")))?
            .to_owned();
        Ok(Self { inner, fixed })
    }
}

impl ServerCertVerifier for FixedNameVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        match self.inner.verify_server_cert(
            end_entity,
            intermediates,
            &self.fixed,
            ocsp_response,
            now,
        ) {
            Err(Error::InvalidCertificate(
                CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. },
            )) => self.inner.verify_server_cert(
                end_entity,
                intermediates,
                server_name,
                ocsp_response,
                now,
            ),
            other => other,
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}
