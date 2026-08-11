//! Digital-voice decoders (PLAN §13 wave 3): DMR, D-Star, System Fusion, NXDN, P25 Phase 1,
//! dPMR and M17.
//!
//! **These decode the call, not the voice.** Every mode but M17 carries an AMBE-family vocoder
//! (AMBE+2, IMBE, AMBE2+ under its various names) and this build ships no vocoder, so no
//! digital-voice channel produces audio — `has_audio` is false for all seven. What they do
//! produce is the signalling around the payload: who transmitted, to which talkgroup or
//! callsign, on which colour code or network, through which repeater, encrypted or not. That is
//! what a scanner log is made of, and it is decodable without a vocoder.
//!
//! Six of the seven share a front end — [`sdrmm_dsp::Fsk4Demod`], four-level FSK at 4800 or
//! 2400 symbols per second — and differ only in their sync patterns, framing and error coding.
//! D-Star is the exception: it is two-level GMSK and demodulates like AIS does.
//!
//! Each mode is a module here with its own `type_id`, because they occupy different bandwidths
//! and an operator picks a mode by name. They all emit the same [`DvFrame`](sdrmm_wire::DvFrame)
//! so one panel, one log filter and (later) one trunking follower serve all of them.

pub(crate) mod dmr;
pub(crate) mod dpmr;
pub(crate) mod dstar;
pub(crate) mod m17;
pub(crate) mod nxdn;
pub(crate) mod p25;
pub(crate) mod ysf;

use std::collections::VecDeque;

pub use dmr::DmrChannel;
pub use dpmr::DpmrChannel;
pub use dstar::DstarChannel;
pub use m17::M17Channel;
pub use nxdn::NxdnChannel;
pub use p25::P25Channel;
use sdrmm_dsp::{Decimator, design_lowpass, fsk4};
pub use ysf::YsfChannel;

use crate::ChannelFilter;

/// Every C4FM mode here runs at this rate: 10 samples per symbol at 4800 baud, 20 at 2400.
pub(crate) const INPUT_RATE_HZ: f64 = 48_000.0;

/// Channel-selection filter for a digital-voice mode of `bandwidth_hz`. Long enough to reject
/// the adjacent channel at 12.5 kHz spacing, which is where these modes live.
pub(crate) fn channel_filter(bandwidth_hz: f64) -> ChannelFilter {
    const TAPS: usize = 129;
    ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(TAPS, bandwidth_hz / 2.0 / INPUT_RATE_HZ),
        1,
    ))
}

/// A rolling window of the most recent soft symbols, with the same window's hard dibits packed
/// into a shift register for sync-word matching.
///
/// Sync patterns are matched against the register; a burst is then read back out of the soft
/// history, because the error-correcting codes above want soft bits and the sync search wants
/// hard ones. Index 0 is always the most recent symbol.
pub(crate) struct SymbolWindow {
    soft: VecDeque<f32>,
    register: u64,
    capacity: usize,
}

impl SymbolWindow {
    /// `capacity` is the longest look-back any caller will ask for, in symbols.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            soft: VecDeque::with_capacity(capacity),
            register: 0,
            capacity,
        }
    }

    pub(crate) fn push(&mut self, symbol: f32) {
        if self.soft.len() == self.capacity {
            self.soft.pop_front();
        }
        self.soft.push_back(symbol);
        self.register = self.register << 2 | u64::from(fsk4::slice(symbol));
    }

    /// Bit distance between the last `bits` bits shifted in and `pattern`.
    pub(crate) fn sync_distance(&self, pattern: u64, bits: u32) -> u32 {
        let mask = if bits >= 64 {
            u64::MAX
        } else {
            (1 << bits) - 1
        };
        sdrmm_dsp::hamming_distance(self.register & mask, pattern & mask)
    }

    /// The soft symbol `back` symbols ago; 0 is the most recent. Zero past the history, which
    /// is a symbol no slicer prefers either way.
    pub(crate) fn soft(&self, back: usize) -> f32 {
        let len = self.soft.len();
        if back >= len {
            return 0.0;
        }
        self.soft[len - 1 - back]
    }

    /// Hard bits of the `symbols` symbols ending `end_back` symbols ago, oldest first, two
    /// bits per symbol.
    pub(crate) fn bits(&self, end_back: usize, symbols: usize, out: &mut Vec<bool>) {
        out.clear();
        for i in (0..symbols).rev() {
            let dibit = fsk4::slice(self.soft(end_back + i));
            out.push(dibit & 0b10 != 0);
            out.push(dibit & 0b01 != 0);
        }
    }

    /// Soft bit values of the same span, for the convolutional decoders.
    pub(crate) fn soft_bits(&self, end_back: usize, symbols: usize, out: &mut Vec<i16>) {
        out.clear();
        for i in (0..symbols).rev() {
            out.extend_from_slice(&fsk4::soft_bits(self.soft(end_back + i)));
        }
    }

    /// Forget everything: the channel moved, and a sync half-matched across the retune would
    /// frame a burst out of two different transmitters.
    pub(crate) fn reset(&mut self) {
        self.soft.clear();
        self.register = 0;
    }
}

/// Read `len` bits from `bits` starting at `offset`, MSB first, as an integer.
pub(crate) fn bits_to_u32(bits: &[bool], offset: usize, len: usize) -> u32 {
    bits[offset..offset + len]
        .iter()
        .fold(0u32, |acc, &b| acc << 1 | u32::from(b))
}

/// Pack `bits` MSB-first into bytes, discarding a trailing partial byte.
pub(crate) fn pack_bytes(bits: &[bool], out: &mut Vec<u8>) {
    out.clear();
    for chunk in bits.as_chunks::<8>().0 {
        out.push(chunk.iter().fold(0u8, |acc, &b| acc << 1 | u8::from(b)));
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use num_complex::Complex;
    use sdrmm_wire::{DecoderEvent, DvFrame};

    use crate::{ChannelOutputs, ChannelRx};

    /// Feed a generated transmission through a channel in deliberately ragged blocks and
    /// collect the frames it decoded. The block sizes are the point: every decoder here carries
    /// timing, sync and reassembly state across calls, and a burst split across two blocks must
    /// decode the same as one that is not.
    pub(crate) fn decode(chan: &mut dyn ChannelRx, iq: &[Complex<f32>]) -> Vec<DvFrame> {
        let mut out = ChannelOutputs::default();
        let mut frames = Vec::new();
        let mut pos = 0;
        for len in [997usize, 1, 4_096, 65, 2_048, 7].iter().cycle() {
            if pos >= iq.len() {
                break;
            }
            let end = (pos + len).min(iq.len());
            out.reset();
            chan.process(&iq[pos..end], &mut out);
            assert!(out.audio_pcm.is_empty(), "a vocoder-less mode made audio");
            for event in out.events.drain(..) {
                match event {
                    DecoderEvent::Dv(frame) => frames.push(frame),
                    other => panic!("unexpected {} event", other.kind()),
                }
            }
            pos = end;
        }
        frames
    }
}
