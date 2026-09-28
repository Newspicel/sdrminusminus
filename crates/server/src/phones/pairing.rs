use jiff::{SignedDuration, Timestamp};
use sdrmm_wire::{
    API_PROTOCOL, OfferState, PairRequest, PairResponse, PairUri, PairingOffer, PairingOfferStatus,
    PhoneEndpoint, PhoneToken,
    phone::{PAIR_OFFER_TTL_SECS, hex, key_check, valid_phone_name},
};

use super::{Phones, token};
use crate::{
    StoreError,
    auth::bytes_eq,
    store::{OfferFailure, OfferRow, PairWrite, PhoneRow, rfc3339},
};

const CODE_SPACE: u32 = 100_000_000;
const CODE_DRAW_LIMIT: u32 = 4_200_000_000;
const OFFER_ID_BYTES: usize = 8;

#[derive(Debug, thiserror::Error)]
pub(crate) enum PairError {
    #[error("No pairing code is open")]
    NoOffer,
    #[error("Code expired")]
    Expired,
    #[error("Wrong code, {left} tries left")]
    WrongCode { left: u32 },
    #[error("Too many tries, make a new code")]
    Burned,
    #[error("The app speaks protocol {phone}, this server {server}")]
    Protocol { phone: u32, server: u32 },
    #[error("Name must be 1 to 64 characters")]
    Name,
    #[error("Turn on Allow phones")]
    NoEndpoint,
    #[error("No randomness: {0}")]
    Random(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub(super) fn draw_code(
    mut draw: impl FnMut() -> Result<u32, PairError>,
) -> Result<String, PairError> {
    loop {
        let value = draw()?;
        if value < CODE_DRAW_LIMIT {
            return Ok(format!("{:08}", value % CODE_SPACE));
        }
    }
}

fn pair_uri(endpoint: &PhoneEndpoint, code: &str, server_name: &str) -> String {
    PairUri {
        hosts: endpoint.hosts.clone(),
        code: code.to_owned(),
        pin: endpoint.pin.clone(),
        protocol: API_PROTOCOL,
        name: Some(server_name.to_owned()),
    }
    .to_uri()
}

fn later(now: Timestamp, seconds: i64) -> Result<Timestamp, PairError> {
    now.checked_add(SignedDuration::from_secs(seconds))
        .map_err(|_| PairError::Store(StoreError::Timestamp(rfc3339(now))))
}

fn trimmed_name(name: &str) -> Result<String, PairError> {
    let name = name.trim();
    if valid_phone_name(name) {
        Ok(name.to_owned())
    } else {
        Err(PairError::Name)
    }
}

impl Phones {
    pub(crate) fn create_offer(
        &self,
        name: Option<&str>,
        endpoint: &PhoneEndpoint,
        server_name: &str,
        now: Timestamp,
    ) -> Result<PairingOffer, PairError> {
        let name = name.map(trimmed_name).transpose()?;
        let code = draw_code(|| token::random::<4>().map(u32::from_le_bytes))?;
        let row = OfferRow {
            id: hex(&token::random::<OFFER_ID_BYTES>()?),
            code,
            name,
            created_at: rfc3339(now),
            expires_at: rfc3339(later(now, PAIR_OFFER_TTL_SECS)?),
            failures: 0,
            state: OfferState::Live,
        };
        self.store.open_offer(&row)?;
        crate::diagnostics::hide_secret(&row.code);
        Ok(PairingOffer {
            uri: pair_uri(endpoint, &row.code, server_name),
            key_check: key_check(&endpoint.pin),
            id: row.id,
            code: row.code,
            expires_at: row.expires_at,
            endpoint: endpoint.clone(),
        })
    }

    pub(crate) fn cancel_offer(&self) -> Result<bool, StoreError> {
        self.store.cancel_offer()
    }

    pub(crate) fn offer_status(
        &self,
        endpoint: Option<&PhoneEndpoint>,
        server_name: &str,
        now: Timestamp,
    ) -> Result<Option<PairingOfferStatus>, StoreError> {
        self.store.live_offer(&rfc3339(now))?;
        let Some(offer) = self.store.latest_offer()? else {
            return Ok(None);
        };
        let live = offer.state == OfferState::Live;
        Ok(Some(PairingOfferStatus {
            uri: endpoint
                .filter(|_| live)
                .map(|endpoint| pair_uri(endpoint, &offer.code, server_name)),
            code: live.then_some(offer.code),
            id: offer.id,
            state: offer.state,
            expires_at: offer.expires_at,
            failures: offer.failures,
        }))
    }

    pub(crate) fn pair(
        &self,
        request: &PairRequest,
        server_id: &str,
        server_name: &str,
        now: Timestamp,
    ) -> Result<PairResponse, PairError> {
        if request.protocol != API_PROTOCOL {
            return Err(PairError::Protocol {
                phone: request.protocol,
                server: API_PROTOCOL,
            });
        }
        let name = trimmed_name(&request.name)?;
        let offer = self.live_offer(now)?;
        if !bytes_eq(request.code.as_bytes(), offer.code.as_bytes()) {
            return Err(match self.store.fail_offer(&offer.id) {
                Ok(OfferFailure::Counted { left }) => PairError::WrongCode { left },
                Ok(OfferFailure::Burned) => PairError::Burned,
                Err(StoreError::OfferGone) => PairError::NoOffer,
                Err(error) => PairError::Store(error),
            });
        }
        let secret = token::mint_secret()?;
        let secret_sha256 = token::hash(&secret);
        let write = match self.rebound(request.rebind.as_deref()) {
            Some(id) => PairWrite::Rotate {
                id,
                name,
                platform: request.platform,
                secret_sha256,
            },
            None => PairWrite::New(PhoneRow {
                id: token::mint_id()?,
                name,
                platform: request.platform,
                secret_sha256,
                created_at: rfc3339(now),
                last_seen: None,
            }),
        };
        let id = match &write {
            PairWrite::New(row) => row.id.clone(),
            PairWrite::Rotate { id, .. } => id.clone(),
        };
        {
            let _step = self.in_step_with_the_store();
            match self.store.pair_with_offer(&offer.id, &write) {
                Ok(()) => {}
                Err(StoreError::OfferGone) => return Err(PairError::NoOffer),
                Err(error) => return Err(error.into()),
            }
            self.remember(&id, secret_sha256);
        }
        Ok(PairResponse {
            phone: self.one(&id, None)?,
            token: PhoneToken::new(id, secret).encode(),
            server_id: server_id.to_owned(),
            server_name: server_name.to_owned(),
            protocol: API_PROTOCOL,
        })
    }

    fn live_offer(&self, now: Timestamp) -> Result<OfferRow, PairError> {
        if let Some(offer) = self.store.live_offer(&rfc3339(now))? {
            return Ok(offer);
        }
        Err(match self.store.latest_offer()?.map(|offer| offer.state) {
            Some(OfferState::Expired) => PairError::Expired,
            Some(OfferState::Burned) => PairError::Burned,
            _ => PairError::NoOffer,
        })
    }

    fn rebound(&self, rebind: Option<&str>) -> Option<String> {
        let token = PhoneToken::parse(rebind?)?;
        self.verify(&token).then_some(token.phone)
    }
}
