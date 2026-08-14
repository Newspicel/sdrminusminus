//! MCP server (PLAN §5, §18, M5): the same engine, exposed as tools an LLM agent can drive,
//! mounted at `/mcp` on this axum app with the same optional token auth as REST.
//!
//! There is no parallel implementation — every tool calls the same `Engine`/`Store` methods
//! the REST handlers do, and returns the same `wire` types as structured JSON. A tool that
//! needed its own logic would be a sign the service layer is in the wrong place.

use std::sync::{Arc, LazyLock};

use axum::Router;
use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use schemars::JsonSchema;
use sdrmm_engine::Engine;
use sdrmm_wire::{
    ChannelParams, ChannelSettings, DecoderLogQuery, DeviceSettings, ScanRange, ScanSettings,
};
use serde::Deserialize;

use crate::{AppState, store::Store};

/// Spectrum bins returned by `spectrum_snapshot`. Deliberately coarse: an agent wants "what is
/// on this band", not a 4096-point array per call.
const SPECTRUM_BINS: usize = 128;
/// How long `spectrum_snapshot` waits for a frame. The tap runs at ~30 fps, so this only has
/// to survive a retune settling.
const SPECTRUM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Mount the MCP endpoint. Returned with the router's state type still open (`nest_service`
/// binds none), so it merges into the app *before* the auth layer and is gated with everything
/// else.
pub(crate) fn router(
    engine: Arc<Engine>,
    store: Arc<Store>,
    recordings_gate: Arc<std::sync::Mutex<()>>,
) -> Router<AppState> {
    let service = StreamableHttpService::new(
        move || {
            Ok(SdrMcp::new(
                engine.clone(),
                store.clone(),
                recordings_gate.clone(),
            ))
        },
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            // Stateless: one POST, one JSON reply, no session to garbage-collect and nothing
            // to lose across a restart. The cost is no server-initiated notifications, which
            // no tool here needs — progress lives in `GET /api/state`.
            .with_legacy_session_mode(false)
            .with_json_response(true)
            // The DNS-rebinding guard defaults to localhost-only, which would 403 every LAN
            // client — and reaching the server over the LAN is the entire deployment model
            // (PLAN §12: LAN-trusted, optional token, a VPN if you want it reachable from
            // outside). The token layer above is what actually gates this endpoint.
            .disable_allowed_hosts(),
    );
    Router::new().nest_service("/mcp", service)
}

#[derive(Clone)]
struct SdrMcp {
    engine: Arc<Engine>,
    store: Arc<Store>,
    /// Shared with the REST handlers: a reconcile that interleaves into a delete's
    /// unlink→row-delete window turns a successful delete into a 404 (see `AppState`).
    recordings_gate: Arc<std::sync::Mutex<()>>,
    tool_router: ToolRouter<Self>,
}

impl SdrMcp {
    fn new(
        engine: Arc<Engine>,
        store: Arc<Store>,
        recordings_gate: Arc<std::sync::Mutex<()>>,
    ) -> Self {
        // The factory runs per request in stateless mode; rebuilding the whole tool map each
        // time would be pure waste, so it is built once and cloned.
        static ROUTER: LazyLock<ToolRouter<SdrMcp>> = LazyLock::new(SdrMcp::tool_router);
        Self {
            engine,
            store,
            recordings_gate,
            tool_router: ROUTER.clone(),
        }
    }
}

/// Serialize any wire type into a tool result. Structured content, so an agent gets the same
/// JSON shape the REST API returns rather than a prose summary.
fn structured<T: serde::Serialize>(value: &T) -> Result<CallToolResult, ErrorData> {
    let json = serde_json::to_value(value)
        .map_err(|e| ErrorData::internal_error(format!("serializing result: {e}"), None))?;
    Ok(CallToolResult::structured(json))
}

