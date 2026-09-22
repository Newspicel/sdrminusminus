use std::{collections::HashMap, sync::Arc, time::Duration};

use sdrmm_engine::Engine;
use sdrmm_wire::{
    AdsbMessage, DecodedRecord, DecoderEvent, EventOutputTarget, ServerEvent,
    event_output::BeastExportStatus,
};
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    sync::{broadcast, watch},
    task::{JoinHandle, JoinSet},
};

use super::Binding;

const MAX_CLIENTS: usize = 16;
const QUEUE_CAPACITY: usize = 256;
const HEARTBEAT: [u8; 11] = [0x1a, b'1', 0, 0, 0, 0, 0, 0, 0, 0, 0];

struct Entry {
    target: EventOutputTarget,
    frames: broadcast::Sender<Arc<[u8]>>,
    status: watch::Sender<BeastExportStatus>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for Entry {
    fn drop(&mut self) {
        if let Some(worker) = &self.worker {
            worker.abort();
        }
    }
}

#[derive(Default)]
pub(super) struct Outputs {
    entries: HashMap<String, Entry>,
}

impl Outputs {
    pub fn configure(&mut self, bindings: &[Binding]) {
        self.entries.retain(|node, entry| {
            bindings
                .iter()
                .any(|binding| binding.node == *node && binding.target == entry.target)
        });
        for binding in bindings {
            let EventOutputTarget::Beast {
                address,
                enabled: true,
            } = &binding.target
            else {
                continue;
            };
            if self.entries.contains_key(&binding.node) {
                continue;
            }
            let (frames, _) = broadcast::channel(QUEUE_CAPACITY);
            let (status, _) = watch::channel(BeastExportStatus {
                node: binding.node.clone(),
                address: address.clone(),
                ..BeastExportStatus::default()
            });
            let listener = std::net::TcpListener::bind(address).and_then(|listener| {
                listener.set_nonblocking(true)?;
                TcpListener::from_std(listener)
            });
            let worker = match listener {
                Ok(listener) => {
                    status.send_modify(|status| status.listening = true);
                    Some(tokio::spawn(serve(
                        listener,
                        frames.clone(),
                        status.clone(),
                    )))
                }
                Err(error) => {
                    report(&status, format!("Cannot listen on {address}: {error}"));
                    None
                }
            };
            self.entries.insert(
                binding.node.clone(),
                Entry {
                    target: binding.target.clone(),
                    frames,
                    status,
                    worker,
                },
            );
        }
    }

    pub fn push(&self, record: &DecodedRecord) {
        let DecoderEvent::Adsb(message) = &record.event else {
            return;
        };
        for node in &record.sinks {
            let Some(entry) = self.entries.get(node) else {
                continue;
            };
            if entry.frames.receiver_count() == 0 {
                continue;
            }
            match encode(message) {
                Ok(frame) => {
                    let _ = entry.frames.send(frame.into());
                }
                Err(error) => report(&entry.status, error.to_owned()),
            }
        }
    }

    pub fn lost(&self, count: u64) {
        for entry in self.entries.values() {
            report(&entry.status, format!("Lost {count} decoder events"));
        }
    }

    pub fn publish_status(&self, engine: &Engine) {
        for entry in self.entries.values() {
            engine.emit_event(ServerEvent::BeastExportStatus(
                entry.status.borrow().clone(),
            ));
        }
    }
}

fn report(status: &watch::Sender<BeastExportStatus>, error: String) {
    tracing::error!(output = %status.borrow().node, %error, "Beast export failed");
    status.send_modify(|status| status.error = Some(error));
}

async fn serve(
    listener: TcpListener,
    frames: broadcast::Sender<Arc<[u8]>>,
    status: watch::Sender<BeastExportStatus>,
) {
    let mut clients = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    if clients.len() == MAX_CLIENTS {
                        report(&status, "Beast client limit reached".to_owned());
                        continue;
                    }
                    if let Err(error) = stream.set_nodelay(true) {
                        report(&status, format!("Beast client setup failed: {error}"));
                        continue;
                    }
                    clients.spawn(write_client(stream, frames.subscribe(), status.clone()));
                    status.send_modify(|status| status.clients = clients.len() as u32);
                }
                Err(error) => {
                    report(&status, format!("Beast accept failed: {error}"));
                    status.send_modify(|status| { status.listening = false; status.clients = 0; });
                    return;
                }
            },
            result = clients.join_next(), if !clients.is_empty() => {
                if let Some(Err(error)) = result { report(&status, format!("Beast client worker failed: {error}")); }
                status.send_modify(|status| status.clients = clients.len() as u32);
            }
        }
    }
}

