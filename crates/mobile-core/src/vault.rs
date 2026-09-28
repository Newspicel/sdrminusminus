use std::{fmt, sync::Arc};

use sdrmm_wire::phone::{self, PhoneToken};
use serde::{Deserialize, Serialize};

use crate::{
    error::{CoreError, VaultError},
    records::SavedServer,
};

#[uniffi::export(foreign)]
pub trait SecretVault: Send + Sync {
    fn load(&self, key: String) -> Result<Option<Vec<u8>>, VaultError>;
    fn store(&self, key: String, value: Vec<u8>) -> Result<(), VaultError>;
    fn delete(&self, key: String) -> Result<(), VaultError>;
    fn keys(&self) -> Result<Vec<String>, VaultError>;
}

pub(crate) const RECORD_VERSION: u32 = 1;
pub(crate) const SERVER_PREFIX: &str = "server/";
pub(crate) const MAX_SAVED_HOSTS: usize = 8;
const MAX_SERVER_ID_LEN: usize = 64;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ServerRecord {
    pub(crate) version: u32,
    pub(crate) server_id: String,
    pub(crate) server_name: String,
    pub(crate) phone_id: String,
    pub(crate) token: String,
    pub(crate) hosts: Vec<String>,
    pub(crate) pin: String,
    pub(crate) paired_at_ms: i64,
}

