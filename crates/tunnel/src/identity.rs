use aws_lc_rs::{
    rand::SystemRandom,
    signature::{ED25519, Ed25519KeyPair, KeyPair as _, UnparsedPublicKey},
};

use crate::frame::{NONCE_LEN, PROOF_LEN};

pub const PUBLIC_KEY_LEN: usize = 32;

const PROOF_CONTEXT: &[u8] = b"sdrmm-tunnel.1 device proof\0";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdentityError {
    #[error("could not generate a device key")]
    Generate,
    #[error("stored device key is not a valid Ed25519 PKCS#8 document")]
    Invalid,
    #[error("Ed25519 produced a {0}-byte value")]
    Size(usize),
}

#[derive(Debug)]
pub struct DeviceKey {
    pair: Ed25519KeyPair,
}

impl DeviceKey {
    pub fn generate() -> Result<(Self, Vec<u8>), IdentityError> {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
            .map_err(|_| IdentityError::Generate)?;
        let key = Self::from_pkcs8(pkcs8.as_ref())?;
        Ok((key, pkcs8.as_ref().to_vec()))
    }

    pub fn from_pkcs8(document: &[u8]) -> Result<Self, IdentityError> {
        Ed25519KeyPair::from_pkcs8(document)
            .map(|pair| Self { pair })
            .map_err(|_| IdentityError::Invalid)
    }

    pub fn public_key(&self) -> Result<[u8; PUBLIC_KEY_LEN], IdentityError> {
        let bytes = self.pair.public_key().as_ref();
        bytes
            .try_into()
            .map_err(|_| IdentityError::Size(bytes.len()))
    }

    pub fn prove(&self, nonce: &[u8; NONCE_LEN]) -> Result<[u8; PROOF_LEN], IdentityError> {
        let signature = self.pair.sign(&proof_message(nonce));
        let bytes = signature.as_ref();
        bytes
            .try_into()
            .map_err(|_| IdentityError::Size(bytes.len()))
    }
}

pub fn verify_proof(
    public_key: &[u8; PUBLIC_KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    proof: &[u8; PROOF_LEN],
) -> bool {
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(&proof_message(nonce), proof)
        .is_ok()
}

fn proof_message(nonce: &[u8; NONCE_LEN]) -> Vec<u8> {
    [PROOF_CONTEXT, nonce].concat()
}
