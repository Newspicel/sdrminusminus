use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{LazyLock, Mutex, MutexGuard},
};

use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value};
use sdrmm_wire::{
    Bookmark, CreateBookmarkRequest, DecodedRecord, DecoderLogEntry, DecoderLogQuery, LogScope,
    PresetInfo, PresetSnapshot, RecordingInfo, UpdateWorkspaceRequest, WorkspaceDetail,
    WorkspaceError, WorkspaceExport, WorkspaceHistory, WorkspaceInfo, WorkspaceSnapshot,
    WorkspaceState, WorkspacesResponse,
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
    #[error("radio operator {0} not found")]
    CpsUserNotFound(i64),
    #[error("radio {0} not found")]
    CpsDeviceNotFound(i64),
    #[error("codeplug {0} not found")]
    CpsCodeplugNotFound(i64),
    #[error("that {0} is not usable")]
    CpsField(&'static str),
    #[error("that name is already taken")]
    CpsNameTaken,
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

pub const DECODER_LOG_LIMIT_DEFAULT: u32 = 200;
pub const DECODER_LOG_LIMIT_MAX: u32 = 2_000;

pub const DECODER_LOG_EXPORT_MAX: u32 = 100_000;

const IMPORT_COPY_LIMIT: u32 = 999;

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
    "
    -- Undo reaches the dial, not only the drawing. `state` is the workspace's settings as they
    -- stood at that entry, null on an entry that left them alone — which is every entry written
    -- before this column, and every arrangement gesture after it. An entry with no state of its
    -- own reads the nearest one behind it, so a layout step between two dial moves does not
    -- pretend the dial went back. `node` names whose dial moved, so a drag that lands a hundred
    -- patches coalesces into the one step the operator made.
    ALTER TABLE workspace_history ADD COLUMN state TEXT;
    ALTER TABLE workspace_history ADD COLUMN node TEXT;
    ",
    "
    -- What an operator wrote on a recording. Mirrored here from the SigMF metadata, which is
    -- where it is stored: the pair on disk is the truth every reconcile re-reads, so an
    -- annotation travels with a downloaded archive and survives this database being thrown away.
    ALTER TABLE recordings ADD COLUMN tags TEXT NOT NULL DEFAULT '[]';
    ALTER TABLE recordings ADD COLUMN note TEXT;
    ",
    "
    CREATE TABLE arrays (
        key TEXT PRIMARY KEY,
        definition TEXT NOT NULL
    );
    ",
    "
    -- An array is drawn on the canvas now, so the bank of radios lives in the patch that uses it
    -- rather than in a table nothing else refers to.
    DROP TABLE arrays;
    ",
    "
    CREATE TABLE cps_users (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE,
        callsign TEXT,
        dmr_id INTEGER,
        note TEXT,
        created_at TEXT NOT NULL
    );
    CREATE TABLE cps_devices (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE,
        model_id TEXT NOT NULL,
        port TEXT,
        serial_number TEXT,
        firmware TEXT,
        owner_id INTEGER REFERENCES cps_users(id) ON DELETE SET NULL,
        note TEXT,
        created_at TEXT NOT NULL
    );
    CREATE TABLE cps_codeplugs (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        model_id TEXT NOT NULL,
        device_id INTEGER REFERENCES cps_devices(id) ON DELETE SET NULL,
        user_id INTEGER REFERENCES cps_users(id) ON DELETE SET NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        codeplug TEXT NOT NULL,
        -- The raw bytes the radio handed over. A write patches this image rather than building
        -- one from scratch, so settings the generic model does not cover survive the round trip.
        image BLOB
    );
    CREATE INDEX cps_codeplugs_model ON cps_codeplugs (model_id, updated_at DESC);
    ",
    "
    -- The name an operator gave a recording, kept beside its tags and note and mirrored from the
    -- same SigMF metadata. Null is a recording nobody has named, which is every one taken before
    -- this column.
    ALTER TABLE recordings ADD COLUMN name TEXT;
    ",
];

pub const WORKSPACE_HISTORY_DEPTH: i64 = 100;

/// How long one settings entry keeps absorbing further moves of the same dial. A drag sends a
/// patch every frame; each one is not a step the operator would want to walk back.
const HISTORY_COALESCE: jiff::SignedDuration = jiff::SignedDuration::from_secs(1);

/// A settings change as the history records it: whose dial moved, and where the workspace stood
/// on either side of the move.
pub struct SettingsStep<'a> {
    pub node: &'a str,
    pub before: &'a WorkspaceState,
    pub after: &'a WorkspaceState,
}

