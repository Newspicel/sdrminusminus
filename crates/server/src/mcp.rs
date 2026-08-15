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
    AudioProcessing, ChannelParams, ChannelSettings, DecoderLogQuery, DeviceSettings, ScanRange,
    ScanSettings,
};
use serde::Deserialize;

use crate::{AppState, store::Store};

const SPECTRUM_BINS: usize = 128;
const SPECTRUM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

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
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .disable_allowed_hosts(),
    );
    Router::new().nest_service("/mcp", service)
}

#[derive(Clone)]
struct SdrMcp {
    engine: Arc<Engine>,
    store: Arc<Store>,
    recordings_gate: Arc<std::sync::Mutex<()>>,
    tool_router: ToolRouter<Self>,
}

impl SdrMcp {
    fn new(
        engine: Arc<Engine>,
        store: Arc<Store>,
        recordings_gate: Arc<std::sync::Mutex<()>>,
    ) -> Self {
        static ROUTER: LazyLock<ToolRouter<SdrMcp>> = LazyLock::new(SdrMcp::tool_router);
        Self {
            engine,
            store,
            recordings_gate,
            tool_router: ROUTER.clone(),
        }
    }
}

fn structured<T: serde::Serialize>(value: &T) -> Result<CallToolResult, ErrorData> {
    let json = serde_json::to_value(value)
        .map_err(|e| ErrorData::internal_error(format!("serializing result: {e}"), None))?;
    Ok(CallToolResult::structured(json))
}

fn engine_error(err: sdrmm_engine::EngineError) -> ErrorData {
    if err.is_not_found() || err.is_bad_request() {
        ErrorData::invalid_params(err.to_string(), None)
    } else {
        ErrorData::internal_error(err.to_string(), None)
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DeviceSetRef {
    device_set: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct OpenDeviceRequest {
    device_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TuneRequest {
    device_set: u32,
    center_hz: Option<f64>,
    sample_rate: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AddChannelRequest {
    device_set: u32,
    stream: Option<u32>,
    channel_type: String,
    offset_hz: f64,
    squelch_db: Option<f32>,
    squelch_auto_db: Option<f32>,
    settings: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ChannelRef {
    device_set: u32,
    channel: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct StartScanRequest {
    device_set: u32,
    ranges: Vec<[f64; 3]>,
    frequencies: Option<Vec<f64>>,
    threshold_db: Option<f32>,
    hold_channel: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RecordRequest {
    device_set: u32,
    start: bool,
    stream: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RecordChannelAudioRequest {
    device_set: u32,
    channel: u32,
    start: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SpectrumSnapshotRequest {
    device_set: u32,
    stream: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DecoderLogRequest {
    kind: Option<String>,
    device_set: Option<u32>,
    since: Option<String>,
    until: Option<String>,
    q: Option<String>,
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
            squelch_auto_db: req.squelch_auto_db,
            audio: AudioProcessing::default_for(params.type_id()),
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
        description = "Start or stop recording one channel's audio to a WAV file — what a \
                       listener on that channel would hear, the channel's own processing \
                       included. Independent of the device's IQ recording; a channel that \
                       produces no audio is refused.",
        annotations(title = "Record channel audio")
    )]
    async fn record_channel_audio(
        &self,
        Parameters(req): Parameters<RecordChannelAudioRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let engine = self.engine.clone();
        let status = tokio::task::spawn_blocking(move || {
            if req.start {
                engine.start_channel_recording(req.device_set, req.channel)
            } else {
                engine.stop_channel_recording(req.device_set, req.channel)
            }
        })
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
        .map_err(engine_error)?;
        structured(&status)
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
                "record_channel_audio",
                "remove_channel",
                "spectrum_snapshot",
                "start_scan",
                "stop_scan",
                "tune_device",
            ]
        );
    }

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
