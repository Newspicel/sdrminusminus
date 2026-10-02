use sdrmm_device::DeviceRegistry;
use sdrmm_wire::{ChannelParams, ChannelSettings, DvMode, Squelch, VoiceCall};

use super::*;

const MS: usize = 48;

fn framed(node: &str) -> Bindings {
    HashMap::from([(
        (1, 2),
        vec![CallBinding {
            node: node.to_owned(),
            device_set: 1,
            channel: 2,
            gate: Gate::Frames,
        }],
    )])
}

fn record(kind: DvFrameKind, slot: u8) -> DecodedRecord {
    DecodedRecord {
        origin: None,
        sinks: Vec::new(),
        device_set: 1,
        channel: 2,
        at: "2026-08-14T10:00:00Z".to_owned(),
        freq_hz: 451_125_000.0,
        event: DecoderEvent::Dv(DvFrame {
            kind,
            slot: Some(slot),
            color_code: Some(3),
            source: Some(1001),
            destination: Some(91),
            group_call: Some(true),
            encrypted: Some(false),
            ..DvFrame::default()
        }),
    }
}

fn samples(frames: usize) -> PcmBlock {
    PcmBlock {
        start_frame: 0,
        channels: 1,
        payload: PcmPayload::Samples(vec![0.25; frames].into()),
    }
}

fn silence(frames: usize) -> PcmBlock {
    PcmBlock {
        start_frame: 0,
        channels: 1,
        payload: PcmPayload::Silence(frames),
    }
}

fn completed(decoded: &mut tokio::sync::broadcast::Receiver<DecodedRecord>) -> Vec<VoiceCall> {
    std::iter::from_fn(|| decoded.try_recv().ok())
        .filter_map(|record| match record.event {
            DecoderEvent::Call(call) => Some(call),
            _ => None,
        })
        .collect()
}

struct Analog {
    engine: Arc<Engine>,
    tracker: Tracker,
    calls: Arc<Calls>,
    device_set: u32,
    channel: u32,
}

fn analog() -> Analog {
    let mut registry = DeviceRegistry::new();
    registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
    let engine = Engine::with_registry(registry, None);
    let device_set = engine
        .create_device_set("virtual:band")
        .expect("open the virtual radio");
    let channel = engine
        .add_channel(
            device_set,
            0,
            ChannelSettings {
                frequency_hz: 144_100_000.0,
                squelch: Squelch::Manual { level_db: -80.0 },
                params: ChannelParams::default_for("am").expect("am"),
                blanker: Default::default(),
            },
        )
        .expect("add channel");
    let calls = Arc::new(Calls::default());
    let mut tracker = Tracker::new(calls.clone(), Arc::downgrade(&engine));
    tracker.rebind(HashMap::from([(
        (device_set, channel),
        vec![CallBinding {
            node: "am".to_owned(),
            device_set,
            channel,
            gate: Gate::Squelch,
        }],
    )]));
    Analog {
        engine,
        tracker,
        calls,
        device_set,
        channel,
    }
}

impl Analog {
    fn feed(&mut self, block: &PcmBlock) {
        self.tracker.pcm(self.device_set, self.channel, block);
    }
}

#[test]
fn a_conventional_transmission_completes_one_call() {
    let engine = Engine::with_registry(DeviceRegistry::new(), None);
    let mut decoded = engine.subscribe_decoded();
    let calls = Arc::new(Calls::default());
    let mut tracker = Tracker::new(calls.clone(), Arc::downgrade(&engine));
    tracker.rebind(framed("dmr"));
    for kind in [
        DvFrameKind::Header,
        DvFrameKind::Voice,
        DvFrameKind::Terminator,
    ] {
        tracker.record(&record(kind, 1));
    }

    assert_eq!(tracker.active(), 0, "the terminator closes the call");
    let announced = completed(&mut decoded);
    assert_eq!(announced.len(), 1);
    assert_eq!(announced[0].node, "dmr");
    assert_eq!(announced[0].mode, "dmr");
    assert_eq!(calls.list().len(), 1);
}

#[test]
fn the_two_slots_of_one_channel_are_two_calls() {
    let engine = Engine::with_registry(DeviceRegistry::new(), None);
    let mut tracker = Tracker::new(Arc::new(Calls::default()), Arc::downgrade(&engine));
    tracker.rebind(framed("trunk"));
    for slot in [1, 2] {
        tracker.record(&record(DvFrameKind::Header, slot));
    }
    assert_eq!(tracker.active(), 2, "the slots merged into one call");
}

