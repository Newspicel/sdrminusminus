pub mod about;
pub mod bandplan;
pub mod channel;
pub mod decode;
pub mod device;
pub mod doctor;
pub mod frame;
pub mod patch;
pub mod position;
pub mod rest;
pub mod scan;
pub mod state;
pub mod workspace;
pub mod workspace_state;
pub mod ws;

pub use about::{AboutResponse, Attribution, ComponentSource, LicenseTextResponse};
pub use bandplan::{
    BandAllocation, BandBlock, BandLane, BandLayerInfo, BandLayerKind, BandPlan, BandRegion,
    BandRegionMatch, BandRegionsResponse, BandService, ItuRegion, LocateQuery,
};
pub use channel::{
    AcarsParams, AdsbParams, AisChannel, AisParams, AmParams, AprsMode, AprsParams, AtvModulation,
    AtvParams, AtvStandard, ChannelDescriptor, ChannelInfo, ChannelParams, ChannelSettings,
    DmrParams, DmrSlots, DpmrParams, DstarParams, M17Params, MorseParams, NavtexParams, NfmParams,
    NfmToneMode, NxdnBandwidth, NxdnParams, P25Params, PocsagBaud, PocsagParams, RttyParams,
    RttyStopBits, Sideband, SsbParams, SubghzModulation, SubghzParams, WfmParams, YsfParams,
};
pub use decode::{
    AcarsMessage, AdsbMessage, AisMessage, AprsPacket, DecodedRecord, DecoderEvent,
    DvChannelDefinition, DvFrame, DvFrameKind, DvMode, DvSlotActivity, DvTrunkProtocol, MorseText,
    NavtexMessage, PocsagMessage, PocsagPayload, RdsUpdate, RttyText, SubghzEncoding, SubghzFrame,
    ToneSquelchStatus, Vendor,
};
pub use device::{
    ArgumentInfo, ArgumentOption, ArgumentType, Capabilities, ChannelCapabilities, DeviceInfo,
    DeviceSettings, Direction, DirectionalCapabilities, Duplex, ExtraSetting, ExtraValue,
    GainStage, GainValue, Range, StreamScope, StreamSettings,
};
pub use doctor::{CheckStatus, DoctorCheck, DoctorReport};
pub use frame::{AudioFrame, FrameKind, HEADER_LEN, PROTOCOL_VERSION, SpectrumFrame, VideoFrame};
pub use patch::{
    ChannelNode, DeviceNode, DeviceRef, DmrTrunkNode, DmrTrunkProtocol, MAX_EDGES, MAX_NODES,
    MAX_STREAMS, NodeBody, NodeCategory, NodeTypeInfo, PatchCatalog, PatchEdge, PatchError,
    PatchGraph, PatchNode, PortBacking, PortCondition, PortDirection, PortRef, PortRepeat,
    PortSpec, PortType, Position, RACK_COLS, RACK_ROWS, RackCell, RackLayout, RackSlot, Size,
    port_stream, stream_port,
};
pub use position::{
    DEFAULT_GPSD_ADDRESS, DEFAULT_NMEA_BAUD, DEFAULT_NMEA_UPDATE_INTERVAL_MS, GpsNode,
    MAX_NMEA_BAUD, MAX_NMEA_UPDATE_INTERVAL_MS, MAX_POSITION_ENDPOINT_LEN, MIN_NMEA_BAUD,
    MIN_NMEA_UPDATE_INTERVAL_MS, NmeaDeviceInfo, NmeaDevicesResponse, PositionFix, PositionSource,
};
pub use rest::{
    ApiError, ApplyTemplateRequest, AuthInfo, Bookmark, ChannelTypesResponse, ClientsResponse,
    CreateBookmarkRequest, CreateChannelRequest, CreateDeviceSetRequest, CreatePresetRequest,
    CreatedId, CreatedRowId, DecoderLogEntry, DecoderLogQuery, DecoderLogResponse, DeletedCount,
    DevicesResponse, EventAudio, ExportFormat, LogScope, MAX_LOG_SOURCES, PRESET_SNAPSHOT_VERSION,
    PlaybackAction, PlaybackRequest, PresetDevice, PresetInfo, PresetSnapshot, RecordAction,
    RecordRequest, RecordingDownloadQuery, RecordingFormat, RecordingInfo, RecordingsResponse,
    TemplateInfo, TemplatesResponse, VoiceCall, VoiceCallsResponse,
};
pub use scan::{
    MAX_SCAN_TARGETS, ScanAction, ScanRange, ScanRequest, ScanSettings, ScanState, ScannerStatus,
};
pub use state::{
    DeviceSet, DeviceSetStatus, PlaybackStatus, RecordingStatus, StateSnapshot, TrunkFollower,
    TrunkProblem, TrunkSystemStatus,
};
pub use workspace::{
    CreateWorkspaceRequest, MAX_NAME_LEN, MAX_REGION_ID_LEN, PatchApplyReport, PatchBinding,
    PatchRefusal, UpdateWorkspaceRequest, WORKSPACE_SNAPSHOT_VERSION, WorkspaceDetail,
    WorkspaceError, WorkspaceInfo, WorkspaceSettings, WorkspaceSnapshot, WorkspacesResponse,
};
pub use workspace_state::{
    WORKSPACE_STATE_VERSION, WorkspaceChannel, WorkspaceDevice, WorkspaceState,
};
pub use ws::{ClientCommand, ServerEvent, StateScope, StreamKind};

