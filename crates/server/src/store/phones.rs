use rusqlite::{OptionalExtension, Row, TransactionBehavior, params, types::Type};
use sdrmm_wire::{OfferState, PhonePlatform, phone::PAIR_MAX_FAILURES};

use super::{Store, StoreError, now_rfc3339, rfc3339};

const OFFER_KEEP: jiff::SignedDuration = jiff::SignedDuration::from_hours(24);
const PHONE_COLUMNS: &str = "id, name, platform, secret_sha256, created_at, last_seen";
const OFFER_COLUMNS: &str = "id, code, name, created_at, expires_at, failures, state, phone";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PhoneRow {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) platform: PhonePlatform,
    pub(crate) secret_sha256: [u8; 32],
    pub(crate) created_at: String,
    pub(crate) last_seen: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OfferRow {
    pub(crate) id: String,
    pub(crate) code: String,
    pub(crate) name: Option<String>,
    pub(crate) created_at: String,
    pub(crate) expires_at: String,
    pub(crate) failures: u32,
    pub(crate) state: OfferState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OfferFailure {
    Counted { left: u32 },
    Burned,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PairWrite {
    New(PhoneRow),
    Rotate {
        id: String,
        name: String,
        platform: PhonePlatform,
        secret_sha256: [u8; 32],
    },
}

fn platform_text(platform: PhonePlatform) -> &'static str {
    match platform {
        PhonePlatform::Ios => "ios",
        PhonePlatform::Android => "android",
    }
}

fn platform_of(text: &str) -> Option<PhonePlatform> {
    match text {
        "ios" => Some(PhonePlatform::Ios),
        "android" => Some(PhonePlatform::Android),
        _ => None,
    }
}

fn state_text(state: &OfferState) -> &'static str {
    match state {
        OfferState::Live => "live",
        OfferState::Used { .. } => "used",
        OfferState::Burned => "burned",
        OfferState::Expired => "expired",
        OfferState::Cancelled => "cancelled",
        OfferState::Superseded => "superseded",
    }
}

fn state_of(text: &str, phone: Option<String>) -> Option<OfferState> {
    Some(match text {
        "live" => OfferState::Live,
        "used" => OfferState::Used { phone: phone? },
        "burned" => OfferState::Burned,
        "expired" => OfferState::Expired,
        "cancelled" => OfferState::Cancelled,
        "superseded" => OfferState::Superseded,
        _ => return None,
    })
}

fn corrupt(column: usize, kind: Type, what: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(column, kind, what.into())
}

fn phone_from(row: &Row<'_>) -> Result<PhoneRow, rusqlite::Error> {
    let platform: String = row.get(2)?;
    let secret: Vec<u8> = row.get(3)?;
    Ok(PhoneRow {
        id: row.get(0)?,
        name: row.get(1)?,
        platform: platform_of(&platform)
            .ok_or_else(|| corrupt(2, Type::Text, "unknown platform"))?,
        secret_sha256: secret
            .try_into()
            .map_err(|_| corrupt(3, Type::Blob, "secret hash is not 32 bytes"))?,
        created_at: row.get(4)?,
        last_seen: row.get(5)?,
    })
}

fn offer_from(row: &Row<'_>) -> Result<OfferRow, rusqlite::Error> {
    let state: String = row.get(6)?;
    Ok(OfferRow {
        id: row.get(0)?,
        code: row.get(1)?,
        name: row.get(2)?,
        created_at: row.get(3)?,
        expires_at: row.get(4)?,
        failures: row.get(5)?,
        state: state_of(&state, row.get(7)?)
            .ok_or_else(|| corrupt(6, Type::Text, "unknown offer state"))?,
    })
}

fn prune_before(created_at: &str) -> Result<String, StoreError> {
    let not_a_time = || StoreError::Timestamp(created_at.to_owned());
    let created: jiff::Timestamp = created_at.parse().map_err(|_| not_a_time())?;
    let cutoff = created
        .saturating_sub(OFFER_KEEP)
        .map_err(|_| not_a_time())?;
    Ok(rfc3339(cutoff))
}

