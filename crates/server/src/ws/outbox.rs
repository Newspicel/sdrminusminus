use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::extract::ws::Message;
use sdrmm_wire::FrameKind;
use tokio::sync::Notify;

const CONTROL_LIMIT: usize = 64;
const DECODED_LIMIT: usize = 1024;
const BYTE_LIMIT: usize = 16 * 1024 * 1024;
const FRAME_LIMIT: usize = 8 * 1024 * 1024;
const AUDIO_AGE: Duration = Duration::from_millis(100);
const MEDIA_AGE: Duration = Duration::from_millis(250);
const AUDIO_LIMIT: usize = 256;
const MEDIA_LIMIT: usize = 128;
pub(super) const WRITE_TIMEOUT: Duration = Duration::from_millis(250);
const STREAM_STOPPED_PREFIX: &str = r#"{"type":"StreamStopped""#;

struct Packet {
    message: Message,
    queued: Instant,
    key: Option<(u8, u16)>,
    barrier: Option<u16>,
    lossy: bool,
}

#[derive(Default)]
struct Queue {
    control: VecDeque<Packet>,
    decoded: VecDeque<Packet>,
    audio: VecDeque<Packet>,
    media: VecDeque<Packet>,
    bytes: usize,
    dropped: u64,
    lost: u64,
    closed: bool,
    senders: usize,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    ready: Notify,
}

pub(super) struct Outbox(Arc<Shared>);
pub(super) struct Output(Arc<Shared>);

pub(super) fn channel() -> (Outbox, Output) {
    let shared = Arc::new(Shared::default());
    shared
        .queue
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .senders = 1;
    (Outbox(shared.clone()), Output(shared))
}

fn packet(message: Message) -> Packet {
    let key = match &message {
        Message::Binary(bytes) if bytes.len() >= 4 => {
            Some((bytes[1], u16::from_le_bytes([bytes[2], bytes[3]])))
        }
        _ => None,
    };
    let barrier = match &message {
        Message::Text(text) if text.starts_with(STREAM_STOPPED_PREFIX) => {
            match serde_json::from_str::<sdrmm_wire::ServerEvent>(text.as_str()) {
                Ok(sdrmm_wire::ServerEvent::StreamStopped { stream_id, .. }) => Some(stream_id),
                _ => None,
            }
        }
        _ => None,
    };
    Packet {
        message,
        queued: Instant::now(),
        key,
        barrier,
        lossy: false,
    }
}

fn decoded_packet(message: Message) -> Packet {
    Packet {
        lossy: true,
        ..packet(message)
    }
}

impl Packet {
    fn len(&self) -> usize {
        match &self.message {
            Message::Text(text) => text.len(),
            Message::Binary(bytes) | Message::Ping(bytes) | Message::Pong(bytes) => bytes.len(),
            Message::Close(_) => 128,
        }
    }
}

impl Queue {
    fn expire(&mut self, now: Instant) {
        for (queue, age) in [(&mut self.audio, AUDIO_AGE), (&mut self.media, MEDIA_AGE)] {
            while queue
                .front()
                .is_some_and(|p| now.duration_since(p.queued) > age)
            {
                if let Some(p) = queue.pop_front() {
                    self.bytes -= p.len();
                    self.dropped += 1;
                }
            }
        }
    }

    fn push(&mut self, packet: Packet) -> Result<(), ()> {
        if self.closed {
            return Err(());
        }
        self.expire(packet.queued);
        if packet.lossy {
            self.push_lossy(packet);
            return Ok(());
        }
        let len = packet.len();
        if packet.key.is_none() {
            if self.control.len() >= CONTROL_LIMIT || len > BYTE_LIMIT {
                self.closed = true;
                return Err(());
            }
            while self.bytes + len > BYTE_LIMIT {
                let Some(old) = self.media.pop_front().or_else(|| self.audio.pop_front()) else {
                    self.closed = true;
                    return Err(());
                };
                self.bytes -= old.len();
                self.dropped += 1;
            }
            self.bytes += len;
            self.control.push_back(packet);
            return Ok(());
        }
        let audio = packet
            .key
            .is_some_and(|(kind, _)| kind == FrameKind::AudioOpus as u8);
        if !audio
            && let Some(index) = self.media.iter().position(|old| old.key == packet.key)
            && let Some(old) = self.media.remove(index)
        {
            self.bytes -= old.len();
            self.dropped += 1;
        }
        let queue = if audio {
            &mut self.audio
        } else {
            &mut self.media
        };
        let limit = if audio { AUDIO_LIMIT } else { MEDIA_LIMIT };
        if len > FRAME_LIMIT || self.bytes + len > BYTE_LIMIT || queue.len() >= limit {
            self.dropped += 1;
            return Ok(());
        }
        self.bytes += len;
        queue.push_back(packet);
        Ok(())
    }

