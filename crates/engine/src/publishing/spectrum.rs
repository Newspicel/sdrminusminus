use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use tokio::sync::broadcast;

use super::Publisher;
use crate::{runtime::SpectrumSnapshot, spectrum::SpectrumFrame};

struct SpectrumPacket {
    seq: u32,
    frame: SpectrumFrame,
    db: Vec<f32>,
}

pub(crate) struct SpectrumPublisher {
    queue: Publisher<SpectrumPacket>,
    dropped: Arc<AtomicU64>,
}

impl SpectrumPublisher {
    pub(crate) fn new(
        tx: broadcast::Sender<SpectrumSnapshot>,
        size: usize,
    ) -> std::io::Result<Self> {
        let dropped = Arc::new(AtomicU64::new(0));
        let count = dropped.clone();
        let mut seen = 0;
        let queue = Publisher::new(
            "sdrmm-spectrum-publish",
            8,
            || SpectrumPacket {
                seq: 0,
                frame: SpectrumFrame {
                    timestamp: 0,
                    center_hz: 0.0,
                    span_hz: 0.0,
                    lo_hz: 0.0,
                },
                db: vec![0.0; size],
            },
            move |packet| {
                let _ = tx.send(SpectrumSnapshot {
                    seq: packet.seq,
                    timestamp: packet.frame.timestamp,
                    center_hz: packet.frame.center_hz,
                    span_hz: packet.frame.span_hz,
                    lo_hz: packet.frame.lo_hz,
                    db: Arc::from(packet.db.as_slice()),
                });
            },
            move || {
                let now = count.load(Ordering::Relaxed);
                if now != seen {
                    tracing::warn!(
                        dropped = now - seen,
                        total = now,
                        "spectrum publication queue overflow"
                    );
                    seen = now;
                }
            },
        )?;
        Ok(Self { queue, dropped })
    }

    pub(crate) fn publish(&mut self, seq: u32, frame: SpectrumFrame, db: &[f32]) {
        if !self.queue.submit(|packet| {
            packet.seq = seq;
            packet.frame = frame;
            packet.db.copy_from_slice(db);
        }) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}
