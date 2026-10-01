use std::f64::consts::TAU;

use sdrmm_wire::WefaxIoc;

use super::{BLACK_HZ, STOP_TONE_HZ, WHITE_HZ, samples};

pub(super) const BLOCK_MS: f64 = 80.0;
const TONE_SHARE: f64 = 0.5;
const MIN_SWING: f64 = 0.25;
const SWING_LIMIT: f32 = 1.5;
const CONFIRM_BLOCKS: u32 = 12;
const MAX_MISSES: u32 = 1;
const CARRIER_SPREAD: f64 = 0.35;
const MIN_ENVELOPE: f64 = 1e-6;
const OUTAGE_BLOCKS: u32 = 25;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ToneKind {
    Start(WefaxIoc),
    Stop,
}

const TONES: [ToneKind; 3] = [
    ToneKind::Start(WefaxIoc::Ioc576),
    ToneKind::Start(WefaxIoc::Ioc288),
    ToneKind::Stop,
];

impl ToneKind {
    fn hz(self) -> f64 {
        match self {
            Self::Start(ioc) => ioc.start_tone_hz(),
            Self::Stop => STOP_TONE_HZ,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Signal {
    Confirmed(ToneKind, u64),
    Ended(ToneKind, u64),
}

#[derive(Clone, Copy)]
struct Run {
    kind: Option<ToneKind>,
    start: u64,
    blocks: u32,
    misses: u32,
    first_miss: u64,
    confirmed: bool,
}

const NO_RUN: Run = Run {
    kind: None,
    start: 0,
    blocks: 0,
    misses: 0,
    first_miss: 0,
    confirmed: false,
};

#[derive(Clone, Copy, Default)]
struct Sums {
    count: usize,
    swing: f64,
    swing_sq: f64,
    envelope: f64,
    envelope_sq: f64,
}

pub(super) struct ToneDetector {
    block: usize,
    block_start: u64,
    coeffs: [f64; 3],
    s1: [f64; 3],
    s2: [f64; 3],
    sums: Sums,
    run: Run,
    quiet_blocks: u32,
    quiet_since: u64,
}

fn swing(freq: f32) -> f64 {
    let mid = ((BLACK_HZ + WHITE_HZ) / 2.0) as f32;
    let half = ((WHITE_HZ - BLACK_HZ) / 2.0) as f32;
    f64::from(((freq - mid) / half).clamp(-SWING_LIMIT, SWING_LIMIT))
}

impl ToneDetector {
    pub(super) fn new(rate: f64) -> Self {
        let block = samples(BLOCK_MS, rate).round().max(1.0) as usize;
        Self {
            block,
            block_start: 0,
            coeffs: TONES.map(|tone| 2.0 * (TAU * tone.hz() / rate).cos()),
            s1: [0.0; 3],
            s2: [0.0; 3],
            sums: Sums::default(),
            run: NO_RUN,
            quiet_blocks: 0,
            quiet_since: 0,
        }
    }

    pub(super) fn block_len(&self) -> usize {
        self.block
    }

    pub(super) fn reset(&mut self, next: u64) {
        self.block_start = next;
        self.clear_block();
        self.run = NO_RUN;
        self.quiet_blocks = 0;
    }

    pub(super) fn outage(&self) -> Option<u64> {
        (self.quiet_blocks >= OUTAGE_BLOCKS).then_some(self.quiet_since)
    }

    pub(super) fn push(&mut self, index: u64, freq: f32, envelope: f32) -> Option<Signal> {
        if self.sums.count == 0 {
            self.block_start = index;
        }
        let x = swing(freq);
        for tone in 0..TONES.len() {
            let next = x + self.coeffs[tone] * self.s1[tone] - self.s2[tone];
            self.s2[tone] = self.s1[tone];
            self.s1[tone] = next;
        }
        let envelope = f64::from(envelope);
        self.sums.count += 1;
        self.sums.swing += x;
        self.sums.swing_sq += x * x;
        self.sums.envelope += envelope;
        self.sums.envelope_sq += envelope * envelope;
        if self.sums.count < self.block {
            return None;
        }
        let heard = self.classify();
        self.track_carrier();
        let start = self.block_start;
        self.clear_block();
        self.track_run(heard, start)
    }

    fn clear_block(&mut self) {
        self.s1 = [0.0; 3];
        self.s2 = [0.0; 3];
        self.sums = Sums::default();
    }

    fn classify(&self) -> Option<ToneKind> {
        let n = self.sums.count as f64;
        let mean = self.sums.swing / n;
        let variance = self.sums.swing_sq / n - mean * mean;
        if variance < MIN_SWING {
            return None;
        }
        let mut best: Option<(ToneKind, f64)> = None;
        for (index, tone) in TONES.iter().enumerate() {
            let (s1, s2, coeff) = (self.s1[index], self.s2[index], self.coeffs[index]);
            let power = s1 * s1 + s2 * s2 - coeff * s1 * s2;
            let share = 2.0 * power / (n * n) / variance;
            if share >= TONE_SHARE && best.is_none_or(|(_, top)| share > top) {
                best = Some((*tone, share));
            }
        }
        best.map(|(tone, _)| tone)
    }

    fn track_carrier(&mut self) {
        let n = self.sums.count as f64;
        let mean = self.sums.envelope / n;
        let spread = (self.sums.envelope_sq / n - mean * mean).max(0.0).sqrt();
        let carrier = mean > MIN_ENVELOPE && spread / mean < CARRIER_SPREAD;
        if carrier {
            self.quiet_blocks = 0;
            return;
        }
        if self.quiet_blocks == 0 {
            self.quiet_since = self.block_start;
        }
        self.quiet_blocks = self.quiet_blocks.saturating_add(1);
    }

    fn track_run(&mut self, heard: Option<ToneKind>, start: u64) -> Option<Signal> {
        if heard.is_some() && heard == self.run.kind {
            self.run.blocks += 1;
            self.run.misses = 0;
            if !self.run.confirmed && self.run.blocks >= CONFIRM_BLOCKS {
                self.run.confirmed = true;
                return self
                    .run
                    .kind
                    .map(|kind| Signal::Confirmed(kind, self.run.start));
            }
            return None;
        }
        if self.run.kind.is_some() && self.run.misses < MAX_MISSES {
            if self.run.misses == 0 {
                self.run.first_miss = start;
            }
            self.run.misses += 1;
            return None;
        }
        let ended = self.run;
        let end = if ended.misses > 0 {
            ended.first_miss
        } else {
            start
        };
        self.run = Run {
            kind: heard,
            start,
            blocks: u32::from(heard.is_some()),
            ..NO_RUN
        };
        match ended.kind {
            Some(kind) if ended.confirmed => Some(Signal::Ended(kind, end)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;
    use sdrmm_dsp::FmDemod;

    use super::*;
    use crate::{synth, testutil::complex_noise};

    const RATE: f64 = 12_000.0;

    fn listen(iq: &[Complex<f32>]) -> (Vec<Signal>, Option<u64>) {
        let mut demod = FmDemod::new(RATE, 1.0);
        let mut freq = Vec::new();
        demod.process(iq, &mut freq);
        let mut detector = ToneDetector::new(RATE);
        let signals = iq
            .iter()
            .zip(&freq)
            .enumerate()
            .filter_map(|(index, (sample, &hz))| detector.push(index as u64, hz, sample.norm()))
            .collect();
        (signals, detector.outage())
    }

    #[test]
    fn each_start_tone_names_its_index_of_cooperation() {
        for ioc in WefaxIoc::ALL {
            let mut iq = synth::wefax::start_signal(ioc, 3_000.0, RATE);
            iq.extend(synth::silence(12_000));
            let (signals, _) = listen(&iq);
            assert!(
                matches!(
                    signals[..],
                    [
                        Signal::Confirmed(ToneKind::Start(found), 0),
                        Signal::Ended(ToneKind::Start(_), end)
                    ] if found == ioc && end.abs_diff(36_000) <= 960
                ),
                "{ioc:?} heard as {signals:?}"
            );
        }
    }

    #[test]
    fn noise_is_neither_a_tone_nor_a_carrier() {
        let noise = complex_noise(0x5eed_1234, 0.4, 120_000);
        let mut filtered = Vec::new();
        super::super::channel_filter(&sdrmm_wire::WefaxParams::default())
            .expect("filter")
            .process(&noise, &mut filtered);
        let (signals, outage) = listen(&filtered);
        assert!(signals.is_empty(), "noise heard as {signals:?}");
        assert_eq!(outage, Some(0));
    }
}
