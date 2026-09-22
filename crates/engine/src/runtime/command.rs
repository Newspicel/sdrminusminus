use sdrmm_device::RxSink;
use sdrmm_wire::PositionFix;

use super::{ChannelHost, subbands::Subbands};
use crate::{
    audio_recording::AudioRecorderTap, network_export::NetworkExportTap,
    publishing::recording::RecordingPublisher, recording::RecorderTap,
    time_machine::TimeMachineTap,
};

pub(crate) enum DspCommand {
    AddMonitor {
        id: u64,
        tap: Box<crate::monitor::MonitorTap>,
    },
    RemoveMonitor {
        id: u64,
    },
    SetSubbands(Box<Subbands>),
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
        reset_state: bool,
    },
    RemoveChannel {
        id: u32,
    },
    RetuneChannel {
        id: u32,
        frequency_hz: f64,
    },
    PositionChanged {
        id: u32,
        fix: Option<PositionFix>,
    },
    SteerChannel {
        id: u32,
        doppler: crate::Doppler,
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
