use sdrmm_device::RxSink;
use sdrmm_wire::{ChannelSettings, PositionFix};

use super::ChannelHost;
use crate::{
    audio_recording::AudioRecorderTap, network_export::NetworkExportTap,
    publishing::recording::RecordingPublisher, recording::RecorderTap,
    time_machine::TimeMachineTap,
};

pub(crate) enum DspCommand {
    ConnectArray {
        id: u32,
        sink: RxSink,
    },
    DisconnectArray {
        id: u32,
    },
    AddChannel {
        id: u32,
        host: Box<ChannelHost>,
    },
    RemoveChannel {
        id: u32,
    },
    Retune {
        id: u32,
        offset_hz: f64,
    },
    ApplySettings {
        id: u32,
        settings: ChannelSettings,
    },
    PositionChanged {
        id: u32,
        fix: Option<PositionFix>,
    },
    StartRecording {
        tap: RecorderTap,
        publisher: RecordingPublisher,
    },
    StopRecording,
    StartChannelRecording {
        id: u32,
        tap: AudioRecorderTap,
    },
    StopChannelRecording {
        id: u32,
    },
    StartBasebandRecording {
        id: u32,
        tap: RecorderTap,
    },
    StopBasebandRecording {
        id: u32,
    },
    StartBasebandExport {
        id: u32,
        tap: NetworkExportTap,
    },
    StopBasebandExport {
        id: u32,
    },
    StartNetworkExport {
        tap: NetworkExportTap,
    },
    StopNetworkExport,
    StartTimeMachine {
        tap: Box<TimeMachineTap>,
    },
    StopTimeMachine,
}