/// A workspace after an undo or a redo, with the settings the step reached — `None` when the step
/// only rearranged the canvas and the radios should be left where they are.
pub struct SteppedWorkspace {
    pub detail: WorkspaceDetail,
    pub settings: Option<WorkspaceState>,
}

struct RecordedSettings<'a> {
    node: &'a str,
    before: &'a str,
    after: &'a str,
}

pub struct RecordingRow {
    pub stem: String,
    pub name: Option<String>,
    pub created_at: String,
    pub device_label: String,
    pub center_hz: f64,
    pub sample_rate: f64,
    pub samples: u64,
    pub bytes: u64,
    pub tags: Vec<String>,
    pub note: Option<String>,
}

pub struct LogOrigin<'a> {
    pub workspace: Option<i64>,
    pub nodes: &'a HashMap<(u32, u32), String>,
}

impl LogOrigin<'static> {
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
            "INSERT INTO recordings (stem, name, created_at, device_label, center_hz, \
             sample_rate, samples, bytes, tags, note) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
             ON CONFLICT(stem) DO UPDATE SET name = excluded.name, \
             created_at = excluded.created_at, \
             device_label = excluded.device_label, center_hz = excluded.center_hz, \
             sample_rate = excluded.sample_rate, samples = excluded.samples, \
             bytes = excluded.bytes, tags = excluded.tags, note = excluded.note",
            params![
                row.stem,
                row.name,
                row.created_at,
                row.device_label,
                row.center_hz,
                row.sample_rate,
                row.samples as i64,
                row.bytes as i64,
                serde_json::to_string(&row.tags)?,
                row.note
            ],
        )?;
        Ok(())
    }

    pub fn list_recordings(&self, dir: &Path) -> Result<Vec<RecordingInfo>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, stem, created_at, device_label, center_hz, sample_rate, samples, bytes, \
             tags, note, name FROM recordings ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            let stem: String = row.get(1)?;
            let sample_rate: f64 = row.get(5)?;
            let samples = row.get::<_, i64>(6)? as u64;
            Ok(RecordingInfo {
                id: row.get(0)?,
                device_id: format!("virtual:file:{}", dir.join(&stem).display()),
                file: stem,
                name: row.get(10)?,
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
                tags: serde_json::from_str(&row.get::<_, String>(8)?).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        8,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })?,
                note: row.get(9)?,
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

    fn decoder_log_predicate(
        &self,
        filter: &DecoderLogQuery,
    ) -> Result<DecoderLogPredicate, StoreError> {
        let workspace = self.active_workspace_id()?;
        DecoderLogPredicate::build(filter, workspace, &self.run_start)
    }

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

    pub fn export_decoder_log(
        &self,
        filter: &DecoderLogQuery,
    ) -> Result<Vec<DecoderLogEntry>, StoreError> {
        let predicate = self.decoder_log_predicate(filter)?;
        let conn = self.lock();
        select_decoder_log(&conn, &predicate, DECODER_LOG_EXPORT_MAX)
    }

    pub fn delete_decoder_log(&self, filter: &DecoderLogQuery) -> Result<u64, StoreError> {
        let predicate = self.decoder_log_predicate(filter)?;
        let deleted = self.lock().execute(
            &format!("DELETE FROM decoder_log{}", predicate.clause),
            params_from_iter(&predicate.params),
        )?;
        Ok(deleted as u64)
    }

    pub fn prune_decoder_log_before(&self, cutoff: &str) -> Result<u64, StoreError> {
        let dropped = self.lock().execute(
            "DELETE FROM decoder_log WHERE at < ?1",
            params![normalize_timestamp(cutoff)?],
        )?;
        Ok(dropped as u64)
    }

    pub fn prune_decoder_log(&self, max_rows: u64) -> Result<u64, StoreError> {
        let dropped = self.lock().execute(
            "DELETE FROM decoder_log WHERE id <= \
             (SELECT id FROM decoder_log ORDER BY id DESC LIMIT 1 OFFSET ?1)",
            params![i64::try_from(max_rows).unwrap_or(i64::MAX)],
        )?;
        Ok(dropped as u64)
    }

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

    pub fn export_workspace(&self, id: i64) -> Result<WorkspaceExport, StoreError> {
        let conn = self.lock();
        let detail = read_workspace(&conn, id)?;
        let state = read_workspace_state(&conn, id)?;
        let mut export = WorkspaceExport::new(detail.info.name, detail.snapshot, state);
        export.forget_absent_nodes();
        Ok(export)
    }

    pub fn import_workspace(&self, export: &WorkspaceExport) -> Result<i64, StoreError> {
        export.validate()?;
        let mut document = export.clone();
        document.forget_absent_nodes();
        let json = serde_json::to_string(&document.snapshot)?;
        let now = now_rfc3339();
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let name = free_workspace_name(&tx, document.name.trim())?;
        tx.execute(
            "INSERT INTO workspaces (name, created_at, updated_at, revision, nodes, snapshot) \
             VALUES (?1, ?2, ?2, 1, ?3, ?4)",
            params![name, now, document.snapshot.graph.nodes.len() as i64, json],
        )
        .map_err(|err| name_taken(err, &name))?;
        let id = tx.last_insert_rowid();
        write_workspace_state(&tx, id, &document.state)?;
        tx.commit()?;
        Ok(id)
    }

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
            record_history(&tx, id, &json, None)?;
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

    pub fn undo_workspace(&self, id: i64) -> Result<SteppedWorkspace, StoreError> {
        self.step_history(id, Step::Undo)
    }

    pub fn redo_workspace(&self, id: i64) -> Result<SteppedWorkspace, StoreError> {
        self.step_history(id, Step::Redo)
    }

    /// Writes down a settings change so undo can reach it, next to the arrangement it belongs to.
    ///
    /// Answers whether the step is a new one: a burst of patches from one drag coalesces into the
    /// entry it started, and a patch that changed nothing records nothing.
    pub fn record_settings(&self, id: i64, step: &SettingsStep<'_>) -> Result<bool, StoreError> {
        let before = serde_json::to_string(step.before)?;
        let after = serde_json::to_string(step.after)?;
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let snapshot: String = tx
            .query_row(
                "SELECT snapshot FROM workspaces WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(StoreError::WorkspaceNotFound(id))?;
        let recorded = record_history(
            &tx,
            id,
            &snapshot,
            Some(RecordedSettings {
                node: step.node,
                before: &before,
                after: &after,
            }),
        )?;
        tx.commit()?;
        Ok(recorded)
    }

    fn step_history(&self, id: i64, step: Step) -> Result<SteppedWorkspace, StoreError> {
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
        let snapshot = parse_workspace_snapshot(&json)?;
        let leaving = state_at(&tx, id, at)?;
        let reaching = state_at(&tx, id, seq)?;
        let settings = match reaching {
            Some(reaching) if Some(&reaching) != leaving.as_ref() => {
                let state = serde_json::from_str::<WorkspaceState>(&reaching)?.current();
                write_workspace_state(&tx, id, &state)?;
                Some(state)
            }
            _ => None,
        };
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
        Ok(SteppedWorkspace { detail, settings })
    }

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
            if let Ok(ids) = serde_json::from_str::<Ids>(&json?) {
                nodes.extend(ids.graph.nodes.into_iter().map(|node| node.id));
            }
        }
        Ok(nodes)
    }

    pub fn delete_workspace(&self, id: i64) -> Result<Option<i64>, StoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let deleted = tx.execute("DELETE FROM workspaces WHERE id = ?1", params![id])?;
        if deleted == 0 {
            return Err(StoreError::WorkspaceNotFound(id));
        }
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

    pub fn workspace_state(&self, workspace_id: i64) -> Result<WorkspaceState, StoreError> {
        let conn = self.lock();
        read_workspace_state(&conn, workspace_id)
    }

    pub fn put_workspace_state(
        &self,
        workspace_id: i64,
        state: &WorkspaceState,
    ) -> Result<(), StoreError> {
        let conn = self.lock();
        write_workspace_state(&conn, workspace_id, state)
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

    pub fn active_workspace_id(&self) -> Result<Option<i64>, StoreError> {
        let conn = self.lock();
        active_workspace(&conn)
    }

    pub fn active_workspace(&self) -> Result<Option<WorkspaceDetail>, StoreError> {
        let conn = self.lock();
        match active_workspace(&conn)? {
            Some(id) => Ok(Some(read_workspace(&conn, id)?)),
            None => Ok(None),
        }
    }

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

fn validate_name(name: &str) -> Result<(), StoreError> {
    if name.trim().is_empty() || name.chars().count() > sdrmm_wire::workspace::MAX_NAME_LEN {
        return Err(StoreError::WorkspaceLayout(WorkspaceError::Name));
    }
    Ok(())
}

fn free_workspace_name(conn: &Connection, wanted: &str) -> Result<String, StoreError> {
    validate_name(wanted)?;
    let taken = |name: &str| -> Result<bool, StoreError> {
        Ok(conn
            .query_row(
                "SELECT 1 FROM workspaces WHERE name = ?1",
                params![name],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some())
    };
    if !taken(wanted)? {
        return Ok(wanted.to_owned());
    }
    for copy in 2..=IMPORT_COPY_LIMIT {
        let suffix = format!(" ({copy})");
        let room = sdrmm_wire::MAX_NAME_LEN.saturating_sub(suffix.chars().count());
        let stem: String = wanted.chars().take(room).collect();
        let candidate = format!("{}{suffix}", stem.trim_end());
        if !taken(&candidate)? {
            return Ok(candidate);
        }
    }
    Err(StoreError::WorkspaceNameTaken(wanted.to_owned()))
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

#[derive(Clone, Copy)]
enum Step {
    Undo,
    Redo,
}

impl Step {
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

fn read_workspace_state(
    conn: &Connection,
    workspace_id: i64,
) -> Result<WorkspaceState, StoreError> {
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

fn write_workspace_state(
    conn: &Connection,
    workspace_id: i64,
    state: &WorkspaceState,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO workspace_state (workspace_id, updated_at, state) VALUES (?1, ?2, ?3) \
         ON CONFLICT(workspace_id) DO UPDATE SET updated_at = ?2, state = ?3",
        params![workspace_id, now_rfc3339(), serde_json::to_string(state)?],
    )?;
    Ok(())
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

fn record_history(
    conn: &Connection,
    id: i64,
    json: &str,
    settings: Option<RecordedSettings<'_>>,
) -> Result<bool, StoreError> {
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
    if let Some(settings) = &settings {
        conn.execute(
            "UPDATE workspace_history SET state = ?3 \
             WHERE workspace_id = ?1 AND seq = ?2 AND state IS NULL",
            params![id, at, settings.before],
        )?;
    }
    let head = read_entry(conn, id, at)?;
    if head.as_ref().is_some_and(|head| head.snapshot == json)
        && match &settings {
            None => true,
            Some(settings) => state_at(conn, id, at)?.as_deref() == Some(settings.after),
        }
    {
        return Ok(false);
    }
    if let (Some(head), Some(settings)) = (&head, &settings)
        && head.node.as_deref() == Some(settings.node)
        && head.snapshot == json
        && within_coalesce(&head.created_at, &now)
    {
        conn.execute(
            "UPDATE workspace_history SET state = ?3, created_at = ?4 \
             WHERE workspace_id = ?1 AND seq = ?2",
            params![id, at, settings.after, now],
        )?;
        return Ok(false);
    }
    conn.execute(
        "DELETE FROM workspace_history WHERE workspace_id = ?1 AND seq > ?2",
        params![id, at],
    )?;
    let seq = at + 1;
    conn.execute(
        "INSERT INTO workspace_history (workspace_id, seq, created_at, snapshot, state, node) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            id,
            seq,
            now,
            json,
            settings.as_ref().map(|settings| settings.after),
            settings.as_ref().map(|settings| settings.node)
        ],
    )?;
    conn.execute(
        "DELETE FROM workspace_history WHERE workspace_id = ?1 AND seq <= ?2",
        params![id, seq - WORKSPACE_HISTORY_DEPTH],
    )?;
    conn.execute(
        "UPDATE workspaces SET history_at = ?2 WHERE id = ?1",
        params![id, seq],
    )?;
    Ok(true)
}

struct HistoryEntry {
    snapshot: String,
    node: Option<String>,
    created_at: String,
}

fn read_entry(conn: &Connection, id: i64, seq: i64) -> Result<Option<HistoryEntry>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT snapshot, node, created_at FROM workspace_history \
             WHERE workspace_id = ?1 AND seq = ?2",
            params![id, seq],
            |row| {
                Ok(HistoryEntry {
                    snapshot: row.get(0)?,
                    node: row.get(1)?,
                    created_at: row.get(2)?,
                })
            },
        )
        .optional()?)
}

fn within_coalesce(entry: &str, now: &str) -> bool {
    let (Ok(entry), Ok(now)) = (
        entry.parse::<jiff::Timestamp>(),
        now.parse::<jiff::Timestamp>(),
    ) else {
        return false;
    };
    let gap = now.duration_since(entry);
    gap >= jiff::SignedDuration::ZERO && gap <= HISTORY_COALESCE
}

/// The settings an entry stands for. Entries that changed none of their own read the nearest one
/// behind them; an entry older than anything the history knows about the settings reads the
/// oldest, which is where the radios stood before the first move it recorded.
fn state_at(conn: &Connection, id: i64, seq: i64) -> Result<Option<String>, StoreError> {
    let behind: Option<String> = conn
        .query_row(
            "SELECT state FROM workspace_history \
             WHERE workspace_id = ?1 AND seq <= ?2 AND state IS NOT NULL \
             ORDER BY seq DESC LIMIT 1",
            params![id, seq],
            |row| row.get(0),
        )
        .optional()?;
    if behind.is_some() {
        return Ok(behind);
    }
    Ok(conn
        .query_row(
            "SELECT state FROM workspace_history \
             WHERE workspace_id = ?1 AND seq > ?2 AND state IS NOT NULL \
             ORDER BY seq ASC LIMIT 1",
            params![id, seq],
            |row| row.get(0),
        )
        .optional()?)
}

fn parse_workspace_snapshot(json: &str) -> Result<WorkspaceSnapshot, serde_json::Error> {
    let mut value: serde_json::Value = serde_json::from_str(json)?;
    migrate_call_buffers(&mut value);
    migrate_trunk_carriers(&mut value);
    migrate_event_outputs(&mut value);
    serde_json::from_value(value)
}

/// Discord and Matrix used to be the only sinks a decoder could feed. Discord is one shape of
/// webhook now, so the node it lived on carries any HTTPS endpoint or an MQTT broker instead.
fn migrate_event_outputs(snapshot: &mut serde_json::Value) {
    let Some(nodes) = snapshot
        .get_mut("graph")
        .and_then(|graph| graph.get_mut("nodes"))
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    for node in nodes {
        if node.get("kind").and_then(serde_json::Value::as_str) != Some("chat_output") {
            continue;
        }
        node["kind"] = serde_json::Value::String("event_output".to_owned());
        let Some(target) = node.get_mut("data").and_then(|data| data.get_mut("target")) else {
            continue;
        };
        if target.get("service").and_then(serde_json::Value::as_str) != Some("discord") {
            continue;
        }
        let url = target
            .get("webhook_url")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::String(String::new()));
        *target = serde_json::json!({ "service": "webhook", "url": url, "format": "discord" });
    }
}

