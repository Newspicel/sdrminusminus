//! The sample half of a SpyServer connection: message framing on the capture thread, and the
//! conversion from whichever quantisation the samples crossed the network in.
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Duration,
};

use num_complex::Complex;
use sdrmm_device::{CaptureStream, Next, Sample, SampleConverter, StreamFailure, lock};

use crate::{
    socket::{Block, BlockPool, Connection, Read, SocketStop},
    spyserver::proto::{HEADER_LEN, IqFormat, MessageHeader, STREAM_TYPE_IQ},
};

/// The largest digital gain worth honouring. The flag is sixteen bits of dB and this backend never
/// asks for more than a few, so anything beyond this is a server that means something else by the
/// field — and dividing by 10^(6000/20) would turn every sample into zero.
const MAX_DIGITAL_GAIN_DB: u16 = 96;

/// How each block is to be read, published by the framer and consumed by the converter.
///
/// One word rather than a lock: it is written once per message on the capture thread and read once
/// per block on the same thread, and packing it keeps the pair — format and the gain that came
/// with *that* format — impossible to tear apart.
#[derive(Clone, Debug, Default)]
pub(crate) struct Coding(Arc<AtomicU32>);

impl Coding {
    fn publish(&self, format: IqFormat, gain_db: u16) {
        self.0.store(
            format.code() | (u32::from(gain_db) << 16),
            Ordering::Relaxed,
        );
    }

    fn read(&self) -> (IqFormat, u16) {
        let word = self.0.load(Ordering::Relaxed);
        let format = match word & 0xFFFF {
            1 => IqFormat::Uint8,
            4 => IqFormat::Float32,
            _ => IqFormat::Int16,
        };
        (format, ((word >> 16) as u16).min(MAX_DIGITAL_GAIN_DB))
    }
}

/// Undo the gain the server applied before quantising.
fn scale(gain_db: u16) -> f32 {
    1.0 / 10f32.powf(f32::from(gain_db) / 20.0)
}

/// SpyServer IQ to `cf32`.
///
/// Allocation-free after the first block, and with no carry byte: unlike a raw byte stream, every
/// block here is exactly one message, and a message contains whole samples — so a block can never
/// end mid-sample and there is nothing to hold back.
#[derive(Debug)]
pub(crate) struct SpyConverter {
    coding: Coding,
    out: Vec<Sample>,
    /// The 8-bit lookup, rebuilt only when the digital gain changes — which is never, in a session
    /// where nothing is retuned.
    table: [f32; 256],
    table_gain: Option<u16>,
}

impl SpyConverter {
    pub(crate) fn new(coding: Coding) -> Self {
        Self {
            coding,
            out: Vec::new(),
            table: [0.0; 256],
            table_gain: None,
        }
    }

    fn uint8_table(&mut self, gain_db: u16) -> &[f32; 256] {
        if self.table_gain != Some(gain_db) {
            let scale = scale(gain_db) / 128.0;
            for (code, value) in self.table.iter_mut().enumerate() {
                *value = (code as f32 - 128.0) * scale;
            }
            self.table_gain = Some(gain_db);
        }
        &self.table
    }
}

impl SampleConverter for SpyConverter {
    fn convert(&mut self, bytes: &[u8]) -> &[Sample] {
        let (format, gain_db) = self.coding.read();
        self.out.clear();
        self.out.reserve(bytes.len() / format.sample_bytes());
        match format {
            IqFormat::Uint8 => {
                let table = *self.uint8_table(gain_db);
                let (pairs, _) = bytes.as_chunks::<2>();
                self.out.extend(
                    pairs
                        .iter()
                        .map(|iq| Complex::new(table[iq[0] as usize], table[iq[1] as usize])),
                );
            }
            IqFormat::Int16 => {
                // Full scale is 32 768, so a 16-bit sample divides by that and by the server's gain.
                let scale = scale(gain_db) / 32_768.0;
                let (pairs, _) = bytes.as_chunks::<4>();
                self.out.extend(pairs.iter().map(|iq| {
                    Complex::new(
                        f32::from(i16::from_le_bytes([iq[0], iq[1]])) * scale,
                        f32::from(i16::from_le_bytes([iq[2], iq[3]])) * scale,
                    )
                }));
            }
            IqFormat::Float32 => {
                let scale = scale(gain_db);
                let (pairs, _) = bytes.as_chunks::<8>();
                self.out.extend(pairs.iter().map(|iq| {
                    Complex::new(
                        f32::from_le_bytes([iq[0], iq[1], iq[2], iq[3]]) * scale,
                        f32::from_le_bytes([iq[4], iq[5], iq[6], iq[7]]) * scale,
                    )
                }));
            }
        }
        &self.out
    }

