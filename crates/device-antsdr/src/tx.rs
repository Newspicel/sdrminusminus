use std::{
    net::UdpSocket,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use sdrmm_device::{DeviceError, Direction, DuplexState, Sample, TxStream, lock};

use crate::{
    board::{Board, Timeline},
    chdr::{self, Header, Kind},
    link::{self, MTU, Ports, Received},
    radio::TX_STREAM_IDS,
};

pub(crate) const PACKET_SAMPLES: usize = (MTU - 16) / chdr::WORD;
const RECEIVE_BUFFER: usize = 1 << 20;
const DEVICE_BUFFER_BYTES: usize = 1_000_000;
const WINDOW: u16 = (DEVICE_BUFFER_BYTES / (PACKET_SAMPLES * chdr::WORD)) as u16;
const UNDERFLOW: u8 = 0x02 | 0x10;
const SEQUENCE_ERROR: u8 = 0x04 | 0x20;
const TIME_ERROR: u8 = 0x08;

struct Lane {
    socket: UdpSocket,
    sid: u32,
    next: u16,
    acked: u16,
    underflows: u64,
}

impl Lane {
    fn in_flight(&self) -> u16 {
        chdr::seq_distance(self.acked, self.next)
    }

    fn drain_acks(&mut self, wait: Duration) -> Result<bool, DeviceError> {
        let mut buf = [0u8; MTU];
        let mut heard = false;
        let mut wait = wait;
        while let Received::Got(n) = link::receive(&self.socket, &mut buf, wait)? {
            wait = Duration::from_millis(1);
            let Ok(packet) = chdr::read(&buf[..n]) else {
                continue;
            };
            if packet.header.kind != Kind::Context || packet.header.sid != chdr::flip(self.sid) {
                continue;
            }
            let word = chdr::word(packet.payload, 0);
            let code = (word | word.swap_bytes()) as u8;
            self.note(code);
            self.acked = (chdr::word(packet.payload, 1) & 0x0fff) as u16;
            heard = true;
        }
        Ok(heard)
    }

    fn note(&mut self, code: u8) {
        if code & UNDERFLOW != 0 {
            self.underflows += 1;
            tracing::trace!(underflows = self.underflows, "antsdr transmit underflow");
        }
        if code & SEQUENCE_ERROR != 0 {
            tracing::warn!("the radio lost transmit packets");
        }
        if code & TIME_ERROR != 0 {
            tracing::debug!("a transmit burst arrived late");
        }
    }
}

pub(crate) struct AntsdrTx {
    lanes: Vec<Lane>,
    board: Arc<Mutex<Board>>,
    timeline: Arc<Timeline>,
    duplex: Arc<Mutex<DuplexState>>,
    packet: Vec<u8>,
    open: bool,
}

impl AntsdrTx {
    pub(crate) fn open(
        host: &str,
        ports: Ports,
        lanes: usize,
        board: Arc<Mutex<Board>>,
        duplex: Arc<Mutex<DuplexState>>,
    ) -> Result<Self, DeviceError> {
        let timeline = {
            let mut board = lock(&board);
            board.set_transmitting(lanes)?;
            board.timeline()
        };
        let sockets = (0..lanes)
            .map(|lane| {
                link::connect(host, ports.tx(lane), RECEIVE_BUFFER).map(|socket| Lane {
                    socket,
                    sid: TX_STREAM_IDS[lane],
                    next: 0,
                    acked: 0x0fff,
                    underflows: 0,
                })
            })
            .collect::<Result<Vec<_>, _>>();
        let sockets = match sockets {
            Ok(sockets) => sockets,
            Err(e) => {
                let _ = lock(&board).set_transmitting(0);
                return Err(e);
            }
        };
        Ok(Self {
            lanes: sockets,
            board,
            timeline,
            duplex,
            packet: vec![0; MTU],
            open: true,
        })
    }

    fn send(&mut self, lane: usize, samples: &[Sample], eob: bool) -> Result<(), DeviceError> {
        let scale = self.timeline.tx_scale();
        let lane = &mut self.lanes[lane];
        let header = Header {
            kind: Kind::Data,
            seq: lane.next,
            eob,
            sid: lane.sid,
            time: None,
        };
        let start = header.write(samples.len() * chdr::WORD, &mut self.packet)?;
        for (slot, sample) in samples.iter().enumerate() {
            chdr::put(&mut self.packet[start..], slot, encode(*sample, scale));
        }
        let length = start + samples.len() * chdr::WORD;
        lane.socket.send(&self.packet[..length]).map_err(link::io)?;
        lane.next = chdr::next_seq(lane.next);
        Ok(())
    }

    fn room(&mut self, deadline: Instant) -> Result<bool, DeviceError> {
        for lane in &mut self.lanes {
            lane.drain_acks(Duration::ZERO)?;
            while lane.in_flight() >= WINDOW {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Ok(false);
                }
                lane.drain_acks(left.min(Duration::from_millis(20)))?;
            }
        }
        Ok(true)
    }

    fn release(&mut self) {
        if !self.open {
            return;
        }
        self.open = false;
        for lane in 0..self.lanes.len() {
            if let Err(e) = self.send(lane, &[], true) {
                tracing::debug!("antsdr end of burst: {e}");
            }
        }
        if let Err(e) = lock(&self.board).set_transmitting(0) {
            tracing::warn!("antsdr transmit stop: {e}");
        }
        lock(&self.duplex).release(Direction::Tx);
    }
}