    fn push_lossy(&mut self, packet: Packet) {
        let len = packet.len();
        while self.decoded.len() >= DECODED_LIMIT || self.bytes + len > BYTE_LIMIT {
            let Some(old) = self.decoded.pop_front() else {
                self.forget_lossy();
                return;
            };
            self.bytes -= old.len();
            self.forget_lossy();
        }
        self.bytes += len;
        self.decoded.push_back(packet);
    }

    fn forget_lossy(&mut self) {
        self.dropped += 1;
        self.lost += 1;
    }

    fn pop(&mut self) -> Option<Message> {
        self.expire(Instant::now());
        if self.lost > 0 {
            let count = std::mem::take(&mut self.lost);
            return Some(super::text_event(&sdrmm_wire::ServerEvent::DecodedLost {
                count,
            }));
        }
        let control = self.control.iter().position(|p| {
            p.barrier.is_none_or(|id| {
                !self
                    .audio
                    .iter()
                    .chain(self.media.iter())
                    .any(|m| m.key.is_some_and(|(_, stream)| stream == id))
            })
        });
        let packet = control
            .and_then(|index| self.control.remove(index))
            .or_else(|| self.audio.pop_front())
            .or_else(|| self.decoded.pop_front())
            .or_else(|| self.media.pop_front())?;
        self.bytes -= packet.len();
        Some(packet.message)
    }
}

impl Outbox {
    pub(super) fn dropped(&self, count: u64) {
        self.0
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .dropped += count;
    }

    pub(super) fn health(&self) -> sdrmm_wire::QueueHealth {
        let queue = self
            .0
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let oldest = queue
            .control
            .iter()
            .chain(queue.decoded.iter())
            .chain(queue.audio.iter())
            .chain(queue.media.iter())
            .map(|packet| packet.queued)
            .min();
        sdrmm_wire::QueueHealth {
            queued: queue.bytes as u64,
            capacity: BYTE_LIMIT as u64,
            oldest_ms: oldest.map_or(0.0, |time| time.elapsed().as_secs_f64() * 1000.0),
            dropped: queue.dropped,
        }
    }

    pub(super) async fn send(&self, message: Message) -> Result<(), ()> {
        self.enqueue(packet(message))
    }

    pub(super) async fn send_decoded(&self, message: Message) -> Result<(), ()> {
        self.enqueue(decoded_packet(message))
    }

    fn enqueue(&self, packet: Packet) -> Result<(), ()> {
        let result = self
            .0
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(packet);
        self.0.ready.notify_one();
        result
    }
}

impl Output {
    pub(super) async fn recv(&mut self) -> Option<Message> {
        loop {
            let ready = self.0.ready.notified();
            {
                let mut queue = self
                    .0
                    .queue
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if queue.closed {
                    return None;
                }
                if let Some(message) = queue.pop() {
                    return Some(message);
                }
                if queue.senders == 0 {
                    return None;
                }
            }
            ready.await;
        }
    }

    #[cfg(test)]
    pub(super) fn try_recv(&mut self) -> Result<Message, ()> {
        self.0
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop()
            .ok_or(())
    }
}

impl Clone for Outbox {
    fn clone(&self) -> Self {
        self.0
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .senders += 1;
        Self(self.0.clone())
    }
}

