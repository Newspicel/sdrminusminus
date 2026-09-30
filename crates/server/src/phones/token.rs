use sdrmm_wire::phone::{PHONE_SECRET_BYTES, hex};
use sha2::{Digest, Sha256};

use super::PairError;

const ID_BYTES: usize = 8;

pub(super) fn random<const N: usize>() -> Result<[u8; N], PairError> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(|error| PairError::Random(error.to_string()))?;
    Ok(bytes)
}

pub(super) fn mint_id() -> Result<String, PairError> {
    Ok(format!("p{}", hex(&random::<ID_BYTES>()?)))
}

pub(super) fn mint_secret() -> Result<[u8; PHONE_SECRET_BYTES], PairError> {
    random()
}

pub(super) fn hash(secret: &[u8]) -> [u8; 32] {
    let mut out = [0; 32];
    out.copy_from_slice(&Sha256::digest(secret));
    out
}
