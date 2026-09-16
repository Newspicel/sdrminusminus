use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use num_complex::Complex;
use sdrmm_device::{
    Block, CaptureStream, Next, Sample, SampleConverter, StreamFailure,
    net::{Incoming, SocketStop, WebSocket},
};

use crate::proto::{Event, PAYLOAD_PREFIX, Payload, PayloadKind, Property, Tuner, as_bool, decode};

const SCALE: f32 = 1.0 / 32_768.0;

#[derive(Debug, Default)]
pub(crate) struct IqConverter {
    out: Vec<Sample>,
}

impl SampleConverter for IqConverter {
    fn convert(&mut self, bytes: &[u8]) -> &[Sample] {
        self.out.clear();
        let body = bytes.get(PAYLOAD_PREFIX..).unwrap_or_default();
        let (pairs, _) = body.as_chunks::<4>();
        self.out.reserve(pairs.len());
        self.out.extend(pairs.iter().map(|iq| {
            Complex::new(
                f32::from(i16::from_le_bytes([iq[0], iq[1]])) * SCALE,
                f32::from(i16::from_le_bytes([iq[2], iq[3]])) * SCALE,
            )
        }));
        &self.out
    }

    fn reset(&mut self) {}
}

#[derive(Debug, Default)]
struct Noted {
    audio: AtomicBool,
    spectrum: AtomicBool,
    unknown: AtomicBool,
}

impl Noted {
    fn first(&self, kind: PayloadKind) -> bool {
        match kind {
            PayloadKind::Audio => !self.audio.swap(true, Ordering::Relaxed),
            PayloadKind::Spectrum => !self.spectrum.swap(true, Ordering::Relaxed),
            PayloadKind::Iq => false,
        }
    }
}

#[derive(Debug)]
pub(crate) struct SdrConnectStream {
    socket: Arc<WebSocket>,
    tuner: Tuner,
    overloaded: AtomicBool,
    running: AtomicBool,
    noted: Noted,
}

impl SdrConnectStream {
    pub(crate) fn new(socket: Arc<WebSocket>, tuner: Tuner) -> Self {
        Self {
            socket,
            tuner,
            overloaded: AtomicBool::new(false),
            running: AtomicBool::new(false),
            noted: Noted::default(),
        }
    }

    /// Reports a stream the capture asked SDRconnect to switch off and is being sent anyway, so
    /// the link quietly carrying what nothing here reads is visible rather than invisible.
    fn spare(&self, kind: PayloadKind, bytes: usize) -> Next<Block> {
        if self.noted.first(kind) {
            tracing::warn!(
                tuner = self.tuner.name(),
                bytes,
                "SDRconnect is sending {} for this tuner although the stream was switched off; \
                 SDR-- demodulates from the IQ and is discarding it",
                match kind {
                    PayloadKind::Audio => "demodulated audio",
                    PayloadKind::Spectrum => "spectrum bins",
                    PayloadKind::Iq => "IQ",
                }
            );
        }
        Next::Idle
    }

    /// Acts on the events that say something about the samples, and puts the rest where an
    /// operator can read them.
    fn observe(&self, text: &str) -> Next<Block> {
        let Ok(notification) = decode(text) else {
            return Next::Idle;
        };
        if notification.tuner != self.tuner
            || !matches!(
                notification.event,
                Event::PropertyChanged | Event::GetPropertyResponse
            )
        {
            return Next::Idle;
        }
        let Some(property) = notification.property else {
            return Next::Idle;
        };
        let value = notification.value.as_str();
        match property {
            Property::Overload => {
                let overloaded = as_bool(value).unwrap_or(false);
                if self.overloaded.swap(overloaded, Ordering::Relaxed) != overloaded {
                    if overloaded {
                        tracing::warn!(
                            tuner = self.tuner.name(),
                            "SDRconnect reports the receiver's ADC is overloading; reduce the RF gain"
                        );
                    } else {
                        tracing::info!("the SDRconnect receiver is no longer overloading");
                    }
                }
                Next::Idle
            }
            Property::Started => {
                let started = as_bool(value).unwrap_or(false);
                if self.running.swap(started, Ordering::Relaxed) && !started {
                    self.socket
                        .fail("SDRconnect stopped the receiver".to_string());
                    return Next::Ended;
                }
                Next::Idle
            }
            Property::RdsPs | Property::RdsPi | Property::RdsPty | Property::RdsRadiotext => {
                tracing::info!(property = property.name(), value, "SDRconnect RDS");
                Next::Idle
            }
            Property::SignalPower | Property::SignalSnr | Property::WfmStereo => {
                tracing::trace!(property = property.name(), value, "SDRconnect measurement");
                Next::Idle
            }
            property => {
                tracing::debug!(
                    property = property.name(),
                    value,
                    "SDRconnect property changed"
                );
                Next::Idle
            }
        }
    }
}

