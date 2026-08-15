//! Digital-voice decoders ( wave 3): DMR, D-Star, System Fusion, NXDN, P25 Phase 1,
//! dPMR and M17.
pub(crate) mod dmr;
pub(crate) mod dpmr;
pub(crate) mod dstar;
pub(crate) mod m17;
pub(crate) mod nxdn;
pub(crate) mod p25;
pub(crate) mod vocoder;
pub(crate) mod ysf;

use std::{collections::VecDeque, sync::LazyLock};

pub use dmr::DmrChannel;
pub use dpmr::DpmrChannel;
pub use dstar::DstarChannel;
pub use m17::M17Channel;
pub use nxdn::NxdnChannel;
pub use p25::P25Channel;
use sdrmm_dsp::{Decimator, design_lowpass, fec::conv::CONFIDENT};
use sdrmm_modem::{
    cpm::{CpmDemod, CpmParams, KnownSymbols, Mapping, TIMING_BW_BURST},
    pulse::{self, Norm},
    soft::SoftBit,
};
use sdrmm_wire::NxdnBandwidth;
pub use ysf::YsfChannel;

use crate::ChannelFilter;

/// Every C4FM mode here runs at this rate: 10 samples per symbol at 4800 baud, 20 at 2400.
pub(crate) const INPUT_RATE_HZ: f64 = 48_000.0;

/// Shaping/matched-filter span either side of the pulse, in symbol periods — the C4FM root
/// pair every mode here transmits and receives with.
pub(crate) const RRC_SPAN: usize = 8;

/// Symbols an anchored level estimate survives without a fresh sync. The longest gap any of
/// the modes leaves between syncs is DMR's 360 ms voice superframe — 1728 symbols — so a
/// channel this long without one is between transmissions, and the next transmitter must meet
/// the front end's own estimates rather than the last transmitter's correction.
const ANCHOR_TIMEOUT_SYMBOLS: u32 = 4_800;

pub(crate) fn dibit_mapping() -> Mapping {
    Mapping::new(vec![1.0, 3.0, -1.0, -3.0])
}

/// A mode's C4FM waveform as modulation-library data: the shared dibit table, h converted from
/// the mode's outer deviation, and the mode's root-raised-cosine as the frequency pulse. The
/// same params drive the reference transmitter in `testgen` and the receiver here, so the two
/// cannot drift apart.
pub(crate) fn c4fm_params(input_rate: f64, baud: f64, deviation_hz: f64, alpha: f64) -> CpmParams {
    let sps = input_rate / baud;
    CpmParams::from_deviation(
        dibit_mapping(),
        deviation_hz,
        baud,
        pulse::root_raised_cosine(sps, alpha, RRC_SPAN, Norm::Area),
        sps,
    )
}

/// The shared four-level front end: the entry's discriminator-tier demodulator with the RRC
/// receive half of the root pair (the frequency pulse itself — C4FM shapes frequency, so the
/// matched filter is the same taps), at the burst timing operating point — every mode here is
/// TDMA or push-to-talk keyed, so the clock must acquire within one burst's preamble and the
/// carrier gate coasts it through the dead time.
pub(crate) fn c4fm_demod(params: &CpmParams) -> CpmDemod {
    CpmDemod::new(params, params.freq_pulse(), TIMING_BW_BURST)
}

/// The level hook every [`SymbolWindow`] carries. The fit reads only the symbol table, which
/// is the one thing all six four-level modes share — they differ in baud, deviation and pulse,
/// and none of that reaches the estimate.
fn window_hook() -> KnownSymbols {
    KnownSymbols::from_mapping(&dibit_mapping(), ANCHOR_TIMEOUT_SYMBOLS)
}

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
///
/// Every symbol read out — hard or soft — passes through a [`KnownSymbols`] correction the
/// modes feed by calling [`anchor`](Self::anchor) on each matched sync. The history keeps the
/// symbols as the front end produced them, so a burst whose sync sits mid-burst is read back
/// against what its *own* sync measured, first half included — the estimate could not have been
/// applied to those symbols as they arrived, because it did not exist yet.
pub(crate) struct SymbolWindow {
    soft: VecDeque<f32>,
    register: u64,
    capacity: usize,
    mapping: Mapping,
    levels: KnownSymbols,
    measured: Vec<f32>,
    pattern: Vec<u8>,
    soft_scratch: Vec<SoftBit>,
}

