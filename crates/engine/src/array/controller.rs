use std::{
    sync::{Arc, Weak, mpsc},
    thread::JoinHandle,
};

use rtrb::Consumer;
use sdrmm_wire::{ArrayCal, ArrayGain, NoiseSource};
use tokio::sync::broadcast;

use super::{AggregatorEvent, ArrayControl, ArrayEvent, CommandQueue, StatusBoard, TierDecision};
use crate::EngineError;

pub(crate) enum ControlCommand {
    Configure(Box<ControlConfig>),
    Recalibrate,
    Resync { coarse: bool },
    NoiseSwitch(Option<NoiseSwitch>),
    Stop,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ControlConfig {
    pub(crate) cal: ArrayCal,
    pub(crate) gain: ArrayGain,
    pub(crate) needs_time: bool,
    pub(crate) needs_phase: bool,
    pub(crate) tier: TierDecision,
    pub(crate) sample_rate: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NoiseSwitch {
    pub(crate) device_set: u32,
    pub(crate) kind: NoiseSource,
    pub(crate) all_lanes_held: bool,
}

#[expect(dead_code)]
pub(crate) struct ControllerIo {
    pub(crate) node: String,
    pub(crate) control: Weak<dyn ArrayControl>,
    pub(crate) commands: mpsc::Receiver<ControlCommand>,
    pub(crate) queue: Arc<CommandQueue>,
    pub(crate) events: Consumer<AggregatorEvent>,
    pub(crate) board: Arc<StatusBoard>,
    pub(crate) array_events: broadcast::Sender<ArrayEvent>,
    pub(crate) config: ControlConfig,
}

pub(crate) fn spawn_controller(
    _name: String,
    _io: ControllerIo,
) -> Result<JoinHandle<()>, EngineError> {
    Err(EngineError::Processor(
        "array sync is not built yet".to_owned(),
    ))
}
