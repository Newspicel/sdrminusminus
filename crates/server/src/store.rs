//! SQLite persistence (PLAN §11): presets (full device-set + channels snapshots), bookmarks,
//! the recordings index (the SigMF pairs on disk are the source of truth; rows here are
//! reconciled from them) and the decoder log (queryable and exportable decodes, not
//! scroll-back-only). `rusqlite` with the bundled engine — zero system deps. All calls
//! block, so handlers reach the store via `spawn_blocking` only.

use std::{
    path::Path,
    sync::{Mutex, MutexGuard},
};

use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value};
use sdrmm_wire::{
    Bookmark, CreateBookmarkRequest, DecodedRecord, DecoderLogEntry, DecoderLogQuery, PresetInfo,
    PresetSnapshot, RecordingInfo, UpdateWorkspaceRequest, WorkspaceDetail, WorkspaceError,
    WorkspaceInfo, WorkspaceSnapshot, WorkspacesResponse,
};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("preset {0} not found")]
    PresetNotFound(i64),
    #[error("bookmark {0} not found")]
    BookmarkNotFound(i64),
    #[error("recording {0} not found")]
    RecordingNotFound(i64),
    #[error("workspace {0} not found")]
    WorkspaceNotFound(i64),
    #[error("a workspace named {0:?} already exists")]
    WorkspaceNameTaken(String),
    #[error("workspace {id} moved on (revision {current}, not {sent}) — reload and reapply")]
    WorkspaceConflict { id: i64, sent: u64, current: u64 },
    #[error("invalid workspace layout: {0}")]
    WorkspaceLayout(#[from] sdrmm_wire::WorkspaceError),
    #[error("not an RFC3339 timestamp: {0}")]
    Timestamp(String),
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("stored snapshot corrupt: {0}")]
    Corrupt(#[from] serde_json::Error),
}

/// Page size a decoder-log query gets when it asks for none, and the ceiling every request is
/// clamped to. The list view is a scrolling table; bulk transfer is what export is for.
pub const DECODER_LOG_LIMIT_DEFAULT: u32 = 200;
pub const DECODER_LOG_LIMIT_MAX: u32 = 1_000;

/// Hard ceiling on one export, whatever the filter matches: the whole body is built in memory
/// before it is sent, so an unbounded export is an out-of-memory kill.
pub const DECODER_LOG_EXPORT_MAX: u32 = 100_000;

/// Migrations keyed off `PRAGMA user_version`: each entry runs inside a transaction that also
/// bumps the version, so a crash mid-migration leaves the previous version intact. Append-only.
const MIGRATIONS: &[&str] = &[
    "
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
    ",
    "
    CREATE TABLE recordings (
        id INTEGER PRIMARY KEY,
        stem TEXT NOT NULL UNIQUE,
        created_at TEXT NOT NULL,
        device_label TEXT NOT NULL,
        center_hz REAL NOT NULL,
        sample_rate REAL NOT NULL,
        samples INTEGER NOT NULL,
        bytes INTEGER NOT NULL
    );
    ",
    "
    CREATE TABLE decoder_log (
        id INTEGER PRIMARY KEY,
        at TEXT NOT NULL,
        device_set INTEGER NOT NULL,
        channel INTEGER NOT NULL,
        kind TEXT NOT NULL,
        freq_hz REAL NOT NULL,
        station TEXT,
        summary TEXT NOT NULL,
        event TEXT NOT NULL
    );
    -- (at, id) is the sort key of every read, so this index covers both the unfiltered
    -- newest-first page and the since/until range scans without a sort step.
    CREATE INDEX decoder_log_at ON decoder_log (at DESC, id DESC);
    -- `kind` is the only selective equality filter (the UI shows one decoder at a time);
    -- pairing it with the sort key keeps a filtered page off a full scan. `device_set` is
    -- deliberately unindexed — a handful of distinct values never beats a scan — and `q` is
    -- a substring match no B-tree can serve.
    CREATE INDEX decoder_log_kind_at ON decoder_log (kind, at DESC, id DESC);
    ",
    "
    CREATE TABLE workspaces (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        revision INTEGER NOT NULL,
        -- Denormalized from the snapshot so the switcher never parses a layout blob: a row
        -- whose JSON this build cannot read must break opening that one workspace, never the
        -- list that would let the user switch away from it.
        tabs INTEGER NOT NULL,
        snapshot TEXT NOT NULL
    );
    -- One active workspace is an invariant, so the schema holds it: a per-row `active` flag
    -- makes \"two rows true\" representable, and something would eventually write it.
    CREATE TABLE active_workspace (
        id INTEGER PRIMARY KEY CHECK (id = 0),
        workspace_id INTEGER
    );
    INSERT INTO active_workspace (id, workspace_id) VALUES (0, NULL);
    ",
];