    /// Nothing to forget: every block is a whole message.
    fn reset(&mut self) {}
}

/// Where the framer is in the message it is reading.
#[derive(Debug)]
enum Phase {
    Header {
        bytes: [u8; HEADER_LEN],
        got: usize,
    },
    Body {
        header: MessageHeader,
        block: Block,
        got: usize,
    },
}

impl Phase {
    fn header() -> Self {
        Self::Header {
            bytes: [0u8; HEADER_LEN],
            got: 0,
        }
    }
}

/// A connection being drained for IQ messages.
#[derive(Debug)]
pub(crate) struct SpyStream {
    connection: Arc<Connection>,
    pool: BlockPool,
    coding: Coding,
    /// Whether an IQ message this backend cannot decode has already been reported. Without it, a
    /// server sending `INT24_IQ` would present as a stream that simply goes quiet.
    warned: AtomicBool,
    /// Touched only by the capture thread; a mutex because the trait's `next_block` takes `&self`.
    phase: Mutex<Phase>,
}

impl SpyStream {
    pub(crate) fn new(connection: Arc<Connection>, pool: BlockPool, coding: Coding) -> Self {
        Self {
            connection,
            pool,
            coding,
            warned: AtomicBool::new(false),
            phase: Mutex::new(Phase::header()),
        }
    }
}

impl CaptureStream for SpyStream {
    type Block = Block;
    type Stop = SocketStop;

    fn stop_handle(&self) -> SocketStop {
        self.connection.stop_handle()
    }

    /// One read per call, advancing the frame by whatever arrived. Anything but a completed IQ
    /// message reads as [`Next::Idle`], which is also what a run of status messages with no samples
    /// behind them looks like — and the supervisor's silence timeout is right to fault that.
    fn next_block(&self, timeout: Duration) -> Next<Block> {
        let mut phase = lock(&self.phase);
        match &mut *phase {
            Phase::Header { bytes, got } => {
                match self.connection.read(&mut bytes[*got..], timeout) {
                    Read::Got(n) => *got += n,
                    Read::Idle => return Next::Idle,
                    Read::Ended => return Next::Ended,
                }
                if *got < HEADER_LEN {
                    return Next::Idle;
                }
                let header = match MessageHeader::parse(bytes) {
                    Ok(header) => header,
                    // A frame that cannot be parsed leaves the byte stream at an unknown offset;
                    // only a fresh connection can recover, which is what ending it asks for.
                    Err(e) => {
                        self.connection.fail(e.to_string());
                        return Next::Ended;
                    }
                };
                *phase = if header.body_size == 0 {
                    Phase::header()
                } else {
                    Phase::Body {
                        header,
                        block: self.pool.take(header.body_size as usize),
                        got: 0,
                    }
                };
                Next::Idle
            }
            Phase::Body { header, block, got } => {
                match self.connection.read(&mut block.as_mut()[*got..], timeout) {
                    Read::Got(n) => *got += n,
                    Read::Idle => return Next::Idle,
                    Read::Ended => return Next::Ended,
                }
                if *got < block.len() {
                    return Next::Idle;
                }
                let format = (header.stream_type == STREAM_TYPE_IQ)
                    .then(|| IqFormat::from_message_type(header.kind))
                    .flatten();
                match format {
                    Some(format) => self.coding.publish(format, header.flags),
                    None if header.stream_type == STREAM_TYPE_IQ
                        && !self.warned.swap(true, Ordering::Relaxed) =>
                    {
                        // Otherwise this presents as a stream that simply goes quiet, and the
                        // supervisor's silence timeout names the symptom rather than the cause.
                        tracing::warn!(
                            message_type = header.kind,
                            "SpyServer is sending IQ in a format sdr-- cannot decode; skipping it"
                        );
                    }
                    None => {}
                }
                let iq = format.is_some();
                let Phase::Body { block, .. } = std::mem::replace(&mut *phase, Phase::header())
                else {
                    unreachable!("the phase was matched as a body")
                };
                // Everything else — the device info and client sync a fresh connection opens with,
                // a pong, an FFT frame from a server that was left in another mode — is read to
                // its end and dropped, which is what keeps the stream framed.
                if iq { Next::Block(block) } else { Next::Idle }
            }
        }
    }

