use std::collections::BTreeMap;

use super::*;

#[derive(Default)]
struct AudioClock {
    next: Option<(u32, u64)>,
    packets: usize,
}

impl AudioClock {
    fn receive(&mut self, bytes: &[u8]) {
        assert!(bytes.len() > sdrmm_wire::HEADER_LEN + 1);
        let sequence = u32::from_le_bytes(bytes[4..8].try_into().expect("sequence"));
        let timestamp = u64::from_le_bytes(bytes[8..16].try_into().expect("timestamp"));
        if let Some(expected) = self.next {
            assert_eq!((sequence, timestamp), expected, "WebSocket audio gap");
        }
        self.next = Some((
            sequence.wrapping_add(1),
            timestamp + sdrmm_engine::audio::OPUS_FRAME_SAMPLES as u64,
        ));
        self.packets += 1;
    }
}

async fn listen(mut socket: WsClient, device_set: u32, channels: &[u32]) {
    send(
        &mut socket,
        &ClientCommand::SubscribeSpectrum {
            device_set,
            stream: 0,
            fps: 30,
            bins: 4096,
        },
    )
    .await;
    send(
        &mut socket,
        &ClientCommand::SubscribeDiagnostics { enabled: true },
    )
    .await;
    for &channel in channels {
        send(
            &mut socket,
            &ClientCommand::SubscribeAudio {
                device_set,
                channel,
                fx: Vec::new(),
            },
        )
        .await;
    }
    let mut audio = BTreeMap::<u16, AudioClock>::new();
    let mut spectra = 0;
    let mut diagnostics = 0;
    timeout(WAIT * 2, async {
        loop {
            let message = socket.next().await.expect("open socket").expect("frame");
            match message {
                tungstenite::Message::Text(text) => {
                    match serde_json::from_str::<ServerEvent>(&text).expect("event") {
                        ServerEvent::AudioStreamStarted { stream_id, .. } => {
                            assert!(audio.insert(stream_id, AudioClock::default()).is_none());
                        }
                        ServerEvent::PipelineHealth { queues, websocket } => {
                            diagnostics += 1;
                            assert_eq!(websocket.dropped, 0, "WebSocket queue lost frames");
                            assert!(queues.iter().all(|queue| queue.health.dropped == 0));
                        }
                        ServerEvent::Error { message } => panic!("stream failed: {message}"),
                        ServerEvent::StreamStopped { .. } => panic!("stream stopped early"),
                        _ => {}
                    }
                }
                tungstenite::Message::Binary(bytes) => {
                    assert!(bytes.len() >= sdrmm_wire::HEADER_LEN);
                    assert_eq!(bytes[0], sdrmm_wire::PROTOCOL_VERSION);
                    let stream = u16::from_le_bytes(bytes[2..4].try_into().expect("stream"));
                    match sdrmm_wire::FrameKind::from_u8(bytes[1]).expect("frame kind") {
                        sdrmm_wire::FrameKind::AudioOpus => {
                            audio
                                .get_mut(&stream)
                                .expect("announced audio")
                                .receive(&bytes);
                        }
                        sdrmm_wire::FrameKind::Spectrum => spectra += 1,
                        other => panic!("unexpected frame {other:?}"),
                    }
                }
                tungstenite::Message::Close(_) => panic!("connection closed early"),
                _ => {}
            }
            if audio.len() == channels.len()
                && audio.values().all(|clock| clock.packets >= 100)
                && spectra >= 30
                && diagnostics > 0
            {
                break;
            }
        }
    })
    .await
    .expect("all audio streams sustained two seconds with spectrum and diagnostics");
    socket.close(None).await.expect("close");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_socket_listeners_keep_audio_continuous_during_retunes() {
    let engine = test_engine();
    let ds = engine.create_device_set("virtual:siggen").expect("radio");
    let channels: Vec<_> = (0..8)
        .map(|index| {
            engine
                .add_channel(ds, 0, nfm_channel(index as f64 * 25_000.0))
                .expect("channel")
        })
        .collect();
    let (address, _) = serve_ws(engine.clone()).await;
    let first = dial(address).await;
    let second = dial(address).await;
    let control = {
        let engine = engine.clone();
        let channels = channels.clone();
        tokio::task::spawn_blocking(move || {
            for round in 0..6 {
                std::thread::sleep(Duration::from_millis(250));
                for (index, &channel) in channels.iter().enumerate() {
                    let offset = index as f64 * 25_000.0 + f64::from(round % 2) * 1_000.0;
                    engine
                        .patch_channel(ds, channel, nfm_channel(offset))
                        .expect("retune");
                }
            }
        })
    };
    tokio::join!(listen(first, ds, &channels), listen(second, ds, &channels));
    control.await.expect("retune task");
    assert_eq!(engine.snapshot().device_sets[0].overruns, 0);
    engine.remove_device_set(ds).expect("remove radio");
}