async fn write_client(
    mut stream: TcpStream,
    mut frames: broadcast::Receiver<Arc<[u8]>>,
    status: watch::Sender<BeastExportStatus>,
) {
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    heartbeat.reset();
    loop {
        let (bytes, frame) = tokio::select! {
            received = frames.recv() => match received {
                Ok(bytes) => (bytes, true),
                Err(broadcast::error::RecvError::Lagged(count)) => {
                    report(&status, format!("Slow Beast client disconnected: lost {count} frames"));
                    return;
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            _ = heartbeat.tick() => (Arc::from(HEARTBEAT), false),
        };
        match tokio::time::timeout(Duration::from_secs(2), stream.write_all(&bytes)).await {
            Ok(Ok(())) => {
                if frame {
                    status.send_modify(|status| status.frames += 1);
                }
            }
            Ok(Err(error)) => {
                report(&status, format!("Beast client disconnected: {error}"));
                return;
            }
            Err(_) => {
                report(
                    &status,
                    "Slow Beast client disconnected: write timed out".to_owned(),
                );
                return;
            }
        }
    }
}

fn encode(message: &AdsbMessage) -> Result<Vec<u8>, &'static str> {
    let kind = match message.raw.len() {
        14 => b'2',
        28 => b'3',
        _ => return Err("Invalid Mode S frame length"),
    };
    let timestamp = message
        .timestamp_12mhz
        .ok_or("ADS-B frame has no sample timestamp")?;
    let mut frame = Vec::with_capacity(44);
    frame.extend_from_slice(&[0x1a, kind]);
    for byte in &timestamp.to_be_bytes()[2..] {
        escape(*byte, &mut frame);
    }
    escape(message.signal_level.unwrap_or(0), &mut frame);
    for pair in message.raw.as_bytes().as_chunks::<2>().0 {
        let high = char::from(pair[0])
            .to_digit(16)
            .ok_or("Invalid Mode S hex")?;
        let low = char::from(pair[1])
            .to_digit(16)
            .ok_or("Invalid Mode S hex")?;
        escape((high * 16 + low) as u8, &mut frame);
    }
    Ok(frame)
}

fn escape(byte: u8, output: &mut Vec<u8>) {
    output.push(byte);
    if byte == 0x1a {
        output.push(byte);
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncReadExt;

    use super::*;

    fn message() -> AdsbMessage {
        AdsbMessage {
            raw: "8D40621D58C382D690C8AC2863A7".to_owned(),
            timestamp_12mhz: Some(0x0102_0304_0506),
            signal_level: Some(128),
            ..AdsbMessage::default()
        }
    }

    #[test]
    fn encodes_known_long_and_short_frames_and_escapes_every_payload_field() {
        assert_eq!(
            encode(&message()).unwrap(),
            [
                0x1a, b'3', 1, 2, 3, 4, 5, 6, 128, 0x8d, 0x40, 0x62, 0x1d, 0x58, 0xc3, 0x82, 0xd6,
                0x90, 0xc8, 0xac, 0x28, 0x63, 0xa7
            ]
        );
        let short = AdsbMessage {
            raw: "1a000000000000".to_owned(),
            timestamp_12mhz: Some(0x1a),
            signal_level: Some(0x1a),
            ..message()
        };
        assert_eq!(
            encode(&short).unwrap(),
            [
                0x1a, b'2', 0, 0, 0, 0, 0, 0x1a, 0x1a, 0x1a, 0x1a, 0x1a, 0x1a, 0, 0, 0, 0, 0, 0
            ]
        );
        for raw in [
            "",
            "8D40621D58C382D690C8AC2863AZ",
            "8D40621D58C382D690C8AC2863",
        ] {
            assert!(
                encode(&AdsbMessage {
                    raw: raw.to_owned(),
                    ..message()
                })
                .is_err()
            );
        }
        assert!(
            encode(&AdsbMessage {
                timestamp_12mhz: None,
                ..message()
            })
            .is_err()
        );
    }

    #[tokio::test]
    async fn only_wired_adsb_frames_reach_clients_and_removal_closes_them() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (frames, _) = broadcast::channel(QUEUE_CAPACITY);
        let (status, mut observed) = watch::channel(BeastExportStatus::default());
        let worker = tokio::spawn(serve(listener, frames.clone(), status.clone()));
        let mut outputs = Outputs::default();
        outputs.entries.insert(
            "beast".to_owned(),
            Entry {
                target: EventOutputTarget::Beast {
                    address: address.to_string(),
                    enabled: true,
                },
                frames,
                status,
                worker: Some(worker),
            },
        );
        let mut first = TcpStream::connect(address).await.unwrap();
        let mut second = TcpStream::connect(address).await.unwrap();
        tokio::time::timeout(
            Duration::from_secs(2),
            observed.wait_for(|status| status.clients == 2),
        )
        .await
        .unwrap()
        .unwrap();
        let mut record = DecodedRecord {
            origin: None,
            sinks: vec!["elsewhere".to_owned()],
            device_set: 1,
            channel: 2,
            at: "2026-09-22T00:00:00Z".to_owned(),
            freq_hz: 1_090_000_000.0,
            event: DecoderEvent::Adsb(message()),
        };
        outputs.push(&record);
        assert!(
            tokio::time::timeout(Duration::from_millis(30), first.read_u8())
                .await
                .is_err()
        );
        record.sinks = vec!["beast".to_owned()];
        outputs.push(&record);
        let expected = encode(&message()).unwrap();
        for client in [&mut first, &mut second] {
            let mut bytes = vec![0; expected.len()];
            tokio::time::timeout(Duration::from_secs(2), client.read_exact(&mut bytes))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(bytes, expected);
        }
        outputs.configure(&[]);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), first.read_u8())
                .await
                .unwrap()
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::UnexpectedEof
        );
    }

    #[tokio::test]
    async fn a_lagged_client_is_disconnected_with_visible_loss() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let (frames, receiver) = broadcast::channel(1);
        let (status, _) = watch::channel(BeastExportStatus::default());
        frames.send(Arc::from([1u8])).unwrap();
        frames.send(Arc::from([2u8])).unwrap();
        tokio::time::timeout(
            Duration::from_secs(2),
            write_client(server, receiver, status.clone()),
        )
        .await
        .unwrap();
        assert!(
            status
                .borrow()
                .error
                .as_ref()
                .unwrap()
                .contains("lost 1 frames")
        );
        drop(client);
    }

    #[tokio::test]
    async fn bind_failure_and_decoder_loss_are_visible_in_status() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let mut outputs = Outputs::default();
        outputs.configure(&[Binding {
            node: "busy".to_owned(),
            target: EventOutputTarget::Beast {
                address,
                enabled: true,
            },
        }]);
        assert!(!outputs.entries["busy"].status.borrow().listening);
        assert!(
            outputs.entries["busy"]
                .status
                .borrow()
                .error
                .as_ref()
                .unwrap()
                .contains("Cannot listen")
        );
        outputs.lost(3);
        assert!(
            outputs.entries["busy"]
                .status
                .borrow()
                .error
                .as_ref()
                .unwrap()
                .contains("Lost 3")
        );
    }
}
