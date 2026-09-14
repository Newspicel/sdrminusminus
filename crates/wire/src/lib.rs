pub mod monitor;
pub use monitor::{EventOrigin, SpectrumMonitorNode, Transmission, TransmissionState};
pub mod about;
pub mod audio;
pub mod bandplan;
pub mod channel;
pub mod coherent;
pub mod cps;
pub mod decode;
pub mod device;
pub mod diagnostics;
pub mod doctor;
pub mod event_output;
pub mod filter;
pub mod frame;
mod pipeline;
pub use pipeline::{PipelineQueue, PipelineStage, QueueHealth};
pub mod hunt;
pub mod network;
pub mod patch;
pub mod position;
pub mod propagation;
pub mod rest;
pub mod satellite;
pub mod scan;
pub mod state;
pub mod timemachine;
pub mod tools;
pub mod units;
pub mod workspace;
pub mod workspace_state;
pub mod ws;

pub use about::{AboutResponse, Attribution, ComponentSource, LicenseTextResponse};
pub use audio::{
    AudioAgcMode, AudioFilterSettings, AudioFxNode, AudioProcessing, AudioRoute,
    ClickRemovalSettings, DenoiseMode, DenoiseSettings, MAX_AUDIO_FX_CHAIN, MAX_AUDIO_NOTCHES,
    MAX_AUDIO_TONE_HZ, MAX_BLANKER_THRESHOLD, MAX_CLICK_THRESHOLD, MAX_NOTCH_WIDTH_HZ,
    MIN_AUDIO_TONE_HZ, MIN_BLANKER_THRESHOLD, MIN_CLICK_THRESHOLD, MIN_NOTCH_WIDTH_HZ,
    NoiseBlankerSettings, NotchSettings,
};
pub use bandplan::{
    BandAllocation, BandBlock, BandLane, BandLayerInfo, BandLayerKind, BandPlan, BandProvision,
    BandRegion, BandRegionMatch, BandRegionsResponse, BandService, ItuRegion, LocateQuery,
};
pub use channel::{
    AcarsParams, AdsbParams, AeroChannel, AisChannel, AisParams, AmParams, AprsMode, AprsParams,
    AtvColor, AtvModulation, AtvParams, AtvStandard, ChannelDescriptor, ChannelInfo, ChannelParams,
    ChannelSettings, CwSkimmerParams, DECT_CARRIER_SPACING_HZ, DEFAULT_FREQUENCY_HZ, DabMode,
    DabParams, DabTransmissionMode, DatvCodeRate, DatvParams, DatvRollOff, DatvStandard,
    DecoderFamily, DectBand, DectParams, DectSides, DmrParams, DmrSlots, DpmrParams, DrmMode,
    DrmParams, DscParams, DstarParams, DvbtBandwidth, DvbtParams, DvbtStandard, ErmesParams,
    FlexParams, FreeDvMode, FreeDvParams, GnssParams, HfdlParams, IdentParams, IlsComponent,
    IlsParams, InmarsatAeroParams, InmarsatStdcParams, IridiumParams, IridiumSpan, M17Params,
    MAX_DATV_SYMBOL_RATE, MAX_IDENT_BANDWIDTH_HZ, MAX_IDENT_INTERVAL_MS, MAX_IDENT_THRESHOLD_DB,
    MAX_NAVAID_REPORT_MS, MAX_SQUELCH_AUTO_MARGIN_DB, MIN_DATV_SYMBOL_RATE, MIN_IDENT_BANDWIDTH_HZ,
    MIN_IDENT_INTERVAL_MS, MIN_IDENT_THRESHOLD_DB, MIN_NAVAID_REPORT_MS,
    MIN_SQUELCH_AUTO_MARGIN_DB, MorseParams, NavtexParams, NfmParams, NfmScramblerMode,
    NfmToneMode, NxdnBandwidth, NxdnParams, P25Params, ParamLimit, PocsagBaud, PocsagParams,
    PskBaud, PskParams, RadioClockParams, RadioClockStandard, RttyParams, RttyStopBits,
    SelcallParams, SelcallSystem, Sideband, Squelch, SsbParams, SstvMode, SstvParams,
    SubghzModulation, SubghzParams, Vdl2Params, VorParams, WfmParams, WsjtParams, WsprParams,
    YsfParams, home_frequency_hz, param_limits,
};
pub use coherent::{
    ArrayElement, ArrayGeometry, CalParams, CalSource, CalState, CfarParams, CoherentParams,
    CombineMode, CombinerParams, DF_SPECTRUM_POINTS, DfAlgorithm, DfBearing, DfEstimate,
    DfFusionState, DfGuidance, DfParams, DfReading, DfStation, EcaParams, GuidanceMode,
    Illuminator, LaneCal, MAX_ARRAY_ELEMENTS, MAX_ARRAY_EXTENT_M, MAX_CPI_MS, MAX_DF_BANDWIDTH_HZ,
    MAX_DF_REPORT_MS, MAX_RANGE_BINS, MAX_STATION_ID_LEN, MIN_ARRAY_ELEMENTS, MIN_CPI_MS,
    MIN_DF_BANDWIDTH_HZ, MIN_DF_REPORT_MS, NavTarget, NavTargetKind, PassiveRadarParams,
    RadarDetection, StitchMode, StitchParams,
};
pub use cps::{
    ALL_CALL_NUMBER, Admit, Bandwidth, CODEPLUG_VERSION, ChannelKind, ChannelMode, Codeplug,
    CodeplugCounts, CodeplugMeta, Contact, ContactKind, ConversionIssue, ConversionReport,
    CpsCodeplugDetail, CpsCodeplugInfo, CpsCodeplugRequest, CpsConvertRequest, CpsConvertResponse,
    CpsDevice, CpsDeviceRequest, CpsIdentifyRequest, CpsJob, CpsJobKind, CpsJobState,
    CpsJobsResponse, CpsLibraryResponse, CpsMergeRequest, CpsPort, CpsPortsResponse,
    CpsReadRequest, CpsUser, CpsUserRequest, CpsWriteRequest, DmrChannel, FmChannel,
    FrequencyRange, GeneralSettings, GroupList, IssueScope, IssueSeverity, MAX_CPS_NAME_LEN,
    MAX_CPS_NOTE_LEN, MergeMode, MergePart, PortMatch, Power, RadioFeatures, RadioId, RadioIdent,
    RadioLimits, RadioModelDescriptor, RadioModelsResponse, ScanList, ScanRevert, ScanTarget,
    TimeSlot, Tone, UsbMatch,
};
pub use decode::{
    AcarsMessage, AdsbMessage, AisMessage, AprsPacket, BroadcastData, BroadcastService,
    BroadcastServiceKind, BroadcastStatus, BroadcastSystem, CwSkimmerSpot, DataLinkMessage,
    DecodedRecord, DecoderEvent, DectArc, DectCapability, DectCipherState, DectFrame, DectIdentity,
    DectSecurity, DectSide, DectUpdate, DvChannelDefinition, DvFrame, DvFrameKind, DvMode,
    DvSlotActivity, DvTrunkProtocol, ErmesMessage, FlexMessage, GnssFrame, IdentFeatures,
    IdentReport, IdentSignal, IlsReading, Modulation, MorseText, NavtexMessage, PagerPayload,
    PocsagMessage, PocsagPayload, ProtocolMatch, PskText, RadioClockFrame, RdsUpdate, RttyText,
    ScramblerStatus, SelcallSequence, SstvPicture, SubghzEncoding, SubghzFrame, SubghzReading,
    ToneSquelchStatus, Vendor, VorReading, WsjtMessage, WsprSpot,
};
pub use device::{
    ARRAY_DRIVER_ID, Agc, AgcGain, AgcSetting, ArgumentInfo, ArgumentOption, ArgumentType,
    ArrayDefinition, BandwidthSetting, Capabilities, ChannelCapabilities, Coherence, DcArtifact,
    DeviceInfo, DeviceProfile, DeviceSettings, Direction, DirectionalCapabilities, Duplex,
    ExtraSetting, ExtraValue, GainKind, GainStage, GainUnit, GainValue, MAX_ARRAY_KEY_LEN,
    MAX_ARRAY_MEMBERS, MAX_RECORDING_STEM_LEN, RECORDING_DRIVER_ID, Range, SIGGEN_DRIVER_ID,
    StreamScope, StreamSettings, Tuning, any_range_holds, recording_stem_valid,
};
pub use diagnostics::{DiagnosticsReport, LogLevel, LogLine, MAX_LOG_LINES, MAX_LOG_MESSAGE_LEN};
pub use doctor::{CheckStatus, DoctorCheck, DoctorReport};
pub use event_output::{
    DEFAULT_POSTGRES_TABLE, EventOutputNode, EventOutputTarget, MAX_INFLUX_NAME_LEN,
    MAX_MATRIX_ROOM_ID_LEN, MAX_MQTT_TOPIC_LEN, MAX_MQTT_USERNAME_LEN, MAX_OUTPUT_SECRET_LEN,
    MAX_OUTPUT_URL_LEN, MAX_SQL_IDENTIFIER_LEN, WebhookFormat, valid_sql_identifier,
};
pub use filter::{
    EventFacet, EventFilterNode, EventKindFacets, FilterMode, MAX_FILTER_DURATION_MS,
    MAX_FILTER_IDS, MAX_FILTER_KINDS, MAX_FILTER_TEXT_LEN, event_facets, facets_of,
};
pub use frame::{
    AudioFrame, FrameHeader, FrameKind, HEADER_LEN, IqFrame, PROTOCOL_VERSION, RangeDopplerFrame,
    SpectrumFrame, SymbolFrame, SymbolPlane, VideoData, VideoFrame, typescript_frames,
};
pub use hunt::{HuntAction, HuntRequest, HuntSettings, HuntStatus};
pub use network::{
    ChannelNetworkExportRequest, MAX_NETWORK_ADDRESS_LEN, NetworkExportAction, NetworkExportNode,
    NetworkExportRequest, NetworkExportSettings, NetworkExportStatus, NetworkSampleFormat,
    NetworkTransport,
};
pub use patch::{
    ArrayNode, ChannelNode, CombinerNode, DEFAULT_DMR_PROBES, DEFAULT_SIGNAL_MAP_BANDWIDTH_HZ,
    DEFAULT_SIGNAL_MAP_OFFSET_HZ, DF_BEAM_PORT, DV_DECODER_KIND, DeviceNode, DeviceRef, DfNode,
    DmrChannelEntry, DmrDiscovery, DmrSearchRange, DmrTrunkNode, DmrTrunkProtocol,
    MAX_DMR_CHANNEL_MAP, MAX_DMR_LOGICAL_CHANNEL, MAX_DMR_PROBES, MAX_DMR_SEARCH_CANDIDATES,
    MAX_DMR_SEARCH_RANGES, MAX_EDGES, MAX_NODES, MAX_SIGNAL_MAP_BANDWIDTH_HZ,
    MAX_SIGNAL_MAP_OFFSET_HZ, MAX_STREAMS, MIN_DMR_SEARCH_STEP_HZ, NodeBody, NodeCategory,
    NodeTypeInfo, PassiveRadarNode, PatchCatalog, PatchEdge, PatchError, PatchGraph, PatchNode,
    PortBacking, PortCondition, PortDirection, PortRef, PortRepeat, PortSpec, PortType, Position,
    RACK_COLS, RACK_ROWS, RADAR_REFERENCE_PORT, RADAR_SURVEILLANCE_PORT, RackCell, RackLayout,
    RackSlot, RecorderNode, RecordingNode, STITCH_WIDE_PORT, SignalGenNode, SignalMapNode, Size,
    StitchNode, port_stream, siggen_key, stream_port,
};
pub use position::{
    DEFAULT_GPSD_ADDRESS, DEFAULT_NMEA_BAUD, DEFAULT_NMEA_UPDATE_INTERVAL_MS, GpsNode,
    MAX_NMEA_BAUD, MAX_NMEA_UPDATE_INTERVAL_MS, MAX_POSITION_ENDPOINT_LEN, MIN_NMEA_BAUD,
    MIN_NMEA_UPDATE_INTERVAL_MS, NmeaDeviceInfo, NmeaDevicesResponse, PositionFix, PositionSource,
};
pub use propagation::{
    DEFAULT_PROPAGATION_HALF_LIFE_MIN, DEFAULT_REFLECTION_HEIGHT_KM, IONOSONDE_MAX_STATIONS,
    IONOSONDE_SOURCE, IONOSONDE_URL, IonosondeReport, IonosondeStation,
    MAX_PROPAGATION_HALF_LIFE_MIN, MAX_REFLECTION_HEIGHT_KM, MIN_PROPAGATION_HALF_LIFE_MIN,
    MIN_REFLECTION_HEIGHT_KM, PropagationNode,
};
pub use rest::{
    AnnotationError, ApiError, ApplyTemplateRequest, AudioRecordingInfo, AudioRecordingsResponse,
    AuthInfo, Bookmark, CapturedImage, CapturedImagesResponse, ChannelTypesResponse,
    ClientsResponse, CreateBookmarkRequest, CreateChannelRequest, CreateDeviceSetRequest,
    CreatePresetRequest, CreatedId, CreatedRowId, DecoderLogEntry, DecoderLogQuery,
    DecoderLogResponse, DeletedCount, DevicesResponse, ErrorCode, EventAudio, EventImage,
    ExportFormat, LogScope, MAX_LOG_SOURCES, MAX_RECORDING_NAME_LEN, MAX_RECORDING_NOTE_LEN,
    MAX_RECORDING_TAG_LEN, MAX_RECORDING_TAGS, MAX_RECORDING_UPLOAD_BYTES, MAX_ROUTE_LEG_M,
    Maneuver, ManeuverKind, OccupancyBucket, OccupancyReport, PRESET_SNAPSHOT_VERSION,
    PlaybackAction, PlaybackRequest, PresetDevice, PresetInfo, PresetSnapshot, RecordingAnnotation,
    RecordingDownloadQuery, RecordingFormat, RecordingInfo, RecordingUpload, RecordingsResponse,
    Route, RoutePoint, RouteRequest, RoutingBackend, TemplateInfo, TemplatesResponse, VoiceCall,
    VoiceCallsResponse,
};
pub use satellite::{
    CatalogSatellite, MAX_CATALOG_RESULTS, MAX_SATELLITE_HZ, MAX_SATELLITE_QUERY_LEN, MAX_TLE_LEN,
    MAX_TRANSMITTER_ID_LEN, SATELLITE_CATALOG_SOURCE, SATELLITE_CATALOG_URL, SatelliteCatalogQuery,
    SatelliteCatalogResponse, SatelliteLook, SatelliteNode, SatellitePass, SatelliteStatus,
    TRANSMITTER_SOURCE, TRANSMITTER_URL, Transmitter, TransmittersResponse,
};
pub use scan::{
    MAX_SCAN_TARGETS, ScanAction, ScanMode, ScanRange, ScanRequest, ScanSettings, ScanState,
    ScannerStatus,
};
pub use state::{
    AudioRecordingStatus, ChannelLevel, DeviceFault, DeviceSet, DeviceSetStatus, ExtraLane,
    PlaybackStatus, RecordingStatus, SettingsRefused, StateSnapshot, TrunkChannel,
    TrunkChannelSource, TrunkControl, TrunkFollower, TrunkProbe, TrunkProblem, TrunkSystemStatus,
};
pub use timemachine::{
    DEFAULT_TIME_MACHINE_SECONDS, MAX_TIME_MACHINE_BYTES, MAX_TIME_MACHINE_SECONDS,
    MIN_TIME_MACHINE_SECONDS, TimeMachineAction, TimeMachineNode, TimeMachineRequest,
    TimeMachineStatus, history_capacity_samples,
};
pub use tools::{
    ANTENNA_TOOL_ID, AntennaDesign, AntennaGeometry, AntennaPart, AntennaPoint, AntennaReport,
    AntennaRequest, AntennaSegment, AntennaSegmentRole, GroundPlaneParams, InvertedVParams,
    MAX_ANTENNA_FREQ_HZ, MAX_APEX_ANGLE_DEG, MAX_FEEDLINE_VELOCITY_FACTOR, MAX_NANOVNA_AVERAGES,
    MAX_NANOVNA_CAL_SLOT, MAX_NANOVNA_FREQ_HZ, MAX_NANOVNA_POINTS, MAX_NANOVNA_PORT_LEN,
    MAX_RADIAL_SLOPE_DEG, MAX_RADIALS, MAX_VELOCITY_FACTOR, MAX_YAGI_DIRECTORS,
    MAX_YAGI_SPACING_WL, MIN_ANTENNA_FREQ_HZ, MIN_APEX_ANGLE_DEG, MIN_FEEDLINE_VELOCITY_FACTOR,
    MIN_NANOVNA_FREQ_HZ, MIN_NANOVNA_POINTS, MIN_VELOCITY_FACTOR, MIN_YAGI_SPACING_WL,
    NANOVNA_TOOL_ID, NanoVnaCalStep, NanoVnaCalibrateRequest, NanoVnaCalibration, NanoVnaComplex,
    NanoVnaDevice, NanoVnaDeviceReport, NanoVnaMatch, NanoVnaPoint, NanoVnaPortRequest,
    NanoVnaRequest, NanoVnaResult, NanoVnaStandard, NanoVnaSweep, NanoVnaSweepRequest,
    NanoVnaSweepState, ToolCategory, ToolDescriptor, ToolRequest, ToolResponse, ToolsResponse,
    YagiParams,
};
pub use workspace::{
    CreateWorkspaceRequest, MAX_NAME_LEN, MAX_REGION_ID_LEN, PatchApplyReport, PatchBinding,
    PatchRefusal, PlacementCoverage, UpdateWorkspaceRequest, WORKSPACE_EXPORT_VERSION,
    WORKSPACE_SNAPSHOT_VERSION, WorkspaceDetail, WorkspaceError, WorkspaceExport, WorkspaceHistory,
    WorkspaceInfo, WorkspaceSettings, WorkspaceSnapshot, WorkspacesResponse,
};
pub use workspace_state::{
    WORKSPACE_STATE_VERSION, WorkspaceChannel, WorkspaceDevice, WorkspaceState, WorkspaceTrunk,
};
pub use ws::{ClientCommand, ServerEvent, StateScope, StreamKind};

