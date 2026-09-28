use std::sync::{Arc, Weak};

use sdrmm_wire::{ArrayCal, ArrayGain, ArrayTune, Coherence};
use tokio::sync::broadcast;

use super::*;
use crate::array::host::tests::frame;

struct Quiet;

impl ArrayControl for Quiet {
    fn switch_array_noise(&self, _node: &str, _on: bool) -> Result<(), EngineError> {
        Ok(())
    }

    fn tune_array_internal(&self, _node: &str, _tune: ArrayTune) -> Result<(), EngineError> {
        Ok(())
    }
}

#[test]
fn an_array_before_its_sync_worker_exists_is_refused() {
    let quiet: Arc<dyn ArrayControl> = Arc::new(Quiet);
    let control: Weak<dyn ArrayControl> = Arc::downgrade(&quiet);
    let mut feeds = Vec::new();
    let mut ports = Vec::new();
    for stream in 0..2 {
        let (port, _writer) = TapPort::new(stream);
        feeds.push(Some(port.lease(48_000.0, 1).expect("lease")));
        ports.push(port);
    }
    let setup = RuntimeSetup {
        node: "array-1".to_owned(),
        feeds,
        frame: frame(2),
        board: Arc::new(StatusBoard::new(2)),
        control,
        config: ControlConfig {
            cal: ArrayCal::default(),
            gain: ArrayGain::default(),
            needs_time: true,
            needs_phase: true,
            tier: TierDecision {
                tier: Coherence::TimeSync,
                devices: 1,
                keeps_phase: false,
                structural_zero_delay: false,
            },
            sample_rate: 48_000.0,
        },
        events: broadcast::channel(16).0,
    };
    match ArrayRuntime::start(setup) {
        Err(EngineError::Processor(message)) => assert_eq!(message, "array sync is not built yet"),
        Err(other) => panic!("unexpected refusal {other}"),
        Ok(_) => panic!("an array started without its sync worker"),
    }
}

fn crosses_threads<T: Send>() {}

fn shared_between_threads<T: Send + Sync>() {}

#[test]
fn runtime_parts_cross_threads() {
    crosses_threads::<ArrayRuntime>();
    crosses_threads::<Command>();
    crosses_threads::<Retired>();
    crosses_threads::<ProcessorHost>();
    crosses_threads::<LaneFeed>();
    crosses_threads::<TapWriter>();
    shared_between_threads::<ArrayEvent>();
    shared_between_threads::<StatusBoard>();
    shared_between_threads::<TapPort>();
    shared_between_threads::<CommandQueue>();
}