pub(crate) fn encode(sample: Sample, scale: f32) -> u32 {
    let part = |value: f32| (value * scale).round().clamp(-32768.0, 32767.0) as i16 as u16;
    u32::from(part(sample.re)) << 16 | u32::from(part(sample.im))
}

impl TxStream for AntsdrTx {
    fn write_channels(
        &mut self,
        channels: &[&[Sample]],
        timeout: Duration,
        end_burst: bool,
    ) -> Result<usize, DeviceError> {
        if !self.open {
            return Err(DeviceError::Io("transmit stream is stopped".to_string()));
        }
        if channels.len() != self.lanes.len() {
            return Err(DeviceError::Unsupported(format!(
                "this stream transmits on {} lanes, got {}",
                self.lanes.len(),
                channels.len()
            )));
        }
        let span = channels.iter().map(|lane| lane.len()).min().unwrap_or(0);
        let deadline = Instant::now() + timeout;
        let mut written = 0;
        while written < span {
            if !self.room(deadline)? {
                break;
            }
            let end = (written + PACKET_SAMPLES).min(span);
            let last = end == span && end_burst;
            for (lane, samples) in channels.iter().enumerate() {
                self.send(lane, &samples[written..end], last)?;
            }
            written = end;
        }
        if span == 0 && end_burst {
            for lane in 0..self.lanes.len() {
                self.send(lane, &[], true)?;
            }
        }
        Ok(written)
    }

    fn stop(&mut self) -> Result<(), DeviceError> {
        self.release();
        Ok(())
    }
}

impl Drop for AntsdrTx {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rx::decode;

    #[test]
    fn a_sample_is_packed_in_phase_high_quadrature_low() {
        let word = encode(Sample::new(0.5, -0.25), 32767.0);
        assert_eq!(word >> 16, 16384);
        assert_eq!(word as u16 as i16, -8192);
        let back = decode(word.to_le_bytes(), 1.0 / 32767.0);
        assert!((back.re - 0.5).abs() < 1e-3 && (back.im + 0.25).abs() < 1e-3);
    }

    #[test]
    fn full_scale_is_clipped_not_wrapped() {
        let word = encode(Sample::new(2.0, -2.0), 32767.0);
        assert_eq!((word >> 16) as u16 as i16, i16::MAX);
        assert_eq!(word as u16 as i16, i16::MIN);
    }

    #[test]
    fn the_window_fits_the_radio_buffer() {
        assert_eq!(PACKET_SAMPLES, 364);
        assert_eq!(WINDOW, 686);
    }
}
