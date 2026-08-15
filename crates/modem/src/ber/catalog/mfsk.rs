use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass};

use super::Measurement;
use crate::{
    ber::{
        impair::{BurstModel, ChannelSpec},
        rng::Rng,
        sweep::Link,
    },
    cpm::{
        CpmDemod, CpmMod, CpmParams, KnownSymbols, Mapping, TIMING_BW_BURST, TIMING_BW_CONTINUOUS,
    },
    pulse::{self, Norm},
};

pub const RATE: f64 = 48_000.0;
pub const BAUD: f64 = 4_800.0;
pub const SPS: f64 = 10.0;
const SPAN: usize = 8;
const CHANNEL_TAPS: usize = 127;

pub const STEADY_PREAMBLE: usize = 1_500;
pub const STEADY_TAIL: usize = 40;

pub const WARMUP_SAMPLES: usize = (4.0 * 96.0 * SPS) as usize + 300 * SPS as usize;

pub struct Entry {
    pub params: CpmParams,
    pub receive_filter: Vec<f32>,
    pub channel_taps: Vec<f32>,
    pub timing_bw: f64,
}

pub fn mfsk2() -> Entry {
    Entry {
        params: CpmParams::from_h(Mapping::natural(2), 0.5, pulse::rect(SPS, Norm::Area), SPS),
        receive_filter: pulse::rect(SPS, Norm::Area),
        channel_taps: design_lowpass(CHANNEL_TAPS, 4_800.0 / RATE),
        timing_bw: TIMING_BW_CONTINUOUS,
    }
}

pub fn dibit_mapping() -> Mapping {
    Mapping::new(vec![1.0, 3.0, -1.0, -3.0])
}

pub fn mfsk4() -> Entry {
    Entry {
        params: CpmParams::from_deviation(
            dibit_mapping(),
            1_944.0,
            BAUD,
            pulse::root_raised_cosine(SPS, 0.2, SPAN, Norm::Area),
            SPS,
        ),
        receive_filter: pulse::root_raised_cosine(SPS, 0.2, SPAN, Norm::Area),
        channel_taps: design_lowpass(CHANNEL_TAPS, 6_000.0 / RATE),
        timing_bw: TIMING_BW_CONTINUOUS,
    }
}

pub fn mfsk4_burst() -> Entry {
    Entry {
        timing_bw: TIMING_BW_BURST,
        ..mfsk4()
    }
}

pub fn mfsk8() -> Entry {
    Entry {
        params: CpmParams::from_h(Mapping::natural(8), 0.3, pulse::rect(SPS, Norm::Area), SPS),
        receive_filter: pulse::rect(SPS, Norm::Area),
        channel_taps: design_lowpass(CHANNEL_TAPS, 9_600.0 / RATE),
        timing_bw: TIMING_BW_CONTINUOUS,
    }
}

pub fn sync4() -> Vec<u8> {
    let bits: u64 = 0x755F_D7DF_75F7;
    (0..24)
        .rev()
        .map(|i| (bits >> (2 * i)) as u8 & 0b11)
        .collect()
}

pub const SYNC2: [u8; 24] = [
    1, 0, 1, 1, 0, 1, 0, 0, 0, 1, 1, 0, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1, 1, 1,
];

pub const SYNC8: [u8; 16] = [7, 0, 5, 2, 6, 1, 4, 3, 0, 7, 3, 4, 1, 6, 2, 5];

pub const M8_FRAME: usize = 128;
pub const M8_PAYLOAD: usize = M8_FRAME - SYNC8.len();
pub const M8_FRAMES: usize = 48;

pub fn preamble(entry: &Entry, len: usize) -> Vec<u8> {
    let m = entry.params.mapping().m() as u32;
    let mut state = 0x9e37_79b9u32;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state % m) as u8
        })
        .collect()
}

pub fn alternating(entry: &Entry, len: usize) -> Vec<u8> {
    let levels = entry.params.mapping().levels();
    let hi = (0..levels.len())
        .max_by(|&a, &b| levels[a].total_cmp(&levels[b]))
        .unwrap_or(0);
    let lo = (0..levels.len())
        .min_by(|&a, &b| levels[a].total_cmp(&levels[b]))
        .unwrap_or(0);
    (0..len)
        .map(|i| if i % 2 == 0 { hi as u8 } else { lo as u8 })
        .collect()
}