#[cfg(test)]
mod contract_tests {
    use super::*;

    /// The WS event JSON shape is a contract the generated TS client switches on; lock it.
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

    /// Every body that gained a `stream` field must keep reading a payload that predates it as
    /// stream 0 — the only stream a single-stream radio has — or old clients and stored rows
    /// would be refused for naming nothing.
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

        let record: RecordRequest = serde_json::from_str(r#"{"action":"start"}"#).unwrap();
        assert_eq!(record.stream, 0);

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

        // Stream 0 is stated, never elided: `#[serde(default)]` reads the past, it does not
        // write it, so a current peer always sees which stream it was given.
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["stream"], 0);
        let json = serde_json::to_value(&recording).unwrap();
        assert_eq!(json["stream"], 0);
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

    /// The `{"type": ..., "settings": ...}` tagging is what the generated TS union
    /// discriminates on; lock it.
    #[test]
    fn channel_params_are_adjacently_tagged() {
        let params = ChannelParams::Ssb(SsbParams {
            sideband: Sideband::Lsb,
            bandwidth_hz: 2_400.0,
            agc: false,
        });
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["type"], "ssb");
        assert_eq!(json["settings"]["sideband"], "lsb");
        assert_eq!(json["settings"]["bandwidth_hz"], 2_400.0);
        assert_eq!(json["settings"]["agc"], false);

