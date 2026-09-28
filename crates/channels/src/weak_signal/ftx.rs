mod engine;
mod ldpc;
mod message;
mod protocol;
mod synth;

#[cfg(test)]
mod comparison;

pub(crate) use engine::Found;
pub(crate) use protocol::{FT4, FT8, Protocol};

use engine::{Engine, Search};
use message::CallBook;

const PASSES: usize = 3;
const OSD_ORDER: usize = 3;

pub(crate) struct FtxDecoder {
    engine: Engine,
    book: CallBook,
}

impl FtxDecoder {
    pub(crate) fn new(protocol: &'static Protocol) -> Self {
        Self {
            engine: Engine::new(protocol),
            book: CallBook::default(),
        }
    }

    pub(crate) fn decode(
        &mut self,
        samples: &[f32],
        low_hz: f32,
        high_hz: f32,
        max_candidates: usize,
    ) -> Vec<Found> {
        let search = Search {
            low_hz,
            high_hz,
            max_candidates,
            passes: PASSES,
            osd_order: OSD_ORDER,
        };
        let book = &mut self.book;
        self.engine.decode(samples, &search, &mut |payload, osd| {
            if osd && !is_everyday(payload) {
                return None;
            }
            message::unpack(payload, book)
        })
    }
}

fn is_everyday(payload: u128) -> bool {
    matches!(payload & 7, 1 | 2 | 4)
}

#[cfg(any(test, feature = "test-signals"))]
pub(crate) fn pack(text: &str) -> Option<u128> {
    message::pack(text)
}

#[cfg(any(test, feature = "test-signals"))]
pub(crate) fn waveform(protocol: &Protocol, payload: u128, frequency_hz: f64) -> Vec<f32> {
    let tones = protocol.tones_for(payload);
    let mut wave = Vec::new();
    synth::waveform(
        protocol,
        &tones[..protocol.symbols],
        frequency_hz,
        &mut wave,
    );
    wave.iter().map(|sample| sample.re).collect()
}