fn engine_error(err: sdrmm_engine::EngineError) -> ErrorData {
    // The engine already distinguishes "your request was wrong" from "something broke"; keep
    // that distinction so an agent knows whether retrying can help.
    if err.is_not_found() || err.is_bad_request() {
        ErrorData::invalid_params(err.to_string(), None)
    } else {
        ErrorData::internal_error(err.to_string(), None)
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DeviceSetRef {
    /// Device set id from `get_state`.
    device_set: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct OpenDeviceRequest {
    /// `driver:key` from `list_devices`, e.g. `rtlsdr:00000001` or `virtual:siggen`.
    device_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TuneRequest {
    device_set: u32,
    /// Centre frequency in Hz.
    center_hz: Option<f64>,
    /// Sample rate in Hz; must be one the device supports.
    sample_rate: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AddChannelRequest {
    device_set: u32,
    /// Which of the device's receive streams the channel taps; omit for 0, the only stream a
    /// single-stream radio has.
    stream: Option<u32>,
    /// Channel type id from `list_channel_types`, e.g. `nfm`, `wfm`, `adsb`, `pocsag`.
    channel_type: String,
    /// Offset from the device centre in Hz (the channel's frequency minus the centre).
    offset_hz: f64,
    /// Squelch threshold in dBFS; omit to leave the gate open.
    squelch_db: Option<f32>,
    /// Mode-specific settings object. Omit for the documented defaults; the accepted keys are
    /// the ones `list_channel_types` describes for this type.
    settings: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ChannelRef {
    device_set: u32,
    /// Channel id from `get_state`.
    channel: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct StartScanRequest {
    device_set: u32,
    /// Ranges to sweep, each `[start_hz, stop_hz, step_hz]`.
    ranges: Vec<[f64; 3]>,
    /// Individual frequencies to include on top of the ranges.
    frequencies: Option<Vec<f64>>,
    /// Level in dBFS at which a frequency counts as active.
    threshold_db: Option<f32>,
    /// Channel to retune onto a hit so its audio follows the scan.
    hold_channel: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RecordRequest {
    device_set: u32,
    /// `true` starts a SigMF recording, `false` stops and finalizes it.
    start: bool,
    /// Which receive stream a start records — one recording per set, on a named stream; omit
    /// for 0. Ignored on stop.
    stream: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SpectrumSnapshotRequest {
    device_set: u32,
    /// Which receive stream's spectrum; omit for 0.
    stream: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DecoderLogRequest {
    /// One decoder kind: `adsb`, `ais`, `aprs`, `pocsag`, `rds`, `rtty` or `morse`.
    kind: Option<String>,
    /// Restrict to one device set.
    device_set: Option<u32>,
    /// RFC3339 lower bound, e.g. `2026-08-09T12:00:00Z`.
    since: Option<String>,
    /// RFC3339 upper bound.
    until: Option<String>,
    /// Case-insensitive substring match against the station and the summary.
    q: Option<String>,
    /// Maximum rows (server-clamped).
    limit: Option<u32>,
}

#[tool_router]
impl SdrMcp {
    #[tool(
        description = "Full server state: every open device set with its settings, channels, \
                       recording and running scan. Start here — the ids other tools take all \
                       come from this.",
        annotations(title = "Get state", read_only_hint = true)
    )]
    async fn get_state(&self) -> Result<CallToolResult, ErrorData> {
        structured(&self.engine.snapshot())
    }

    #[tool(
        description = "Discover attached SDR hardware, recorded files exposed as virtual \
                       playback devices, and synthetic radios available in development builds.",
        annotations(title = "List devices", read_only_hint = true)
    )]
    async fn list_devices(&self) -> Result<CallToolResult, ErrorData> {
        let engine = self.engine.clone();
        let devices = tokio::task::spawn_blocking(move || engine.probe_devices())
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        structured(&devices)
    }

    #[tool(
        description = "The channel types this build offers, with the bandwidth each needs and \
                       whether it produces audio or decoded data.",
        annotations(title = "List channel types", read_only_hint = true)
    )]
    async fn list_channel_types(&self) -> Result<CallToolResult, ErrorData> {
        structured(&self.engine.channel_types())
    }

    #[tool(
        description = "Open a device into a new device set and start streaming.",
        annotations(title = "Open device")
    )]
    async fn open_device(
        &self,
        Parameters(req): Parameters<OpenDeviceRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let engine = self.engine.clone();
        let id = tokio::task::spawn_blocking(move || engine.create_device_set(&req.device_id))
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
            .map_err(engine_error)?;
        structured(&serde_json::json!({ "device_set": id }))
    }

    #[tool(
        description = "Close a device set and release its hardware.",
        annotations(title = "Close device set", destructive_hint = true)
    )]
    async fn close_device_set(
        &self,
        Parameters(req): Parameters<DeviceSetRef>,
    ) -> Result<CallToolResult, ErrorData> {
        let engine = self.engine.clone();
        tokio::task::spawn_blocking(move || engine.remove_device_set(req.device_set))
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
            .map_err(engine_error)?;
        structured(&serde_json::json!({ "closed": req.device_set }))
    }

    #[tool(
        description = "Retune a device set's centre frequency and/or sample rate. Channels are \
                       offset from the centre, so they move with it.",
        annotations(title = "Tune device", idempotent_hint = true)
    )]
    async fn tune_device(
        &self,
        Parameters(req): Parameters<TuneRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let engine = self.engine.clone();
        let delta = DeviceSettings {
            center_hz: req.center_hz,
            sample_rate: req.sample_rate,
            ..DeviceSettings::default()
        };
        tokio::task::spawn_blocking(move || engine.patch_device(req.device_set, delta))
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
            .map_err(engine_error)?;
        structured(&self.engine.snapshot())
    }

    #[tool(
        description = "Add a demodulator or decoder channel to a device set.",
        annotations(title = "Add channel")
    )]
    async fn add_channel(
        &self,
        Parameters(req): Parameters<AddChannelRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        // Built through the wire enum rather than a parallel MCP-only settings model, so the
        // accepted keys are exactly the ones REST accepts (CLAUDE.md non-negotiable #1).
        let params: ChannelParams = serde_json::from_value(serde_json::json!({
            "type": req.channel_type,
            "settings": req.settings.unwrap_or_else(|| serde_json::json!({})),
        }))
        .map_err(|e| {
            ErrorData::invalid_params(
                format!(
                    "unusable settings for channel type {}: {e}",
                    req.channel_type
                ),
                None,
            )
        })?;
        let settings = ChannelSettings {
            offset_hz: req.offset_hz,
            squelch_db: req.squelch_db,
            params,
        };
        let engine = self.engine.clone();
        let stream = req.stream.unwrap_or_default();
        let id = tokio::task::spawn_blocking(move || {
            engine.add_channel(req.device_set, stream, settings)
        })
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
        .map_err(engine_error)?;
        structured(&serde_json::json!({ "channel": id }))
    }

    #[tool(
        description = "Remove a channel from a device set.",
        annotations(title = "Remove channel", destructive_hint = true)
    )]
    async fn remove_channel(
        &self,
        Parameters(req): Parameters<ChannelRef>,
    ) -> Result<CallToolResult, ErrorData> {
        let engine = self.engine.clone();
        tokio::task::spawn_blocking(move || engine.remove_channel(req.device_set, req.channel))
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
            .map_err(engine_error)?;
        structured(&serde_json::json!({ "removed": req.channel }))
    }

    #[tool(
        description = "Sweep a device set across frequency ranges and park on anything above \
                       the threshold. While a scan runs it owns the device's tuning.",
        annotations(title = "Start scan")
    )]
    async fn start_scan(
        &self,
        Parameters(req): Parameters<StartScanRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let settings = ScanSettings {
            ranges: req
                .ranges
                .iter()
                .map(|[start_hz, stop_hz, step_hz]| ScanRange {
                    start_hz: *start_hz,
                    stop_hz: *stop_hz,
                    step_hz: *step_hz,
                })
                .collect(),
            frequencies: req.frequencies.unwrap_or_default(),
            hold_channel: req.hold_channel,
            ..ScanSettings::default()
        };
        let settings = match req.threshold_db {
            Some(threshold_db) => ScanSettings {
                threshold_db,
                ..settings
            },
            None => settings,
        };
        let engine = self.engine.clone();
        let status =
            tokio::task::spawn_blocking(move || engine.start_scan(req.device_set, settings))
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
                .map_err(engine_error)?;
        structured(&status)
    }

    #[tool(
        description = "Stop a running scan, leaving the device where the scan left it.",
        annotations(title = "Stop scan")
    )]
    async fn stop_scan(
        &self,
        Parameters(req): Parameters<DeviceSetRef>,
    ) -> Result<CallToolResult, ErrorData> {
        let engine = self.engine.clone();
        let status = tokio::task::spawn_blocking(move || engine.stop_scan(req.device_set))
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
            .map_err(engine_error)?;
        structured(&status)
    }

    #[tool(
        description = "Start or stop a lossless SigMF IQ recording of a device set.",
        annotations(title = "Record")
    )]
    async fn record(
        &self,
        Parameters(req): Parameters<RecordRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let engine = self.engine.clone();
        let store = self.store.clone();
        let gate = self.recordings_gate.clone();
        let ds = req.device_set;
        let result = tokio::task::spawn_blocking(move || {
            if req.start {
                engine
                    .start_recording(ds, req.stream.unwrap_or_default())
                    .map_err(engine_error)?;
                return Ok(serde_json::json!({ "recording": true }));
            }
            let finalized = engine.stop_recording(ds).map_err(engine_error)?;
            // Exactly what the REST stop path does after the same call: without the
            // reconcile the finalized pair is never indexed, and without the scope no client
            // ever learns the library changed (the DeviceSet scope invalidates state, not
            // recordings).
            if let Some(dir) = engine.recordings_dir() {
                {
                    let _gate = crate::rest::lock_gate(&gate);
                    crate::rest::reconcile_recordings(dir, &store).map_err(|e| {
                        ErrorData::internal_error(format!("indexing the recording: {e:?}"), None)
                    })?;
                }
                engine.emit_scope(sdrmm_wire::StateScope::Recordings);
            }
            Ok(serde_json::json!({
                "recording": false,
                "file": finalized.stem.display().to_string(),
                "samples": finalized.samples,
                "error": finalized.error,
            }))
        })
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))??;
        structured(&result)
    }

    #[tool(
        description = "Query the stored decoder log: aircraft, ships, pager messages, APRS \
                       packets, RDS text and more, newest first.",
        annotations(title = "Query decoder log", read_only_hint = true)
    )]
    async fn query_decoder_log(
        &self,
        Parameters(req): Parameters<DecoderLogRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let filter = DecoderLogQuery {
            kind: req.kind,
            device_set: req.device_set,
            // A wire-scoped filter has no meaning to a caller that is not looking at the canvas.
            nodes: None,
            sources: None,
            since: req.since,
            until: req.until,
            q: req.q,
            limit: req.limit,
        };
        let store = self.store.clone();
        let (entries, total) =
            tokio::task::spawn_blocking(move || store.query_decoder_log(&filter))
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
                .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        structured(&serde_json::json!({ "entries": entries, "total": total }))
    }

    #[tool(
        description = "One spectrum frame from a device set, reduced to 128 power bins in \
                       dBFS — enough to answer 'is anything on this band'.",
        annotations(title = "Spectrum snapshot", read_only_hint = true)
    )]
    async fn spectrum_snapshot(
        &self,
        Parameters(req): Parameters<SpectrumSnapshotRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let engine = self.engine.clone();
        let ds = req.device_set;
        let stream = req.stream.unwrap_or_default();
        let mut rx = tokio::task::spawn_blocking(move || engine.subscribe_spectrum(ds, stream))
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
            .map_err(engine_error)?;
        let snapshot = tokio::time::timeout(SPECTRUM_TIMEOUT, rx.recv())
            .await
            .map_err(|_| {
                ErrorData::internal_error(
                    "the device produced no spectrum within 2 s".to_string(),
                    None,
                )
            })?
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let mut bins = vec![0.0f32; SPECTRUM_BINS];
        sdrmm_dsp::decimate_max(&snapshot.db, &mut bins);
        structured(&serde_json::json!({
            "center_hz": snapshot.center_hz,
            "span_hz": snapshot.span_hz,
            "bins_db": bins,
        }))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SdrMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("sdr--", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Control an sdr-- software-defined-radio server. Call get_state first: device \
                 sets and channels are created explicitly and every other tool works from \
                 those ids. Frequencies are always in Hz. Channels are offset from their \
                 device set's centre frequency, so retuning the device moves them with it.",
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tool list is the MCP contract; a rename is a breaking change for every agent that
    /// learned it, and a duplicate would silently shadow one implementation.
    #[test]
    fn tool_names_are_unique_and_stable() {
        let tools = SdrMcp::tool_router().list_all();
        let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        names.sort_unstable();
        let unique: std::collections::HashSet<&&str> = names.iter().collect();
        assert_eq!(
            unique.len(),
            names.len(),
            "duplicate tool name in {names:?}"
        );
        assert_eq!(
            names,
            [
                "add_channel",
                "close_device_set",
                "get_state",
                "list_channel_types",
                "list_devices",
                "open_device",
                "query_decoder_log",
                "record",
                "remove_channel",
                "spectrum_snapshot",
                "start_scan",
                "stop_scan",
                "tune_device",
            ]
        );
    }

    /// An agent picks a tool from its description; an empty one is unusable, and a missing
    /// input schema means it cannot construct the call at all.
    #[test]
    fn every_tool_is_described_and_has_an_input_schema() {
        for tool in SdrMcp::tool_router().list_all() {
            assert!(
                tool.description.as_ref().is_some_and(|d| d.len() > 20),
                "{} has no usable description",
                tool.name
            );
            assert!(
                tool.input_schema.contains_key("type"),
                "{} has no input schema",
                tool.name
            );
        }
    }
}