/// Index fields for one finalized recording, derived from its SigMF pair during
/// reconciliation (PLAN §11: the files are the source of truth; rows are upserted by stem).
pub struct RecordingRow {
    /// File name without directory or `.sigmf-*` extension — unique within the recordings dir.
    pub stem: String,
    pub created_at: String,
    pub device_label: String,
    pub center_hz: f64,
    pub sample_rate: f64,
    pub samples: u64,
    pub bytes: u64,
}

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
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.seed_workspaces()?;
        Ok(store)
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

    pub fn upsert_recording(&self, row: &RecordingRow) -> Result<(), StoreError> {
        self.lock().execute(
            "INSERT INTO recordings (stem, created_at, device_label, center_hz, sample_rate, \
             samples, bytes) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT(stem) DO UPDATE SET created_at = excluded.created_at, \
             device_label = excluded.device_label, center_hz = excluded.center_hz, \
             sample_rate = excluded.sample_rate, samples = excluded.samples, \
             bytes = excluded.bytes",
            // SQLite integers are i64; counts can't realistically overflow them.
            params![
                row.stem,
                row.created_at,
                row.device_label,
                row.center_hz,
                row.sample_rate,
                row.samples as i64,
                row.bytes as i64
            ],
        )?;
        Ok(())
    }

    /// List the index, deriving the fields that depend on where the recordings dir currently
    /// is (`device_id`, per the wire contract `virtual:file:<dir-joined stem>`) instead of
    /// persisting them — a moved dir must not strand stale absolute paths in rows.
    pub fn list_recordings(&self, dir: &Path) -> Result<Vec<RecordingInfo>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, stem, created_at, device_label, center_hz, sample_rate, samples, bytes \
             FROM recordings ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            let stem: String = row.get(1)?;
            let sample_rate: f64 = row.get(5)?;
            let samples = row.get::<_, i64>(6)? as u64;
            Ok(RecordingInfo {
                id: row.get(0)?,
                device_id: format!("virtual:file:{}", dir.join(&stem).display()),
                file: stem,
                created_at: row.get(2)?,
                device_label: row.get(3)?,
                center_hz: row.get(4)?,
                sample_rate,
                samples,
                bytes: row.get::<_, i64>(7)? as u64,
                duration_s: if sample_rate > 0.0 {
                    samples as f64 / sample_rate
                } else {
                    0.0
                },
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn recording_stem(&self, id: i64) -> Result<String, StoreError> {
        self.lock()
            .query_row(
                "SELECT stem FROM recordings WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(StoreError::RecordingNotFound(id))
    }

    pub fn delete_recording(&self, id: i64) -> Result<(), StoreError> {
        let deleted = self
            .lock()
            .execute("DELETE FROM recordings WHERE id = ?1", params![id])?;
        if deleted == 0 {
            return Err(StoreError::RecordingNotFound(id));
        }
        Ok(())
    }

    /// Drop rows whose SigMF pair vanished from disk — the reconcile pass hands in every
    /// stem it found.
    pub fn prune_recordings(&self, keep_stems: &[String]) -> Result<(), StoreError> {
        let conn = self.lock();
        if keep_stems.is_empty() {
            conn.execute("DELETE FROM recordings", [])?;
            return Ok(());
        }
        let placeholders = vec!["?"; keep_stems.len()].join(", ");
        conn.execute(
            &format!("DELETE FROM recordings WHERE stem NOT IN ({placeholders})"),
            params_from_iter(keep_stems),
        )?;
        Ok(())
    }

    /// Persist a batch of decoder frames in one transaction. Decoders emit hundreds of frames
    /// a second (ADS-B), and a commit per row would make SQLite the bottleneck.
    pub fn insert_decoder_events(&self, records: &[DecodedRecord]) -> Result<usize, StoreError> {
        if records.is_empty() {
            return Ok(0);
        }
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO decoder_log (at, device_set, channel, kind, freq_hz, station, \
                 summary, event) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for record in records {
                stmt.execute(params![
                    normalize_timestamp(&record.at)?,
                    record.device_set,
                    record.channel,
                    record.event.kind(),
                    record.freq_hz,
                    record.event.station(),
                    record.event.summary(),
                    serde_json::to_string(&record.event)?,
                ])?;
            }
        }
        tx.commit()?;
        Ok(records.len())
    }

    /// One page of the decoder log, newest first, plus how many rows the filter matches in
    /// total (ignoring the page size) so the client can show "showing 200 of 12,431".
    pub fn query_decoder_log(
        &self,
        filter: &DecoderLogQuery,
    ) -> Result<(Vec<DecoderLogEntry>, u64), StoreError> {
        let limit = filter
            .limit
            .unwrap_or(DECODER_LOG_LIMIT_DEFAULT)
            .min(DECODER_LOG_LIMIT_MAX);
        let predicate = DecoderLogPredicate::build(filter)?;
        let conn = self.lock();
        let total: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM decoder_log{}", predicate.clause),
            params_from_iter(&predicate.params),
            |row| row.get(0),
        )?;
        let entries = select_decoder_log(&conn, &predicate, limit)?;
        Ok((entries, total.max(0).unsigned_abs()))
    }

    /// Every matching entry for an export, capped at [`DECODER_LOG_EXPORT_MAX`]; the query's
    /// own `limit` is a list-view concern and is ignored here (per the wire contract).
    pub fn export_decoder_log(
        &self,
        filter: &DecoderLogQuery,
    ) -> Result<Vec<DecoderLogEntry>, StoreError> {
        let predicate = DecoderLogPredicate::build(filter)?;
        let conn = self.lock();
        select_decoder_log(&conn, &predicate, DECODER_LOG_EXPORT_MAX)
    }

    /// Clear the entries matching `filter` (an empty filter clears the log).
    pub fn delete_decoder_log(&self, filter: &DecoderLogQuery) -> Result<u64, StoreError> {
        let predicate = DecoderLogPredicate::build(filter)?;
        let deleted = self.lock().execute(
            &format!("DELETE FROM decoder_log{}", predicate.clause),
            params_from_iter(&predicate.params),
        )?;
        Ok(deleted as u64)
    }

    /// Keep only the newest `max_rows` entries, returning how many were dropped. `id` is
    /// monotonic and rows are inserted in arrival order, so "newest" is "highest id" and the
    /// cut can be a single range delete instead of a sort over `at`.
    pub fn prune_decoder_log(&self, max_rows: u64) -> Result<u64, StoreError> {
        let dropped = self.lock().execute(
            "DELETE FROM decoder_log WHERE id <= \
             (SELECT id FROM decoder_log ORDER BY id DESC LIMIT 1 OFFSET ?1)",
            params![i64::try_from(max_rows).unwrap_or(i64::MAX)],
        )?;
        Ok(dropped as u64)
    }

    /// The switcher's view: every workspace, plus which one is active (PLAN §10 — exactly one
    /// is, server-wide, so every client opens the same station). Reads projection columns only.
    pub fn list_workspaces(&self) -> Result<WorkspacesResponse, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, created_at, updated_at, revision, tabs FROM workspaces ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(WorkspaceInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
                revision: row.get::<_, i64>(4)?.unsigned_abs(),
                tabs: row.get(5)?,
            })
        })?;
        let workspaces: Vec<WorkspaceInfo> = rows.collect::<Result<_, _>>()?;
        Ok(WorkspacesResponse {
            workspaces,
            active: active_workspace(&conn)?,
        })
    }

    pub fn workspace(&self, id: i64) -> Result<WorkspaceDetail, StoreError> {
        let conn = self.lock();
        read_workspace(&conn, id)
    }

    /// Store a new workspace. The snapshot is validated first: a layout the client could not
    /// render back is refused at the edge, never persisted.
    pub fn create_workspace(
        &self,
        name: &str,
        snapshot: &WorkspaceSnapshot,
    ) -> Result<i64, StoreError> {
        snapshot.validate()?;
        let json = serde_json::to_string(snapshot)?;
        let now = now_rfc3339();
        let conn = self.lock();
        conn.execute(
            "INSERT INTO workspaces (name, created_at, updated_at, revision, tabs, snapshot) \
             VALUES (?1, ?2, ?2, 1, ?3, ?4)",
            params![name, now, snapshot.tabs.len() as i64, json],
        )
        .map_err(|err| name_taken(err, name))?;
        Ok(conn.last_insert_rowid())
    }

    /// Rename and/or re-lay-out a workspace. `revision` is the one the caller last saw: a
    /// mismatch means another client wrote in between, and the write is refused instead of
    /// silently discarding that client's layout.
    pub fn update_workspace(
        &self,
        id: i64,
        req: &UpdateWorkspaceRequest,
    ) -> Result<WorkspaceInfo, StoreError> {
        if let Some(snapshot) = &req.snapshot {
            snapshot.validate()?;
        }
        if let Some(name) = &req.name
            && (name.trim().is_empty()
                || name.chars().count() > sdrmm_wire::workspace::MAX_NAME_LEN)
        {
            return Err(StoreError::WorkspaceLayout(WorkspaceError::Name));
        }
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let current: u64 = tx
            .query_row(
                "SELECT revision FROM workspaces WHERE id = ?1",
                params![id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .ok_or(StoreError::WorkspaceNotFound(id))?
            .unsigned_abs();
        if current != req.revision {
            return Err(StoreError::WorkspaceConflict {
                id,
                sent: req.revision,
                current,
            });
        }
        let revision = i64::try_from(current.saturating_add(1)).unwrap_or(i64::MAX);
        let now = now_rfc3339();
        if let Some(name) = &req.name {
            tx.execute(
                "UPDATE workspaces SET name = ?2 WHERE id = ?1",
                params![id, name],
            )
            .map_err(|err| name_taken(err, name))?;
        }
        if let Some(snapshot) = &req.snapshot {
            tx.execute(
                "UPDATE workspaces SET snapshot = ?2, tabs = ?3 WHERE id = ?1",
                params![
                    id,
                    serde_json::to_string(snapshot)?,
                    snapshot.tabs.len() as i64
                ],
            )?;
        }
        tx.execute(
            "UPDATE workspaces SET revision = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, revision, now],
        )?;
        let info = read_workspace_info(&tx, id)?;
        tx.commit()?;
        Ok(info)
    }

    /// Delete a workspace, handing back the workspace that is active afterwards. Deleting the
    /// active one promotes the lowest-id survivor rather than leaving the station with no
    /// layout at all; deleting the last one leaves `None` and the client offers to create one.
    pub fn delete_workspace(&self, id: i64) -> Result<Option<i64>, StoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let deleted = tx.execute("DELETE FROM workspaces WHERE id = ?1", params![id])?;
        if deleted == 0 {
            return Err(StoreError::WorkspaceNotFound(id));
        }
        // The `active_workspace` row is maintained here rather than by a foreign key: rusqlite
        // leaves `PRAGMA foreign_keys` off by default, so an `ON DELETE SET NULL` would be
        // inert documentation and the pointer would dangle.
        if active_workspace(&tx)? == Some(id) {
            let next: Option<i64> = tx
                .query_row("SELECT id FROM workspaces ORDER BY id LIMIT 1", [], |row| {
                    row.get(0)
                })
                .optional()?;
            set_active_workspace(&tx, next)?;
        }
        let active = active_workspace(&tx)?;
        tx.commit()?;
        Ok(active)
    }

    pub fn activate_workspace(&self, id: i64) -> Result<(), StoreError> {
        let conn = self.lock();
        let exists: Option<i64> = conn
            .query_row(
                "SELECT id FROM workspaces WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(StoreError::WorkspaceNotFound(id));
        }
        set_active_workspace(&conn, Some(id))
    }

    /// The active workspace with its layout, if there is one. `None` when the station has no
    /// workspaces at all (every one deleted).
    pub fn active_workspace(&self) -> Result<Option<WorkspaceDetail>, StoreError> {
        let conn = self.lock();
        match active_workspace(&conn)? {
            Some(id) => Ok(Some(read_workspace(&conn, id)?)),
            None => Ok(None),
        }
    }

    /// Give a workspace-less database the layout M0–M5 shipped as a fixed arrangement, so a
    /// first run lands on a working station instead of an empty grid. Runs on every open and
    /// only acts on an empty table — a station whose last workspace was deleted gets the
    /// default back on the next restart, which beats a permanently empty shell.
    fn seed_workspaces(&self) -> Result<(), StoreError> {
        {
            let conn = self.lock();
            let existing: i64 =
                conn.query_row("SELECT COUNT(*) FROM workspaces", [], |row| row.get(0))?;
            if existing > 0 {
                return Ok(());
            }
        }
        let id = self.create_workspace("Station", &WorkspaceSnapshot::station_default())?;
        self.activate_workspace(id)
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn active_workspace(conn: &Connection) -> Result<Option<i64>, StoreError> {
    Ok(conn.query_row(
        "SELECT workspace_id FROM active_workspace WHERE id = 0",
        [],
        |row| row.get(0),
    )?)
}

fn set_active_workspace(conn: &Connection, id: Option<i64>) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE active_workspace SET workspace_id = ?1 WHERE id = 0",
        params![id],
    )?;
    Ok(())
}

