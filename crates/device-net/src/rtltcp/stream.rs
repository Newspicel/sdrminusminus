//! The sample half of an rtl_tcp connection: an unframed byte stream of interleaved unsigned
//! 8-bit IQ, and the table that turns those codes into `cf32`.
//!
//! There is nothing to parse — everything after the twelve-byte greeting is samples — so the
//! capture stream is a read into a pooled block and the conversion is the shared
//! [`LutConverter`](sdrmm_device::LutConverter) over the RTL2832U's coding.

use std::{sync::Arc, time::Duration};

use sdrmm_device::{CaptureStream, LutConverter, Next, StreamFailure};

use crate::socket::{Block, BlockPool, Connection, Read, SocketStop};

/// Bytes per read. Two per sample, so this is the capture supervisor's default push size of
/// 32 768 samples — one block in, one block out, with no re-chunking on the sample path.
const BLOCK_BYTES: usize = 65_536;

/// Mid-scale of the RTL2832U's 8-bit ADC. It is 127.4, not the arithmetic 127.5: librtlsdr's own
/// tools and SoapyRTLSDR centre on this measured DC bias, and the mid-point leaves a visible DC
/// spike in the centre bin. The same constants and table as the in-tree RTL-SDR driver's
/// `convert` module — deliberately a second copy rather than a dependency on that crate, which
/// would drag a USB stack into a build that only speaks TCP.
const DC_OFFSET: f32 = 127.4;
const FULL_SCALE: f32 = 127.5;

static CODE_TO_F32: [f32; 256] = build_table();

const fn build_table() -> [f32; 256] {
    let mut table = [0.0f32; 256];
    let mut code = 0usize;
    while code < table.len() {
        table[code] = (code as f32 - DC_OFFSET) / FULL_SCALE;
        code += 1;
    }
    table
}

/// A converter for one capture thread, sized so a whole block fits without growing.
pub(crate) fn converter() -> LutConverter {
    LutConverter::new(&CODE_TO_F32, BLOCK_BYTES / 2)
}

/// A connection being drained for samples.
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
        match self.connection.read(block.as_mut(), timeout) {
            Read::Got(n) => {
                block.truncate(n);
                Next::Block(block)
            }
            Read::Idle => Next::Idle,
            Read::Ended => Next::Ended,
        }
    }

    /// Always zero. rtl_tcp has no way to say it dropped anything: osmocom's server closes a
    /// client it cannot keep up with rather than skipping samples, so a gap arrives as the end of
    /// the stream and is counted as a restart, not as dropped blocks.
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
        CODE_TO_F32[code as usize]
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

    /// The dongle on the far side is the one the in-tree driver reads over USB, so a block has to
    /// arrive as the same samples whichever transport carried it.
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
