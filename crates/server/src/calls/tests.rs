use sdrmm_device::DeviceRegistry;
use sdrmm_wire::{DecodedRecord, DecoderEvent, DvFrameKind, DvMode};

use super::*;

fn new_call(encrypted: bool) -> NewCall {
    NewCall {
        node: "calls".to_owned(),
        started_at: "2026-08-14T10:00:00Z".to_owned(),
        ended_at: "2026-08-14T10:00:01Z".to_owned(),
        duration_ms: 1_000,
        device_set: 1,
        channel: 2,
        freq_hz: 451_125_000.0,
        mode: "dmr".to_owned(),
        frame: Some(DvFrame {
            encrypted: Some(encrypted),
            ..DvFrame::new(DvMode::Dmr, DvFrameKind::Header)
        }),
        audio_error: None,
    }
}

fn binding(gate: Gate) -> CallBinding {
    CallBinding {
        node: "radio".to_owned(),
        device_set: 1,
        channel: 2,
        gate,
    }
}

#[test]
fn a_plain_channel_binds_for_calls_with_no_trunk_system_present() {
    let engine = Engine::with_registry(DeviceRegistry::new(), None);
    let policy = CallPolicy {
        channels: vec![binding(Gate::Squelch)],
        ..CallPolicy::default()
    };

    let bindings = resolve_bindings(&engine, &policy);

    let bound = bindings.get(&(1, 2)).expect("the channel is bound");
    assert_eq!(bound, &vec![binding(Gate::Squelch)]);
}

#[tokio::test(start_paused = true)]
async fn a_binding_that_arrives_without_a_state_event_still_records() {
    let engine = Engine::with_registry(DeviceRegistry::new(), None);
    let calls = Arc::new(Calls::default());
    let (policy, watched) = watch::channel(Recording::default());
    let task = tokio::spawn(run(Arc::downgrade(&engine), calls.clone(), watched));
    tokio::time::sleep(Duration::from_millis(10)).await;

    policy.send_if_modified(|current| {
        *current = Arc::new(CallPolicy {
            channels: vec![binding(Gate::Frames)],
            ..CallPolicy::default()
        });
        false
    });
    tokio::time::sleep(RECONCILE_INTERVAL * 2).await;
    for kind in [
        DvFrameKind::Header,
        DvFrameKind::Voice,
        DvFrameKind::Terminator,
    ] {
        engine.publish_decoded(DecodedRecord {
            origin: None,
            sinks: Vec::new(),
            device_set: 1,
            channel: 2,
            at: "2026-08-14T10:00:00Z".to_owned(),
            freq_hz: 451_125_000.0,
            event: DecoderEvent::Dv(DvFrame {
                slot: Some(1),
                destination: Some(91),
                ..DvFrame::new(DvMode::Dmr, kind)
            }),
        });
    }

    let listed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let listed = calls.list();
            if !listed.is_empty() {
                return listed;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a binding that no state event announced is still picked up");
    task.abort();

    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].node, "radio");
    assert_eq!(listed[0].mode, "dmr");
}

#[test]
fn wav_is_mono_8k_pcm() {
    let bytes = wav(&[0, i16::MAX, i16::MIN]);
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 8_000);
    assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 6);
}

#[test]
fn encrypted_completion_has_no_audio() {
    let calls = Calls::default();
    let (stored, _) = calls.push(new_call(true), None, Duration::from_secs(30));
    assert!(stored.encrypted);
    assert!(stored.audio.is_none());
    assert!(calls.audio(stored.id).is_none());
    assert_eq!(calls.list().len(), 1);
}

#[test]
fn a_call_without_frames_has_no_digital_identity() {
    let calls = Calls::default();
    let (stored, _) = calls.push(
        NewCall {
            mode: "am".to_owned(),
            frame: None,
            ..new_call(false)
        },
        None,
        Duration::from_secs(30),
    );
    assert_eq!(stored.mode, "am");
    assert_eq!(stored.mode_label(), "AM");
    assert_eq!(
        (stored.source, stored.destination, stored.slot),
        (None, None, None)
    );
    assert!(!stored.encrypted && !stored.emergency);
}

#[test]
fn evicting_audio_keeps_the_call_and_says_why() {
    let calls = Calls::default();
    let big = Bytes::from(vec![0u8; MAX_STORED_AUDIO_BYTES / 2 + 1]);
    let mut evictions = 0;
    for _ in 0..3 {
        let (_, evicted) = calls.push(new_call(false), Some(big.clone()), Duration::from_secs(30));
        evictions += usize::from(evicted);
    }
    assert!(evictions >= 1, "eviction was never reported");
    let listed = calls.list();
    assert_eq!(listed.len(), 3);
    let evicted = listed.iter().filter(|call| call.audio.is_none()).count();
    assert!(evicted >= 1, "nothing was evicted over the byte limit");
    assert!(
        listed
            .iter()
            .filter(|call| call.audio.is_none())
            .all(|call| call.audio_error.is_some())
    );
    let inner = calls.lock();
    assert!(inner.audio_bytes <= MAX_STORED_AUDIO_BYTES);
}
