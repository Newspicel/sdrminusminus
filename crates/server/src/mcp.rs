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
use sdrmm_tools::{ToolError, ToolRegistry};
use sdrmm_wire::{
    AntennaDesign, AntennaRequest, AudioProcessing, ChannelParams, ChannelSettings,
    DecoderLogQuery, DeviceSettings, GroundPlaneParams, InvertedVParams, NanoVnaCalStep,
    NanoVnaCalibrateRequest, NanoVnaPortRequest, NanoVnaRequest, NanoVnaSweepRequest,
    NanoVnaSweepState, ScanRange, ScanSettings, ToolRequest, ToolResponse, ToolsResponse,
    YagiParams,
};
use serde::Deserialize;

use crate::{AppState, store::Store};

const SPECTRUM_BINS: usize = 128;
const SPECTRUM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const MAX_MCP_SWEEP_POINTS: u32 = 401;
const ANTENNA_DESIGNS: &str = "dipole, inverted_v, ground_plane, five_eighths_vertical, \
                               folded_dipole, j_pole, yagi, quad_loop, end_fed_half_wave";
const CALIBRATION_STEPS: &str = "status, reset, open, short, load, thru, isolation, finish, \
                                 enable, disable, save, recall";

pub(crate) fn router(
    engine: Arc<Engine>,
    store: Arc<Store>,
    tools: Arc<ToolRegistry>,
    recordings_gate: Arc<std::sync::Mutex<()>>,
) -> Router<AppState> {
    let service = StreamableHttpService::new(
        move || {
            Ok(SdrMcp::new(
                engine.clone(),
                store.clone(),
                tools.clone(),
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
    tools: Arc<ToolRegistry>,
    recordings_gate: Arc<std::sync::Mutex<()>>,
    tool_router: ToolRouter<Self>,
}

impl SdrMcp {
    fn new(
        engine: Arc<Engine>,
        store: Arc<Store>,
        tools: Arc<ToolRegistry>,
        recordings_gate: Arc<std::sync::Mutex<()>>,
    ) -> Self {
        static ROUTER: LazyLock<ToolRouter<SdrMcp>> = LazyLock::new(SdrMcp::tool_router);
        Self {
            engine,
            store,
            tools,
            recordings_gate,
            tool_router: ROUTER.clone(),
        }
    }

    async fn run_tool(&self, request: ToolRequest) -> Result<ToolResponse, ErrorData> {
        let tools = self.tools.clone();
        tokio::task::spawn_blocking(move || tools.run(request))
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
            .map_err(tool_error)
    }

    async fn nanovna(&self, request: NanoVnaRequest) -> Result<CallToolResult, ErrorData> {
        let response = self.run_tool(ToolRequest::NanoVna(request)).await?;
        let ToolResponse::NanoVna(result) = response else {
            return Err(ErrorData::internal_error(
                "the NanoVNA tool answered under another tool's tag".to_string(),
                None,
            ));
        };
        structured(&result)
    }
}

fn structured<T: serde::Serialize>(value: &T) -> Result<CallToolResult, ErrorData> {
    let json = serde_json::to_value(value)
        .map_err(|e| ErrorData::internal_error(format!("serializing result: {e}"), None))?;
    Ok(CallToolResult::structured(json))
}

fn engine_error(err: sdrmm_engine::EngineError) -> ErrorData {
    if err.is_not_found() || err.is_bad_request() || err.is_conflict() {
        ErrorData::invalid_params(err.to_string(), None)
    } else {
        ErrorData::internal_error(err.to_string(), None)
    }
}

fn tool_error(err: ToolError) -> ErrorData {
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

#[derive(Debug, Deserialize, JsonSchema)]
struct AntennaDesignRequest {
    frequency_hz: f64,
    design: String,
    velocity_factor: Option<f64>,
    feedline_velocity_factor: Option<f64>,
    apex_angle_deg: Option<f64>,
    radials: Option<u8>,
    radial_slope_deg: Option<f64>,
    directors: Option<u8>,
    spacing_wavelengths: Option<f64>,
}

impl AntennaDesignRequest {
    fn design(&self) -> Result<AntennaDesign, ErrorData> {
        let inverted_v = InvertedVParams::default();
        let ground_plane = GroundPlaneParams::default();
        let yagi = YagiParams::default();
        Ok(match self.design.as_str() {
            "dipole" => AntennaDesign::Dipole,
            "inverted_v" => AntennaDesign::InvertedV(InvertedVParams {
                apex_angle_deg: self.apex_angle_deg.unwrap_or(inverted_v.apex_angle_deg),
            }),
            "ground_plane" => AntennaDesign::GroundPlane(GroundPlaneParams {
                radials: self.radials.unwrap_or(ground_plane.radials),
                radial_slope_deg: self
                    .radial_slope_deg
                    .unwrap_or(ground_plane.radial_slope_deg),
            }),
            "five_eighths_vertical" => AntennaDesign::FiveEighthsVertical,
            "folded_dipole" => AntennaDesign::FoldedDipole,
            "j_pole" => AntennaDesign::JPole,
            "yagi" => AntennaDesign::Yagi(YagiParams {
                directors: self.directors.unwrap_or(yagi.directors),
                spacing_wavelengths: self.spacing_wavelengths.unwrap_or(yagi.spacing_wavelengths),
            }),
            "quad_loop" => AntennaDesign::QuadLoop,
            "end_fed_half_wave" => AntennaDesign::EndFedHalfWave,
            other => {
                return Err(ErrorData::invalid_params(
                    format!("no antenna design {other}; this build offers {ANTENNA_DESIGNS}"),
                    None,
                ));
            }
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct NanoVnaPortParams {
    port: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct NanoVnaSweepParams {
    port: String,
    start_hz: u64,
    stop_hz: u64,
    points: u32,
    averages: Option<u16>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct NanoVnaCalibrateParams {
    port: String,
    step: String,
    slot: Option<u8>,
    start_hz: Option<u64>,
    stop_hz: Option<u64>,
    points: Option<u32>,
}

impl NanoVnaCalibrateParams {
    fn step(&self) -> Result<NanoVnaCalStep, ErrorData> {
        let slot = || {
            self.slot.ok_or_else(|| {
                ErrorData::invalid_params(
                    format!("the {} step needs the calibration slot", self.step),
                    None,
                )
            })
        };
        Ok(match self.step.as_str() {
            "status" => NanoVnaCalStep::Status,
            "reset" => NanoVnaCalStep::Reset,
            "open" => NanoVnaCalStep::Open,
            "short" => NanoVnaCalStep::Short,
            "load" => NanoVnaCalStep::Load,
            "thru" => NanoVnaCalStep::Thru,
            "isolation" => NanoVnaCalStep::Isolation,
            "finish" => NanoVnaCalStep::Finish,
            "enable" => NanoVnaCalStep::Enable,
            "disable" => NanoVnaCalStep::Disable,
            "save" => NanoVnaCalStep::Save { slot: slot()? },
            "recall" => NanoVnaCalStep::Recall { slot: slot()? },
            other => {
                return Err(ErrorData::invalid_params(
                    format!("no calibration step {other}; the NanoVNA takes {CALIBRATION_STEPS}"),
                    None,
                ));
            }
        })
    }

    fn range(&self) -> Result<Option<NanoVnaSweepState>, ErrorData> {
        match (self.start_hz, self.stop_hz, self.points) {
            (None, None, None) => Ok(None),
            (Some(start_hz), Some(stop_hz), Some(points)) => Ok(Some(NanoVnaSweepState {
                start_hz,
                stop_hz,
                points,
            })),
            _ => Err(ErrorData::invalid_params(
                "a calibration range needs start_hz, stop_hz and points together".to_string(),
                None,
            )),
        }
    }
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

    #[tool(
        description = "The bench tools this build carries beside the receiver: calculators and \
                       instruments that own no device set and no channel. A tool whose hardware \
                       support is not compiled in is simply not listed, so check here before \
                       reaching for one.",
        annotations(title = "List tools", read_only_hint = true)
    )]
    async fn list_tools(&self) -> Result<CallToolResult, ErrorData> {
        structured(&ToolsResponse {
            tools: self.tools.descriptors(),
        })
    }

    #[tool(
        description = "Cut an antenna for one frequency: element lengths, boom positions, feed \
                       geometry and a feedpoint estimate. Designs are dipole, inverted_v, \
                       ground_plane, five_eighths_vertical, folded_dipole, j_pole, yagi, \
                       quad_loop and end_fed_half_wave. apex_angle_deg belongs to inverted_v, \
                       radials and radial_slope_deg to ground_plane, directors and \
                       spacing_wavelengths to yagi; each falls back to its default. Lengths come \
                       back in metres.",
        annotations(title = "Design antenna", read_only_hint = true)
    )]
    async fn design_antenna(
        &self,
        Parameters(req): Parameters<AntennaDesignRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let defaults = AntennaRequest::default();
        let request = AntennaRequest {
            frequency_hz: req.frequency_hz,
            velocity_factor: req.velocity_factor.unwrap_or(defaults.velocity_factor),
            feedline_velocity_factor: req
                .feedline_velocity_factor
                .unwrap_or(defaults.feedline_velocity_factor),
            design: req.design()?,
        };
        let response = self.run_tool(ToolRequest::Antenna(request)).await?;
        let ToolResponse::Antenna(report) = response else {
            return Err(ErrorData::internal_error(
                "the antenna tool answered under another tool's tag".to_string(),
                None,
            ));
        };
        structured(&report)
    }

    #[tool(
        description = "Serial ports carrying a NanoVNA, confirmed by USB identity or only \
                       probable from the port's name. The port string is what every other \
                       NanoVNA tool takes.",
        annotations(title = "List NanoVNAs", read_only_hint = true)
    )]
    async fn nanovna_list_devices(&self) -> Result<CallToolResult, ErrorData> {
        self.nanovna(NanoVnaRequest::ListDevices).await
    }

    #[tool(
        description = "Interrogate one NanoVNA: firmware and board, battery, its current sweep \
                       range, which calibration standards are stored and whether calibration is \
                       applied.",
        annotations(title = "Describe NanoVNA", read_only_hint = true)
    )]
    async fn nanovna_describe(
        &self,
        Parameters(req): Parameters<NanoVnaPortParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.nanovna(NanoVnaRequest::Describe(NanoVnaPortRequest {
            port: req.port,
        }))
        .await
    }

    #[tool(
        description = "Sweep a NanoVNA and return raw S11 and S21 per frequency, with the \
                       device's own report of how it was configured. Calibrate first — an \
                       uncalibrated sweep measures the fixture as much as the antenna. averages \
                       defaults to 1. Ask for at most 401 points here; use POST /api/tools/run \
                       for a full-resolution sweep.",
        annotations(title = "Sweep NanoVNA")
    )]
    async fn nanovna_sweep(
        &self,
        Parameters(req): Parameters<NanoVnaSweepParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if req.points > MAX_MCP_SWEEP_POINTS {
            return Err(ErrorData::invalid_params(
                format!("a sweep over MCP carries at most {MAX_MCP_SWEEP_POINTS} points"),
                None,
            ));
        }
        self.nanovna(NanoVnaRequest::Sweep(NanoVnaSweepRequest {
            port: req.port,
            start_hz: req.start_hz,
            stop_hz: req.stop_hz,
            points: req.points,
            averages: req.averages.unwrap_or(1),
        }))
        .await
    }

    #[tool(
        description = "Walk a NanoVNA through its SOLT calibration and report what the device \
                       holds afterwards. Steps are status, reset, open, short, load, thru, \
                       isolation, finish, enable, disable, save and recall; save and recall need \
                       a slot. Give start_hz, stop_hz and points together to set the range the \
                       calibration is taken over. Each measuring step needs its standard \
                       physically attached first, and reset discards the stored calibration.",
        annotations(title = "Calibrate NanoVNA", destructive_hint = true)
    )]
    async fn nanovna_calibrate(
        &self,
        Parameters(req): Parameters<NanoVnaCalibrateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let step = req.step()?;
        let range = req.range()?;
        self.nanovna(NanoVnaRequest::Calibrate(NanoVnaCalibrateRequest {
            port: req.port,
            range,
            step,
        }))
        .await
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
                 device set's centre frequency, so retuning the device moves them with it. \
                 Beside the receiver stands a bench of tools — an antenna calculator, a \
                 NanoVNA — that own no device set; list_tools says which of them this build \
                 carries.",
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
                "design_antenna",
                "get_state",
                "list_channel_types",
                "list_devices",
                "list_tools",
                "nanovna_calibrate",
                "nanovna_describe",
                "nanovna_list_devices",
                "nanovna_sweep",
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