impl SymbolWindow {
    /// `capacity` is the longest look-back any caller will ask for, in symbols — the sync
    /// patterns handed to [`anchor`](Self::anchor) included.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            soft: VecDeque::with_capacity(capacity),
            register: 0,
            capacity,
            mapping: dibit_mapping(),
            levels: window_hook(),
            measured: Vec::new(),
            pattern: Vec::new(),
            soft_scratch: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, symbol: f32) {
        if self.soft.len() == self.capacity {
            self.soft.pop_front();
        }
        self.soft.push_back(symbol);
        self.levels.tick();
        self.register =
            self.register << 2 | u64::from(self.mapping.slice(self.levels.correct(symbol)));
    }

    /// A sync pattern of `bits` bits just matched, ending at the most recent symbol: measure
    /// the transmitter's centre and level from it. The sync is the one stretch of a burst
    /// whose transmitted levels are known exactly, so the estimate is data-aided — the loops
    /// in the front end have to learn from payload and dead time, and this does not.
    pub(crate) fn anchor(&mut self, pattern: u64, bits: u32) {
        self.measured.clear();
        self.pattern.clear();
        for i in 0..bits as usize / 2 {
            self.measured.push(self.raw(i));
            self.pattern.push((pattern >> (2 * i)) as u8 & 0b11);
        }
        self.levels.anchor(&self.pattern, &self.measured);
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
        self.levels.correct(self.raw(back))
    }

    /// The same symbol as the front end produced it, before the sync-anchored correction —
    /// what [`anchor`](Self::anchor) measures from, so each sync's fit stands on its own
    /// rather than on the corrections before it.
    fn raw(&self, back: usize) -> f32 {
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
            let dibit = self.mapping.slice(self.soft(end_back + i));
            out.push(dibit & 0b10 != 0);
            out.push(dibit & 0b01 != 0);
        }
    }

    /// Soft bit values of the same span, for the convolutional decoders — the mapping's ±1
    /// full-confidence demap rescaled to the Viterbi's `CONFIDENT` unit, which on the dibit
    /// table reproduces the historical `fsk4::soft_bits` values exactly.
    pub(crate) fn soft_bits(&mut self, end_back: usize, symbols: usize, out: &mut Vec<i16>) {
        out.clear();
        for i in (0..symbols).rev() {
            self.soft_scratch.clear();
            self.mapping
                .soft_bits(self.soft(end_back + i), &mut self.soft_scratch);
            out.extend(
                self.soft_scratch
                    .iter()
                    .map(|b| (b.0 * f32::from(CONFIDENT)) as i16),
            );
        }
    }

    /// Forget everything: the channel moved, and a sync half-matched across the retune would
    /// frame a burst out of two different transmitters.
    pub(crate) fn reset(&mut self) {
        self.soft.clear();
        self.register = 0;
        self.levels.reset();
    }
}

/// One mode's waveform and the framing that proves it, for the signal identifier's mode search.
///
/// The seven digital-voice modes are the one family a spectrum measurement cannot separate:
/// DMR, P25, System Fusion and 12.5 kHz NXDN are all four-level, all 4800 symbols a second, all
/// ±1944 Hz in 12.5 kHz. Nothing about the *signal* distinguishes them — only their frame
/// syncs do, so the identifier demodulates against each candidate and looks for one.
///
/// Every field here is read from the mode's own decoder, so a sync pattern corrected in one
/// place is corrected for both.
pub(crate) struct ModeSignature {
    pub(crate) name: &'static str,
    /// The channel type that decodes it.
    pub(crate) type_id: &'static str,
    pub(crate) baud: f64,
    pub(crate) deviation_hz: f64,
    pub(crate) bandwidth_hz: f64,
    pub(crate) params: CpmParams,
    /// Matched filter the front end runs, in the mode's own construction.
    pub(crate) receive_filter: Vec<f32>,
    pub(crate) sync_bits: u32,
    pub(crate) tolerance: u32,
    pub(crate) patterns: Vec<u64>,
    /// Matches a search must find before the mode counts as recognised. A short pattern is one
    /// noise turns up on its own — M17's is sixteen bits, which a random dibit stream produces
    /// several times a second — so it has to arrive more than once to mean anything.
    pub(crate) min_hits: u32,
}

/// Bit errors the identifier allows in a *short* sync, against the wider allowance the decoders
/// themselves run at.
///
/// The two are answering different questions. A decoder that misses a sync loses one burst and
/// finds the next, so it buys sensitivity with tolerance; an identifier that matches a sync it
/// should not have has told the operator the wrong protocol, and nothing downstream will catch
/// it. Over a whole observation window a 16- or 20-bit pattern at the decoders' tolerance is
/// something *noise* produces several times, which is why M17's allowance here is zero and why
/// the short patterns must also arrive more than once.
const IDENT_SHORT_SYNC_TOLERANCE: u32 = 1;

