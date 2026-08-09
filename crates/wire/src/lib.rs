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

pub use channel::{ChannelDescriptor, ChannelInfo, ChannelSettings};
pub use device::{
    Capabilities, DeviceInfo, DeviceSettings, ExtraSetting, ExtraValue, GainStage, GainValue, Range,
};
pub use frame::{FrameKind, HEADER_LEN, PROTOCOL_VERSION, SpectrumFrame};
pub use rest::{
    ApiError, CreateChannelRequest, CreateDeviceSetRequest, CreatedId, DevicesResponse,
};
pub use state::{DeviceSet, DeviceSetStatus, StateSnapshot};
pub use ws::{ClientCommand, ServerEvent, StateScope};

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
    fn scope_all_serializes_without_id() {
        let json = serde_json::to_value(StateScope::All).unwrap();
        assert_eq!(json["scope"], "all");
        assert!(json.get("id").is_none());
    }
}