    /// Always zero. This is a TCP stream: nothing is lost in flight, and a server that cannot keep
    /// up stops sending rather than skipping — which arrives as the end of the stream and is
    /// counted as a restart, not as dropped blocks.
    fn dropped(&self) -> u64 {
        0
    }

    fn failure(&self) -> StreamFailure {
        self.connection.failure()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn converted(format: IqFormat, gain_db: u16, bytes: &[u8]) -> Vec<Sample> {
        let coding = Coding::default();
        coding.publish(format, gain_db);
        SpyConverter::new(coding).convert(bytes).to_vec()
    }

    #[test]
    fn uint8_is_offset_binary_around_128() {
        let samples = converted(IqFormat::Uint8, 0, &[128, 255, 0, 128]);
        assert_eq!(samples.len(), 2);
        assert!((samples[0].re - 0.0).abs() < 1e-6);
        assert!((samples[0].im - 0.9921875).abs() < 1e-6);
        assert!((samples[1].re + 1.0).abs() < 1e-6);
    }

    #[test]
    fn int16_and_float_reach_the_same_full_scale() {
        let int16 = converted(IqFormat::Int16, 0, &16_384i16.to_le_bytes().repeat(2));
        assert!((int16[0].re - 0.5).abs() < 1e-6);
        assert!((int16[0].im - 0.5).abs() < 1e-6);
        let float = converted(IqFormat::Float32, 0, &0.5f32.to_le_bytes().repeat(2));
        assert!((float[0].re - 0.5).abs() < 1e-6);
    }

    /// The header's flags say how far the server scaled the samples up before quantising; a client
    /// that ignored them would report every level 6 dB high.
    #[test]
    fn the_servers_digital_gain_is_divided_back_out() {
        let plain = converted(IqFormat::Int16, 0, &16_384i16.to_le_bytes().repeat(2));
        let scaled = converted(IqFormat::Int16, 6, &16_384i16.to_le_bytes().repeat(2));
        assert!(scaled[0].re < plain[0].re);
        assert!((scaled[0].re - plain[0].re / 10f32.powf(0.3)).abs() < 1e-6);

        let plain = converted(IqFormat::Uint8, 0, &[255, 128]);
        let scaled = converted(IqFormat::Uint8, 6, &[255, 128]);
        assert!((scaled[0].re - plain[0].re / 10f32.powf(0.3)).abs() < 1e-6);
    }

    /// A server that reports a gain this backend never asked for must not silently mute the radio.
    #[test]
    fn an_absurd_digital_gain_is_clamped_rather_than_believed() {
        let samples = converted(
            IqFormat::Int16,
            u16::MAX,
            &16_384i16.to_le_bytes().repeat(2),
        );
        assert!(samples[0].re > 0.0, "clamped to something representable");
    }

    #[test]
    fn a_body_that_ends_mid_sample_yields_only_whole_ones() {
        assert_eq!(converted(IqFormat::Uint8, 0, &[128, 200, 64]).len(), 1);
        assert_eq!(converted(IqFormat::Int16, 0, &[0, 1, 2]).len(), 0);
    }

    #[test]
    fn the_output_buffer_is_reused_across_blocks() {
        let coding = Coding::default();
        coding.publish(IqFormat::Int16, 0);
        let mut converter = SpyConverter::new(coding);
        let block = vec![0u8; 4096];
        let first = converter.convert(&block).as_ptr();
        assert_eq!(
            converter.convert(&block).as_ptr(),
            first,
            "the capture thread must not allocate per block"
        );
    }
}
