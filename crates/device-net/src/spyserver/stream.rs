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

const MAX_DIGITAL_GAIN_DB: u16 = 96;

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

fn scale(gain_db: u16) -> f32 {
    1.0 / 10f32.powf(f32::from(gain_db) / 20.0)
}

#[derive(Debug)]
pub(crate) struct SpyConverter {
    coding: Coding,
    out: Vec<Sample>,
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

    fn reset(&mut self) {}
}

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

#[derive(Debug)]
pub(crate) struct SpyStream {
    connection: Arc<Connection>,
    pool: BlockPool,
    coding: Coding,
    warned: AtomicBool,
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
                if iq { Next::Block(block) } else { Next::Idle }
            }
        }
    }

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
