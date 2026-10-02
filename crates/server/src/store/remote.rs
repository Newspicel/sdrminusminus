use rusqlite::{OptionalExtension, params};

use super::{Store, StoreError, now_rfc3339};

#[derive(Clone, PartialEq, Eq)]
pub struct RemotePairing {
    pub device_id: String,
    pub relay_url: String,
    pub key: Vec<u8>,
    pub paired_at: String,
}

impl std::fmt::Debug for RemotePairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemotePairing")
            .field("device_id", &self.device_id)
            .field("relay_url", &self.relay_url)
            .field("paired_at", &self.paired_at)
            .finish_non_exhaustive()
    }
}

impl RemotePairing {
    pub fn new(device_id: String, relay_url: String, key: Vec<u8>) -> Self {
        Self {
            device_id,
            relay_url,
            key,
            paired_at: now_rfc3339(),
        }
    }
}

impl Store {
    pub fn remote_pairing(&self) -> Result<Option<RemotePairing>, StoreError> {
        Ok(self
            .lock()
            .query_row(
                "SELECT device_id, relay_url, key, paired_at FROM remote_access WHERE id = 1",
                [],
                |row| {
                    Ok(RemotePairing {
                        device_id: row.get(0)?,
                        relay_url: row.get(1)?,
                        key: row.get(2)?,
                        paired_at: row.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn save_remote_pairing(&self, pairing: &RemotePairing) -> Result<(), StoreError> {
        self.lock().execute(
            "INSERT INTO remote_access (id, device_id, relay_url, key, paired_at) \
             VALUES (1, ?1, ?2, ?3, ?4) \
             ON CONFLICT(id) DO UPDATE SET device_id = excluded.device_id, \
             relay_url = excluded.relay_url, key = excluded.key, paired_at = excluded.paired_at",
            params![
                pairing.device_id,
                pairing.relay_url,
                pairing.key,
                pairing.paired_at
            ],
        )?;
        Ok(())
    }

    pub fn forget_remote_pairing(&self) -> Result<(), StoreError> {
        self.lock()
            .execute("DELETE FROM remote_access WHERE id = 1", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pairing_is_kept_replaced_and_forgotten() {
        let store = Store::open(None).expect("store");
        assert_eq!(store.remote_pairing().expect("read"), None);
        let first = RemotePairing::new(
            "0123456789abcdefghjkmnpqrs".to_string(),
            "wss://sdrmm.link/v1/device/0123456789abcdefghjkmnpqrs".to_string(),
            vec![1, 2, 3],
        );
        store.save_remote_pairing(&first).expect("save");
        assert_eq!(store.remote_pairing().expect("read"), Some(first.clone()));
        let second = RemotePairing {
            device_id: "zzzzzzzzzzzzzzzzzzzzzzzzzz".to_string(),
            ..first
        };
        store.save_remote_pairing(&second).expect("replace");
        assert_eq!(store.remote_pairing().expect("read"), Some(second));
        store.forget_remote_pairing().expect("forget");
        assert_eq!(store.remote_pairing().expect("read"), None);
        store
            .forget_remote_pairing()
            .expect("forgetting twice is fine");
    }

    #[test]
    fn the_key_never_shows_in_debug_output() {
        let pairing = RemotePairing {
            paired_at: "2026-01-01T00:00:00Z".to_string(),
            ..RemotePairing::new("id".to_string(), "wss://x".to_string(), vec![42; 8])
        };
        assert!(!format!("{pairing:?}").contains("42"));
    }
}
