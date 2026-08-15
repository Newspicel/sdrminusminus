use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{LazyLock, Mutex, MutexGuard},
};

use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value};
use sdrmm_wire::{
    Bookmark, CreateBookmarkRequest, DecodedRecord, DecoderLogEntry, DecoderLogQuery, LogScope,
    PresetInfo, PresetSnapshot, RecordingInfo, UpdateWorkspaceRequest, WorkspaceDetail,
    WorkspaceError, WorkspaceHistory, WorkspaceInfo, WorkspaceSnapshot, WorkspaceState,
    WorkspacesResponse,
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
    #[error("workspace {id} has nothing to {step}")]
    WorkspaceHistoryEnd { id: i64, step: &'static str },
    #[error("invalid workspace layout: {0}")]
    WorkspaceLayout(#[from] sdrmm_wire::WorkspaceError),
    #[error("not an RFC3339 timestamp: {0}")]
    Timestamp(String),
    #[error("not a device_set:channel list: {0}")]
    Sources(String),
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
    "
    -- M7, the canvas (CANVAS §8 phase ⑤). A stored M6 workspace is a tabs-and-dockview tree the
    -- patch model cannot express, so the rows go rather than a converter nobody would want: the
    -- next open re-seeds the starter workspace.
    DELETE FROM workspaces;
    UPDATE active_workspace SET workspace_id = NULL;
    ALTER TABLE workspaces RENAME COLUMN tabs TO nodes;
    ",
    "
    -- Where a workspace is tuned, as opposed to what it is made of (). Its own table and
    -- not a column on `workspaces` because the writers are different: the canvas re-persists
    -- the layout under a revision check on every arrangement gesture, while this row is written
    -- by the server from the engine's own snapshot. Sharing a row would make an operator nudging
    -- a node while the dial moves a 409.
    CREATE TABLE station_state (
        workspace_id INTEGER PRIMARY KEY,
        updated_at TEXT NOT NULL,
        state TEXT NOT NULL
    );
    ",
    "
    -- `station` was the old name for what a workspace holds; the word now means only a
    -- transmitting station on the air (RDS, AIS, APRS). Renamed rather than recreated so a
    -- database keeps where its radios were tuned.
    ALTER TABLE station_state RENAME TO workspace_state;
    ",
    "
    -- A preset covers the whole workspace now (`PresetSnapshot` v2). A stored v1 row is one
    -- radio's settings and names no workspace, so there is nothing to say which of a patch's
    -- radios it meant: the rows go, like the M6 workspaces above, rather than a converter that
    -- would guess. `device_id` goes with them, replaced by the radio *count* the switcher shows.
    DELETE FROM presets;
    ALTER TABLE presets DROP COLUMN device_id;
    ALTER TABLE presets ADD COLUMN devices INTEGER NOT NULL DEFAULT 0;
    ",
    "
    -- The durable half of a row's origin. `channel` is an engine id, allocated per run and
    -- reused (CANVAS §3), so a decoder-log node scoped by it would be handed another node's
    -- history after a restart. The patch node id is stable for the node's life, which is what
    -- the scope needs — and null on every row written before this column, which is why the
    -- query keeps the (device_set, channel) pair as the fallback for exactly those rows.
    ALTER TABLE decoder_log ADD COLUMN node TEXT;
    -- Paired with the sort key like `kind`, and for the same reason: a wire-scoped page is the
    -- common read now, and it must not be a full scan.
    CREATE INDEX decoder_log_node_at ON decoder_log (node, at DESC, id DESC);
    ",
    "
    -- Which workspace's binding produced `node`. A node id is an identity only inside one
    -- workspace: templates author theirs as slugs (`ch0`, `log`) and `merge_patch` makes them
    -- unique only within the workspace it merges into, so two workspaces built from templates
    -- hold the same ids — and a log node scoped on the id alone is handed the other workspace's
    -- history. Null on rows written before this column, which the scope reads as unattributable
    -- rather than as a match for every workspace.
    ALTER TABLE decoder_log ADD COLUMN workspace INTEGER;
    ",
    "
    -- Undo/redo for the canvas. One list per workspace and not per connection: every client
    -- draws the same arrangement, so a per-browser history would let two of them undo past each
    -- other's work and each believe it had won.
    CREATE TABLE workspace_history (
        workspace_id INTEGER NOT NULL,
        -- Monotonic per workspace. Entries are states, not deltas — the same shape the row
        -- itself is stored in, so restoring one is a copy rather than an inverse operation
        -- nothing else in the app knows how to compute.
        seq INTEGER NOT NULL,
        created_at TEXT NOT NULL,
        snapshot TEXT NOT NULL,
        PRIMARY KEY (workspace_id, seq)
    );
    -- Which entry the row's own snapshot is. Zero until the first edit records one, which is
    -- also what every workspace stored before this table reads as.
    ALTER TABLE workspaces ADD COLUMN history_at INTEGER NOT NULL DEFAULT 0;
    ",
];

/// How many arrangements one workspace remembers. Deep enough that a session's worth of nudges
/// stays reachable, bounded because every entry is a whole snapshot and the table is never
/// pruned by anything else.
pub const WORKSPACE_HISTORY_DEPTH: i64 = 100;

/// Index fields for one finalized recording, derived from its SigMF pair during
/// reconciliation (: the files are the source of truth; rows are upserted by stem).
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

/// Where a batch of decoder rows came from, beyond the coordinates the frames carry themselves.
///
/// The two travel together because neither means anything alone: a node id names a decoder only
/// inside the workspace whose binding produced it, and a workspace without a binding attributes
/// nothing. With no active workspace both are empty and every row is stored unattributed.
pub struct LogOrigin<'a> {
    pub workspace: Option<i64>,
    /// `(device set, channel)` → the patch node that channel belongs to.
    pub nodes: &'a HashMap<(u32, u32), String>,
}

impl LogOrigin<'static> {
    /// The origin of a batch nothing can attribute — no active workspace, hence no binding. Its
    /// rows are stored on the coordinates they carry themselves, which is also the shape of every
    /// row written before the log recorded a node.
    #[must_use]
    pub fn unattributed() -> Self {
        static NONE: LazyLock<HashMap<(u32, u32), String>> = LazyLock::new(HashMap::new);
        Self {
            workspace: None,
            nodes: &NONE,
        }
    }
}