/// A trunk system used to be fed by DMR decoders wired into it. It runs its own decoders now, so
/// the wires that fed it name a port that no longer exists and would refuse the whole patch.
fn migrate_trunk_carriers(snapshot: &mut serde_json::Value) {
    let Some(graph) = snapshot.get_mut("graph") else {
        return;
    };
    let systems: HashSet<String> = graph
        .get("nodes")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|node| node.get("kind").and_then(serde_json::Value::as_str) == Some("dmr_trunk"))
        .filter_map(|node| node.get("id")?.as_str().map(str::to_owned))
        .collect();
    if systems.is_empty() {
        return;
    }
    let Some(edges) = graph
        .get_mut("edges")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    edges.retain(|edge| {
        let Some(end) = edge.get("to") else {
            return true;
        };
        let lands = end
            .get("node")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|id| systems.contains(id));
        let port = end.get("port").and_then(serde_json::Value::as_str);
        !(lands && matches!(port, Some("events" | "carriers")))
    });
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
            let kept = settings
                .get("retention_seconds")
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|seconds| seconds > 0);
            data.insert("record_calls".to_owned(), serde_json::Value::Bool(kept));
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

fn name_taken(err: rusqlite::Error, name: &str) -> StoreError {
    match err.sqlite_error_code() {
        Some(rusqlite::ErrorCode::ConstraintViolation) => {
            StoreError::WorkspaceNameTaken(name.to_string())
        }
        _ => StoreError::Db(err),
    }
}

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