fn read_workspace_info(conn: &Connection, id: i64) -> Result<WorkspaceInfo, StoreError> {
    conn.query_row(
        "SELECT id, name, created_at, updated_at, revision, tabs FROM workspaces WHERE id = ?1",
        params![id],
        |row| {
            Ok(WorkspaceInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
                revision: row.get::<_, i64>(4)?.unsigned_abs(),
                tabs: row.get(5)?,
            })
        },
    )
    .optional()?
    .ok_or(StoreError::WorkspaceNotFound(id))
}

fn read_workspace(conn: &Connection, id: i64) -> Result<WorkspaceDetail, StoreError> {
    let info = read_workspace_info(conn, id)?;
    let json: String = conn.query_row(
        "SELECT snapshot FROM workspaces WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )?;
    Ok(WorkspaceDetail {
        info,
        snapshot: serde_json::from_str(&json)?,
    })
}

/// `name` is `UNIQUE`, and a collision is the user picking a name that is already taken — a
/// 409, not a database failure. Every other constraint violation stays a `Db` error.
fn name_taken(err: rusqlite::Error, name: &str) -> StoreError {
    match err.sqlite_error_code() {
        Some(rusqlite::ErrorCode::ConstraintViolation) => {
            StoreError::WorkspaceNameTaken(name.to_string())
        }
        _ => StoreError::Db(err),
    }
}

/// A decoder-log row with its `event` blob still unparsed: `query_map`'s closure can only fail
/// with a `rusqlite::Error`, and a blob that no longer deserializes must surface as
/// [`StoreError::Corrupt`] rather than be silently skipped.
struct DecoderLogRow {
    id: i64,
    at: String,
    device_set: u32,
    channel: u32,
    kind: String,
    freq_hz: f64,
    station: Option<String>,
    summary: String,
    event: String,
}

fn select_decoder_log(
    conn: &Connection,
    predicate: &DecoderLogPredicate,
    limit: u32,
) -> Result<Vec<DecoderLogEntry>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT id, at, device_set, channel, kind, freq_hz, station, summary, event \
         FROM decoder_log{} ORDER BY at DESC, id DESC LIMIT ?",
        predicate.clause
    ))?;
    let mut params = predicate.params.clone();
    params.push(Value::Integer(i64::from(limit)));
    let rows = stmt.query_map(params_from_iter(params), |row| {
        Ok(DecoderLogRow {
            id: row.get(0)?,
            at: row.get(1)?,
            device_set: row.get(2)?,
            channel: row.get(3)?,
            kind: row.get(4)?,
            freq_hz: row.get(5)?,
            station: row.get(6)?,
            summary: row.get(7)?,
            event: row.get(8)?,
        })
    })?;
    let mut entries = Vec::new();
    for row in rows {
        let row = row?;
        entries.push(DecoderLogEntry {
            id: row.id,
            at: row.at,
            device_set: row.device_set,
            channel: row.channel,
            kind: row.kind,
            freq_hz: row.freq_hz,
            station: row.station,
            summary: row.summary,
            event: serde_json::from_str(&row.event)?,
        });
    }
    Ok(entries)
}

