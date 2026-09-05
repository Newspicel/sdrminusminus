pub(crate) mod dmr;
pub(crate) mod dpmr;
pub(crate) mod dstar;
pub(crate) mod freedv;
pub(crate) mod m17;
pub(crate) mod nxdn;
pub(crate) mod p25;
pub(crate) mod vocoder;
pub(crate) mod ysf;

use std::{collections::VecDeque, sync::LazyLock};

pub use dmr::DmrChannel;
pub use dpmr::DpmrChannel;
pub use dstar::DstarChannel;
pub use freedv::FreeDvChannel;
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

use crate::{ChannelFilter, ChannelOutputs};

pub(crate) const INPUT_RATE_HZ: f64 = 48_000.0;

pub(crate) const RRC_SPAN: usize = 8;

const ANCHOR_TIMEOUT_SYMBOLS: u32 = 4_800;

pub(crate) fn dibit_mapping() -> Mapping {
    Mapping::new(vec![1.0, 3.0, -1.0, -3.0])
}

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

pub(crate) fn c4fm_demod(params: &CpmParams) -> CpmDemod {
    CpmDemod::new(params, params.freq_pulse(), TIMING_BW_BURST)
}

pub(crate) fn tap_symbols(
    out: &mut ChannelOutputs,
    demod: &CpmDemod,
    symbols: &[f32],
    mapping: &Mapping,
    baud: f64,
    input_rate: f64,
) {
    out.symbols.levels(
        symbols,
        demod.settled(),
        mapping,
        baud,
        demod.frequency_error_cycles_per_sample() * input_rate,
    );
}

pub(crate) fn tap_c4fm(
    out: &mut ChannelOutputs,
    demod: &CpmDemod,
    symbols: &[f32],
    baud: f64,
    input_rate: f64,
) {
    if !out.symbols.wanted() {
        return;
    }
    tap_symbols(out, demod, symbols, &dibit_mapping(), baud, input_rate);
}

fn window_hook() -> KnownSymbols {
    KnownSymbols::from_mapping(&dibit_mapping(), ANCHOR_TIMEOUT_SYMBOLS)
}

pub(crate) fn channel_filter(bandwidth_hz: f64) -> ChannelFilter {
    const TAPS: usize = 129;
    ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(TAPS, bandwidth_hz / 2.0 / INPUT_RATE_HZ),
        1,
    ))
}

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

    pub(crate) fn anchor(&mut self, pattern: u64, bits: u32) {
        self.measured.clear();
        self.pattern.clear();
        for i in 0..bits as usize / 2 {
            self.measured.push(self.raw(i));
            self.pattern.push((pattern >> (2 * i)) as u8 & 0b11);
        }
        self.levels.anchor(&self.pattern, &self.measured);
    }

    pub(crate) fn sync_distance(&self, pattern: u64, bits: u32) -> u32 {
        let mask = if bits >= 64 {
            u64::MAX
        } else {
            (1 << bits) - 1
        };
        sdrmm_dsp::hamming_distance(self.register & mask, pattern & mask)
    }

    pub(crate) fn soft(&self, back: usize) -> f32 {
        self.levels.correct(self.raw(back))
    }

    fn raw(&self, back: usize) -> f32 {
        let len = self.soft.len();
        if back >= len {
            return 0.0;
        }
        self.soft[len - 1 - back]
    }

    pub(crate) fn bits(&self, end_back: usize, symbols: usize, out: &mut Vec<bool>) {
        out.clear();
        for i in (0..symbols).rev() {
            let dibit = self.mapping.slice(self.soft(end_back + i));
            out.push(dibit & 0b10 != 0);
            out.push(dibit & 0b01 != 0);
        }
    }

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

    pub(crate) fn vocoder_soft_bits(&mut self, end_back: usize, symbols: usize, out: &mut Vec<i8>) {
        out.clear();
        for i in (0..symbols).rev() {
            self.soft_scratch.clear();
            self.mapping
                .soft_bits(self.soft(end_back + i), &mut self.soft_scratch);
            out.extend(
                self.soft_scratch
                    .iter()
                    .map(|b| (b.0 * f32::from(i8::MAX)) as i8),
            );
        }
    }

    pub(crate) fn reset(&mut self) {
        self.soft.clear();
        self.register = 0;
        self.levels.reset();
    }
}

pub(crate) struct ModeSignature {
    pub(crate) name: &'static str,
    pub(crate) type_id: &'static str,
    pub(crate) baud: f64,
    pub(crate) deviation_hz: f64,
    pub(crate) bandwidth_hz: f64,
    pub(crate) params: CpmParams,
    pub(crate) receive_filter: Vec<f32>,
    pub(crate) sync_bits: u32,
    pub(crate) tolerance: u32,
    pub(crate) patterns: Vec<u64>,
    pub(crate) min_hits: u32,
}

const IDENT_SHORT_SYNC_TOLERANCE: u32 = 1;

struct C4fm {
    name: &'static str,
    type_id: &'static str,
    baud: f64,
    deviation_hz: f64,
    alpha: f64,
    bandwidth_hz: f64,
}

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

pub(crate) fn bits_to_u32(bits: &[bool], offset: usize, len: usize) -> u32 {
    bits[offset..offset + len]
        .iter()
        .fold(0u32, |acc, &b| acc << 1 | u32::from(b))
}

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
        assert!(
            audio.iter().all(|sample| sample.abs() < 1.0),
            "presentation gain drove the vocoder into full-scale clipping"
        );
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