struct DecoderLogPredicate {
    clause: String,
    params: Vec<Value>,
}

impl DecoderLogPredicate {
    fn build(
        filter: &DecoderLogQuery,
        workspace: Option<i64>,
        run_start: &str,
    ) -> Result<Self, StoreError> {
        let sources_term: String;
        let mut terms: Vec<&str> = Vec::new();
        let mut params = Vec::new();
        let kinds_term: String;
        if let Some(kind) = &filter.kind {
            terms.push("kind = ?");
            params.push(Value::Text(kind.clone()));
        }
        let kinds = filter
            .kind_list()
            .map_err(|bad| StoreError::Sources(bad.to_owned()))?;
        if !kinds.is_empty() {
            let holes = vec!["?"; kinds.len()].join(", ");
            kinds_term = format!("kind IN ({holes})");
            terms.push(&kinds_term);
            for kind in kinds {
                params.push(Value::Text(kind));
            }
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

fn normalize_timestamp(at: &str) -> Result<String, StoreError> {
    let ts: jiff::Timestamp = at
        .parse()
        .map_err(|_| StoreError::Timestamp(at.to_string()))?;
    Ok(rfc3339(ts))
}

pub(crate) fn rfc3339(ts: jiff::Timestamp) -> String {
    format!("{ts:.9}")
}

fn migrate(conn: &Connection) -> Result<(), rusqlite::Error> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let done = usize::try_from(version).unwrap_or(0);
    for (i, migration) in MIGRATIONS.iter().enumerate().skip(done) {
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

#[must_use]
pub fn rfc3339_now() -> String {
    now_rfc3339()
}

mod cps;

#[cfg(test)]
mod tests;