#[test]
fn a_framed_call_keeps_its_audio() {
    let engine = Engine::with_registry(DeviceRegistry::new(), None);
    let calls = Arc::new(Calls::default());
    let mut tracker = Tracker::new(calls.clone(), Arc::downgrade(&engine));
    tracker.rebind(framed("trunk"));
    tracker.record(&record(DvFrameKind::Header, 1));
    tracker.pcm(1, 2, &samples(100 * MS));
    tracker.record(&record(DvFrameKind::Terminator, 1));

    let listed = calls.list();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].destination, Some(91));
    let audio = calls.audio(listed[0].id).expect("audio");
    assert!(audio.len() > 44 && audio.len() <= 44 + 800 * 2);
}

#[test]
fn a_partial_link_control_does_not_split_a_call() {
    let full = DvFrame {
        slot: Some(1),
        source: Some(1001),
        destination: Some(91),
        ..DvFrame::new(DvMode::Dmr, DvFrameKind::Voice)
    };
    let partial = DvFrame {
        source: None,
        ..full.clone()
    };
    assert!(same_call(&full, &partial));
    let other = DvFrame {
        source: Some(2002),
        ..full.clone()
    };
    assert!(!same_call(&full, &other));
}

#[test]
fn an_open_squelch_starts_a_call_with_the_channel_mode_and_frequency() {
    let mut analog = analog();
    let mut decoded = analog.engine.subscribe_decoded();
    analog.feed(&samples(200 * MS));
    analog.feed(&silence(SQUELCH_HOLD_FRAMES));

    let calls = completed(&mut decoded);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].mode, "am");
    assert_eq!(calls[0].freq_hz, 144_100_000.0);
    assert_eq!(calls[0].source, None);
    let audio = analog.calls.audio(calls[0].id).expect("audio");
    assert_eq!(audio.len(), 44 + 1_600 * 2, "trailing hold is not kept");
}

#[test]
fn a_short_squelch_drop_stays_inside_one_call() {
    let mut analog = analog();
    let mut decoded = analog.engine.subscribe_decoded();
    analog.feed(&samples(100 * MS));
    analog.feed(&silence(500 * MS));
    analog.feed(&samples(100 * MS));
    analog.feed(&silence(SQUELCH_HOLD_FRAMES));

    let calls = completed(&mut decoded);
    assert_eq!(calls.len(), 1);
    let audio = analog.calls.audio(calls[0].id).expect("audio");
    assert_eq!(audio.len(), 44 + 5_600 * 2, "the pause is part of the call");
}

#[test]
fn a_closed_squelch_ends_the_call_and_the_next_opening_starts_another() {
    let mut analog = analog();
    let mut decoded = analog.engine.subscribe_decoded();
    for _ in 0..2 {
        analog.feed(&samples(100 * MS));
        analog.feed(&silence(SQUELCH_HOLD_FRAMES));
    }
    analog.feed(&silence(SQUELCH_HOLD_FRAMES));

    assert_eq!(completed(&mut decoded).len(), 2);
    assert_eq!(analog.tracker.active(), 0);
}

#[test]
fn a_call_at_the_length_limit_continues_in_a_new_call() {
    let mut analog = analog();
    let mut decoded = analog.engine.subscribe_decoded();
    analog.feed(&samples(10 * MS));
    let key = CallKey {
        node: "am".to_owned(),
        device_set: analog.device_set,
        channel: analog.channel,
        slot: None,
    };
    let call = analog.tracker.active.get_mut(&key).expect("open call");
    call.audio.samples.resize(MAX_CALL_SAMPLES - 10, 0);
    analog.feed(&samples(10 * MS));

    let calls = completed(&mut decoded);
    assert_eq!(calls.len(), 1, "the full call was handed on");
    assert_eq!(calls[0].audio_error, None, "nothing was cut");
    assert_eq!(analog.tracker.active(), 1, "the transmission goes on");
}

#[test]
fn unbinding_a_channel_completes_its_open_call() {
    let mut analog = analog();
    let mut decoded = analog.engine.subscribe_decoded();
    analog.feed(&samples(100 * MS));
    analog.tracker.rebind(Bindings::new());

    assert_eq!(completed(&mut decoded).len(), 1);
    assert_eq!(analog.tracker.active(), 0);
}
