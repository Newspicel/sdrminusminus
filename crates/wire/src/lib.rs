//! `sdrmm-wire` — the single source of truth for everything on the wire (PLAN §4).
//!
//! REST DTOs, WS message enums, device/channel/state models, and the binary frame layout
//! all live here with `serde` + `utoipa::ToSchema` derives. TypeScript is generated from the
//! OpenAPI these types produce; hand-writing a TS mirror of any of them is a review-blocking
//! offense (CLAUDE.md non-negotiable #1). This crate has no internal dependencies so anything
//! may use it.

pub mod channel;
pub mod device;
pub mod frame;
pub mod rest;
pub mod state;
pub mod ws;

pub use channel::{
    AmParams, ChannelDescriptor, ChannelInfo, ChannelParams, ChannelSettings, NfmParams, Sideband,
    SsbParams, WfmParams,
};
pub use device::{
    Capabilities, DeviceInfo, DeviceSettings, ExtraSetting, ExtraValue, GainStage, GainValue, Range,
};
pub use frame::{AudioFrame, FrameKind, HEADER_LEN, PROTOCOL_VERSION, SpectrumFrame};
pub use rest::{
    ApiError, ApplyPresetRequest, Bookmark, ChannelTypesResponse, CreateBookmarkRequest,
    CreateChannelRequest, CreateDeviceSetRequest, CreatePresetRequest, CreatedId, CreatedRowId,
    DevicesResponse, PresetInfo, PresetSnapshot,
};
pub use state::{DeviceSet, DeviceSetStatus, StateSnapshot};
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
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let back: ClientCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, back);
    }

    #[test]
    fn unit_scopes_serialize_without_id() {
        for (scope, tag) in [
            (StateScope::All, "all"),
            (StateScope::Presets, "presets"),
            (StateScope::Bookmarks, "bookmarks"),
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
                bandwidth_hz: 12_500.0
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
                deemphasis_us: 75.0
            })
        );
    }

    /// `overruns` was added after M1: snapshots from older peers omit it and must read as 0,
    /// and every serialized set must carry it so clients can render ring-drop health.
    #[test]
    fn device_set_overruns_default_and_roundtrip() {
        let set = DeviceSet {
            id: 1,
            device: DeviceInfo {
                driver: "virtual".to_owned(),
                key: "siggen".to_owned(),
                label: "Signal Generator".to_owned(),
                serial: None,
            },
            capabilities: Capabilities {
                freq_ranges: Vec::new(),
                sample_rates: Vec::new(),
                sample_rate_range: None,
                gains: Vec::new(),
                antennas: Vec::new(),
                bandwidths: Vec::new(),
                extra: Vec::new(),
                tx_capable: false,
            },
            settings: DeviceSettings::default(),
            status: DeviceSetStatus::Running,
            channels: Vec::new(),
            overruns: 42,
            error: None,
        };
        let mut json = serde_json::to_value(&set).unwrap();
        assert_eq!(json["overruns"], 42);

        json.as_object_mut().unwrap().remove("overruns");
        let back: DeviceSet = serde_json::from_value(json).unwrap();
        assert_eq!(back.overruns, 0);
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