impl Store {
    pub(crate) fn phones(&self) -> Result<Vec<PhoneRow>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {PHONE_COLUMNS} FROM phones ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map([], phone_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub(crate) fn phone(&self, id: &str) -> Result<PhoneRow, StoreError> {
        self.lock()
            .query_row(
                &format!("SELECT {PHONE_COLUMNS} FROM phones WHERE id = ?1"),
                params![id],
                phone_from,
            )
            .optional()?
            .ok_or_else(|| StoreError::PhoneNotFound(id.to_owned()))
    }

    pub(crate) fn rename_phone(&self, id: &str, name: &str) -> Result<(), StoreError> {
        let changed = self.lock().execute(
            "UPDATE phones SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        if changed == 0 {
            return Err(StoreError::PhoneNotFound(id.to_owned()));
        }
        Ok(())
    }

    pub(crate) fn delete_phone(&self, id: &str) -> Result<(), StoreError> {
        let changed = self
            .lock()
            .execute("DELETE FROM phones WHERE id = ?1", params![id])?;
        if changed == 0 {
            return Err(StoreError::PhoneNotFound(id.to_owned()));
        }
        Ok(())
    }

    pub(crate) fn touch_phones(&self, seen: &[(String, String)]) -> Result<(), StoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        for (id, at) in seen {
            tx.execute(
                "UPDATE phones SET last_seen = ?2
                 WHERE id = ?1 AND (last_seen IS NULL OR last_seen < ?2)",
                params![id, at],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn open_offer(&self, offer: &OfferRow) -> Result<(), StoreError> {
        let cutoff = prune_before(&offer.created_at)?;
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE phone_offers SET state = 'superseded' WHERE state = 'live'",
            [],
        )?;
        tx.execute(
            "DELETE FROM phone_offers WHERE created_at < ?1",
            params![cutoff],
        )?;
        tx.execute(
            "INSERT INTO phone_offers (id, code, name, created_at, expires_at, failures, state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                offer.id,
                offer.code,
                offer.name,
                offer.created_at,
                offer.expires_at,
                offer.failures,
                state_text(&offer.state),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn live_offer(&self, now: &str) -> Result<Option<OfferRow>, StoreError> {
        let conn = self.lock();
        conn.execute(
            "UPDATE phone_offers SET state = 'expired' WHERE state = 'live' AND expires_at <= ?1",
            params![now],
        )?;
        Ok(conn
            .query_row(
                &format!("SELECT {OFFER_COLUMNS} FROM phone_offers WHERE state = 'live'"),
                [],
                offer_from,
            )
            .optional()?)
    }

    pub(crate) fn latest_offer(&self) -> Result<Option<OfferRow>, StoreError> {
        Ok(self
            .lock()
            .query_row(
                &format!(
                    "SELECT {OFFER_COLUMNS} FROM phone_offers
                     ORDER BY created_at DESC, rowid DESC LIMIT 1"
                ),
                [],
                offer_from,
            )
            .optional()?)
    }

    #[cfg(test)]
    pub(crate) fn offer(&self, id: &str) -> Result<Option<OfferRow>, StoreError> {
        Ok(self
            .lock()
            .query_row(
                &format!("SELECT {OFFER_COLUMNS} FROM phone_offers WHERE id = ?1"),
                params![id],
                offer_from,
            )
            .optional()?)
    }

    pub(crate) fn fail_offer(&self, id: &str) -> Result<OfferFailure, StoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let failures: u32 = tx
            .query_row(
                "UPDATE phone_offers SET failures = failures + 1
                 WHERE id = ?1 AND state = 'live' RETURNING failures",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(StoreError::OfferGone)?;
        let outcome = if failures >= PAIR_MAX_FAILURES {
            tx.execute(
                "UPDATE phone_offers SET state = 'burned' WHERE id = ?1",
                params![id],
            )?;
            OfferFailure::Burned
        } else {
            OfferFailure::Counted {
                left: PAIR_MAX_FAILURES - failures,
            }
        };
        tx.commit()?;
        Ok(outcome)
    }

    pub(crate) fn pair_with_offer(
        &self,
        offer_id: &str,
        write: &PairWrite,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let phone = match write {
            PairWrite::New(row) => &row.id,
            PairWrite::Rotate { id, .. } => id,
        };
        let used = tx.execute(
            "UPDATE phone_offers SET state = 'used', phone = ?2 WHERE id = ?1 AND state = 'live'",
            params![offer_id, phone],
        )?;
        if used == 0 {
            return Err(StoreError::OfferGone);
        }
        match write {
            PairWrite::New(row) => insert_phone(&tx, row)?,
            PairWrite::Rotate {
                id,
                name,
                platform,
                secret_sha256,
            } => {
                let rotated = tx.execute(
                    "UPDATE phones SET name = ?2, platform = ?3, secret_sha256 = ?4 WHERE id = ?1",
                    params![id, name, platform_text(*platform), secret_sha256.as_slice()],
                )?;
                if rotated == 0 {
                    insert_phone(
                        &tx,
                        &PhoneRow {
                            id: id.clone(),
                            name: name.clone(),
                            platform: *platform,
                            secret_sha256: *secret_sha256,
                            created_at: now_rfc3339(),
                            last_seen: None,
                        },
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn cancel_offer(&self) -> Result<bool, StoreError> {
        let changed = self.lock().execute(
            "UPDATE phone_offers SET state = 'cancelled' WHERE state = 'live'",
            [],
        )?;
        Ok(changed > 0)
    }
}

fn insert_phone(tx: &rusqlite::Transaction<'_>, row: &PhoneRow) -> Result<(), rusqlite::Error> {
    tx.execute(
        "INSERT INTO phones (id, name, platform, secret_sha256, created_at, last_seen)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            row.id,
            row.name,
            platform_text(row.platform),
            row.secret_sha256.as_slice(),
            row.created_at,
            row.last_seen,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