pub fn bits_to_symbols(bits: &[bool], bits_per_symbol: usize) -> Vec<u8> {
    bits.chunks(bits_per_symbol)
        .map(|chunk| chunk.iter().fold(0u8, |acc, &b| (acc << 1) | u8::from(b)))
        .collect()
}

pub fn push_symbol_bits(symbol: u8, bits_per_symbol: usize, out: &mut Vec<bool>) {
    for k in (0..bits_per_symbol).rev() {
        out.push(symbol >> k & 1 == 1);
    }
}

fn quiet(seed: u64, len: usize) -> Vec<Complex<f32>> {
    let mut rng = Rng::new(seed);
    (0..len)
        .map(|_| {
            let re = (rng.uniform() * 2.0 - 1.0) * 0.01;
            let im = (rng.uniform() * 2.0 - 1.0) * 0.01;
            Complex::new(re as f32, im as f32)
        })
        .collect()
}

pub fn recovered_soft(entry: &Entry, wave: &[Complex<f32>], warm_up: bool) -> Vec<f32> {
    let mut filter = Decimator::new(&entry.channel_taps, 1);
    let mut demod = CpmDemod::new(&entry.params, &entry.receive_filter, entry.timing_bw);
    let mut filtered = Vec::new();
    if warm_up {
        let mut discard = Vec::new();
        filter.process(&quiet(0x1157, WARMUP_SAMPLES), &mut filtered);
        demod.process(&filtered, &mut discard);
    }
    let mut soft = Vec::new();
    filter.process(wave, &mut filtered);
    demod.process(&filtered, &mut soft);
    soft
}

fn pattern_distance(sliced: &[u8], at: usize, pattern: &[u8]) -> usize {
    pattern
        .iter()
        .enumerate()
        .filter(|&(i, &s)| sliced[at + i] != s)
        .count()
}

pub fn find_pattern(sliced: &[u8], lo: usize, hi: usize, pattern: &[u8]) -> Option<usize> {
    let last = hi.min(sliced.len().checked_sub(pattern.len())?);
    (lo..=last).min_by_key(|&at| pattern_distance(sliced, at, pattern))
}

pub fn modulate(entry: &Entry, symbols: &[u8]) -> Vec<Complex<f32>> {
    let mut modulator = CpmMod::new(entry.params.clone());
    let mut out = Vec::new();
    modulator.modulate(symbols, &mut out);
    modulator.flush(&mut out);
    out
}

pub const STEADY_PAYLOAD: usize = 6_144;

pub fn steady_link(
    make_entry: fn() -> Entry,
    sync: Vec<u8>,
    label: &str,
    payload_symbols: usize,
) -> Link {
    let entry = make_entry();
    let bits_per_symbol = entry.params.mapping().bits_per_symbol() as usize;
    let tx_sync = sync.clone();
    let demod_entry = make_entry();
    Link {
        label: label.to_string(),
        bits_per_trial: payload_symbols * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let mut symbols = preamble(&entry, STEADY_PREAMBLE);
            symbols.extend_from_slice(&tx_sync);
            symbols.extend(bits_to_symbols(bits, bits_per_symbol));
            symbols.extend(alternating(&entry, STEADY_TAIL));
            modulate(&entry, &symbols)
        }),
        demodulate: Box::new(move |wave| {
            let soft = recovered_soft(&demod_entry, wave, true);
            let mapping = demod_entry.params.mapping();
            let sliced: Vec<u8> = soft.iter().map(|&s| mapping.slice(s)).collect();
            let Some(at) = find_pattern(&sliced, STEADY_PREAMBLE, STEADY_PREAMBLE + 72, &sync)
            else {
                return Vec::new();
            };
            let mut bits = Vec::with_capacity(payload_symbols * bits_per_symbol);
            for k in 0..payload_symbols {
                let symbol = sliced.get(at + sync.len() + k).copied().unwrap_or(0);
                push_symbol_bits(symbol, bits_per_symbol, &mut bits);
            }
            bits
        }),
    }
}

pub fn mfsk2_link_sized(payload_symbols: usize) -> Link {
    steady_link(
        mfsk2,
        SYNC2.to_vec(),
        "mfsk2 CPFSK h=0.5 rect, +-4.8 kHz select -> CpmDemod bw 0.003, \
         1500-sym preamble + 24-sym sync overhead in Eb, release",
        payload_symbols,
    )
}

pub fn mfsk2_link() -> Link {
    mfsk2_link_sized(STEADY_PAYLOAD)
}