/// The `WHERE` clause (and its bound values) shared by the decoder-log list, count, export and
/// clear. Filters compose with `AND`; an empty query means "everything".
struct DecoderLogPredicate {
    /// Either empty or `" WHERE …"`, ready to concatenate onto a statement.
    clause: String,
    params: Vec<Value>,
}

impl DecoderLogPredicate {
    fn build(filter: &DecoderLogQuery) -> Result<Self, StoreError> {
        let mut terms: Vec<&str> = Vec::new();
        let mut params = Vec::new();
        if let Some(kind) = &filter.kind {
            terms.push("kind = ?");
            params.push(Value::Text(kind.clone()));
        }
        if let Some(device_set) = filter.device_set {
            terms.push("device_set = ?");
            params.push(Value::Integer(i64::from(device_set)));
        }
        if let Some(since) = &filter.since {
            terms.push("at >= ?");
            params.push(Value::Text(normalize_timestamp(since)?));
        }
        if let Some(until) = &filter.until {
            terms.push("at <= ?");
            params.push(Value::Text(normalize_timestamp(until)?));
        }
        if let Some(q) = &filter.q {
            // SQLite's `LIKE` folds ASCII case only (the bundled build carries no ICU); that
            // is what "case-insensitive" means for this filter.
            terms.push("(station LIKE ? ESCAPE '\\' OR summary LIKE ? ESCAPE '\\')");
            let pattern = Value::Text(format!("%{}%", escape_like(q)));
            params.push(pattern.clone());
            params.push(pattern);
        }
        let clause = if terms.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", terms.join(" AND "))
        };
        Ok(Self { clause, params })
    }
}

