use std::{error::Error as StdError, sync::Arc};

use rustls::{
    CertificateError, DigitallySignedStruct, Error, OtherError, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use sdrmm_wire::phone;

use super::Seen;

const MAX_CHAIN_DEPTH: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("certificate {seen} is not pinned")]
pub(crate) struct PinMismatch {
    pub(crate) seen: String,
}

#[derive(Debug)]
pub(crate) struct PinnedVerifier {
    pins: Vec<String>,
    provider: Arc<CryptoProvider>,
}

impl PinnedVerifier {
    pub(crate) fn new(pins: Vec<String>, provider: Arc<CryptoProvider>) -> Self {
        Self { pins, provider }
    }
}

#[derive(Debug)]
pub(crate) struct RecordingVerifier {
    seen: Seen,
    provider: Arc<CryptoProvider>,
}

impl RecordingVerifier {
    pub(crate) fn new(seen: Seen, provider: Arc<CryptoProvider>) -> Self {
        Self { seen, provider }
    }
}

pub(crate) fn fingerprint_of(der: &[u8]) -> Option<String> {
    phone::spki_pin(der).ok()
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn mismatch(seen: String) -> Error {
    Error::InvalidCertificate(CertificateError::Other(OtherError(Arc::new(PinMismatch {
        seen,
    }))))
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let seen = fingerprint_of(end_entity).unwrap_or_default();
        if self
            .pins
            .iter()
            .any(|pin| same(pin.as_bytes(), seen.as_bytes()))
        {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(mismatch(seen))
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
            &self.provider.signature_verification_algorithms,
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
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

impl ServerCertVerifier for RecordingVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let seen = fingerprint_of(end_entity)
            .ok_or(Error::InvalidCertificate(CertificateError::BadEncoding))?;
        self.seen.record(seen);
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
            &self.provider.signature_verification_algorithms,
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
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub(crate) fn pin_mismatch_in(error: &(dyn StdError + 'static)) -> Option<String> {
    search(error, MAX_CHAIN_DEPTH)
}

fn search(error: &(dyn StdError + 'static), depth: usize) -> Option<String> {
    let mut current = Some(error);
    let mut steps = 0;
    while let Some(error) = current {
        if steps == depth {
            return None;
        }
        steps += 1;
        if let Some(found) = direct(error) {
            return Some(found);
        }
        if let Some(inner) = error
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
            && let Some(found) = search(inner, depth - steps)
        {
            return Some(found);
        }
        current = error.source();
    }
    None
}

fn direct(error: &(dyn StdError + 'static)) -> Option<String> {
    if let Some(mismatch) = error.downcast_ref::<PinMismatch>() {
        return Some(mismatch.seen.clone());
    }
    match error.downcast_ref::<Error>()? {
        Error::InvalidCertificate(CertificateError::Other(OtherError(inner))) => inner
            .downcast_ref::<PinMismatch>()
            .map(|mismatch| mismatch.seen.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use super::*;
    use crate::{stub_server::StubIdentity, tls::provider};

    fn verify(verifier: &dyn ServerCertVerifier, identity: &StubIdentity) -> Result<(), Error> {
        let name = ServerName::try_from("localhost").expect("name");
        verifier
            .verify_server_cert(&identity.cert, &[], &name, &[], UnixTime::now())
            .map(|_| ())
    }

    #[test]
    fn the_pinned_verifier_accepts_the_pinned_certificate() {
        let identity = StubIdentity::generate();
        let verifier = PinnedVerifier::new(vec![identity.pin.clone()], provider());
        assert!(verify(&verifier, &identity).is_ok());
        assert_eq!(fingerprint_of(&identity.cert), Some(identity.pin));
    }

    #[test]
    fn the_pinned_verifier_refuses_another_certificate_and_reports_what_it_saw() {
        let pinned = StubIdentity::generate();
        let other = StubIdentity::generate();
        let verifier = PinnedVerifier::new(vec![pinned.pin.clone()], provider());
        let refused = verify(&verifier, &other).expect_err("another key");
        assert_eq!(pin_mismatch_in(&refused), Some(other.pin));
    }

    #[test]
    fn the_pinned_verifier_accepts_either_of_two_pins() {
        let first = StubIdentity::generate();
        let second = StubIdentity::generate();
        let verifier = PinnedVerifier::new(vec![first.pin.clone(), second.pin.clone()], provider());
        assert!(verify(&verifier, &first).is_ok());
        assert!(verify(&verifier, &second).is_ok());
        assert!(verify(&verifier, &StubIdentity::generate()).is_err());
    }

    #[test]
    fn an_unparsable_certificate_is_a_mismatch() {
        let identity = StubIdentity::generate();
        let verifier = PinnedVerifier::new(vec![identity.pin], provider());
        let name = ServerName::try_from("localhost").expect("name");
        let junk = CertificateDer::from(vec![0x30, 0x03, 0x02, 0x01, 0x00]);
        let refused = verifier
            .verify_server_cert(&junk, &[], &name, &[], UnixTime::now())
            .expect_err("junk");
        assert_eq!(pin_mismatch_in(&refused), Some(String::new()));
    }

    #[derive(Debug)]
    struct Wrapper(Box<dyn StdError + Send + Sync>);

    impl fmt::Display for Wrapper {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("request failed")
        }
    }

    impl StdError for Wrapper {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            Some(self.0.as_ref())
        }
    }

    #[test]
    fn pin_mismatch_is_found_through_a_wrapped_error_chain() {
        let seen = "ab".repeat(32);
        let io = std::io::Error::new(std::io::ErrorKind::InvalidData, mismatch(seen.clone()));
        let wrapped = Wrapper(Box::new(Wrapper(Box::new(io))));
        assert_eq!(pin_mismatch_in(&wrapped), Some(seen));
        let unrelated = Wrapper(Box::new(std::io::Error::other("reset")));
        assert_eq!(pin_mismatch_in(&unrelated), None);
    }
}