#[cfg(test)]
mod contract_tests {
    use super::*;

    #[test]
    fn server_event_is_adjacently_tagged() {
        let ev = ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(3),
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "StateChanged");
        assert_eq!(json["data"]["scope"]["scope"], "device_set");
        assert_eq!(json["data"]["scope"]["id"], 3);

        let hello = ServerEvent::Hello { revision: 9 };
        let json = serde_json::to_value(&hello).unwrap();
        assert_eq!(json["type"], "Hello");
        assert_eq!(json["data"]["revision"], 9);
    }

    #[test]
    fn client_command_roundtrips() {
        let cmd = ClientCommand::SubscribeSpectrum {
            device_set: 1,
            fps: 20,
            bins: 1024,
            stream: 2,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let back: ClientCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, back);
    }

    #[test]
    fn stream_fields_default_to_zero_for_older_peers() {
        let cmd: ClientCommand = serde_json::from_str(
            r#"{"type":"SubscribeSpectrum","data":{"device_set":1,"fps":20,"bins":1024}}"#,
        )
        .unwrap();
        assert_eq!(
            cmd,
            ClientCommand::SubscribeSpectrum {
                device_set: 1,
                fps: 20,
                bins: 1024,
                stream: 0,
            }
        );

        let create: CreateChannelRequest =
            serde_json::from_str(r#"{"settings":{"params":{"type":"nfm","settings":{}}}}"#)
                .unwrap();
        assert_eq!(create.stream, 0);

        let info: ChannelInfo =
            serde_json::from_str(r#"{"id":3,"settings":{"params":{"type":"nfm","settings":{}}}}"#)
                .unwrap();
        assert_eq!(info.stream, 0);

        let recording: RecordingStatus = serde_json::from_str(
            r#"{"file":"rec","started_at":"2026-08-09T12:00:00Z","samples":1,"bytes":8,"overruns":0}"#,
        )
        .unwrap();
        assert_eq!(recording.stream, 0);

        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["stream"], 0);
        let json = serde_json::to_value(&recording).unwrap();
        assert_eq!(json["stream"], 0);
    }

    #[test]
    fn an_annotation_normalizes_to_trimmed_unique_tags_and_a_note_or_nothing() {
        let annotation = RecordingAnnotation {
            name: Some("  Tower watch  ".to_owned()),
            tags: vec![
                "  airband ".to_owned(),
                "AIRBAND".to_owned(),
                String::new(),
                "tower".to_owned(),
            ],
            note: Some("  EDDF ground  ".to_owned()),
        };
        let normalized = annotation.normalized().unwrap();
        assert_eq!(normalized.tags, ["airband", "tower"]);
        assert_eq!(normalized.note.as_deref(), Some("EDDF ground"));
        assert_eq!(normalized.name.as_deref(), Some("Tower watch"));

        let blank = RecordingAnnotation {
            name: Some(" ".to_owned()),
            tags: vec!["   ".to_owned()],
            note: Some("  ".to_owned()),
        };
        assert_eq!(blank.normalized().unwrap(), RecordingAnnotation::default());
    }

    #[test]
    fn an_oversized_annotation_is_refused_rather_than_truncated() {
        let long_tag = RecordingAnnotation {
            name: None,
            tags: vec!["t".repeat(MAX_RECORDING_TAG_LEN + 1)],
            note: None,
        };
        assert_eq!(
            long_tag.normalized(),
            Err(AnnotationError::TagLen(MAX_RECORDING_TAG_LEN + 1))
        );

        let many = RecordingAnnotation {
            name: None,
            tags: (0..=MAX_RECORDING_TAGS).map(|i| format!("t{i}")).collect(),
            note: None,
        };
        assert_eq!(
            many.normalized(),
            Err(AnnotationError::TagCount(MAX_RECORDING_TAGS + 1))
        );

        let long_note = RecordingAnnotation {
            name: None,
            tags: Vec::new(),
            note: Some("n".repeat(MAX_RECORDING_NOTE_LEN + 1)),
        };
        assert_eq!(
            long_note.normalized(),
            Err(AnnotationError::NoteLen(MAX_RECORDING_NOTE_LEN + 1))
        );

        let long_name = RecordingAnnotation {
            name: Some("n".repeat(MAX_RECORDING_NAME_LEN + 1)),
            tags: Vec::new(),
            note: None,
        };
        assert_eq!(
            long_name.normalized(),
            Err(AnnotationError::NameLen(MAX_RECORDING_NAME_LEN + 1))
        );
    }

    #[test]
    fn a_recording_without_an_annotation_deserializes() {
        let info: RecordingInfo = serde_json::from_str(
            r#"{"id":1,"file":"rec","device_id":"virtual:file:/rec","device_label":"hw",
                "center_hz":1.0,"sample_rate":2.0,"samples":3,"bytes":24,"duration_s":1.5,
                "created_at":"2026-08-09T12:00:00Z"}"#,
        )
        .unwrap();
        assert!(info.tags.is_empty());
        assert_eq!(info.note, None);
        assert_eq!(info.name, None);
    }

    #[test]
    fn unit_scopes_serialize_without_id() {
        for (scope, tag) in [
            (StateScope::All, "all"),
            (StateScope::Presets, "presets"),
            (StateScope::Bookmarks, "bookmarks"),
            (StateScope::Recordings, "recordings"),
            (StateScope::DecoderLog, "decoder_log"),
            (StateScope::Workspaces, "workspaces"),
        ] {
            let json = serde_json::to_value(&scope).unwrap();
            assert_eq!(json["scope"], tag);
            assert!(json.get("id").is_none());
        }
    }

    #[test]
    fn channel_params_are_adjacently_tagged() {
        let params = ChannelParams::Ssb(SsbParams {
            sideband: Sideband::Lsb,
            bandwidth_hz: 2_400.0,
        });
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["type"], "ssb");
        assert_eq!(json["settings"]["sideband"], "lsb");
        assert_eq!(json["settings"]["bandwidth_hz"], 2_400.0);

        let back: ChannelParams = serde_json::from_value(json).unwrap();
        assert_eq!(back, params);
    }

    #[test]
    fn channel_params_default_from_empty_settings() {
        for (json, expected) in [
            (
                r#"{"type":"nfm","settings":{}}"#,
                ChannelParams::Nfm(NfmParams::default()),
            ),
            (
                r#"{"type":"selcall","settings":{}}"#,
                ChannelParams::Selcall(SelcallParams::default()),
            ),
            (
                r#"{"type":"am","settings":{}}"#,
                ChannelParams::Am(AmParams::default()),
            ),
            (
                r#"{"type":"ssb","settings":{}}"#,
                ChannelParams::Ssb(SsbParams::default()),
            ),
            (
                r#"{"type":"wfm","settings":{}}"#,
                ChannelParams::Wfm(WfmParams::default()),
            ),
            (
                r#"{"type":"freedv","settings":{}}"#,
                ChannelParams::Freedv(FreeDvParams::default()),
            ),
        ] {
            let parsed: ChannelParams = serde_json::from_str(json).unwrap();
            assert_eq!(parsed, expected);
            assert_eq!(parsed.type_id(), expected.type_id());
        }

        let nfm: ChannelParams = serde_json::from_str(r#"{"type":"nfm","settings":{}}"#).unwrap();
        assert_eq!(
            nfm,
            ChannelParams::Nfm(NfmParams {
                bandwidth_hz: 12_500.0,
                tone_mode: NfmToneMode::Off,
                ctcss_hz: None,
                dcs_code: None,
                scrambler_mode: NfmScramblerMode::default(),
                inversion_hz: None,
                compander: false,
            })
        );
        let ssb: ChannelParams = serde_json::from_str(r#"{"type":"ssb","settings":{}}"#).unwrap();
        assert_eq!(
            ssb,
            ChannelParams::Ssb(SsbParams {
                sideband: Sideband::Usb,
                bandwidth_hz: 2_700.0,
            })
        );
    }

    #[test]
    fn a_legacy_audio_chain_keeps_its_blanker_and_drops_the_rest() {
        let json = r#"{"params":{"type":"am","settings":{}},"audio":{"blanker":{"enabled":true,"threshold":7.0},"agc":"fast"}}"#;
        let settings: ChannelSettings = serde_json::from_str(json).unwrap();
        assert_eq!(
            settings.blanker,
            NoiseBlankerSettings {
                enabled: true,
                threshold: 7.0,
            }
        );
        let round_tripped: ChannelSettings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(round_tripped, settings);
    }

    #[test]
    fn a_payload_that_names_no_blanker_has_it_off() {
        let json = r#"{"params":{"type":"ssb","settings":{}}}"#;
        let settings: ChannelSettings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.blanker, NoiseBlankerSettings::default());
    }

    #[test]
    fn channel_settings_default_to_the_mode_home_and_no_squelch() {
        let json = r#"{"params":{"type":"adsb","settings":{}}}"#;
        let adsb: ChannelSettings = serde_json::from_str(json).unwrap();
        assert_eq!(
            adsb.frequency_hz,
            home_frequency_hz("adsb").expect("a home")
        );

        let json = r#"{"params":{"type":"wfm","settings":{"deemphasis_us":75.0}}}"#;
        let settings: ChannelSettings = serde_json::from_str(json).unwrap();
        assert_eq!(
            home_frequency_hz("wfm"),
            None,
            "wfm sits anywhere in the band"
        );
        assert_eq!(settings.frequency_hz, DEFAULT_FREQUENCY_HZ);
        assert_eq!(settings.squelch, Squelch::Off);
        assert_eq!(
            settings.params,
            ChannelParams::Wfm(WfmParams {
                deemphasis_us: 75.0,
                stereo: true,
            })
        );
    }

    #[test]
    fn a_squelch_roundtrips_as_one_tagged_value() {
        let plain: ChannelSettings =
            serde_json::from_str(r#"{"params":{"type":"nfm","settings":{}}}"#).unwrap();
        let json = serde_json::to_value(&plain).unwrap();
        assert_eq!(json["squelch"], serde_json::json!({"mode": "off"}));

        let auto: ChannelSettings = serde_json::from_str(
            r#"{"squelch":{"mode":"auto","margin_db":8.0},"params":{"type":"nfm","settings":{}}}"#,
        )
        .unwrap();
        assert_eq!(auto.squelch, Squelch::Auto { margin_db: 8.0 });
        let back: ChannelSettings =
            serde_json::from_str(&serde_json::to_string(&auto).unwrap()).unwrap();
        assert_eq!(back, auto);
    }

    #[test]
    fn a_squelch_stated_the_old_way_still_reads() {
        for (json, wanted) in [
            (
                r#"{"squelch_db":-70.0}"#,
                Squelch::Manual { level_db: -70.0 },
            ),
            (r#"{"squelch_db":null}"#, Squelch::Off),
            (
                r#"{"squelch_db":-70.0,"squelch_auto_db":8.0}"#,
                Squelch::Auto { margin_db: 8.0 },
            ),
            (
                r#"{"squelch_auto_db":8.0}"#,
                Squelch::Auto { margin_db: 8.0 },
            ),
        ] {
            let body = format!(
                r#"{{"params":{{"type":"nfm","settings":{{}}}},{}"#,
                &json[1..]
            );
            let settings: ChannelSettings = serde_json::from_str(&body).unwrap();
            assert_eq!(settings.squelch, wanted, "{json}");
        }
    }

    #[test]
    fn a_squelch_is_checked_before_it_reaches_a_gate() {
        assert!(Squelch::Off.validate().is_ok());
        assert!(Squelch::Manual { level_db: -60.0 }.validate().is_ok());
        assert!(Squelch::Manual { level_db: f32::NAN }.validate().is_err());
        assert!(Squelch::Auto { margin_db: 8.0 }.validate().is_ok());
        assert!(Squelch::Auto { margin_db: 1.0 }.validate().is_err());
        assert!(Squelch::Auto { margin_db: 41.0 }.validate().is_err());
    }

    #[test]
    fn numeric_settings_are_held_to_the_limits_their_decoder_publishes() {
        let mut morse = ChannelSettings::default_for("morse").expect("morse");
        assert!(morse.check_limits().is_ok());
        morse.params = ChannelParams::Morse(MorseParams {
            bandwidth_hz: 10.0,
            wpm: None,
        });
        let error = morse.check_limits().expect_err("10 Hz is too narrow");
        assert!(error.contains("bandwidth_hz"), "{error}");
        morse.params = ChannelParams::Morse(MorseParams {
            bandwidth_hz: 400.0,
            wpm: Some(200.0),
        });
        assert!(morse.check_limits().is_err());
        let gated = ChannelSettings {
            squelch: Squelch::Auto { margin_db: 0.5 },
            ..ChannelSettings::default_for("adsb").expect("adsb")
        };
        assert!(gated.check_limits().is_err());
        for descriptor_type in ["nfm", "ssb", "rtty", "gnss", "vor", "ident", "wspr"] {
            let limits = param_limits(descriptor_type);
            assert!(!limits.is_empty(), "{descriptor_type} publishes limits");
            assert!(
                limits.iter().all(|limit| limit.min < limit.max),
                "{descriptor_type}"
            );
        }
    }

    #[test]
    fn a_channel_level_carries_the_gate_it_is_measured_against() {
        let open = ChannelLevel {
            channel: 1,
            level_db: -42.0,
            peak_db: -30.0,
            squelch_db: None,
        };
        let json = serde_json::to_value(open).unwrap();
        assert!(json.get("squelch_db").is_none());
        let gated = ChannelLevel {
            squelch_db: Some(-61.5),
            ..open
        };
        assert_eq!(
            serde_json::to_value(gated).unwrap()["squelch_db"],
            serde_json::json!(-61.5)
        );
        let back: ChannelLevel =
            serde_json::from_str(r#"{"channel":1,"level_db":-42.0,"peak_db":-30.0}"#).unwrap();
        assert_eq!(back, open);
    }

    #[test]
    fn a_channel_states_its_audio_recordings_only_while_they_run() {
        let mut info: ChannelInfo =
            serde_json::from_str(r#"{"id":3,"settings":{"params":{"type":"nfm","settings":{}}}}"#)
                .unwrap();
        assert!(info.audio_recordings.is_empty());
        assert!(
            serde_json::to_value(&info)
                .unwrap()
                .get("audio_recordings")
                .is_none()
        );

        info.audio_recordings = vec![AudioRecordingStatus {
            fx: vec!["fx".to_owned()],
            file: "ch_1_3_20260815T120000Z.wav".to_owned(),
            started_at: "2026-08-15T12:00:00Z".to_owned(),
            channels: 1,
            frames: 48_000,
            bytes: 96_000,
            error: None,
        }];
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["audio_recordings"][0]["frames"], 48_000);
        assert_eq!(json["audio_recordings"][0]["fx"][0], "fx");
        assert!(json["audio_recordings"][0].get("error").is_none());
        let back: ChannelInfo = serde_json::from_value(json).unwrap();
        assert_eq!(back, info);
    }

    #[test]
    fn dab_transmission_mode_defaults_and_round_trips() {
        let old: DabParams = serde_json::from_str(r#"{"mode":"dab_plus","service_id":49569}"#)
            .expect("old settings");
        assert_eq!(old.transmission_mode, DabTransmissionMode::I);
        for (mode, name) in [
            (DabTransmissionMode::I, "i"),
            (DabTransmissionMode::Ii, "ii"),
            (DabTransmissionMode::Iii, "iii"),
            (DabTransmissionMode::Iv, "iv"),
        ] {
            let params = DabParams {
                transmission_mode: mode,
                ..old
            };
            let json = serde_json::to_value(params).expect("serialized settings");
            assert_eq!(json["transmission_mode"], name);
            assert_eq!(
                serde_json::from_value::<DabParams>(json).expect("settings"),
                params
            );
        }
        assert!(serde_json::from_str::<DabParams>(r#"{"transmission_mode":"v"}"#).is_err());
    }

    #[test]
    fn decoder_params_default_from_empty_settings() {
        use channel::{
            AcarsParams, AdsbParams, AisParams, AprsParams, DabParams, DatvParams, DrmParams,
            GnssParams, MorseParams, NavtexParams, PocsagParams, PskBaud, PskParams,
            RadioClockParams, RttyParams, SubghzParams, WsjtParams, WsprParams,
        };
        for (json, expected) in [
            (
                r#"{"type":"pocsag","settings":{}}"#,
                ChannelParams::Pocsag(PocsagParams::default()),
            ),
            (
                r#"{"type":"adsb","settings":{}}"#,
                ChannelParams::Adsb(AdsbParams::default()),
            ),
            (
                r#"{"type":"ais","settings":{}}"#,
                ChannelParams::Ais(AisParams::default()),
            ),
            (
                r#"{"type":"aprs","settings":{}}"#,
                ChannelParams::Aprs(AprsParams::default()),
            ),
            (
                r#"{"type":"rtty","settings":{}}"#,
                ChannelParams::Rtty(RttyParams::default()),
            ),
            (
                r#"{"type":"morse","settings":{}}"#,
                ChannelParams::Morse(MorseParams::default()),
            ),
            (
                r#"{"type":"navtex","settings":{}}"#,
                ChannelParams::Navtex(NavtexParams::default()),
            ),
            (
                r#"{"type":"acars","settings":{}}"#,
                ChannelParams::Acars(AcarsParams::default()),
            ),
            (
                r#"{"type":"subghz","settings":{}}"#,
                ChannelParams::Subghz(SubghzParams::default()),
            ),
            (
                r#"{"type":"ft8","settings":{}}"#,
                ChannelParams::Ft8(WsjtParams::default()),
            ),
            (
                r#"{"type":"ft4","settings":{}}"#,
                ChannelParams::Ft4(WsjtParams::default()),
            ),
            (
                r#"{"type":"psk","settings":{}}"#,
                ChannelParams::Psk(PskParams::default()),
            ),
            (
                r#"{"type":"psk","settings":{"baud":"psk250"}}"#,
                ChannelParams::Psk(PskParams {
                    baud: PskBaud::Psk250,
                    invert: false,
                }),
            ),
            (
                r#"{"type":"wspr","settings":{}}"#,
                ChannelParams::Wspr(WsprParams::default()),
            ),
            (
                r#"{"type":"dab","settings":{}}"#,
                ChannelParams::Dab(DabParams::default()),
            ),
            (
                r#"{"type":"datv","settings":{}}"#,
                ChannelParams::Datv(DatvParams::default()),
            ),
            (
                r#"{"type":"dvbt","settings":{}}"#,
                ChannelParams::Dvbt(crate::DvbtParams::default()),
            ),
            (
                r#"{"type":"drm","settings":{}}"#,
                ChannelParams::Drm(DrmParams::default()),
            ),
            (
                r#"{"type":"radio_clock","settings":{}}"#,
                ChannelParams::RadioClock(RadioClockParams::default()),
            ),
            (
                r#"{"type":"gnss","settings":{}}"#,
                ChannelParams::Gnss(GnssParams::default()),
            ),
        ] {
            let parsed: ChannelParams = serde_json::from_str(json).unwrap();
            assert_eq!(parsed, expected, "{json}");
            assert_eq!(parsed.type_id(), expected.type_id());
        }

        let rtty: ChannelParams = serde_json::from_str(r#"{"type":"rtty","settings":{}}"#).unwrap();
        assert_eq!(
            rtty,
            ChannelParams::Rtty(RttyParams {
                baud: 45.45,
                shift_hz: 170.0,
                stop_bits: channel::RttyStopBits::OneAndHalf,
                invert: false,
                unshift_on_space: true,
            })
        );
    }

    #[test]
    fn decoded_event_shape() {
        let ev = ServerEvent::Decoded(Box::new(decode::DecodedRecord {
            origin: None,
            sinks: Vec::new(),
            device_set: 1,
            channel: 4,
            at: "2026-08-09T12:00:00Z".to_owned(),
            freq_hz: 162_025_000.0,
            event: DecoderEvent::Ais(decode::AisMessage {
                mmsi: 211_234_560,
                msg_type: 1,
                ais_channel: 'B',
                nmea: "!AIVDM,1,1,,B,test,0*00".to_owned(),
                ..decode::AisMessage::default()
            }),
        }));
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "Decoded");
        assert_eq!(json["data"]["channel"], 4);
        assert_eq!(json["data"]["event"]["kind"], "ais");
        assert_eq!(json["data"]["event"]["data"]["mmsi"], 211_234_560);
        let back: ServerEvent = serde_json::from_value(json).unwrap();
        assert_eq!(back, ev);

        let lost = serde_json::to_value(ServerEvent::DecodedLost { count: 7 }).unwrap();
        assert_eq!(lost["type"], "DecodedLost");
        assert_eq!(lost["data"]["count"], 7);
    }

    #[test]
    fn channel_descriptor_flags_default_for_older_peers() {
        let mut json = serde_json::json!({
            "type_id": "nfm",
            "name": "NFM",
            "bandwidth_hz": 12_500.0,
            "input_rate_hz": 48_000.0,
        });
        let back: ChannelDescriptor = serde_json::from_value(json.clone()).unwrap();
        assert!(back.has_audio);
        assert_eq!(back.decoder_kind, None);
        assert!(!back.can_transmit);

        json["has_audio"] = serde_json::json!(false);
        json["decoder_kind"] = serde_json::json!("adsb");
        json["can_transmit"] = serde_json::json!(true);
        let back: ChannelDescriptor = serde_json::from_value(json).unwrap();
        assert!(!back.has_audio);
        assert_eq!(back.decoder_kind.as_deref(), Some("adsb"));
        assert!(back.can_transmit);
    }

    fn sample_device_set() -> DeviceSet {
        DeviceSet {
            id: 1,
            device: DeviceInfo {
                driver: "virtual".to_owned(),
                key: "siggen".to_owned(),
                label: "Signal Generator".to_owned(),
                serial: None,
                profile: None,
            },
            capabilities: Capabilities {
                freq_ranges: Vec::new(),
                sample_rates: Vec::new(),
                sample_rate_ranges: Vec::new(),
                gains: Vec::new(),
                antennas: Vec::new(),
                bandwidths: Vec::new(),
                bandwidth_ranges: Vec::new(),
                bandwidth_auto: false,
                bias_tee: false,
                agc: crate::device::Agc::None,
                extra: Vec::new(),
                ppm: false,
                duplex: Duplex::RxOnly,
                rx_streams: 1,
                tx_streams: 0,
                per_stream: StreamScope::default(),
                directional: None,
                dc_artifact: DcArtifact::Operator,
                hardware_sweep: false,
                coherence: Coherence::None,
                noise_source: false,
            },
            settings: DeviceSettings::default(),
            status: DeviceSetStatus::Running,
            channels: Vec::new(),
            overruns: 0,
            clipping: Vec::new(),
            error: None,
            fault: None,
            refused: None,
            recording: None,
            network_export: None,
            time_machine: None,
            scanners: Vec::new(),
            hunts: Vec::new(),
            playback: None,
            extra_lane: None,
            agc_gains: Vec::new(),
        }
    }

    #[test]
    fn an_undeclared_ppm_capability_reads_as_unsupported() {
        let json = serde_json::to_value(sample_device_set()).unwrap();
        let mut caps = json["capabilities"].clone();
        caps.as_object_mut().unwrap().remove("ppm");
        assert!(
            !serde_json::from_value::<Capabilities>(caps).unwrap().ppm,
            "an absent capability is not a supported one"
        );

        for supported in [true, false] {
            let mut set = sample_device_set();
            set.capabilities.ppm = supported;
            let json = serde_json::to_value(&set).unwrap();
            assert_eq!(json["capabilities"]["ppm"], supported);
            assert_eq!(
                serde_json::from_value::<DeviceSet>(json)
                    .unwrap()
                    .capabilities
                    .ppm,
                supported
            );
        }
    }

    #[test]
    fn a_set_without_a_transport_omits_the_playback_field() {
        let json = serde_json::to_value(sample_device_set()).unwrap();
        assert!(json.get("playback").is_none());

        let mut set = sample_device_set();
        set.playback = Some(PlaybackStatus {
            position_samples: 4_096,
            total_samples: 48_000,
            paused: true,
        });
        let json = serde_json::to_value(&set).unwrap();
        assert_eq!(json["playback"]["position_samples"], 4_096);
        assert_eq!(json["playback"]["paused"], true);
        assert_eq!(
            serde_json::from_value::<DeviceSet>(json).unwrap().playback,
            set.playback
        );
    }

    #[test]
    fn device_set_overruns_default_and_roundtrip() {
        let mut set = sample_device_set();
        set.overruns = 42;
        let mut json = serde_json::to_value(&set).unwrap();
        assert_eq!(json["overruns"], 42);

        json.as_object_mut().unwrap().remove("overruns");
        let back: DeviceSet = serde_json::from_value(json).unwrap();
        assert_eq!(back.overruns, 0);
    }

    #[test]
    fn a_device_set_names_its_clipping_lanes_only_when_there_are_any() {
        let mut set = sample_device_set();
        let json = serde_json::to_value(&set).unwrap();
        assert!(json.get("clipping").is_none());

        set.clipping = vec![0, 3];
        let json = serde_json::to_value(&set).unwrap();
        assert_eq!(json["clipping"], serde_json::json!([0, 3]));
        let back: DeviceSet = serde_json::from_value(json).unwrap();
        assert_eq!(back.clipping, [0, 3]);
    }

    #[test]
    fn device_set_recording_default_and_roundtrip() {
        let mut set = sample_device_set();
        let json = serde_json::to_value(&set).unwrap();
        assert!(json.get("recording").is_none());

        set.recording = Some(RecordingStatus {
            file: "rec-20260809-120000".to_owned(),
            stream: 0,
            started_at: "2026-08-09T12:00:00Z".to_owned(),
            samples: 2_400_000,
            bytes: 19_200_000,
            overruns: 0,
            error: None,
        });
        let mut json = serde_json::to_value(&set).unwrap();
        assert_eq!(json["recording"]["file"], "rec-20260809-120000");
        assert_eq!(json["recording"]["samples"], 2_400_000);
        assert!(json["recording"].get("error").is_none());
        let back: DeviceSet = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back, set);

        json.as_object_mut().unwrap().remove("recording");
        let back: DeviceSet = serde_json::from_value(json).unwrap();
        assert_eq!(back.recording, None);
    }

    #[test]
    fn device_set_network_export_default_and_roundtrip() {
        let mut set = sample_device_set();
        assert!(
            serde_json::to_value(&set)
                .unwrap()
                .get("network_export")
                .is_none()
        );

        set.network_export = Some(NetworkExportStatus {
            node: "net".to_owned(),
            stream: 0,
            settings: NetworkExportSettings::default(),
            sample_rate: 2_048_000,
            center_hz: 100_000_000,
            samples: 4_096,
            bytes: 32_768,
            packets: 24,
            clients: 0,
            overruns: 0,
            error: None,
        });
        let mut json = serde_json::to_value(&set).unwrap();
        assert_eq!(json["network_export"]["settings"]["format"], "cf32_le");
        assert!(json["network_export"].get("error").is_none());
        assert_eq!(
            serde_json::from_value::<DeviceSet>(json.clone()).unwrap(),
            set
        );

        json.as_object_mut().unwrap().remove("network_export");
        assert_eq!(
            serde_json::from_value::<DeviceSet>(json)
                .unwrap()
                .network_export,
            None
        );
    }

    #[test]
    fn a_channel_states_its_baseband_sinks_only_while_they_run() {
        let mut info: ChannelInfo =
            serde_json::from_str(r#"{"id":3,"settings":{"params":{"type":"nfm","settings":{}}}}"#)
                .unwrap();
        assert_eq!(info.baseband_recording, None);
        assert_eq!(info.network_export, None);
        let json = serde_json::to_value(&info).unwrap();
        assert!(json.get("baseband_recording").is_none());
        assert!(json.get("network_export").is_none());

        info.baseband_recording = Some(RecordingStatus {
            file: "bb_1_3_20260815T120000Z".to_owned(),
            stream: 0,
            started_at: "2026-08-15T12:00:00Z".to_owned(),
            samples: 48_000,
            bytes: 384_000,
            overruns: 0,
            error: None,
        });
        info.network_export = Some(NetworkExportStatus {
            node: "net".to_owned(),
            stream: 0,
            settings: NetworkExportSettings::default(),
            sample_rate: 48_000,
            center_hz: 100_012_500,
            samples: 4_096,
            bytes: 32_768,
            packets: 24,
            clients: 0,
            overruns: 0,
            error: None,
        });
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["baseband_recording"]["samples"], 48_000);
        assert_eq!(json["network_export"]["sample_rate"], 48_000);
        assert_eq!(serde_json::from_value::<ChannelInfo>(json).unwrap(), info);
    }

    #[test]
    fn a_time_machine_reports_its_window_and_only_names_a_capture_while_one_runs() {
        let mut set = sample_device_set();
        assert!(
            serde_json::to_value(&set)
                .unwrap()
                .get("time_machine")
                .is_none()
        );

        set.time_machine = Some(TimeMachineStatus {
            node: "history".to_owned(),
            stream: 0,
            history_seconds: 10,
            sample_rate: 2_048_000,
            center_hz: 100_000_000,
            held_samples: 10_240_000,
            capacity_samples: 20_480_000,
            overruns: 0,
            capture: None,
            error: None,
        });
        let mut json = serde_json::to_value(&set).unwrap();
        assert_eq!(json["time_machine"]["held_samples"], 10_240_000);
        assert!(json["time_machine"].get("capture").is_none());
        assert_eq!(
            serde_json::from_value::<DeviceSet>(json.clone()).unwrap(),
            set
        );
        assert_eq!(
            set.time_machine.as_ref().unwrap().held_seconds(),
            5.0,
            "the window reads in seconds, not samples"
        );

        json.as_object_mut().unwrap().remove("time_machine");
        assert_eq!(
            serde_json::from_value::<DeviceSet>(json)
                .unwrap()
                .time_machine,
            None
        );
    }

    #[test]
    fn a_time_machine_request_defaults_its_stream_and_window() {
        let request: TimeMachineRequest =
            serde_json::from_str(r#"{"action":"capture","node":"history"}"#).unwrap();
        assert_eq!(request.stream, 0);
        assert_eq!(request.action, TimeMachineAction::Capture);
        assert_eq!(
            request.settings.history_seconds,
            DEFAULT_TIME_MACHINE_SECONDS
        );
        assert!(request.settings.valid());

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["action"], "capture");
        assert_eq!(json["settings"]["history_seconds"], 10);
    }

    #[test]
    fn device_set_scanners_default_and_roundtrip() {
        let mut set = sample_device_set();
        let json = serde_json::to_value(&set).unwrap();
        assert!(json.get("scanners").is_none());

        set.scanners = vec![scan::ScannerStatus {
            state: scan::ScanState::Holding,
            settings: scan::ScanSettings {
                ranges: vec![scan::ScanRange {
                    start_hz: 144_000_000.0,
                    stop_hz: 146_000_000.0,
                    step_hz: 12_500.0,
                }],
                ..scan::ScanSettings::for_channel(2)
            },
            targets: 161,
            first_hz: 144_000_000.0,
            last_hz: 146_000_000.0,
            current_hz: 145_500_000.0,
            current_db: Some(-31.5),
            sweeps: 4,
            hits: 9,
            hardware_sweep: false,
            error: None,
        }];
        let mut json = serde_json::to_value(&set).unwrap();
        assert_eq!(json["scanners"][0]["state"], "holding");
        assert_eq!(json["scanners"][0]["current_hz"], 145_500_000.0);
        assert_eq!(json["scanners"][0]["settings"]["channel"], 2);
        assert!(json["scanners"][0].get("error").is_none());
        let back: DeviceSet = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back, set);

        json.as_object_mut().unwrap().remove("scanners");
        let back: DeviceSet = serde_json::from_value(json).unwrap();
        assert!(back.scanners.is_empty());
    }

    #[test]
    fn scan_settings_default_from_minimal_body() {
        let settings: scan::ScanSettings =
            serde_json::from_str(r#"{"channel":3,"frequencies":[162550000.0]}"#).unwrap();
        assert_eq!(
            settings,
            scan::ScanSettings {
                frequencies: vec![162_550_000.0],
                ..scan::ScanSettings::for_channel(3)
            }
        );
        assert_eq!(settings.threshold_db, -55.0);
        assert_eq!(settings.dwell_ms, 250);
        assert_eq!(settings.resume_ms, 1_500);
        assert_eq!(settings.measure_bw_hz, None);
        assert!(settings.skip.is_empty());
        assert!(
            serde_json::from_str::<scan::ScanSettings>(r#"{"frequencies":[162550000.0]}"#).is_err(),
            "a scan without a decoder to feed is refused"
        );

        let json = serde_json::to_value(scan::ScanRequest {
            action: scan::ScanAction::Start,
            settings: Some(scan::ScanSettings::for_channel(3)),
        })
        .unwrap();
        assert_eq!(json["action"], "start");
        assert_eq!(
            serde_json::to_value(scan::ScanAction::Stop).unwrap(),
            "stop"
        );
        assert_eq!(
            serde_json::to_value(scan::ScanAction::Skip).unwrap(),
            "skip"
        );

        let stop: scan::ScanRequest = serde_json::from_str(r#"{"action":"stop"}"#).unwrap();
        assert_eq!(stop.settings, None);
    }

    #[test]
    fn hunt_update_event_shape() {
        let ev = ServerEvent::HuntUpdate {
            device_set: 2,
            status: Box::new(hunt::HuntStatus {
                settings: hunt::HuntSettings {
                    channel: 7,
                    interval_ms: 50,
                },
                freq_hz: 433_920_000.0,
                bw_hz: 12_500.0,
                level_db: Some(-58.5),
                smooth_db: Some(-59.0),
                floor_db: Some(-92.0),
                best_db: Some(-40.0),
                strength: 0.63,
                closing: true,
                readings: 17,
                error: None,
            }),
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "HuntUpdate");
        assert_eq!(json["data"]["device_set"], 2);
        assert_eq!(json["data"]["status"]["closing"], true);
        assert_eq!(json["data"]["status"]["settings"]["channel"], 7);
        assert_eq!(json["data"]["status"]["freq_hz"], 433_920_000.0);
        assert!(
            json["data"]["status"].get("error").is_none(),
            "a hunt with nothing wrong must not carry a null fault"
        );
        let back: ServerEvent = serde_json::from_value(json).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn a_hunt_status_fills_in_what_an_older_client_leaves_out() {
        let status: hunt::HuntStatus =
            serde_json::from_str(r#"{"settings":{"channel":4},"readings":0}"#).unwrap();
        assert_eq!(status.settings.interval_ms, 50);
        assert_eq!(status.freq_hz, 0.0);
        assert_eq!(status.strength, 0.0);
        assert!(!status.closing);
        assert_eq!(status.level_db, None);
    }

    #[test]
    fn a_scan_request_that_names_no_mode_still_scans_the_listed_frequencies() {
        let settings: scan::ScanSettings =
            serde_json::from_str(r#"{"channel":1,"frequencies":[145500000.0]}"#).unwrap();
        assert_eq!(settings.mode, scan::ScanMode::Targets);
        assert_eq!(settings.margin_db, 12.0);
        assert!(
            settings.hardware_sweep,
            "a client that says nothing must still get the radio's own sweep"
        );
    }

    #[test]
    fn scanner_update_event_shape() {
        let ev = ServerEvent::ScannerUpdate {
            device_set: 3,
            status: Box::new(scan::ScannerStatus {
                state: scan::ScanState::Scanning,
                settings: scan::ScanSettings::for_channel(1),
                targets: 0,
                first_hz: 446_000_000.0,
                last_hz: 446_000_000.0,
                current_hz: 446_000_000.0,
                current_db: None,
                sweeps: 0,
                hits: 0,
                hardware_sweep: false,
                error: None,
            }),
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "ScannerUpdate");
        assert_eq!(json["data"]["device_set"], 3);
        assert_eq!(json["data"]["status"]["state"], "scanning");
        let back: ServerEvent = serde_json::from_value(json).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn doctor_report_worst_status_wins() {
        let check = |status| DoctorCheck {
            id: "x".to_owned(),
            name: "X".to_owned(),
            status,
            detail: String::new(),
            hint: None,
        };
        let report = |checks: Vec<DoctorCheck>| DoctorReport {
            version: "0".to_owned(),
            platform: "test".to_owned(),
            checks,
        };
        assert_eq!(report(Vec::new()).worst(), CheckStatus::Ok);
        assert_eq!(
            report(vec![check(CheckStatus::Ok)]).worst(),
            CheckStatus::Ok
        );
        assert_eq!(
            report(vec![check(CheckStatus::Ok), check(CheckStatus::Warn)]).worst(),
            CheckStatus::Warn
        );
        assert_eq!(
            report(vec![
                check(CheckStatus::Warn),
                check(CheckStatus::Fail),
                check(CheckStatus::Ok)
            ])
            .worst(),
            CheckStatus::Fail
        );

        let json = serde_json::to_value(check(CheckStatus::Warn)).unwrap();
        assert_eq!(json["status"], "warn");
        assert!(json.get("hint").is_none());
    }

    #[test]
    fn audio_subscription_commands_roundtrip() {
        for cmd in [
            ClientCommand::SubscribeAudio {
                device_set: 1,
                channel: 2,
                fx: vec!["fx".to_owned()],
            },
            ClientCommand::UnsubscribeAudio {
                device_set: 1,
                channel: 2,
                fx: Vec::new(),
            },
        ] {
            let json = serde_json::to_value(&cmd).unwrap();
            assert_eq!(json["data"]["device_set"], 1);
            assert_eq!(json["data"]["channel"], 2);
            let back: ClientCommand = serde_json::from_value(json).unwrap();
            assert_eq!(back, cmd);
        }
    }

    #[test]
    fn an_audio_subscription_without_fx_is_the_bare_channel() {
        let json = r#"{"type":"SubscribeAudio","data":{"device_set":1,"channel":2}}"#;
        let parsed: ClientCommand = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed,
            ClientCommand::SubscribeAudio {
                device_set: 1,
                channel: 2,
                fx: Vec::new(),
            }
        );
    }

    #[test]
    fn audio_stream_started_shape() {
        let ev = ServerEvent::AudioStreamStarted {
            stream_id: 4,
            device_set: 1,
            channel: 2,
            fx: vec!["fx".to_owned()],
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "AudioStreamStarted");
        assert_eq!(json["data"]["stream_id"], 4);
        assert_eq!(json["data"]["device_set"], 1);
        assert_eq!(json["data"]["channel"], 2);
        assert_eq!(json["data"]["fx"][0], "fx");
    }

    #[test]
    fn tool_envelopes_are_adjacently_tagged() {
        let request = ToolRequest::Antenna(AntennaRequest {
            frequency_hz: 145_500_000.0,
            design: AntennaDesign::Yagi(YagiParams {
                directors: 3,
                spacing_wavelengths: 0.2,
            }),
            ..AntennaRequest::default()
        });
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["tool"], "antenna");
        assert_eq!(json["request"]["design"]["type"], "yagi");
        assert_eq!(json["request"]["design"]["settings"]["directors"], 3);
        assert_eq!(request.tool_id(), "antenna");
        assert_eq!(
            serde_json::from_value::<ToolRequest>(json).unwrap(),
            request
        );

        let response = ToolResponse::Antenna(AntennaReport {
            design: AntennaDesign::Dipole,
            frequency_hz: 145_500_000.0,
            wavelength_m: 2.06,
            velocity_factor: 0.95,
            parts: Vec::new(),
            geometry: AntennaGeometry {
                segments: vec![AntennaSegment {
                    label: "Leg".to_owned(),
                    role: AntennaSegmentRole::Driven,
                    from: AntennaPoint::ORIGIN,
                    to: AntennaPoint::new(0.49, 0.0, 0.0),
                }],
                feed: AntennaPoint::ORIGIN,
            },
            feedpoint_ohms: Some(73.0),
            balanced: true,
            notes: Vec::new(),
        });
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["tool"], "antenna");
        assert_eq!(json["result"]["design"]["type"], "dipole");
        assert_eq!(json["result"]["geometry"]["segments"][0]["role"], "driven");
        assert_eq!(json["result"]["geometry"]["feed"]["x_m"], 0.0);
        assert_eq!(response.tool_id(), "antenna");
        assert_eq!(
            serde_json::from_value::<ToolResponse>(json).unwrap(),
            response
        );
    }

    #[test]
    fn antenna_request_defaults_from_a_minimal_body() {
        let request: AntennaRequest = serde_json::from_str(
            r#"{"frequency_hz":14200000.0,"design":{"type":"yagi","settings":{}}}"#,
        )
        .unwrap();
        assert_eq!(request.velocity_factor, 0.95);
        assert_eq!(request.feedline_velocity_factor, 0.66);
        assert_eq!(request.design, AntennaDesign::Yagi(YagiParams::default()));
        assert_eq!(request.design.type_id(), "yagi");

        let bare: AntennaRequest =
            serde_json::from_str(r#"{"frequency_hz":14200000.0,"design":{"type":"dipole"}}"#)
                .unwrap();
        assert_eq!(bare.design, AntennaDesign::Dipole);
    }

    #[test]
    fn stream_stopped_carries_kind() {
        for (kind, tag) in [
            (StreamKind::Spectrum, "spectrum"),
            (StreamKind::Audio, "audio"),
        ] {
            let ev = ServerEvent::StreamStopped { stream_id: 7, kind };
            let json = serde_json::to_value(&ev).unwrap();
            assert_eq!(json["type"], "StreamStopped");
            assert_eq!(json["data"]["stream_id"], 7);
            assert_eq!(json["data"]["kind"], tag);
            let back: ServerEvent = serde_json::from_value(json).unwrap();
            assert_eq!(back, ev);
        }
    }
}