impl CaptureStream for SdrConnectStream {
    type Block = Block;
    type Stop = SocketStop;

    fn stop_handle(&self) -> SocketStop {
        self.socket.stop_handle()
    }

    fn next_block(&self, timeout: Duration) -> Next<Block> {
        match self.socket.next(timeout) {
            Incoming::Binary(block) => match Payload::split(&block) {
                Some((payload, body)) if payload.tuner == self.tuner => match payload.kind {
                    PayloadKind::Iq => Next::Block(block),
                    kind => self.spare(kind, body.len()),
                },
                Some(_) => Next::Idle,
                None => {
                    if !self.noted.unknown.swap(true, Ordering::Relaxed) {
                        tracing::warn!(
                            "SDRconnect sent a binary payload type SDR-- does not know; skipping it"
                        );
                    }
                    Next::Idle
                }
            },
            Incoming::Text(text) => self.observe(&text),
            Incoming::Idle => Next::Idle,
            Incoming::Ended => Next::Ended,
        }
    }

    fn dropped(&self) -> u64 {
        0
    }

    fn failure(&self) -> StreamFailure {
        self.socket.failure()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(code: u16, samples: &[i16]) -> Vec<u8> {
        let mut bytes = code.to_le_bytes().to_vec();
        bytes.extend(samples.iter().flat_map(|sample| sample.to_le_bytes()));
        bytes
    }

    #[test]
    fn interleaved_int16_reaches_full_scale_at_the_top_of_the_range() {
        let mut converter = IqConverter::default();
        let samples = converter
            .convert(&message(2, &[16_384, -16_384, 32_767, 0]))
            .to_vec();
        assert_eq!(samples.len(), 2);
        assert!((samples[0].re - 0.5).abs() < 1e-6);
        assert!((samples[0].im + 0.5).abs() < 1e-6);
        assert!((samples[1].re - 1.0).abs() < 1e-3);
        assert!(samples[1].im.abs() < 1e-6);
    }

    #[test]
    fn a_message_that_ends_mid_sample_yields_only_whole_ones() {
        let mut converter = IqConverter::default();
        assert_eq!(converter.convert(&message(2, &[1, 2, 3])).len(), 1);
        assert!(converter.convert(&[2, 0, 1]).is_empty());
        assert!(converter.convert(&[]).is_empty(), "not even a payload type");
    }

    #[test]
    fn the_output_buffer_is_reused_across_blocks() {
        let mut converter = IqConverter::default();
        let block = message(2, &vec![0i16; 4096]);
        let first = converter.convert(&block).as_ptr();
        assert_eq!(
            converter.convert(&block).as_ptr(),
            first,
            "the capture thread must not allocate per block"
        );
    }

    #[test]
    fn a_payload_kind_is_reported_once_and_not_on_every_message() {
        let noted = Noted::default();
        assert!(noted.first(PayloadKind::Audio));
        assert!(!noted.first(PayloadKind::Audio));
        assert!(noted.first(PayloadKind::Spectrum));
        assert!(!noted.first(PayloadKind::Spectrum));
        assert!(
            !noted.first(PayloadKind::Iq),
            "the samples this backend is here for are never a surprise"
        );
    }
}