/// A four-level mode's waveform, as its own decoder states it.
struct C4fm {
    name: &'static str,
    type_id: &'static str,
    baud: f64,
    deviation_hz: f64,
    alpha: f64,
    bandwidth_hz: f64,
}

/// The framing to look for in it.
struct Framing {
    bits: u32,
    tolerance: u32,
    patterns: Vec<u64>,
    min_hits: u32,
}

fn c4fm_signature(mode: &C4fm, framing: Framing) -> ModeSignature {
    let params = c4fm_params(INPUT_RATE_HZ, mode.baud, mode.deviation_hz, mode.alpha);
    let receive_filter = params.freq_pulse().to_vec();
    ModeSignature {
        name: mode.name,
        type_id: mode.type_id,
        baud: mode.baud,
        deviation_hz: mode.deviation_hz,
        bandwidth_hz: mode.bandwidth_hz,
        params,
        receive_filter,
        sync_bits: framing.bits,
        tolerance: framing.tolerance,
        patterns: framing.patterns,
        min_hits: framing.min_hits,
    }
}

/// Every mode the identifier can recognise by its framing, at the shared 48 kHz front-end rate.
pub(crate) static MODE_SIGNATURES: LazyLock<Vec<ModeSignature>> = LazyLock::new(|| {
    let (nxdn_narrow_baud, nxdn_narrow_dev, nxdn_narrow_bw) = nxdn::shape(NxdnBandwidth::Narrow);
    let (nxdn_wide_baud, nxdn_wide_dev, nxdn_wide_bw) = nxdn::shape(NxdnBandwidth::Wide);
    let dstar_sps = INPUT_RATE_HZ / dstar::BAUD;
    let dstar_params = dstar::cpm_params(dstar_sps);
    vec![
        c4fm_signature(
            &C4fm {
                name: "DMR",
                type_id: "dmr",
                baud: dmr::BAUD,
                deviation_hz: dmr::DEVIATION_HZ,
                alpha: dmr::RRC_ALPHA,
                bandwidth_hz: dmr::BANDWIDTH_HZ,
            },
            Framing {
                bits: dmr::SYNC_BITS,
                tolerance: dmr::SYNC_TOLERANCE,
                patterns: dmr::SYNC_PATTERNS.to_vec(),
                min_hits: 1,
            },
        ),
        c4fm_signature(
            &C4fm {
                name: "P25 Phase 1",
                type_id: "p25",
                baud: p25::BAUD,
                deviation_hz: p25::DEVIATION_HZ,
                alpha: p25::RRC_ALPHA,
                bandwidth_hz: p25::BANDWIDTH_HZ,
            },
            Framing {
                bits: p25::SYNC_BITS,
                tolerance: p25::SYNC_TOLERANCE,
                patterns: vec![p25::SYNC],
                min_hits: 1,
            },
        ),
        c4fm_signature(
            &C4fm {
                name: "System Fusion",
                type_id: "ysf",
                baud: ysf::BAUD,
                deviation_hz: ysf::DEVIATION_HZ,
                alpha: ysf::RRC_ALPHA,
                bandwidth_hz: ysf::BANDWIDTH_HZ,
            },
            Framing {
                bits: ysf::SYNC_BITS,
                tolerance: ysf::SYNC_TOLERANCE,
                patterns: vec![ysf::SYNC],
                min_hits: 1,
            },
        ),
        c4fm_signature(
            &C4fm {
                name: "NXDN (6.25 kHz)",
                type_id: "nxdn",
                baud: nxdn_narrow_baud,
                deviation_hz: nxdn_narrow_dev,
                alpha: nxdn::RRC_ALPHA,
                bandwidth_hz: nxdn_narrow_bw,
            },
            Framing {
                bits: nxdn::FSW_BITS,
                tolerance: IDENT_SHORT_SYNC_TOLERANCE,
                patterns: vec![nxdn::FSW],
                min_hits: 2,
            },
        ),
        c4fm_signature(
            &C4fm {
                name: "NXDN (12.5 kHz)",
                type_id: "nxdn",
                baud: nxdn_wide_baud,
                deviation_hz: nxdn_wide_dev,
                alpha: nxdn::RRC_ALPHA,
                bandwidth_hz: nxdn_wide_bw,
            },
            Framing {
                bits: nxdn::FSW_BITS,
                tolerance: IDENT_SHORT_SYNC_TOLERANCE,
                patterns: vec![nxdn::FSW],
                min_hits: 2,
            },
        ),
        c4fm_signature(
            &C4fm {
                name: "dPMR",
                type_id: "dpmr",
                baud: dpmr::BAUD,
                deviation_hz: dpmr::DEVIATION_HZ,
                alpha: dpmr::RRC_ALPHA,
                bandwidth_hz: dpmr::BANDWIDTH_HZ,
            },
            Framing {
                bits: dpmr::LONG_SYNC_BITS,
                tolerance: dpmr::LONG_TOLERANCE,
                patterns: vec![dpmr::FS1, dpmr::FS4],
                min_hits: 1,
            },
        ),
        c4fm_signature(
            &C4fm {
                name: "M17",
                type_id: "m17",
                baud: m17::BAUD,
                deviation_hz: m17::DEVIATION_HZ,
                alpha: m17::RRC_ALPHA,
                bandwidth_hz: m17::BANDWIDTH_HZ,
            },
            Framing {
                bits: m17::SYNC_BITS,
                tolerance: 0,
                patterns: vec![m17::SYNC_LSF, m17::SYNC_STREAM, m17::SYNC_PACKET],
                min_hits: 3,
            },
        ),
        ModeSignature {
            name: "D-STAR",
            type_id: "dstar",
            baud: dstar::BAUD,
            deviation_hz: dstar::DEVIATION_HZ,
            bandwidth_hz: dstar::BANDWIDTH_HZ,
            params: dstar_params,
            receive_filter: pulse::gaussian(dstar_sps, dstar::BT, dstar::MATCHED_SPAN, Norm::Area),
            sync_bits: dstar::SYNC_BITS,
            tolerance: dstar::SYNC_TOLERANCE,
            patterns: vec![u64::from(dstar::SYNC)],
            min_hits: 2,
        },
    ]
});

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

    use super::INPUT_RATE_HZ;
    use crate::{AUDIO_RATE, ChannelOutputs, ChannelRx};

    /// Feed a generated transmission through a channel in deliberately ragged blocks and
    /// collect the frames it decoded. The block sizes are the point: every decoder here carries
    /// timing, sync and reassembly state across calls, and a burst split across two blocks must
    /// decode the same as one that is not.
    pub(crate) fn decode(chan: &mut dyn ChannelRx, iq: &[Complex<f32>]) -> Vec<DvFrame> {
        decode_with_audio(chan, iq).0
    }

    pub(crate) fn decode_with_audio(
        chan: &mut dyn ChannelRx,
        iq: &[Complex<f32>],
    ) -> (Vec<DvFrame>, Vec<f32>) {
        let mut out = ChannelOutputs::default();
        let mut frames = Vec::new();
        let mut audio = Vec::new();
        let quiet = crate::testutil::complex_noise(0x1157, 0.01, 4 * INPUT_RATE_HZ as usize / 10);
        chan.process(&quiet, &mut out);
        out.reset();

        let mut pos = 0;
        for len in [997usize, 1, 4_096, 65, 2_048, 7].iter().cycle() {
            if pos >= iq.len() {
                break;
            }
            let end = (pos + len).min(iq.len());
            out.reset();
            chan.process(&iq[pos..end], &mut out);
            audio.extend_from_slice(&out.audio_pcm);
            for event in out.events.drain(..) {
                match event {
                    DecoderEvent::Dv(frame) => frames.push(frame),
                    other => panic!("unexpected {} event", other.kind()),
                }
            }
            pos = end;
        }
        (frames, audio)
    }

    pub(crate) fn assert_tone_audio(audio: &[f32], frames: usize) {
        assert!(
            (audio.len() as isize - (frames * 960) as isize).abs() <= 1,
            "expected {frames} vocoder frames, got {} PCM samples",
            audio.len()
        );
        assert!(audio.iter().all(|sample| sample.is_finite()));
        assert!(audio.iter().all(|sample| sample.abs() <= 1.0));
        let settled = &audio[3 * 960..];
        let rms = crate::testutil::rms(settled);
        let (frequency, _) = crate::testutil::dominant_tone(settled, f64::from(AUDIO_RATE));
        assert!(rms > 0.005, "decoded tone is silent: rms {rms}");
        assert!(
            (frequency - 440.0).abs() < 50.0,
            "decoded tone shifted to {frequency} Hz"
        );
    }
}