pub fn mfsk4_link_sized(payload_symbols: usize) -> Link {
    steady_link(
        mfsk4,
        sync4(),
        "mfsk4 CPFSK ETSI dibits h=0.27 RRC a=0.2, +-6 kHz select -> CpmDemod bw 0.003, \
         1500-sym preamble + 24-sym sync overhead in Eb, release",
        payload_symbols,
    )
}

pub fn mfsk4_link() -> Link {
    mfsk4_link_sized(STEADY_PAYLOAD)
}

pub fn mfsk8_link() -> Link {
    mfsk8_link_sized(M8_FRAMES)
}

pub fn mfsk8_link_sized(frames: usize) -> Link {
    let entry = mfsk8();
    let demod_entry = mfsk8();
    Link {
        label: "mfsk8 CPFSK h=0.3 rect natural map, +-9.6 kHz select -> CpmDemod bw 0.003 \
                + KnownSymbols per 128-sym frame, preamble + 16/128 sync overhead in Eb, \
                release"
            .to_string(),
        bits_per_trial: frames * M8_PAYLOAD * 3,
        modulate: Box::new(move |bits| {
            let mut symbols = preamble(&entry, STEADY_PREAMBLE);
            let payload = bits_to_symbols(bits, 3);
            for frame in payload.chunks(M8_PAYLOAD) {
                symbols.extend_from_slice(&SYNC8);
                symbols.extend_from_slice(frame);
            }
            symbols.extend(alternating(&entry, STEADY_TAIL));
            modulate(&entry, &symbols)
        }),
        demodulate: Box::new(move |wave| {
            let soft = recovered_soft(&demod_entry, wave, true);
            let mapping = demod_entry.params.mapping();
            let sliced: Vec<u8> = soft.iter().map(|&s| mapping.slice(s)).collect();
            let mut bits = Vec::with_capacity(frames * M8_PAYLOAD * 3);
            let Some(at0) = find_pattern(&sliced, STEADY_PREAMBLE, STEADY_PREAMBLE + 72, &SYNC8)
            else {
                return Vec::new();
            };
            let mut hook = KnownSymbols::new(&demod_entry.params, (4 * M8_FRAME) as u32);
            for frame in 0..frames {
                let expect = at0 + frame * M8_FRAME;
                let at = find_pattern(&sliced, expect.saturating_sub(3), expect + 3, &SYNC8)
                    .unwrap_or(expect);
                if let Some(window) = soft.get(at..at + SYNC8.len()) {
                    hook.anchor(&SYNC8, window);
                }
                for k in SYNC8.len()..M8_FRAME {
                    hook.tick();
                    let symbol = soft
                        .get(at + k)
                        .map_or(0, |&s| mapping.slice(hook.correct(s)));
                    push_symbol_bits(symbol, 3, &mut bits);
                }
            }
            bits
        }),
    }
}

pub const BURST_LEAD_SAMPLES: usize = 12_000;

pub const BURST_FRAMES: usize = 6;

#[derive(Clone, Copy)]
pub struct BurstRecipe {
    pub payload_symbols: usize,
    pub off_symbols: usize,
    pub payload_frames: usize,
    pub level_step_db: f64,
}

impl BurstRecipe {
    pub fn reference(payload_frames: usize) -> Self {
        Self {
            payload_symbols: 108,
            off_symbols: 156,
            payload_frames,
            level_step_db: 0.0,
        }
    }

    pub fn content(&self) -> usize {
        sync4().len() + self.payload_symbols
    }

    pub fn frame_symbols(&self) -> usize {
        self.content() + self.off_symbols
    }

    fn on_samples(&self) -> usize {
        self.content() * SPS as usize + 150
    }

    fn lead_frames(&self) -> usize {
        BURST_LEAD_SAMPLES.div_ceil(self.frame_symbols() * SPS as usize)
    }

    pub fn bits(&self) -> usize {
        2 * self.payload_symbols * self.payload_frames
    }

    fn symbols(&self, entry: &Entry, payload: &[bool]) -> Vec<u8> {
        let frame = self.frame_symbols();
        let mut symbols = preamble(entry, frame * (self.payload_frames + 1));
        let sync = sync4();
        let dibits = bits_to_symbols(payload, 2);
        for p in 0..self.payload_frames {
            let base = frame * (p + 1);
            symbols[base..base + sync.len()].copy_from_slice(&sync);
            let src = &dibits[p * self.payload_symbols..(p + 1) * self.payload_symbols];
            symbols[base + sync.len()..base + self.content()].copy_from_slice(src);
        }
        symbols
    }