impl Drop for Outbox {
    fn drop(&mut self) {
        self.0
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .senders -= 1;
        self.0.ready.notify_one();
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.0
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed = true;
        self.0.ready.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(kind: FrameKind, id: u16, value: u8) -> Message {
        Message::Binary(vec![1, kind as u8, id as u8, (id >> 8) as u8, value].into())
    }

    #[tokio::test]
    async fn control_precedes_audio_and_visuals_keep_only_the_latest_per_stream() {
        let (tx, mut rx) = channel();
        tx.send(media(FrameKind::Spectrum, 1, 1))
            .await
            .expect("send");
        tx.send(media(FrameKind::Spectrum, 1, 2))
            .await
            .expect("replace");
        tx.send(media(FrameKind::AudioOpus, 2, 3))
            .await
            .expect("audio");
        tx.send(Message::Text("control".into()))
            .await
            .expect("control");
        assert_eq!(rx.recv().await, Some(Message::Text("control".into())));
        assert_eq!(rx.recv().await, Some(media(FrameKind::AudioOpus, 2, 3)));
        assert_eq!(rx.recv().await, Some(media(FrameKind::Spectrum, 1, 2)));
        assert!(rx.try_recv().is_err());
    }

    fn stream_stopped(stream_id: u16) -> Message {
        let event = sdrmm_wire::ServerEvent::StreamStopped {
            stream_id,
            kind: sdrmm_wire::StreamKind::Spectrum,
        };
        Message::Text(serde_json::to_string(&event).expect("json").into())
    }

    #[tokio::test]
    async fn a_stream_stops_behind_its_own_frames_and_ahead_of_everyone_else_s() {
        let (tx, mut rx) = channel();
        tx.send(media(FrameKind::Spectrum, 7, 1))
            .await
            .expect("send");
        tx.send(media(FrameKind::Spectrum, 8, 2))
            .await
            .expect("send");
        tx.send(stream_stopped(7)).await.expect("send");
        tx.send(Message::Text("control".into()))
            .await
            .expect("send");

        assert_eq!(rx.recv().await, Some(Message::Text("control".into())));
        assert_eq!(rx.recv().await, Some(media(FrameKind::Spectrum, 7, 1)));
        assert_eq!(rx.recv().await, Some(stream_stopped(7)));
        assert_eq!(rx.recv().await, Some(media(FrameKind::Spectrum, 8, 2)));
    }

    #[test]
    fn stale_audio_is_discarded_and_control_congestion_closes_the_connection() {
        let mut queue = Queue::default();
        let mut old = packet(media(FrameKind::AudioOpus, 1, 0));
        old.queued -= AUDIO_AGE * 2;
        queue.push(old).expect("audio");
        assert!(queue.pop().is_none());
        assert_eq!(queue.dropped, 1);
        for _ in 0..CONTROL_LIMIT {
            queue
                .push(packet(Message::Text("x".into())))
                .expect("control");
        }
        assert!(
            queue
                .push(packet(Message::Text("overflow".into())))
                .is_err()
        );
        assert!(queue.closed);
    }

    fn decoded(id: u8) -> Message {
        Message::Text(format!(r#"{{"type":"Decoded","data":{id}}}"#).into())
    }

    #[test]
    fn a_decoded_burst_sheds_its_oldest_records_instead_of_closing_the_connection() {
        let mut queue = Queue::default();
        let overshoot = 5;
        for id in 0..DECODED_LIMIT + overshoot {
            queue
                .push(decoded_packet(decoded(id as u8)))
                .expect("decoded records never congest the connection");
        }
        assert!(!queue.closed);

        let lost = sdrmm_wire::ServerEvent::DecodedLost {
            count: overshoot as u64,
        };
        assert_eq!(queue.pop(), Some(super::super::text_event(&lost)));
        assert_eq!(queue.pop(), Some(decoded(overshoot as u8)));
    }

    #[test]
    fn decoded_records_wait_behind_control_and_audio_but_lead_the_visuals() {
        let mut queue = Queue::default();
        queue
            .push(packet(media(FrameKind::Spectrum, 1, 1)))
            .expect("media");
        queue.push(decoded_packet(decoded(9))).expect("decoded");
        queue
            .push(packet(media(FrameKind::AudioOpus, 2, 2)))
            .expect("audio");
        queue
            .push(packet(Message::Text("control".into())))
            .expect("control");

        assert_eq!(queue.pop(), Some(Message::Text("control".into())));
        assert_eq!(queue.pop(), Some(media(FrameKind::AudioOpus, 2, 2)));
        assert_eq!(queue.pop(), Some(decoded(9)));
        assert_eq!(queue.pop(), Some(media(FrameKind::Spectrum, 1, 1)));
    }

    #[test]
    fn bulky_frames_cannot_exceed_the_byte_budget_or_block_control() {
        let mut queue = Queue::default();
        for id in 0..32 {
            let mut bytes = vec![0; FRAME_LIMIT];
            bytes[..4].copy_from_slice(&[1, FrameKind::VideoRgb as u8, id, 0]);
            queue
                .push(packet(Message::Binary(bytes.into())))
                .expect("nonblocking");
            assert!(queue.bytes <= BYTE_LIMIT);
        }
        queue
            .push(packet(Message::Text("stop".into())))
            .expect("control");
        assert_eq!(queue.pop(), Some(Message::Text("stop".into())));
        assert!(queue.dropped > 0);
    }
}