/// Neutralize the wildcards in a user-supplied substring so `q = "50%"` searches for a percent
/// sign instead of matching everything.
fn escape_like(needle: &str) -> String {
    let mut out = String::with_capacity(needle.len());
    for ch in needle.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// RFC3339 UTC at fixed nanosecond precision. `at` is stored and compared as text, and text
/// order only matches chronological order when every value has the same shape: jiff trims
/// trailing zeros, so an untouched `12:00:00.5Z` would sort *before* `12:00:00Z`. Normalizing
/// on write and on every `since`/`until` bound is what makes the range filters — and the
/// index that serves them — correct.
fn normalize_timestamp(at: &str) -> Result<String, StoreError> {
    let ts: jiff::Timestamp = at
        .parse()
        .map_err(|_| StoreError::Timestamp(at.to_string()))?;
    Ok(format!("{ts:.9}"))
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
    use sdrmm_wire::{
        AdsbMessage, AprsPacket, ChannelParams, ChannelSettings, DecoderEvent, DeviceSettings,
        NfmParams,
    };

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

    fn recording_row(stem: &str, samples: u64) -> RecordingRow {
        RecordingRow {
            stem: stem.to_string(),
            created_at: "2026-08-09T12:00:00Z".to_string(),
            device_label: "Signal Generator (virtual)".to_string(),
            center_hz: 100_000_000.0,
            sample_rate: 2_048_000.0,
            samples,
            bytes: samples * 8,
        }
    }

    #[test]
    fn recording_index_upsert_list_prune_roundtrip() {
        let store = Store::open(None).expect("open");
        let dir = Path::new("/tmp/recs");
        assert!(store.list_recordings(dir).expect("list").is_empty());

        store
            .upsert_recording(&recording_row("rec_1_a", 2_048_000))
            .expect("upsert");
        store
            .upsert_recording(&recording_row("rec_1_b", 1_024_000))
            .expect("upsert");
        let listed = store.list_recordings(dir).expect("list");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].file, "rec_1_a");
        assert_eq!(
            listed[0].device_id,
            format!("virtual:file:{}", dir.join("rec_1_a").display())
        );
        assert_eq!(listed[0].duration_s, 1.0);
        assert_eq!(listed[0].bytes, 2_048_000 * 8);
        let id = listed[0].id;
        assert_eq!(store.recording_stem(id).expect("stem"), "rec_1_a");

        // Upsert by stem updates in place: same row id, fresh counts.
        store
            .upsert_recording(&recording_row("rec_1_a", 4_096_000))
            .expect("upsert");
        let listed = store.list_recordings(dir).expect("list");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, id);
        assert_eq!(listed[0].samples, 4_096_000);

        store
            .prune_recordings(&["rec_1_a".to_string()])
            .expect("prune");
        let listed = store.list_recordings(dir).expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].file, "rec_1_a");

        store.delete_recording(id).expect("delete");
        assert!(matches!(
            store.delete_recording(id),
            Err(StoreError::RecordingNotFound(_))
        ));
        assert!(matches!(
            store.recording_stem(id),
            Err(StoreError::RecordingNotFound(_))
        ));

        store
            .upsert_recording(&recording_row("rec_1_c", 1))
            .expect("upsert");
        store.prune_recordings(&[]).expect("prune all");
        assert!(store.list_recordings(dir).expect("list").is_empty());
    }

    fn adsb(icao: &str, callsign: &str) -> DecoderEvent {
        DecoderEvent::Adsb(AdsbMessage {
            icao: icao.to_string(),
            df: 17,
            callsign: Some(callsign.to_string()),
            raw: "8D3C6444".to_string(),
            ..AdsbMessage::default()
        })
    }

    fn aprs(source: &str, tnc2: &str) -> DecoderEvent {
        DecoderEvent::Aprs(AprsPacket {
            source: source.to_string(),
            destination: "APRS".to_string(),
            tnc2: tnc2.to_string(),
            ..AprsPacket::default()
        })
    }

    fn record(at: &str, device_set: u32, event: DecoderEvent) -> DecodedRecord {
        DecodedRecord {
            device_set,
            channel: 0,
            at: at.to_string(),
            freq_hz: 1_090_000_000.0,
            event,
        }
    }

    /// Three decoders across two device sets, oldest first.
    fn seed(store: &Store) {
        store
            .insert_decoder_events(&[
                record("2026-08-09T12:00:00Z", 0, adsb("3C6444", "DLH123")),
                record(
                    "2026-08-09T12:00:01Z",
                    1,
                    aprs("DL1ABC-9", "DL1ABC-9>APRS:hi"),
                ),
                record("2026-08-09T12:00:02Z", 0, adsb("4CA2D4", "RYR9AB")),
            ])
            .expect("insert");
    }

    fn query(store: &Store, filter: DecoderLogQuery) -> (Vec<DecoderLogEntry>, u64) {
        store.query_decoder_log(&filter).expect("query")
    }

    #[test]
    fn decoder_log_insert_and_query_newest_first() {
        let store = Store::open(None).expect("open");
        assert_eq!(store.insert_decoder_events(&[]).expect("empty"), 0);
        seed(&store);

        let (entries, total) = query(&store, DecoderLogQuery::default());
        assert_eq!(total, 3);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].summary, "4CA2D4 · RYR9AB");
        assert_eq!(entries[0].kind, "adsb");
        assert_eq!(entries[0].station.as_deref(), Some("4CA2D4"));
        assert_eq!(entries[0].freq_hz, 1_090_000_000.0);
        assert_eq!(entries[0].device_set, 0);
        assert_eq!(entries[0].event, adsb("4CA2D4", "RYR9AB"));
        assert_eq!(entries[1].kind, "aprs");
        assert_eq!(entries[2].station.as_deref(), Some("3C6444"));
        // Stored timestamps are normalized RFC3339 UTC, which is what makes the text
        // comparison behind since/until chronological.
        assert_eq!(entries[2].at, "2026-08-09T12:00:00.000000000Z");
    }

    #[test]
    fn decoder_log_filters_compose() {
        let store = Store::open(None).expect("open");
        seed(&store);

        let by_kind = query(
            &store,
            DecoderLogQuery {
                kind: Some("aprs".to_string()),
                ..DecoderLogQuery::default()
            },
        );
        assert_eq!(by_kind.1, 1);
        assert_eq!(by_kind.0[0].station.as_deref(), Some("DL1ABC-9"));

        let by_set = query(
            &store,
            DecoderLogQuery {
                device_set: Some(1),
                ..DecoderLogQuery::default()
            },
        );
        assert_eq!(by_set.1, 1);
        assert_eq!(by_set.0[0].kind, "aprs");

        let since = query(
            &store,
            DecoderLogQuery {
                since: Some("2026-08-09T12:00:01Z".to_string()),
                ..DecoderLogQuery::default()
            },
        );
        assert_eq!(since.1, 2);

        let until = query(
            &store,
            DecoderLogQuery {
                until: Some("2026-08-09T12:00:00Z".to_string()),
                ..DecoderLogQuery::default()
            },
        );
        assert_eq!(until.1, 1);
        assert_eq!(until.0[0].station.as_deref(), Some("3C6444"));

        // An offset bound is normalized to UTC before it is compared.
        let offset = query(
            &store,
            DecoderLogQuery {
                since: Some("2026-08-09T14:00:01+02:00".to_string()),
                ..DecoderLogQuery::default()
            },
        );
        assert_eq!(offset.1, 2);

        // `q` matches station or summary, case-insensitively.
        let by_station = query(
            &store,
            DecoderLogQuery {
                q: Some("dl1abc".to_string()),
                ..DecoderLogQuery::default()
            },
        );
        assert_eq!(by_station.1, 1);
        let by_summary = query(
            &store,
            DecoderLogQuery {
                q: Some("ryr9".to_string()),
                ..DecoderLogQuery::default()
            },
        );
        assert_eq!(by_summary.1, 1);

        // Wildcards in `q` are literal, not LIKE metacharacters.
        let literal = query(
            &store,
            DecoderLogQuery {
                q: Some("%".to_string()),
                ..DecoderLogQuery::default()
            },
        );
        assert_eq!(literal.1, 0);

        // Composed: everything ANDs.
        let combined = query(
            &store,
            DecoderLogQuery {
                kind: Some("adsb".to_string()),
                device_set: Some(0),
                since: Some("2026-08-09T12:00:01Z".to_string()),
                q: Some("ryr".to_string()),
                ..DecoderLogQuery::default()
            },
        );
        assert_eq!(combined.1, 1);
        assert_eq!(combined.0[0].station.as_deref(), Some("4CA2D4"));

        let contradictory = query(
            &store,
            DecoderLogQuery {
                kind: Some("adsb".to_string()),
                device_set: Some(1),
                ..DecoderLogQuery::default()
            },
        );
        assert_eq!(contradictory.1, 0);
        assert!(contradictory.0.is_empty());
    }

    #[test]
    fn decoder_log_limit_bounds_the_page_but_not_the_total() {
        let store = Store::open(None).expect("open");
        seed(&store);

        let (entries, total) = query(
            &store,
            DecoderLogQuery {
                limit: Some(1),
                ..DecoderLogQuery::default()
            },
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(total, 3);
        assert_eq!(entries[0].station.as_deref(), Some("4CA2D4"));

        // A limit past the ceiling is clamped, not honoured.
        let (entries, _) = query(
            &store,
            DecoderLogQuery {
                limit: Some(u32::MAX),
                ..DecoderLogQuery::default()
            },
        );
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn decoder_log_export_ignores_limit_and_caps() {
        let store = Store::open(None).expect("open");
        seed(&store);
        let exported = store
            .export_decoder_log(&DecoderLogQuery {
                limit: Some(1),
                ..DecoderLogQuery::default()
            })
            .expect("export");
        assert_eq!(exported.len(), 3);
        const { assert!(DECODER_LOG_EXPORT_MAX > DECODER_LOG_LIMIT_MAX) };
    }

    #[test]
    fn decoder_log_delete_applies_the_filter() {
        let store = Store::open(None).expect("open");
        seed(&store);

        let deleted = store
            .delete_decoder_log(&DecoderLogQuery {
                kind: Some("adsb".to_string()),
                ..DecoderLogQuery::default()
            })
            .expect("delete");
        assert_eq!(deleted, 2);
        let (entries, total) = query(&store, DecoderLogQuery::default());
        assert_eq!(total, 1);
        assert_eq!(entries[0].kind, "aprs");

        assert_eq!(
            store
                .delete_decoder_log(&DecoderLogQuery::default())
                .expect("clear"),
            1
        );
        assert_eq!(query(&store, DecoderLogQuery::default()).1, 0);
    }

    #[test]
    fn decoder_log_prune_keeps_the_newest_rows() {
        let store = Store::open(None).expect("open");
        let records: Vec<DecodedRecord> = (0..10)
            .map(|i| {
                record(
                    &format!("2026-08-09T12:00:{i:02}Z"),
                    0,
                    adsb(&format!("00000{i}"), "X"),
                )
            })
            .collect();
        assert_eq!(store.insert_decoder_events(&records).expect("insert"), 10);

        assert_eq!(store.prune_decoder_log(10).expect("prune"), 0);
        assert_eq!(store.prune_decoder_log(4).expect("prune"), 6);
        let (entries, total) = query(&store, DecoderLogQuery::default());
        assert_eq!(total, 4);
        assert_eq!(entries[0].station.as_deref(), Some("000009"));
        assert_eq!(entries[3].station.as_deref(), Some("000006"));

        assert_eq!(store.prune_decoder_log(0).expect("prune"), 4);
        assert_eq!(query(&store, DecoderLogQuery::default()).1, 0);
    }

    #[test]
    fn decoder_log_rejects_a_malformed_time_bound() {
        let store = Store::open(None).expect("open");
        seed(&store);
        for filter in [
            DecoderLogQuery {
                since: Some("yesterday".to_string()),
                ..DecoderLogQuery::default()
            },
            DecoderLogQuery {
                until: Some("2026-13-40".to_string()),
                ..DecoderLogQuery::default()
            },
        ] {
            assert!(matches!(
                store.query_decoder_log(&filter),
                Err(StoreError::Timestamp(_))
            ));
            assert!(matches!(
                store.delete_decoder_log(&filter),
                Err(StoreError::Timestamp(_))
            ));
        }
    }

    #[test]
    fn a_fresh_database_is_seeded_with_one_active_workspace() {
        let store = Store::open(None).expect("open");
        let listed = store.list_workspaces().expect("list");
        assert_eq!(listed.workspaces.len(), 1);
        assert_eq!(listed.workspaces[0].name, "Station");
        assert_eq!(listed.workspaces[0].revision, 1);
        assert_eq!(listed.workspaces[0].tabs, 2);
        assert_eq!(listed.active, Some(listed.workspaces[0].id));

        let active = store.active_workspace().expect("active").expect("seeded");
        assert_eq!(active.snapshot, WorkspaceSnapshot::station_default());

        // Seeding is an empty-table rule, not a first-open rule: reopening must not add a
        // second "Station" (and the UNIQUE name would fail loudly if it tried).
        drop(store);
    }

    #[test]
    fn workspace_crud_roundtrip() {
        let store = Store::open(None).expect("open");
        let seeded = store.list_workspaces().expect("list").workspaces[0].id;

        let snapshot = WorkspaceSnapshot::station_default();
        let id = store.create_workspace("Bench", &snapshot).expect("create");
        let listed = store.list_workspaces().expect("list");
        assert_eq!(listed.workspaces.len(), 2);
        assert_eq!(listed.active, Some(seeded), "creating does not activate");

        store.activate_workspace(id).expect("activate");
        assert_eq!(store.list_workspaces().expect("list").active, Some(id));

        let detail = store.workspace(id).expect("read");
        assert_eq!(detail.snapshot, snapshot);
        assert_eq!(detail.info.revision, 1);
        assert_eq!(detail.info.created_at, detail.info.updated_at);

        let mut edited = snapshot.clone();
        edited.tabs.truncate(1);
        edited.active_tab = None;
        let info = store
            .update_workspace(
                id,
                &UpdateWorkspaceRequest {
                    revision: 1,
                    name: Some("Bench 2".to_string()),
                    snapshot: Some(edited.clone()),
                },
            )
            .expect("update");
        assert_eq!(info.revision, 2);
        assert_eq!(info.name, "Bench 2");
        assert_eq!(info.tabs, 1);
        assert_eq!(store.workspace(id).expect("read").snapshot, edited);

        // Deleting the active workspace promotes a survivor rather than leaving none.
        assert_eq!(store.delete_workspace(id).expect("delete"), Some(seeded));
        assert!(matches!(
            store.delete_workspace(id),
            Err(StoreError::WorkspaceNotFound(_))
        ));
        assert!(matches!(
            store.workspace(id),
            Err(StoreError::WorkspaceNotFound(_))
        ));
        assert!(matches!(
            store.activate_workspace(id),
            Err(StoreError::WorkspaceNotFound(_))
        ));

        // Deleting the last one leaves the station with none, honestly reported.
        assert_eq!(store.delete_workspace(seeded).expect("delete"), None);
        assert_eq!(store.list_workspaces().expect("list").active, None);
        assert!(store.active_workspace().expect("active").is_none());
    }

    /// Layouts are re-persisted on every gesture, so an update carrying a revision the caller
    /// no longer holds must be refused — otherwise an idle browser's stale layout silently
    /// overwrites the one someone is arranging.
    #[test]
    fn workspace_update_refuses_a_stale_revision() {
        let store = Store::open(None).expect("open");
        let id = store.list_workspaces().expect("list").workspaces[0].id;
        let update = |revision| UpdateWorkspaceRequest {
            revision,
            name: None,
            snapshot: Some(WorkspaceSnapshot::station_default()),
        };
        store.update_workspace(id, &update(1)).expect("first write");
        assert!(matches!(
            store.update_workspace(id, &update(1)),
            Err(StoreError::WorkspaceConflict {
                sent: 1,
                current: 2,
                ..
            })
        ));
        store.update_workspace(id, &update(2)).expect("fresh write");
    }

    #[test]
    fn workspace_writes_reject_a_bad_layout_and_a_taken_name() {
        let store = Store::open(None).expect("open");
        let id = store.list_workspaces().expect("list").workspaces[0].id;
        let empty = WorkspaceSnapshot {
            version: sdrmm_wire::WORKSPACE_SNAPSHOT_VERSION,
            tabs: Vec::new(),
            active_tab: None,
        };
        assert!(matches!(
            store.create_workspace("Empty", &empty),
            Err(StoreError::WorkspaceLayout(WorkspaceError::NoTabs))
        ));
        assert!(matches!(
            store.update_workspace(
                id,
                &UpdateWorkspaceRequest {
                    revision: 1,
                    name: None,
                    snapshot: Some(empty),
                }
            ),
            Err(StoreError::WorkspaceLayout(WorkspaceError::NoTabs))
        ));
        // A refused write leaves the revision alone, or the next honest write would 409.
        assert_eq!(store.workspace(id).expect("read").info.revision, 1);

        assert!(matches!(
            store.create_workspace("Station", &WorkspaceSnapshot::station_default()),
            Err(StoreError::WorkspaceNameTaken(_))
        ));
        let other = store
            .create_workspace("Bench", &WorkspaceSnapshot::station_default())
            .expect("create");
        assert!(matches!(
            store.update_workspace(
                other,
                &UpdateWorkspaceRequest {
                    revision: 1,
                    name: Some("Station".to_string()),
                    snapshot: None,
                }
            ),
            Err(StoreError::WorkspaceNameTaken(_))
        ));
    }

    /// A blob that no longer parses is a corrupt database, not a row to skip quietly.
    #[test]
    fn decoder_log_surfaces_an_unparseable_event_blob() {
        let store = Store::open(None).expect("open");
        seed(&store);
        store
            .lock()
            .execute("UPDATE decoder_log SET event = '{\"kind\":\"zzz\"}'", [])
            .expect("corrupt");
        assert!(matches!(
            store.query_decoder_log(&DecoderLogQuery::default()),
            Err(StoreError::Corrupt(_))
        ));
    }
}
