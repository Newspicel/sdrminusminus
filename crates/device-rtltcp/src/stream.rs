use std::{sync::Arc, time::Duration};

use sdrmm_device::{
    Block, BlockPool, ByteCoding, ByteConverter, CaptureStream, Next, StreamFailure,
    net::{Connection, Read, SocketStop},
};

const BLOCK_BYTES: usize = 65_536;

const DC_OFFSET: f32 = 127.4;
const FULL_SCALE: f32 = 127.5;

const CODING: ByteCoding = ByteCoding::OffsetBinary {
    offset: DC_OFFSET,
    full_scale: FULL_SCALE,
};

pub(crate) fn converter() -> ByteConverter {
    ByteConverter::new(CODING, BLOCK_BYTES / 2)
}

#[derive(Debug)]
pub(crate) struct RtlTcpStream {
    connection: Arc<Connection>,
    pool: BlockPool,
}

impl RtlTcpStream {
    pub(crate) fn new(connection: Arc<Connection>, pool: BlockPool) -> Self {
        Self { connection, pool }
    }
}

impl CaptureStream for RtlTcpStream {
    type Block = Block;
    type Stop = SocketStop;

    fn stop_handle(&self) -> SocketStop {
        self.connection.stop_handle()
    }

    fn next_block(&self, timeout: Duration) -> Next<Block> {
        let mut block = self.pool.take(BLOCK_BYTES);
        match self.connection.read(block.bytes_mut(), timeout) {
            Read::Got(n) => {
                block.truncate(n);
                Next::Block(block)
            }
            Read::Idle => Next::Idle,
            Read::Ended => Next::Ended,
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
    use sdrmm_device::SampleConverter;

    use super::*;

    fn code(code: u8) -> f32 {
        CODING.level(code)
    }

    #[test]
    fn every_code_converts_as_the_former_table_did() {
        let codes: Vec<u8> = (0..=255u8).collect();
        let samples = converter().convert(&codes).to_vec();
        for (pair, sample) in codes.as_chunks::<2>().0.iter().zip(samples) {
            let [i, q] = pair.map(|code| ((code as f32 - DC_OFFSET) / FULL_SCALE).to_bits());
            assert_eq!((sample.re.to_bits(), sample.im.to_bits()), (i, q));
        }
    }

    #[test]
    fn codes_map_across_full_scale() {
        for (raw, expected) in [
            (0u8, -0.999_215_7f32),
            (127, -0.003_137_3),
            (128, 0.004_705_9),
            (255, 1.000_784_3),
        ] {
            assert!((code(raw) - expected).abs() < 1e-6, "code {raw}");
        }
    }

    #[test]
    fn a_block_arrives_as_interleaved_complex_samples() {
        let samples = converter().convert(&[0, 255, 127, 128]).to_vec();
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].re, code(0));
        assert_eq!(samples[0].im, code(255));
        assert_eq!(samples[1].re, code(127));
        assert_eq!(samples[1].im, code(128));
    }

    #[test]
    fn the_converter_is_sized_for_a_whole_block() {
        let block = vec![0u8; BLOCK_BYTES];
        let mut converter = converter();
        let first = converter.convert(&block).as_ptr();
        assert_eq!(
            converter.convert(&block).as_ptr(),
            first,
            "the capture thread must not allocate per block"
        );
    }
}