impl fmt::Debug for ServerRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerRecord")
            .field("version", &self.version)
            .field("server_id", &self.server_id)
            .field("server_name", &self.server_name)
            .field("phone_id", &self.phone_id)
            .field("token", &format_args!("<redacted>"))
            .field("hosts", &self.hosts)
            .field("pin", &self.pin)
            .field("paired_at_ms", &self.paired_at_ms)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RecordError {
    #[error("not a server record")]
    Json,
    #[error("newer app wrote this")]
    Newer,
    #[error("bad {0}")]
    Field(&'static str),
}

impl ServerRecord {
    pub(crate) fn key(&self) -> String {
        server_key(&self.server_id)
    }

    pub(crate) fn decode(server_id: &str, bytes: &[u8]) -> Result<Self, RecordError> {
        let record: Self = serde_json::from_slice(bytes).map_err(|_| RecordError::Json)?;
        record.validate()?;
        if record.server_id != server_id {
            return Err(RecordError::Field("server_id"));
        }
        Ok(record)
    }

    pub(crate) fn validate(&self) -> Result<(), RecordError> {
        if self.version > RECORD_VERSION {
            return Err(RecordError::Newer);
        }
        let checks = [
            (self.version == RECORD_VERSION, "version"),
            (valid_server_id(&self.server_id), "server_id"),
            (valid_server_name(&self.server_name), "server_name"),
            (phone::valid_phone_id(&self.phone_id), "phone_id"),
            (self.token_matches_phone(), "token"),
            (valid_hosts(&self.hosts), "hosts"),
            (phone::valid_pin(&self.pin), "pin"),
            (self.paired_at().is_some(), "paired_at_ms"),
        ];
        match checks.into_iter().find(|(ok, _)| !ok) {
            Some((_, field)) => Err(RecordError::Field(field)),
            None => Ok(()),
        }
    }

    pub(crate) fn saved(&self) -> SavedServer {
        SavedServer {
            id: self.server_id.clone(),
            name: self.server_name.clone(),
            hosts: self.hosts.clone(),
            fingerprint_short: phone::key_check(&self.pin),
            phone_id: self.phone_id.clone(),
            paired_at: self
                .paired_at()
                .map(|at| at.to_string())
                .unwrap_or_default(),
        }
    }

    fn token_matches_phone(&self) -> bool {
        PhoneToken::parse(&self.token).is_some_and(|token| token.phone == self.phone_id)
    }

    fn paired_at(&self) -> Option<jiff::Timestamp> {
        jiff::Timestamp::from_millisecond(self.paired_at_ms).ok()
    }
}

pub(crate) fn server_key(server_id: &str) -> String {
    format!("{SERVER_PREFIX}{server_id}")
}

pub(crate) fn valid_server_id(id: &str) -> bool {
    (1..=MAX_SERVER_ID_LEN).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_server_name(name: &str) -> bool {
    name.trim() == name
        && (1..=phone::MAX_SERVER_NAME_LEN).contains(&name.chars().count())
        && !name.chars().any(char::is_control)
}

fn valid_hosts(hosts: &[String]) -> bool {
    (1..=MAX_SAVED_HOSTS).contains(&hosts.len())
        && hosts.iter().all(|host| phone::valid_pair_host(host))
        && hosts
            .iter()
            .enumerate()
            .all(|(index, host)| !hosts[..index].contains(host))
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Listing {
    pub(crate) records: Vec<ServerRecord>,
    pub(crate) unreadable: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct Vault {
    store: Arc<dyn SecretVault>,
}

impl Vault {
    pub(crate) fn new(store: Arc<dyn SecretVault>) -> Self {
        Self { store }
    }

    pub(crate) fn servers(&self) -> Result<Listing, CoreError> {
        let mut listing = Listing::default();
        let mut keys = self.store.keys().map_err(vault_error)?;
        keys.sort_unstable();
        keys.dedup();
        for key in keys {
            let Some(server_id) = key.strip_prefix(SERVER_PREFIX) else {
                continue;
            };
            match self.read(server_id) {
                Ok(Some(record)) => listing.records.push(record),
                Ok(None) => {}
                Err(Unreadable::Corrupt) => listing.unreadable.push(server_id.to_owned()),
                Err(Unreadable::Vault(error)) => return Err(error),
            }
        }
        listing.records.sort_by(|a, b| {
            a.server_name
                .cmp(&b.server_name)
                .then_with(|| a.server_id.cmp(&b.server_id))
        });
        Ok(listing)
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn load(&self, server_id: &str) -> Result<ServerRecord, CoreError> {
        match self.read(server_id) {
            Ok(Some(record)) => Ok(record),
            Ok(None) => Err(CoreError::internal(format!("No saved server {server_id}"))),
            Err(Unreadable::Corrupt) => Err(CoreError::internal(format!(
                "Saved server {server_id} unreadable"
            ))),
            Err(Unreadable::Vault(error)) => Err(error),
        }
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn store(&self, record: &ServerRecord) -> Result<(), CoreError> {
        record
            .validate()
            .map_err(|error| CoreError::internal(format!("Server record: {error}")))?;
        let bytes = serde_json::to_vec(record)
            .map_err(|error| CoreError::internal(format!("Server record: {error}")))?;
        self.store.store(record.key(), bytes).map_err(vault_error)
    }

    pub(crate) fn delete(&self, server_id: &str) -> Result<(), CoreError> {
        if !valid_server_id(server_id) {
            return Err(CoreError::internal(format!("Bad server id {server_id}")));
        }
        self.store
            .delete(server_key(server_id))
            .map_err(vault_error)
    }

    fn read(&self, server_id: &str) -> Result<Option<ServerRecord>, Unreadable> {
        let bytes = match self.store.load(server_key(server_id)) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Ok(None),
            Err(VaultError::Corrupt) => return Err(Unreadable::Corrupt),
            Err(error) => return Err(Unreadable::Vault(vault_error(error))),
        };
        ServerRecord::decode(server_id, &bytes)
            .map(Some)
            .map_err(|error| {
                tracing::warn!(server_id, %error, "saved server unreadable");
                Unreadable::Corrupt
            })
    }
}

enum Unreadable {
    Corrupt,
    Vault(CoreError),
}

fn vault_error(error: VaultError) -> CoreError {
    match error {
        VaultError::Os { status } => CoreError::Vault { status },
        VaultError::Corrupt => CoreError::internal("Vault corrupt"),
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::{
        collections::BTreeMap,
        sync::{Mutex, PoisonError},
    };

    use super::*;

    pub(crate) const SERVER_ID: &str = "00112233445566778899aabbccddeeff";
    pub(crate) const PHONE_ID: &str = "p0123456789abcdef";
    pub(crate) const PIN: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";

    pub(crate) fn token() -> String {
        PhoneToken::new(PHONE_ID.to_owned(), [7; phone::PHONE_SECRET_BYTES]).encode()
    }

    pub(crate) fn record() -> ServerRecord {
        ServerRecord {
            version: RECORD_VERSION,
            server_id: SERVER_ID.to_owned(),
            server_name: "SDR-- shack".to_owned(),
            phone_id: PHONE_ID.to_owned(),
            token: token(),
            hosts: vec!["192.168.1.20:8443".to_owned(), "[fe80::1]:8443".to_owned()],
            pin: PIN.to_owned(),
            paired_at_ms: 1_790_000_000_000,
        }
    }

    #[derive(Default)]
    pub(crate) struct MemoryVault {
        items: Mutex<BTreeMap<String, Result<Vec<u8>, VaultError>>>,
    }

    impl MemoryVault {
        pub(crate) fn put(&self, key: &str, item: Result<Vec<u8>, VaultError>) {
            self.lock().insert(key.to_owned(), item);
        }

        pub(crate) fn raw(&self, key: &str) -> Option<Vec<u8>> {
            self.lock().get(key).cloned().and_then(Result::ok)
        }

        fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Result<Vec<u8>, VaultError>>> {
            self.items.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    impl SecretVault for MemoryVault {
        fn load(&self, key: String) -> Result<Option<Vec<u8>>, VaultError> {
            self.lock().get(&key).cloned().transpose()
        }

        fn store(&self, key: String, value: Vec<u8>) -> Result<(), VaultError> {
            self.lock().insert(key, Ok(value));
            Ok(())
        }

        fn delete(&self, key: String) -> Result<(), VaultError> {
            self.lock().remove(&key);
            Ok(())
        }

        fn keys(&self) -> Result<Vec<String>, VaultError> {
            Ok(self.lock().keys().cloned().collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{testing::*, *};

    fn vault() -> (Arc<MemoryVault>, Vault) {
        let memory = Arc::new(MemoryVault::default());
        (memory.clone(), Vault::new(memory))
    }

    #[test]
    fn vault_records_round_trip_and_hide_the_token() {
        let (memory, vault) = vault();
        let record = record();
        vault.store(&record).expect("stored");
        let raw = memory
            .raw(&format!("server/{SERVER_ID}"))
            .expect("one item per server");
        let json: serde_json::Value = serde_json::from_slice(&raw).expect("utf-8 json");
        let mut fields: Vec<&str> = json
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        fields.sort_unstable();
        assert_eq!(
            fields,
            [
                "hosts",
                "paired_at_ms",
                "phone_id",
                "pin",
                "server_id",
                "server_name",
                "token",
                "version"
            ]
        );
        assert_eq!(vault.load(SERVER_ID).expect("loaded"), record);
        let listing = vault.servers().expect("listed");
        assert_eq!(listing.records, vec![record.clone()]);
        let saved = record.saved();
        assert_eq!(saved.id, SERVER_ID);
        assert_eq!(saved.fingerprint_short, "A1B2 C3D4 E5F6 0718 293A");
        assert_eq!(saved.paired_at, "2026-09-21T14:13:20Z");
        assert!(!format!("{saved:?}").contains(&token()));
        assert!(!format!("{record:?}").contains(&token()));
        assert!(format!("{record:?}").contains("<redacted>"));
    }

    #[test]
    fn a_corrupt_vault_item_is_listed_as_unreadable() {
        let (memory, vault) = vault();
        vault.store(&record()).expect("stored");
        memory.put("server/0a", Ok(b"not json".to_vec()));
        memory.put("server/0b", Err(VaultError::Corrupt));
        let listing = vault.servers().expect("listed");
        assert_eq!(listing.records, vec![record()]);
        assert_eq!(listing.unreadable, ["0a", "0b"]);
        assert!(matches!(
            vault.load("0a"),
            Err(CoreError::Internal { message }) if message == "Saved server 0a unreadable"
        ));
    }

    #[test]
    fn an_os_error_fails_the_listing() {
        let (memory, vault) = vault();
        memory.put("server/0a", Err(VaultError::Os { status: -25300 }));
        assert_eq!(vault.servers(), Err(CoreError::Vault { status: -25300 }));
    }

    #[test]
    fn other_keys_and_vanished_items_are_skipped() {
        let (memory, vault) = vault();
        memory.put("settings/x", Ok(b"{}".to_vec()));
        assert_eq!(vault.servers().expect("listed"), Listing::default());
        assert_eq!(
            vault.load(SERVER_ID),
            Err(CoreError::internal(format!("No saved server {SERVER_ID}")))
        );
    }

    #[test]
    fn a_record_under_another_id_is_unreadable() {
        let (memory, vault) = vault();
        memory.put(
            "server/0a",
            Ok(serde_json::to_vec(&record()).expect("json")),
        );
        assert_eq!(vault.servers().expect("listed").unreadable, ["0a"]);
    }

    #[test]
    fn a_record_from_a_newer_app_is_refused() {
        let mut newer = record();
        newer.version = RECORD_VERSION + 1;
        assert_eq!(newer.validate(), Err(RecordError::Newer));
        let mut older = record();
        older.version = 0;
        assert_eq!(older.validate(), Err(RecordError::Field("version")));
    }

    type Spoil = fn(&mut ServerRecord);

    #[test]
    fn validate_refuses_bad_records() {
        let cases: [(&str, Spoil); 12] = [
            ("server_id", |r| r.server_id = "XYZ".to_owned()),
            ("server_id", |r| r.server_id.clear()),
            ("server_name", |r| r.server_name = " shack".to_owned()),
            ("phone_id", |r| r.phone_id = "p12".to_owned()),
            ("token", |r| r.token = "sdrmm-phone.bad".to_owned()),
            ("token", |r| {
                r.token = PhoneToken::new("pfedcba9876543210".to_owned(), [1; 32]).encode();
            }),
            ("hosts", |r| r.hosts.clear()),
            ("hosts", |r| {
                r.hosts = (1..=9).map(|port| format!("pi.local:{port}")).collect();
            }),
            ("hosts", |r| r.hosts = vec!["pi.local".to_owned()]),
            ("hosts", |r| {
                r.hosts = vec!["pi.local:1".to_owned(), "pi.local:1".to_owned()];
            }),
            ("pin", |r| r.pin = "AB".repeat(32)),
            ("paired_at_ms", |r| r.paired_at_ms = i64::MAX),
        ];
        for (field, spoil) in cases {
            let mut bad = record();
            spoil(&mut bad);
            assert_eq!(bad.validate(), Err(RecordError::Field(field)), "{field}");
        }
        assert_eq!(record().validate(), Ok(()));
    }

    #[test]
    fn storing_refuses_an_invalid_record_and_delete_needs_a_valid_id() {
        let (_, vault) = vault();
        let mut bad = record();
        bad.pin.clear();
        assert!(vault.store(&bad).is_err());
        assert!(vault.delete("../x").is_err());
        vault.store(&record()).expect("stored");
        vault.delete(SERVER_ID).expect("deleted");
        assert_eq!(vault.servers().expect("listed"), Listing::default());
    }
}