        let back: ChannelParams = serde_json::from_value(json).unwrap();
        assert_eq!(back, params);
    }

    /// Empty `settings` must deserialize to the documented defaults for every type, and
    /// those defaults must match the `Default` impls the engine constructs from.
    #[test]
    fn channel_params_default_from_empty_settings() {
        for (json, expected) in [
            (
                r#"{"type":"nfm","settings":{}}"#,
                ChannelParams::Nfm(NfmParams::default()),
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
            })
        );
        let ssb: ChannelParams = serde_json::from_str(r#"{"type":"ssb","settings":{}}"#).unwrap();
        assert_eq!(
            ssb,
            ChannelParams::Ssb(SsbParams {
                sideband: Sideband::Usb,
                bandwidth_hz: 2_700.0,
                agc: true,
            })
        );
    }

    #[test]
    fn channel_settings_defaults_offset_and_squelch() {
        let json = r#"{"params":{"type":"wfm","settings":{"deemphasis_us":75.0}}}"#;
        let settings: ChannelSettings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.offset_hz, 0.0);
        assert_eq!(settings.squelch_db, None);
        assert_eq!(
            settings.params,
            ChannelParams::Wfm(WfmParams {
                deemphasis_us: 75.0,
                stereo: true,
            })
        );
    }

    /// Every M4 decoder type must deserialize from an empty `settings` object at its
    /// documented defaults — the client sends exactly that when adding a channel.
    #[test]
    fn decoder_params_default_from_empty_settings() {
        use channel::{
            AcarsParams, AdsbParams, AisParams, AprsParams, MorseParams, NavtexParams,
            PocsagParams, RttyParams, SubghzParams,
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

    /// A decoder frame reaches the client as `{"type":"Decoded","data":{…record…}}`; the
    /// nested `kind` is what the panel switches on.
    #[test]
    fn decoded_event_shape() {
        let ev = ServerEvent::Decoded(Box::new(decode::DecodedRecord {
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

    /// `has_audio` post-dates M3 and `can_transmit` post-dates it again: snapshots from older
    /// peers omit both. The audio flag must read back as an audio channel, and the transmit flag
    /// must read back as receive-only — a peer that cannot say whether it modulates has not
    /// claimed that it does.
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
                sample_rate_range: None,
                gains: Vec::new(),
                antennas: Vec::new(),
                bandwidths: Vec::new(),
                extra: Vec::new(),
                ppm: false,
                duplex: Duplex::RxOnly,
                rx_streams: 1,
                tx_streams: 0,
                per_stream: StreamScope::default(),
                directional: None,
            },
            settings: DeviceSettings::default(),
            status: DeviceSetStatus::Running,
            channels: Vec::new(),
            overruns: 0,
            error: None,
            recording: None,
            scanner: None,
            playback: None,
        }
    }

    /// A capability set that predates the field describes a radio that never declared a
    /// frequency correction, and the client must not draw one on that guess — the same rule
    /// `duplex` follows, where a device cannot advertise a transmitter by omission.
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

    /// A radio has no transport, and the field is what the client keys its player strip on:
    /// it must stay off the wire entirely rather than serialize as an explicit null that a
    /// `!= null` check would read as "this is a recording".
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

    /// `overruns` was added after M1: snapshots from older peers omit it and must read as 0,
    /// and every serialized set must carry it so clients can render ring-drop health.
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

    /// `recording` was added in M3: an idle set must not serialize the key, snapshots from
    /// older peers omit it and must read as `None`, and a live recording must roundtrip.
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

    /// `RecordRequest.action` is a bare snake_case string the generated TS union
    /// discriminates on; lock it.
    #[test]
    fn record_request_action_shape() {
        let json = serde_json::to_value(RecordRequest {
            action: RecordAction::Start,
            stream: 0,
        })
        .unwrap();
        assert_eq!(json["action"], "start");
        assert_eq!(serde_json::to_value(RecordAction::Stop).unwrap(), "stop");

        let back: RecordRequest = serde_json::from_value(json).unwrap();
        assert_eq!(back.action, RecordAction::Start);
    }

    /// `scanner` was added in M5: an idle set must not serialize the key, snapshots from
    /// older peers omit it and must read as `None`, and a live scan must roundtrip.
    #[test]
    fn device_set_scanner_default_and_roundtrip() {
        let mut set = sample_device_set();
        let json = serde_json::to_value(&set).unwrap();
        assert!(json.get("scanner").is_none());

        set.scanner = Some(scan::ScannerStatus {
            state: scan::ScanState::Holding,
            settings: scan::ScanSettings {
                ranges: vec![scan::ScanRange {
                    start_hz: 144_000_000.0,
                    stop_hz: 146_000_000.0,
                    step_hz: 12_500.0,
                }],
                hold_channel: Some(2),
                ..scan::ScanSettings::default()
            },
            targets: 161,
            current_hz: 145_500_000.0,
            current_db: Some(-31.5),
            sweeps: 4,
            hits: 9,
            error: None,
        });
        let mut json = serde_json::to_value(&set).unwrap();
        assert_eq!(json["scanner"]["state"], "holding");
        assert_eq!(json["scanner"]["current_hz"], 145_500_000.0);
        assert_eq!(json["scanner"]["settings"]["hold_channel"], 2);
        assert!(json["scanner"].get("error").is_none());
        let back: DeviceSet = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back, set);

        json.as_object_mut().unwrap().remove("scanner");
        let back: DeviceSet = serde_json::from_value(json).unwrap();
        assert_eq!(back.scanner, None);
    }

    /// The scanner's defaults are what a client sends when it posts a bare range list;
    /// they must be the same numbers the engine would have chosen.
    #[test]
    fn scan_settings_default_from_minimal_body() {
        let settings: scan::ScanSettings =
            serde_json::from_str(r#"{"frequencies":[162550000.0]}"#).unwrap();
        assert_eq!(
            settings,
            scan::ScanSettings {
                frequencies: vec![162_550_000.0],
                ..scan::ScanSettings::default()
            }
        );
        assert_eq!(settings.threshold_db, -55.0);
        assert_eq!(settings.dwell_ms, 250);
        assert_eq!(settings.resume_ms, 1_500);
        assert_eq!(settings.measure_bw_hz, 12_500.0);
        assert_eq!(settings.hold_channel, None);

        let json = serde_json::to_value(scan::ScanRequest {
            action: scan::ScanAction::Start,
            settings: Some(scan::ScanSettings::default()),
        })
        .unwrap();
        assert_eq!(json["action"], "start");
        assert_eq!(
            serde_json::to_value(scan::ScanAction::Stop).unwrap(),
            "stop"
        );

        let stop: scan::ScanRequest = serde_json::from_str(r#"{"action":"stop"}"#).unwrap();
        assert_eq!(stop.settings, None);
    }

    /// Scanner progress is its own event so it never triggers a state refetch; lock the shape
    /// the client switches on.
    #[test]
    fn scanner_update_event_shape() {
        let ev = ServerEvent::ScannerUpdate {
            device_set: 3,
            status: Box::new(scan::ScannerStatus {
                state: scan::ScanState::Scanning,
                settings: scan::ScanSettings::default(),
                targets: 0,
                current_hz: 446_000_000.0,
                current_db: None,
                sweeps: 0,
                hits: 0,
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

    /// `--doctor` and `GET /api/doctor` render the same report; the worst status is what the
    /// CLI turns into an exit code, so it must not be forgiving.
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
            },
            ClientCommand::UnsubscribeAudio {
                device_set: 1,
                channel: 2,
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
    fn audio_stream_started_shape() {
        let ev = ServerEvent::AudioStreamStarted {
            stream_id: 4,
            device_set: 1,
            channel: 2,
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "AudioStreamStarted");
        assert_eq!(json["data"]["stream_id"], 4);
        assert_eq!(json["data"]["device_set"], 1);
        assert_eq!(json["data"]["channel"], 2);
    }

    /// Spectrum and audio stream ids come from different spaces, so a `StreamStopped`
    /// without a kind is undecidable client-side; lock the disambiguated shape.
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
