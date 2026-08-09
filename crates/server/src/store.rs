//! SQLite persistence (PLAN §11): presets (full device-set + channels snapshots) and
//! bookmarks. `rusqlite` with the bundled engine — zero system deps. All calls block, so
//! handlers reach the store via `spawn_blocking` only.

use std::{
    path::Path,
    sync::{Mutex, MutexGuard},
};

use rusqlite::{Connection, OptionalExtension, params};
use sdrmm_wire::{Bookmark, CreateBookmarkRequest, PresetInfo, PresetSnapshot};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("preset {0} not found")]
    PresetNotFound(i64),
    #[error("bookmark {0} not found")]
    BookmarkNotFound(i64),
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("stored snapshot corrupt: {0}")]
    Corrupt(#[from] serde_json::Error),
}

/// Migrations keyed off `PRAGMA user_version`: each entry runs inside a transaction that also
/// bumps the version, so a crash mid-migration leaves the previous version intact. Append-only.
const MIGRATIONS: &[&str] = &["
    CREATE TABLE presets (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        created_at TEXT NOT NULL,
        device_id TEXT NOT NULL,
        snapshot TEXT NOT NULL
    );
    CREATE TABLE bookmarks (
        id INTEGER PRIMARY KEY,
        label TEXT NOT NULL,
        freq_hz REAL NOT NULL,
        mode TEXT,
        grp TEXT
    );
    "];

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Open (creating and migrating as needed); `None` is a private in-memory database.
    pub fn open(path: Option<&Path>) -> Result<Self, StoreError> {
        let conn = match path {
            Some(path) => Connection::open(path)?,
            None => Connection::open_in_memory()?,
        };
        migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn create_preset(&self, name: &str, snapshot: &PresetSnapshot) -> Result<i64, StoreError> {
        let json = serde_json::to_string(snapshot)?;
        let conn = self.lock();
        conn.execute(
            "INSERT INTO presets (name, created_at, device_id, snapshot) VALUES (?1, ?2, ?3, ?4)",
            params![name, now_rfc3339(), snapshot.device_id, json],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_presets(&self) -> Result<Vec<PresetInfo>, StoreError> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare("SELECT id, name, created_at, device_id FROM presets ORDER BY id")?;
        let rows = stmt.query_map([], |row| {
            Ok(PresetInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
                device_id: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn preset_snapshot(&self, id: i64) -> Result<PresetSnapshot, StoreError> {
        let json: String = self
            .lock()
            .query_row(
                "SELECT snapshot FROM presets WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(StoreError::PresetNotFound(id))?;
        Ok(serde_json::from_str(&json)?)
    }

    pub fn delete_preset(&self, id: i64) -> Result<(), StoreError> {
        let deleted = self
            .lock()
            .execute("DELETE FROM presets WHERE id = ?1", params![id])?;
        if deleted == 0 {
            return Err(StoreError::PresetNotFound(id));
        }
        Ok(())
    }

    pub fn create_bookmark(&self, req: &CreateBookmarkRequest) -> Result<i64, StoreError> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO bookmarks (label, freq_hz, mode, grp) VALUES (?1, ?2, ?3, ?4)",
            params![req.label, req.freq_hz, req.mode, req.group],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_bookmarks(&self) -> Result<Vec<Bookmark>, StoreError> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare("SELECT id, label, freq_hz, mode, grp FROM bookmarks ORDER BY id")?;
        let rows = stmt.query_map([], |row| {
            Ok(Bookmark {
                id: row.get(0)?,
                label: row.get(1)?,
                freq_hz: row.get(2)?,
                mode: row.get(3)?,
                group: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn delete_bookmark(&self, id: i64) -> Result<(), StoreError> {
        let deleted = self
            .lock()
            .execute("DELETE FROM bookmarks WHERE id = ?1", params![id])?;
        if deleted == 0 {
            return Err(StoreError::BookmarkNotFound(id));
        }
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn migrate(conn: &Connection) -> Result<(), rusqlite::Error> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let done = usize::try_from(version).unwrap_or(0);
    for (i, migration) in MIGRATIONS.iter().enumerate().skip(done) {
        // PRAGMA can't take a bound parameter; `i` comes from the const table, not input.
        conn.execute_batch(&format!(
            "BEGIN;\n{migration}\nPRAGMA user_version = {};\nCOMMIT;",
            i + 1
        ))?;
    }
    Ok(())
}

fn now_rfc3339() -> String {
    jiff::Timestamp::now().to_string()
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{ChannelParams, ChannelSettings, DeviceSettings, NfmParams};

    use super::*;

    fn snapshot() -> PresetSnapshot {
        PresetSnapshot {
            version: 1,
            device_id: "virtual:siggen".to_string(),
            settings: DeviceSettings {
                center_hz: Some(100_000_000.0),
                sample_rate: Some(2_048_000.0),
                ..DeviceSettings::default()
            },
            channels: vec![ChannelSettings {
                offset_hz: 100_000.0,
                squelch_db: Some(-60.0),
                params: ChannelParams::Nfm(NfmParams::default()),
            }],
        }
    }

    #[test]
    fn migration_is_idempotent() {
        let conn = Connection::open_in_memory().expect("open");
        migrate(&conn).expect("first migrate");
        migrate(&conn).expect("second migrate");
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("version");
        assert_eq!(version, MIGRATIONS.len() as i64);
    }

    #[test]
    fn preset_crud_roundtrip() {
        let store = Store::open(None).expect("open");
        assert!(store.list_presets().expect("list").is_empty());

        let snap = snapshot();
        let id = store.create_preset("fm broadcast", &snap).expect("create");
        let listed = store.list_presets().expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
        assert_eq!(listed[0].name, "fm broadcast");
        assert_eq!(listed[0].device_id, "virtual:siggen");
        // RFC3339 UTC: parseable and Z-suffixed.
        assert!(
            listed[0].created_at.ends_with('Z'),
            "{}",
            listed[0].created_at
        );
        listed[0]
            .created_at
            .parse::<jiff::Timestamp>()
            .expect("rfc3339 timestamp");

        assert_eq!(store.preset_snapshot(id).expect("snapshot"), snap);

        store.delete_preset(id).expect("delete");
        assert!(store.list_presets().expect("list").is_empty());
        assert!(matches!(
            store.delete_preset(id),
            Err(StoreError::PresetNotFound(_))
        ));
        assert!(matches!(
            store.preset_snapshot(id),
            Err(StoreError::PresetNotFound(_))
        ));
    }

    #[test]
    fn bookmark_crud_roundtrip() {
        let store = Store::open(None).expect("open");
        assert!(store.list_bookmarks().expect("list").is_empty());

        let id = store
            .create_bookmark(&CreateBookmarkRequest {
                label: "tower".to_string(),
                freq_hz: 118_700_000.0,
                mode: Some("am".to_string()),
                group: Some("airband".to_string()),
            })
            .expect("create");
        let bare_id = store
            .create_bookmark(&CreateBookmarkRequest {
                label: "repeater".to_string(),
                freq_hz: 439_000_000.0,
                mode: None,
                group: None,
            })
            .expect("create");

        let listed = store.list_bookmarks().expect("list");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, id);
        assert_eq!(listed[0].label, "tower");
        assert_eq!(listed[0].freq_hz, 118_700_000.0);
        assert_eq!(listed[0].mode.as_deref(), Some("am"));
        assert_eq!(listed[0].group.as_deref(), Some("airband"));
        assert_eq!(listed[1].mode, None);
        assert_eq!(listed[1].group, None);

        store.delete_bookmark(id).expect("delete");
        assert_eq!(store.list_bookmarks().expect("list").len(), 1);
        assert!(matches!(
            store.delete_bookmark(id),
            Err(StoreError::BookmarkNotFound(_))
        ));
        store.delete_bookmark(bare_id).expect("delete");
    }
}