    pub fn channel(&self) -> ChannelSpec {
        let frame_samples = self.frame_symbols() * SPS as usize;
        ChannelSpec::default().burst(BurstModel::new(
            self.on_samples(),
            frame_samples - self.on_samples(),
            SPS as usize,
            self.level_step_db,
            40.0,
        ))
    }

    pub fn link(&self, label: &str) -> Link {
        let recipe = *self;
        let demod_recipe = *self;
        let entry = mfsk4_burst();
        Link {
            label: label.to_string(),
            bits_per_trial: self.bits(),
            modulate: Box::new(move |bits| {
                let mut wave = vec![
                    Complex::default();
                    recipe.lead_frames() * recipe.frame_symbols() * SPS as usize
                ];
                wave.extend(modulate(&entry, &recipe.symbols(&entry, bits)));
                wave
            }),
            demodulate: Box::new(move |wave| demod_recipe.demodulate(wave)),
        }
    }

    fn demodulate(&self, wave: &[Complex<f32>]) -> Vec<bool> {
        let entry = mfsk4_burst();
        let soft = recovered_soft(&entry, wave, false);
        let mapping = entry.params.mapping();
        let sliced: Vec<u8> = soft.iter().map(|&s| mapping.slice(s)).collect();
        let sync = sync4();
        let frame = self.frame_symbols();
        let lead = self.lead_frames() * frame;
        let mut hook = KnownSymbols::new(&entry.params, 4_800);
        let mut bits = Vec::with_capacity(self.bits());
        let mut delay: usize = 0;
        for p in 0..self.payload_frames {
            let expect = lead + frame * (p + 1);
            let (lo, hi) = if p == 0 {
                (expect, expect + 72)
            } else {
                ((expect + delay).saturating_sub(4), expect + delay + 4)
            };
            let at = find_pattern(&sliced, lo, hi, &sync);
            if let Some(at) = at {
                delay = at.saturating_sub(expect);
                if let Some(window) = soft.get(at..at + sync.len()) {
                    hook.anchor(&sync, window);
                }
            }
            for k in 0..self.payload_symbols {
                hook.tick();
                let symbol = at
                    .and_then(|at| soft.get(at + sync.len() + k))
                    .map_or(0, |&s| mapping.slice(hook.correct(s)));
                push_symbol_bits(symbol, 2, &mut bits);
            }
        }
        bits
    }
}

pub const M2_GRID: &[f64] = &[7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0];
pub const M4_GRID: &[f64] = &[
    6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 18.0, 19.0, 20.0,
];
pub const M8_GRID: &[f64] = &[
    10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 18.0, 19.0, 20.0,
];

pub const M2_SEED: u64 = 0x2f5c;
pub const M4_SEED: u64 = 0x4f5c;
pub const M8_SEED: u64 = 0x8f5c;

pub const FULL_CAP: u64 = 6_000_000;

pub const M2_THEORY_OFFSET_DB: f64 = 1.29;
pub const M2_OFFSET_TOL_DB: f64 = 0.4;

pub const M2_AWGN: &str = "cpm/mfsk2_cpfsk_awgn";
pub const M4_AWGN: &str = "cpm/mfsk4_cpfsk_awgn";
pub const M8_AWGN: &str = "cpm/mfsk8_cpfsk_awgn";
pub const M4_LIMITS: &str = "cpm/mfsk4_limits";

pub const MEASUREMENTS: &[Measurement] = &[
    Measurement {
        reference: super::Reference::OffsetOracle {
            name: "noncoherent orthogonal 2-FSK",
            ber: m2_theory_ber,
            at_ber: 1e-3,
            offset_db: M2_THEORY_OFFSET_DB,
            tolerance_db: M2_OFFSET_TOL_DB,
        },
        ..Measurement::committed(M2_AWGN, mfsk2_link, M2_GRID, M2_SEED, FULL_CAP)
    },
    Measurement::committed(M4_AWGN, mfsk4_link, M4_GRID, M4_SEED, FULL_CAP),
    Measurement::committed(M8_AWGN, mfsk8_link, M8_GRID, M8_SEED, FULL_CAP),
];

fn m2_theory_ber(ebn0_db: f64) -> f64 {
    crate::ber::theory::mfsk_noncoherent_ber(2, ebn0_db)
}

pub const PERF: &str = "cpm/mfsk_perf";