pub struct Store {
    conn: Mutex<Connection>,
    run_start: String,
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
            run_start: now_rfc3339(),
        };
        store.seed_workspaces()?;
        Ok(store)
    }

    pub fn create_preset(&self, name: &str, snapshot: &PresetSnapshot) -> Result<i64, StoreError> {
        let json = serde_json::to_string(snapshot)?;
        // Denormalized for the same reason `workspaces.nodes` is: a preset whose blob this build
        // cannot read must break applying that one preset, never the list it is named in.
        let devices = u32::try_from(snapshot.devices.len()).unwrap_or(u32::MAX);
        let conn = self.lock();
        conn.execute(
            "INSERT INTO presets (name, created_at, devices, snapshot) VALUES (?1, ?2, ?3, ?4)",
            params![name, now_rfc3339(), devices, json],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_presets(&self) -> Result<Vec<PresetInfo>, StoreError> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare("SELECT id, name, created_at, devices FROM presets ORDER BY id")?;
        let rows = stmt.query_map([], |row| {
            Ok(PresetInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
                devices: row.get(3)?,
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
    ///
    /// `origin` resolves a frame's `(device set, channel)` to the patch node that channel belongs
    /// to, which — with the workspace that node lives in — is the row's durable origin
    /// (`DecoderLogEntry::node`). A channel it does not know is stored without one rather than
    /// guessed at: the query has a fallback for exactly those rows, and a wrong node would
    /// attribute a frame to a decoder that never heard it.
    pub fn insert_decoder_events(
        &self,
        records: &[DecodedRecord],
        origin: &LogOrigin<'_>,
    ) -> Result<usize, StoreError> {
        if records.is_empty() {
            return Ok(0);
        }
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO decoder_log (at, device_set, channel, workspace, node, kind, \
                 freq_hz, station, summary, event) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            for record in records {
                stmt.execute(params![
                    normalize_timestamp(&record.at)?,
                    record.device_set,
                    record.channel,
                    origin.workspace,
                    origin.nodes.get(&(record.device_set, record.channel)),
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

    /// The `WHERE` clause behind every decoder-log read, clear and export.
    ///
    /// The wire scope in `filter` names patch nodes, and a node id is an identity only within one
    /// workspace — so the scope is read against the active one, which is the workspace every
    /// canvas drawing that scope is looking at. Resolving it here rather than taking it from the
    /// query keeps a caller from asking for another workspace's rows under its own node ids.
    fn decoder_log_predicate(
        &self,
        filter: &DecoderLogQuery,
    ) -> Result<DecoderLogPredicate, StoreError> {
        // Read before the callers take the connection: this locks, and the lock is not reentrant.
        let workspace = self.active_workspace_id()?;
        DecoderLogPredicate::build(filter, workspace, &self.run_start)
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
        let predicate = self.decoder_log_predicate(filter)?;
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
        let predicate = self.decoder_log_predicate(filter)?;
        let conn = self.lock();
        select_decoder_log(&conn, &predicate, DECODER_LOG_EXPORT_MAX)
    }

    /// Clear the entries matching `filter` (an empty filter clears the log).
    pub fn delete_decoder_log(&self, filter: &DecoderLogQuery) -> Result<u64, StoreError> {
        let predicate = self.decoder_log_predicate(filter)?;
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

    /// The switcher's view: every workspace, plus which one is active ( — exactly one
    /// is, server-wide, so every client opens the same workspace). Reads projection columns only.
    pub fn list_workspaces(&self) -> Result<WorkspacesResponse, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, created_at, updated_at, revision, nodes FROM workspaces ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(WorkspaceInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
                revision: row.get::<_, i64>(4)?.unsigned_abs(),
                nodes: row.get(5)?,
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
        validate_name(name)?;
        snapshot.validate()?;
        let json = serde_json::to_string(snapshot)?;
        let now = now_rfc3339();
        let conn = self.lock();
        conn.execute(
            "INSERT INTO workspaces (name, created_at, updated_at, revision, nodes, snapshot) \
             VALUES (?1, ?2, ?2, 1, ?3, ?4)",
            params![name, now, snapshot.graph.nodes.len() as i64, json],
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
        if let Some(name) = &req.name {
            validate_name(name)?;
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
            let json = serde_json::to_string(snapshot)?;
            record_history(&tx, id, &json)?;
            tx.execute(
                "UPDATE workspaces SET snapshot = ?2, nodes = ?3 WHERE id = ?1",
                params![id, json, snapshot.graph.nodes.len() as i64],
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

    /// Step the workspace back to the arrangement before its last change, and hand back what it
    /// now holds.
    ///
    /// The step is stored, not local: the row itself moves, so every client's next read — the one
    /// the `workspaces` scope asks for — sees the same canvas. What was undone stays reachable
    /// through [`redo_workspace`](Self::redo_workspace) until the next edit branches away from it.
    pub fn undo_workspace(&self, id: i64) -> Result<WorkspaceDetail, StoreError> {
        self.step_history(id, Step::Undo)
    }

    /// Step forward again through what [`undo_workspace`](Self::undo_workspace) walked back.
    pub fn redo_workspace(&self, id: i64) -> Result<WorkspaceDetail, StoreError> {
        self.step_history(id, Step::Redo)
    }

    fn step_history(&self, id: i64, step: Step) -> Result<WorkspaceDetail, StoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let at = history_at(&tx, id)?;
        let target: Option<(i64, String)> = tx
            .query_row(step.query(), params![id, at], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()?;
        let Some((seq, json)) = target else {
            return Err(StoreError::WorkspaceHistoryEnd {
                id,
                step: step.name(),
            });
        };
        // Re-serialized rather than written back verbatim: an entry recorded by an older build is
        // brought up to today's shape on the way through, exactly as a read does it.
        let snapshot = parse_workspace_snapshot(&json)?;
        tx.execute(
            "UPDATE workspaces SET snapshot = ?2, nodes = ?3, history_at = ?4, \
             revision = revision + 1, updated_at = ?5 WHERE id = ?1",
            params![
                id,
                serde_json::to_string(&snapshot)?,
                snapshot.graph.nodes.len() as i64,
                seq,
                now_rfc3339()
            ],
        )?;
        let detail = read_workspace(&tx, id)?;
        tx.commit()?;
        Ok(detail)
    }

    /// Every node id this workspace's history still names.
    ///
    /// What a node was tuned to is kept per node and pruned when the node leaves the graph
    /// (`WorkspaceState::retain_nodes`). Undo puts a deleted node back, so pruning on the graph
    /// alone would hand it back at its type's defaults — a channel returning on the wrong
    /// frequency. A node stays remembered for as long as a step can bring it back.
    pub fn history_nodes(&self, workspace_id: i64) -> Result<HashSet<String>, StoreError> {
        #[derive(serde::Deserialize)]
        struct Ids {
            graph: Graph,
        }
        #[derive(serde::Deserialize)]
        struct Graph {
            nodes: Vec<Node>,
        }
        #[derive(serde::Deserialize)]
        struct Node {
            id: String,
        }

        let conn = self.lock();
        let mut stmt =
            conn.prepare("SELECT snapshot FROM workspace_history WHERE workspace_id = ?1")?;
        let rows = stmt.query_map(params![workspace_id], |row| row.get::<_, String>(0))?;
        let mut nodes = HashSet::new();
        for json in rows {
            // A single unreadable entry costs the ids it names, not the save this is guarding:
            // erring towards keeping settings is the whole point of the set.
            if let Ok(ids) = serde_json::from_str::<Ids>(&json?) {
                nodes.extend(ids.graph.nodes.into_iter().map(|node| node.id));
            }
        }
        Ok(nodes)
    }

    /// Delete a workspace, handing back the workspace that is active afterwards. Deleting the
    /// active one promotes the lowest-id survivor rather than leaving the app with no
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
        tx.execute(
            "DELETE FROM workspace_state WHERE workspace_id = ?1",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM workspace_history WHERE workspace_id = ?1",
            params![id],
        )?;
        let active = active_workspace(&tx)?;
        tx.commit()?;
        Ok(active)
    }

    /// Where a workspace's radios were last tuned, or an empty state if it has never been
    /// captured — a workspace that has never run is not an error, it just applies at defaults.
    pub fn workspace_state(&self, workspace_id: i64) -> Result<WorkspaceState, StoreError> {
        let conn = self.lock();
        let stored: Option<String> = conn
            .query_row(
                "SELECT state FROM workspace_state WHERE workspace_id = ?1",
                params![workspace_id],
                |row| row.get(0),
            )
            .optional()?;
        match stored {
            Some(json) => Ok(serde_json::from_str::<WorkspaceState>(&json)?.current()),
            None => Ok(WorkspaceState::new()),
        }
    }

    /// Persist a workspace's settings, replacing whatever was there. No revision check: the
    /// server is the only writer and the engine's snapshot is by definition the newest truth.
    pub fn put_workspace_state(
        &self,
        workspace_id: i64,
        state: &WorkspaceState,
    ) -> Result<(), StoreError> {
        let json = serde_json::to_string(state)?;
        let conn = self.lock();
        conn.execute(
            "INSERT INTO workspace_state (workspace_id, updated_at, state) VALUES (?1, ?2, ?3) \
             ON CONFLICT(workspace_id) DO UPDATE SET updated_at = ?2, state = ?3",
            params![workspace_id, now_rfc3339(), json],
        )?;
        Ok(())
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

    /// Which workspace is active, without reading its layout. A corrupt or future-version
    /// snapshot must not take down the paths that only need the id — deleting the broken
    /// workspace is the recovery, and it cannot be the thing that fails.
    pub fn active_workspace_id(&self) -> Result<Option<i64>, StoreError> {
        let conn = self.lock();
        active_workspace(&conn)
    }

    /// The active workspace with its layout, if there is one. `None` when the app has no
    /// workspaces at all (every one deleted).
    pub fn active_workspace(&self) -> Result<Option<WorkspaceDetail>, StoreError> {
        let conn = self.lock();
        match active_workspace(&conn)? {
            Some(id) => Ok(Some(read_workspace(&conn, id)?)),
            None => Ok(None),
        }
    }

    /// Give a workspace-less database the starter workspace, so a first run lands on a device
    /// node and a scope instead of an empty canvas. Runs on every open and only acts on an empty
    /// table — an install whose last workspace was deleted gets the default back on the next
    /// restart, which beats a permanently empty shell, and it is also what re-seeds a database
    /// whose M6 workspaces migration 5 cleared.
    fn seed_workspaces(&self) -> Result<(), StoreError> {
        {
            let conn = self.lock();
            let existing: i64 =
                conn.query_row("SELECT COUNT(*) FROM workspaces", [], |row| row.get(0))?;
            if existing > 0 {
                return Ok(());
            }
        }
        let id = self.create_workspace("Workspace", &WorkspaceSnapshot::starter())?;
        self.activate_workspace(id)
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The same bound on both write paths: a workspace is picked by name in the switcher, so an
/// empty or unprintably long one is a broken row, not a preference.
fn validate_name(name: &str) -> Result<(), StoreError> {
    if name.trim().is_empty() || name.chars().count() > sdrmm_wire::workspace::MAX_NAME_LEN {
        return Err(StoreError::WorkspaceLayout(WorkspaceError::Name));
    }
    Ok(())
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
        "SELECT id, name, created_at, updated_at, revision, nodes FROM workspaces WHERE id = ?1",
        params![id],
        |row| {
            Ok(WorkspaceInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
                revision: row.get::<_, i64>(4)?.unsigned_abs(),
                nodes: row.get(5)?,
            })
        },
    )
    .optional()?
    .ok_or(StoreError::WorkspaceNotFound(id))
}

fn read_workspace(conn: &Connection, id: i64) -> Result<WorkspaceDetail, StoreError> {
    let info = read_workspace_info(conn, id)?;
    let (json, at): (String, i64) = conn.query_row(
        "SELECT snapshot, history_at FROM workspaces WHERE id = ?1",
        params![id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(WorkspaceDetail {
        info,
        snapshot: parse_workspace_snapshot(&json)?,
        history: read_history(conn, id, at)?,
    })
}

/// Which way [`Store::step_history`] walks, and the entry that lands.
#[derive(Clone, Copy)]
enum Step {
    Undo,
    Redo,
}

impl Step {
    /// The nearest recorded state on this side of where the workspace stands.
    fn query(self) -> &'static str {
        match self {
            Self::Undo => {
                "SELECT seq, snapshot FROM workspace_history \
                 WHERE workspace_id = ?1 AND seq < ?2 ORDER BY seq DESC LIMIT 1"
            }
            Self::Redo => {
                "SELECT seq, snapshot FROM workspace_history \
                 WHERE workspace_id = ?1 AND seq > ?2 ORDER BY seq ASC LIMIT 1"
            }
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Undo => "undo",
            Self::Redo => "redo",
        }
    }
}

fn history_at(conn: &Connection, id: i64) -> Result<i64, StoreError> {
    conn.query_row(
        "SELECT history_at FROM workspaces WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )
    .optional()?
    .ok_or(StoreError::WorkspaceNotFound(id))
}

fn read_history(conn: &Connection, id: i64, at: i64) -> Result<WorkspaceHistory, StoreError> {
    let (undo, redo): (bool, bool) = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspace_history WHERE workspace_id = ?1 AND seq < ?2), \
                EXISTS(SELECT 1 FROM workspace_history WHERE workspace_id = ?1 AND seq > ?2)",
        params![id, at],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(WorkspaceHistory {
        can_undo: undo,
        can_redo: redo,
    })
}

/// Record the state a write is producing, so the write can be walked back out of.
///
/// The entry list is the workspace's own line of arrangements: the first write also records what
/// it is *leaving*, since undo has to be able to return there and a workspace stored before this
/// table has no entry for where it stands. Editing after an undo branches — what redo would have
/// led to is dropped, the same rule a text editor follows.
fn record_history(conn: &Connection, id: i64, json: &str) -> Result<(), StoreError> {
    let at = history_at(conn, id)?;
    let now = now_rfc3339();
    let at = if at == 0 {
        let previous: String = conn.query_row(
            "SELECT snapshot FROM workspaces WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        conn.execute(
            "DELETE FROM workspace_history WHERE workspace_id = ?1",
            params![id],
        )?;
        conn.execute(
            "INSERT INTO workspace_history (workspace_id, seq, created_at, snapshot) \
             VALUES (?1, 1, ?2, ?3)",
            params![id, now, previous],
        )?;
        1
    } else {
        at
    };
    // A write that changes nothing is not a step: the canvas re-persists the whole workspace on
    // every gesture, and an undo that visibly does nothing is worse than one fewer step.
    let unchanged: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspace_history \
         WHERE workspace_id = ?1 AND seq = ?2 AND snapshot = ?3)",
        params![id, at, json],
        |row| row.get(0),
    )?;
    if unchanged {
        return Ok(());
    }
    conn.execute(
        "DELETE FROM workspace_history WHERE workspace_id = ?1 AND seq > ?2",
        params![id, at],
    )?;
    let seq = at + 1;
    conn.execute(
        "INSERT INTO workspace_history (workspace_id, seq, created_at, snapshot) \
         VALUES (?1, ?2, ?3, ?4)",
        params![id, seq, now, json],
    )?;
    conn.execute(
        "DELETE FROM workspace_history WHERE workspace_id = ?1 AND seq <= ?2",
        params![id, seq - WORKSPACE_HISTORY_DEPTH],
    )?;
    conn.execute(
        "UPDATE workspaces SET history_at = ?2 WHERE id = ?1",
        params![id, seq],
    )?;
    Ok(())
}

fn parse_workspace_snapshot(json: &str) -> Result<WorkspaceSnapshot, serde_json::Error> {
    let mut value: serde_json::Value = serde_json::from_str(json)?;
    migrate_call_buffers(&mut value);
    serde_json::from_value(value)
}

fn migrate_call_buffers(snapshot: &mut serde_json::Value) {
    let Some(graph) = snapshot.get_mut("graph") else {
        return;
    };
    let legacy: HashMap<String, serde_json::Value> = graph
        .get("nodes")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|node| node.get("kind").and_then(serde_json::Value::as_str) == Some("call_buffer"))
        .filter_map(|node| {
            Some((
                node.get("id")?.as_str()?.to_owned(),
                node.get("data").cloned().unwrap_or_default(),
            ))
        })
        .collect();
    if legacy.is_empty() {
        return;
    }
    let legacy_ids: HashSet<&str> = legacy.keys().map(String::as_str).collect();
    let system_settings: HashMap<String, serde_json::Value> = graph
        .get("edges")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|edge| {
            let source = edge.get("from")?.get("node")?.as_str()?;
            let target = edge.get("to")?.get("node")?.as_str()?;
            legacy
                .get(target)
                .cloned()
                .map(|settings| (source.to_owned(), settings))
        })
        .collect();
    let systems: HashSet<String> = graph
        .get("nodes")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|node| node.get("kind").and_then(serde_json::Value::as_str) == Some("dmr_trunk"))
        .filter_map(|node| node.get("id")?.as_str().map(str::to_owned))
        .collect();
    if let Some(nodes) = graph
        .get_mut("nodes")
        .and_then(serde_json::Value::as_array_mut)
    {
        nodes.retain(|node| {
            node.get("id")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|id| !legacy_ids.contains(id))
        });
        for node in nodes {
            let Some(id) = node.get("id").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let Some(settings) = system_settings
                .get(id)
                .and_then(serde_json::Value::as_object)
            else {
                continue;
            };
            let Some(data) = node
                .get_mut("data")
                .and_then(serde_json::Value::as_object_mut)
            else {
                continue;
            };
            if let Some(value) = settings.get("retention_seconds") {
                data.insert("retention_seconds".to_owned(), value.clone());
            }
        }
    }
    if let Some(edges) = graph
        .get_mut("edges")
        .and_then(serde_json::Value::as_array_mut)
    {
        edges.retain(|edge| {
            let source = edge
                .get("from")
                .and_then(|end| end.get("node"))
                .and_then(serde_json::Value::as_str);
            let target = edge
                .get("to")
                .and_then(|end| end.get("node"))
                .and_then(serde_json::Value::as_str);
            !source.is_some_and(|id| legacy_ids.contains(id))
                && !target.is_some_and(|id| legacy_ids.contains(id))
                && !(source.is_some_and(|id| systems.contains(id))
                    && edge
                        .get("from")
                        .and_then(|end| end.get("port"))
                        .and_then(serde_json::Value::as_str)
                        == Some("trunk_audio"))
        });
        for edge in edges {
            let target_is_system = edge
                .get("to")
                .and_then(|end| end.get("node"))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|id| systems.contains(id));
            let source_is_system = edge
                .get("from")
                .and_then(|end| end.get("node"))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|id| systems.contains(id));
            if target_is_system
                && let Some(port) = edge.get_mut("to").and_then(|end| end.get_mut("port"))
                && port.as_str() == Some("carriers")
            {
                *port = serde_json::Value::String("events".to_owned());
            }
            if source_is_system
                && let Some(port) = edge.get_mut("from").and_then(|end| end.get_mut("port"))
                && port.as_str() == Some("trunk_events")
            {
                *port = serde_json::Value::String("events".to_owned());
            }
        }
    }
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
    node: Option<String>,
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
        "SELECT id, at, device_set, channel, node, kind, freq_hz, station, summary, event \
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
            node: row.get(4)?,
            kind: row.get(5)?,
            freq_hz: row.get(6)?,
            station: row.get(7)?,
            summary: row.get(8)?,
            event: row.get(9)?,
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
            node: row.node,
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
    fn build(
        filter: &DecoderLogQuery,
        workspace: Option<i64>,
        run_start: &str,
    ) -> Result<Self, StoreError> {
        // Every other term is a literal; this one is as wide as the channel list, so it is built
        // here to outlive the borrows in `terms`.
        let sources_term: String;
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
        let scope = filter
            .scope()
            .map_err(|bad| StoreError::Sources(bad.to_owned()))?;
        if let Some(scope) = scope {
            sources_term = scope_clause(&scope, workspace, run_start, &mut params);
            terms.push(&sources_term);
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

/// The `WHERE` fragment for a wire scope, binding its values into `params`.
///
/// The two halves are an OR, and each carries the guard that makes its half an identity rather
/// than a name something else also answers to:
///
/// * A node id is unique only within its workspace — templates author theirs as slugs and
///   `merge_patch` deduplicates only inside the workspace it merges into — so the node half is
///   read against the workspace that produced it. With no active workspace nothing can be
///   attributed by node at all, and the half is dropped.
/// * The coordinate half exists for rows that carry no node (written before the log recorded one,
///   or in the window before the binding caught up with a new channel), and `(device_set,
///   channel)` names a decoder only for the run that allocated it. Bounding it to this run is
///   what keeps a channel from inheriting the frames of whoever held its id last time.
///
/// The null guard on the second half is what keeps the two from crossing: a row that *has* a node
/// is attributable, so it must be matched by node alone.
///
/// An empty scope is a node with nothing wired into it, and matches no row. "Show me the frames
/// from nothing" is not "show me everything", and the same clause backs the clear endpoint.
fn scope_clause(
    scope: &LogScope,
    workspace: Option<i64>,
    run_start: &str,
    params: &mut Vec<Value>,
) -> String {
    let mut halves: Vec<String> = Vec::new();
    if !scope.nodes.is_empty()
        && let Some(workspace) = workspace
    {
        params.push(Value::Integer(workspace));
        for node in &scope.nodes {
            params.push(Value::Text(node.clone()));
        }
        let slots = vec!["?"; scope.nodes.len()].join(", ");
        halves.push(format!("(workspace = ? AND node IN ({slots}))"));
    }
    if !scope.channels.is_empty() {
        params.push(Value::Text(run_start.to_owned()));
        for (device_set, channel) in &scope.channels {
            params.push(Value::Integer(i64::from(*device_set)));
            params.push(Value::Integer(i64::from(*channel)));
        }
        let pairs = vec!["(device_set = ? AND channel = ?)"; scope.channels.len()];
        halves.push(format!(
            "(node IS NULL AND at >= ? AND ({}))",
            pairs.join(" OR ")
        ));
    }
    if halves.is_empty() {
        return "0".to_owned();
    }
    format!("({})", halves.join(" OR "))
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
    Ok(rfc3339(ts))
}

/// Fixed-width fractional seconds. Every timestamp column is ordered and range-compared as
/// text by SQLite, and jiff's own `Display` trims trailing zeros — `.9812Z` then compares
/// greater than the later `.98125Z`.
fn rfc3339(ts: jiff::Timestamp) -> String {
    format!("{ts:.9}")
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
    rfc3339(jiff::Timestamp::now())
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{
        AdsbMessage, AprsPacket, ChannelParams, ChannelSettings, DecoderEvent, DeviceSettings,
        NfmParams, PRESET_SNAPSHOT_VERSION, PresetDevice,
    };

    use super::*;

    fn snapshot() -> PresetSnapshot {
        PresetSnapshot {
            version: PRESET_SNAPSHOT_VERSION,
            devices: vec![PresetDevice {
                node: "device".to_string(),
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
        assert_eq!(listed[0].devices, 1);
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

    fn bound(workspace: i64, nodes: &HashMap<(u32, u32), String>) -> LogOrigin<'_> {
        LogOrigin {
            workspace: Some(workspace),
            nodes,
        }
    }

    /// The workspace `Store::open` seeded and activated — the one a scoped query is read against.
    fn active(store: &Store) -> i64 {
        store
            .active_workspace_id()
            .expect("read the active workspace")
            .expect("open seeds and activates one")
    }

    /// Three decoders across two device sets, oldest first.
    fn seed(store: &Store) {
        store
            .insert_decoder_events(
                &[
                    record("2026-08-09T12:00:00Z", 0, adsb("3C6444", "DLH123")),
                    record(
                        "2026-08-09T12:00:01Z",
                        1,
                        aprs("DL1ABC-9", "DL1ABC-9>APRS:hi"),
                    ),
                    record("2026-08-09T12:00:02Z", 0, adsb("4CA2D4", "RYR9AB")),
                ],
                &LogOrigin::unattributed(),
            )
            .expect("insert");
    }

    fn query(store: &Store, filter: DecoderLogQuery) -> (Vec<DecoderLogEntry>, u64) {
        store.query_decoder_log(&filter).expect("query")
    }

    #[test]
    fn decoder_log_insert_and_query_newest_first() {
        let store = Store::open(None).expect("open");
        assert_eq!(
            store
                .insert_decoder_events(&[], &LogOrigin::unattributed())
                .expect("empty"),
            0
        );
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

        let offset = query(
            &store,
            DecoderLogQuery {
                since: Some("2026-08-09T14:00:01+02:00".to_string()),
                ..DecoderLogQuery::default()
            },
        );
        assert_eq!(offset.1, 2);

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

    /// The filter a canvas node draws with its wires. Channel ids are allocated per device set,
    /// so `0:1` and `1:1` are different channels and the pair — never the channel alone — is what
    /// a term matches. Written in this run, because that is the only run these coordinates name.
    #[test]
    fn decoder_log_sources_filter_names_channels_not_device_sets() {
        let store = Store::open(None).expect("open");
        let now = now_rfc3339();
        let on = |device_set: u32, channel: u32, icao: &str| DecodedRecord {
            channel,
            ..record(&now, device_set, adsb(icao, "FLIGHT"))
        };
        store
            .insert_decoder_events(
                &[on(0, 1, "AAAAAA"), on(0, 2, "BBBBBB"), on(1, 1, "CCCCCC")],
                &LogOrigin::unattributed(),
            )
            .expect("insert");
        let stations = |sources: &str| {
            query(
                &store,
                DecoderLogQuery {
                    sources: Some(sources.to_owned()),
                    ..DecoderLogQuery::default()
                },
            )
            .0
            .into_iter()
            .filter_map(|entry| entry.station)
            .collect::<Vec<_>>()
        };
        assert_eq!(stations("0:1"), ["AAAAAA"]);
        assert_eq!(stations("1:1"), ["CCCCCC"]);
        assert_eq!(stations("0:2,1:1"), ["CCCCCC", "BBBBBB"]);

        // A node with nothing wired into it matches nothing — and, crucially, its Clear button
        // deletes nothing rather than the whole log.
        assert!(stations("").is_empty());
        let empty = DecoderLogQuery {
            sources: Some(String::new()),
            ..DecoderLogQuery::default()
        };
        assert_eq!(store.delete_decoder_log(&empty).expect("clear"), 0);
        assert_eq!(query(&store, DecoderLogQuery::default()).1, 3);

        // A list that will not parse is refused, never quietly dropped — dropping it would widen
        // the query the caller asked to narrow.
        let malformed = DecoderLogQuery {
            sources: Some("0:1,nonsense".to_owned()),
            ..DecoderLogQuery::default()
        };
        assert!(matches!(
            store.query_decoder_log(&malformed),
            Err(StoreError::Sources(_))
        ));
        assert!(matches!(
            store.delete_decoder_log(&malformed),
            Err(StoreError::Sources(_))
        ));
    }

    /// The whole point of the node column: an engine channel id is reused between runs, so a
    /// scope built on one hands a node whatever else has held that id. The node id is stable for
    /// the node's life, and it wins wherever a row carries one.
    /// `run_start` is compared against stored `at` values as text, so the two have to be written
    /// to the same width. They were not: stored rows were normalized to nine fractional digits
    /// while `run_start` took jiff's trimmed `Display`, and a `run_start` whose microseconds
    /// ended in a zero compared greater than rows logged after it.
    #[test]
    fn a_trimmed_fraction_never_outsorts_a_later_timestamp() {
        let trimmed = jiff::Timestamp::from_nanosecond(1_000_000_000_981_200_000).expect("ts");
        let later = jiff::Timestamp::from_nanosecond(1_000_000_000_981_250_000).expect("ts");
        assert!(trimmed < later);
        assert!(rfc3339(trimmed) < rfc3339(later));
        assert_eq!(
            normalize_timestamp(&rfc3339(trimmed)).expect("normalize"),
            rfc3339(trimmed),
            "a stored timestamp and the run start it is compared against must agree"
        );
        assert_eq!(now_rfc3339().len(), rfc3339(trimmed).len());
    }

    #[test]
    fn decoder_log_scope_prefers_the_node_over_the_reused_channel_id() {
        let store = Store::open(None).expect("open");
        let workspace = active(&store);
        let now = now_rfc3339();
        let on = |channel: u32, icao: &str| DecodedRecord {
            channel,
            ..record(&now, 0, adsb(icao, "FLIGHT"))
        };
        // Two nodes that held channel 1 in different runs, plus a row from before the column
        // existed — the three cases the scope has to keep apart.
        store
            .insert_decoder_events(
                &[on(1, "AAAAAA")],
                &bound(
                    workspace,
                    &HashMap::from([((0, 1), "channel:old".to_owned())]),
                ),
            )
            .expect("insert");
        store
            .insert_decoder_events(
                &[on(1, "BBBBBB")],
                &bound(
                    workspace,
                    &HashMap::from([((0, 1), "channel:new".to_owned())]),
                ),
            )
            .expect("insert");
        store
            .insert_decoder_events(&[on(1, "LEGACY")], &LogOrigin::unattributed())
            .expect("insert");

        let stations = |nodes: &str, sources: &str| {
            query(
                &store,
                DecoderLogQuery {
                    nodes: Some(nodes.to_owned()),
                    sources: Some(sources.to_owned()),
                    ..DecoderLogQuery::default()
                },
            )
            .0
            .into_iter()
            .filter_map(|entry| entry.station)
            .collect::<Vec<_>>()
        };

        assert_eq!(stations("channel:new", "0:1"), ["LEGACY", "BBBBBB"]);
        assert_eq!(stations("channel:old", "0:1"), ["LEGACY", "AAAAAA"]);
        // Without the fallback the nodeless row is unreachable, which is what makes it a fallback
        // and not the filter.
        assert_eq!(stations("channel:new", ""), ["BBBBBB"]);
        // And the fallback alone reaches only the rows that have no better identity.
        assert_eq!(stations("", "0:1"), ["LEGACY"]);
        assert!(stations("", "").is_empty());

        let entries = query(&store, DecoderLogQuery::default()).0;
        assert_eq!(entries[0].node, None, "the legacy row carries no node");
        assert_eq!(entries[2].node.as_deref(), Some("channel:old"));
    }

    /// The fallback half names coordinates, and coordinates are only an identity for the run that
    /// allocated them: channel `0:1` is a different decoder in every run that had one. A nodeless
    /// row from an earlier run is therefore out of every scope — it is history the log keeps, not
    /// something the decoder wired in now ever heard.
    #[test]
    fn decoder_log_scope_fallback_stops_at_the_start_of_this_run() {
        let store = Store::open(None).expect("open");
        let on = |at: &str, icao: &str| DecodedRecord {
            channel: 1,
            ..record(at, 0, adsb(icao, "FLIGHT"))
        };
        store
            .insert_decoder_events(
                &[
                    on("2026-08-09T12:00:00Z", "LASTRUN"),
                    on(&now_rfc3339(), "THISRUN"),
                ],
                &LogOrigin::unattributed(),
            )
            .expect("insert");

        let scoped = query(
            &store,
            DecoderLogQuery {
                sources: Some("0:1".to_owned()),
                ..DecoderLogQuery::default()
            },
        );
        assert_eq!(
            scoped
                .0
                .into_iter()
                .filter_map(|entry| entry.station)
                .collect::<Vec<_>>(),
            ["THISRUN"]
        );
        // Both rows are still in the log; it is the wire scope that cannot reach the older one.
        assert_eq!(query(&store, DecoderLogQuery::default()).1, 2);
    }

    /// A node id is an identity only inside its workspace. Templates author theirs as slugs
    /// (`ch0`, `log`), so two workspaces built from templates hold the same ids — and without the
    /// workspace guard a log node would be handed the other workspace's history, which is the
    /// leak the column exists to close.
    #[test]
    fn decoder_log_scope_does_not_cross_workspaces_sharing_a_node_id() {
        let store = Store::open(None).expect("open");
        let first = active(&store);
        let second = store
            .create_workspace("second", &WorkspaceSnapshot::starter())
            .expect("create");
        let now = now_rfc3339();
        let nodes = HashMap::from([((0, 1), "ch0".to_owned())]);
        let on = |icao: &str| DecodedRecord {
            channel: 1,
            ..record(&now, 0, adsb(icao, "FLIGHT"))
        };
        store
            .insert_decoder_events(&[on("FIRSTWS")], &bound(first, &nodes))
            .expect("insert");
        store
            .insert_decoder_events(&[on("SECONDWS")], &bound(second, &nodes))
            .expect("insert");

        let stations = || {
            query(
                &store,
                DecoderLogQuery {
                    nodes: Some("ch0".to_owned()),
                    ..DecoderLogQuery::default()
                },
            )
            .0
            .into_iter()
            .filter_map(|entry| entry.station)
            .collect::<Vec<_>>()
        };
        assert_eq!(stations(), ["FIRSTWS"]);
        store.activate_workspace(second).expect("activate");
        assert_eq!(stations(), ["SECONDWS"]);
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
        assert_eq!(
            store
                .insert_decoder_events(&records, &LogOrigin::unattributed())
                .expect("insert"),
            10
        );

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
        assert_eq!(listed.workspaces[0].name, "Workspace");
        assert_eq!(listed.workspaces[0].revision, 1);
        assert_eq!(listed.workspaces[0].nodes, 3);
        assert_eq!(listed.active, Some(listed.workspaces[0].id));

        let active = store.active_workspace().expect("active").expect("seeded");
        assert_eq!(active.snapshot, WorkspaceSnapshot::starter());

        // Seeding is an empty-table rule, not a first-open rule: reopening must not add a
        // second "Workspace" (and the UNIQUE name would fail loudly if it tried).
        drop(store);
    }

    /// Upgrading an existing log must keep every row it already has. The origin columns arrive
    /// null on all of them, which is not a gap to be filled in — nothing recorded which node or
    /// workspace those frames came from, and inventing one would attribute them to whichever
    /// decoder happens to hold that channel id now.
    ///
    /// So they stay in the log, queryable and exportable, and out of every wire scope: a row that
    /// names neither and predates this run is history, not something a decoder wired in now was
    /// ever heard by.
    #[test]
    fn adding_the_origin_columns_keeps_the_rows_already_logged() {
        let file = tempfile::NamedTempFile::new().expect("temp db");
        {
            let conn = Connection::open(file.path()).expect("open");
            // A database as the release that first shipped a decoder log left it — found by the
            // migration rather than counted from the end, so a later one cannot silently move it.
            let created = MIGRATIONS
                .iter()
                .position(|migration| migration.contains("CREATE TABLE decoder_log"))
                .expect("the log has a migration");
            for (i, migration) in MIGRATIONS.iter().take(created + 1).enumerate() {
                conn.execute_batch(&format!(
                    "BEGIN;\n{migration}\nPRAGMA user_version = {};\nCOMMIT;",
                    i + 1
                ))
                .expect("migrate");
            }
            conn.execute(
                "INSERT INTO decoder_log (at, device_set, channel, kind, freq_hz, station, \
                 summary, event) VALUES ('2026-08-09T12:00:00.000000000Z', 0, 1, 'adsb', \
                 1090000000.0, 'LEGACY', 'LEGACY', '{\"kind\":\"adsb\",\"data\":{\"icao\":\
                 \"LEGACY\",\"df\":17,\"raw\":\"8d\"}}')",
                [],
            )
            .expect("a row from before the columns");
        }

        let store = Store::open(Some(file.path())).expect("reopen");
        let (entries, total) = query(&store, DecoderLogQuery::default());
        assert_eq!(total, 1, "the upgrade kept the row");
        assert_eq!(entries[0].node, None);
        assert_eq!(
            store
                .export_decoder_log(&DecoderLogQuery::default())
                .expect("export")
                .len(),
            1,
            "and the export still reaches it"
        );

        let scoped = |nodes: &str, sources: &str| {
            query(
                &store,
                DecoderLogQuery {
                    nodes: Some(nodes.to_owned()),
                    sources: Some(sources.to_owned()),
                    ..DecoderLogQuery::default()
                },
            )
            .1
        };
        assert_eq!(scoped("channel:whatever", "0:1"), 0);
        assert_eq!(scoped("channel:whatever", ""), 0);
    }

    #[test]
    fn the_canvas_migration_clears_m6_workspaces_and_re_seeds() {
        let file = tempfile::NamedTempFile::new().expect("temp db");
        {
            let conn = Connection::open(file.path()).expect("open");
            for (i, migration) in MIGRATIONS.iter().take(4).enumerate() {
                conn.execute_batch(&format!(
                    "BEGIN;\n{migration}\nPRAGMA user_version = {};\nCOMMIT;",
                    i + 1
                ))
                .expect("migrate");
            }
            conn.execute(
                "INSERT INTO workspaces (name, created_at, updated_at, revision, tabs, snapshot) \
                 VALUES ('Old', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 7, 2, \
                 '{\"version\":1,\"tabs\":[]}')",
                [],
            )
            .expect("an M6 row");
            conn.execute("UPDATE active_workspace SET workspace_id = 1", [])
                .expect("active");
        }

        let store = Store::open(Some(file.path())).expect("reopen");
        let listed = store.list_workspaces().expect("list");
        assert_eq!(listed.workspaces.len(), 1, "the M6 row is gone");
        assert_eq!(listed.workspaces[0].name, "Workspace");
        assert_eq!(listed.active, Some(listed.workspaces[0].id));
        assert_eq!(
            store
                .active_workspace()
                .expect("active")
                .expect("seeded")
                .snapshot,
            WorkspaceSnapshot::starter()
        );
    }

    #[test]
    fn a_stored_call_buffer_is_folded_into_its_dmr_system() {
        let mut value = serde_json::to_value(WorkspaceSnapshot::starter()).expect("snapshot");
        let graph = value
            .get_mut("graph")
            .and_then(serde_json::Value::as_object_mut)
            .expect("graph");
        let nodes = graph
            .get_mut("nodes")
            .and_then(serde_json::Value::as_array_mut)
            .expect("nodes");
        nodes.extend([
            serde_json::json!({
                "id": "carrier",
                "position": { "x": 0.0, "y": 0.0 },
                "kind": "channel",
                "data": { "channel_type": "dmr" }
            }),
            serde_json::json!({
                "id": "system",
                "position": { "x": 100.0, "y": 0.0 },
                "kind": "dmr_trunk",
                "data": { "protocol": "auto" }
            }),
            serde_json::json!({
                "id": "buffer",
                "position": { "x": 200.0, "y": 0.0 },
                "kind": "call_buffer",
                "data": { "record_audio": false, "retention_seconds": 900 }
            }),
        ]);
        let edges = graph
            .get_mut("edges")
            .and_then(serde_json::Value::as_array_mut)
            .expect("edges");
        edges.extend([
            serde_json::json!({
                "from": { "node": "carrier", "port": "events" },
                "to": { "node": "system", "port": "carriers" }
            }),
            serde_json::json!({
                "from": { "node": "system", "port": "trunk_events" },
                "to": { "node": "buffer", "port": "trunk_events" }
            }),
            serde_json::json!({
                "from": { "node": "system", "port": "trunk_audio" },
                "to": { "node": "buffer", "port": "trunk_audio" }
            }),
        ]);

        let migrated = parse_workspace_snapshot(&value.to_string()).expect("migrated");
        migrated.validate().expect("valid");
        assert!(migrated.graph.node("buffer").is_none());
        let system = migrated.graph.node("system").expect("system");
        let sdrmm_wire::NodeBody::DmrTrunk(settings) = &system.body else {
            panic!("DMR system");
        };
        assert_eq!(settings.retention_seconds, 900);
        assert!(migrated.graph.edges.iter().any(|edge| {
            edge.from.node == "carrier"
                && edge.from.port == "events"
                && edge.to.node == "system"
                && edge.to.port == "events"
        }));
    }

    #[test]
    fn workspace_crud_roundtrip() {
        let store = Store::open(None).expect("open");
        let seeded = store.list_workspaces().expect("list").workspaces[0].id;

        let snapshot = WorkspaceSnapshot::starter();
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
        edited.graph.nodes.retain(|node| node.id != "speaker");
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
        assert_eq!(info.nodes, 2);
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

        assert_eq!(store.delete_workspace(seeded).expect("delete"), None);
        assert_eq!(store.list_workspaces().expect("list").active, None);
        assert!(store.active_workspace().expect("active").is_none());
    }

    /// A workspace without the node named here — the shape a canvas edit has, since every
    /// gesture re-persists the whole arrangement.
    fn without(node: &str) -> WorkspaceSnapshot {
        let mut snapshot = WorkspaceSnapshot::starter();
        snapshot.graph.nodes.retain(|held| held.id != node);
        snapshot
            .graph
            .edges
            .retain(|edge| edge.from.node != node && edge.to.node != node);
        snapshot
    }

    fn write(store: &Store, id: i64, snapshot: &WorkspaceSnapshot) -> u64 {
        let revision = store.workspace(id).expect("read").info.revision;
        store
            .update_workspace(
                id,
                &UpdateWorkspaceRequest {
                    revision,
                    name: None,
                    snapshot: Some(snapshot.clone()),
                },
            )
            .expect("update")
            .revision
    }

    /// The history is the workspace's, not a client's: a step is stored, so the next read every
    /// browser makes is the one that was stepped to.
    #[test]
    fn workspace_history_walks_back_out_of_its_own_edits() {
        let store = Store::open(None).expect("open");
        let id = store.list_workspaces().expect("list").workspaces[0].id;
        let starter = WorkspaceSnapshot::starter();

        // Nothing has been edited, so there is nowhere to step.
        let fresh = store.workspace(id).expect("read");
        assert_eq!(fresh.history, WorkspaceHistory::default());
        assert!(matches!(
            store.undo_workspace(id),
            Err(StoreError::WorkspaceHistoryEnd { step: "undo", .. })
        ));

        write(&store, id, &without("speaker"));
        write(&store, id, &without("scope"));

        let back = store.undo_workspace(id).expect("undo");
        assert_eq!(back.snapshot, without("speaker"));
        assert!(back.history.can_undo && back.history.can_redo);
        // A step is a change like any other: the revision moves, so a client holding the old one
        // cannot overwrite it without reading first.
        assert!(back.info.revision > 3);
        assert_eq!(back.info.nodes, 2);

        let base = store.undo_workspace(id).expect("undo to the start");
        assert_eq!(base.snapshot, starter, "the state the first edit left");
        assert!(!base.history.can_undo && base.history.can_redo);
        assert!(matches!(
            store.undo_workspace(id),
            Err(StoreError::WorkspaceHistoryEnd { step: "undo", .. })
        ));

        assert_eq!(
            store.redo_workspace(id).expect("redo").snapshot,
            without("speaker")
        );
        let forward = store.redo_workspace(id).expect("redo");
        assert_eq!(forward.snapshot, without("scope"));
        assert!(!forward.history.can_redo);
        assert!(matches!(
            store.redo_workspace(id),
            Err(StoreError::WorkspaceHistoryEnd { step: "redo", .. })
        ));
    }

    /// Editing after an undo branches: what redo would have led to is gone, exactly as it is in
    /// every editor. Keeping it would offer a step forward into an arrangement nobody can reach
    /// from what is on screen.
    #[test]
    fn an_edit_after_an_undo_drops_what_redo_would_have_reached() {
        let store = Store::open(None).expect("open");
        let id = store.list_workspaces().expect("list").workspaces[0].id;
        write(&store, id, &without("speaker"));
        write(&store, id, &without("scope"));
        store.undo_workspace(id).expect("undo");

        write(&store, id, &without("device"));
        let now = store.workspace(id).expect("read");
        assert_eq!(now.snapshot, without("device"));
        assert!(now.history.can_undo && !now.history.can_redo);
        assert_eq!(
            store.undo_workspace(id).expect("undo").snapshot,
            without("speaker"),
            "the branch it was made from"
        );
    }

    /// A gesture that ends where it started still re-persists the whole workspace. Recording it
    /// would cost a step that visibly does nothing.
    #[test]
    fn a_write_that_changes_nothing_is_not_a_step() {
        let store = Store::open(None).expect("open");
        let id = store.list_workspaces().expect("list").workspaces[0].id;
        write(&store, id, &WorkspaceSnapshot::starter());
        assert!(!store.workspace(id).expect("read").history.can_undo);
        write(&store, id, &without("speaker"));
        write(&store, id, &without("speaker"));
        assert_eq!(
            store.undo_workspace(id).expect("undo").snapshot,
            WorkspaceSnapshot::starter()
        );
    }

    /// The list is bounded, and it is the oldest end that goes: an operator undoing reaches for
    /// the last few gestures, never the first ones of the session.
    #[test]
    fn the_history_forgets_its_oldest_arrangements() {
        let store = Store::open(None).expect("open");
        let id = store.list_workspaces().expect("list").workspaces[0].id;
        let labelled = |at: usize| {
            let mut snapshot = WorkspaceSnapshot::starter();
            snapshot.graph.nodes[1].label = Some(format!("scope {at}"));
            snapshot
        };
        let writes = usize::try_from(WORKSPACE_HISTORY_DEPTH).expect("depth fits") + 20;
        for at in 0..writes {
            write(&store, id, &labelled(at));
        }
        let entries: i64 = store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM workspace_history WHERE workspace_id = ?1",
                params![id],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(entries, WORKSPACE_HISTORY_DEPTH);

        for at in (writes - usize::try_from(WORKSPACE_HISTORY_DEPTH).expect("depth fits")..writes)
            .rev()
            .skip(1)
        {
            assert_eq!(
                store.undo_workspace(id).expect("undo").snapshot,
                labelled(at)
            );
        }
        assert!(matches!(
            store.undo_workspace(id),
            Err(StoreError::WorkspaceHistoryEnd { .. })
        ));
    }

    /// Where a node was tuned outlives the node while a step can bring it back — otherwise the
    /// autosave forgets a deleted channel's frequency seconds before an undo restores it.
    #[test]
    fn the_history_keeps_a_deleted_nodes_settings_reachable() {
        let store = Store::open(None).expect("open");
        let id = store.list_workspaces().expect("list").workspaces[0].id;
        assert!(store.history_nodes(id).expect("nodes").is_empty());

        write(&store, id, &without("speaker"));
        let nodes = store.history_nodes(id).expect("nodes");
        assert!(nodes.contains("speaker"), "the deleted node is recoverable");
        assert!(nodes.contains("scope"));

        store.delete_workspace(id).expect("delete");
        assert!(store.history_nodes(id).expect("nodes").is_empty());
        let rows: i64 = store
            .lock()
            .query_row("SELECT COUNT(*) FROM workspace_history", [], |row| {
                row.get(0)
            })
            .expect("count");
        assert_eq!(rows, 0, "deleting a workspace takes its history with it");
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
            snapshot: Some(WorkspaceSnapshot::starter()),
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
        // A wire into a node that is not there: the graph is refused whole rather than stored
        // with a wire the canvas would drop on read.
        let mut dangling = WorkspaceSnapshot::starter();
        dangling.graph.edges.push(sdrmm_wire::PatchEdge {
            from: sdrmm_wire::PortRef {
                node: "device".to_string(),
                port: "iq".to_string(),
            },
            to: sdrmm_wire::PortRef {
                node: "ghost".to_string(),
                port: "iq".to_string(),
            },
        });
        assert!(matches!(
            store.create_workspace("Broken", &dangling),
            Err(StoreError::WorkspaceLayout(WorkspaceError::Patch(_)))
        ));
        assert!(matches!(
            store.update_workspace(
                id,
                &UpdateWorkspaceRequest {
                    revision: 1,
                    name: None,
                    snapshot: Some(dangling),
                }
            ),
            Err(StoreError::WorkspaceLayout(WorkspaceError::Patch(_)))
        ));
        // A refused write leaves the revision alone, or the next honest write would 409.
        assert_eq!(store.workspace(id).expect("read").info.revision, 1);

        assert!(matches!(
            store.create_workspace("Workspace", &WorkspaceSnapshot::starter()),
            Err(StoreError::WorkspaceNameTaken(_))
        ));
        for blank in [
            "",
            "   ",
            &"x".repeat(sdrmm_wire::workspace::MAX_NAME_LEN + 1),
        ] {
            assert!(matches!(
                store.create_workspace(blank, &WorkspaceSnapshot::starter()),
                Err(StoreError::WorkspaceLayout(WorkspaceError::Name))
            ));
        }
        let other = store
            .create_workspace("Bench", &WorkspaceSnapshot::starter())
            .expect("create");
        assert!(matches!(
            store.update_workspace(
                other,
                &UpdateWorkspaceRequest {
                    revision: 1,
                    name: Some("Workspace".to_string()),
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
