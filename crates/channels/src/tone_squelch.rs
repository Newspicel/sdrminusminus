use sdrmm_dsp::{BitSync, DcBlocker, RealDecimator, ToneCorrelator, design_lowpass, golay23_ok};

pub(crate) const INPUT_RATE_HZ: f64 = 48_000.0;

const TONE_RATE: f64 = 1_200.0;
const STAGE1_DECIM: usize = 10;
const STAGE2_DECIM: usize = 4;
const STAGE1_TAPS: usize = 63;
const STAGE2_TAPS: usize = 63;
const TONE_CUTOFF_HZ: f64 = 300.0;

pub(crate) const AUDIO_CORNER_HZ: f64 = 300.0;

pub const CTCSS_TONES_HZ: [f64; 50] = [
    67.0, 69.3, 71.9, 74.4, 77.0, 79.7, 82.5, 85.4, 88.5, 91.5, 94.8, 97.4, 100.0, 103.5, 107.2,
    110.9, 114.8, 118.8, 123.0, 127.3, 131.8, 136.5, 141.3, 146.2, 151.4, 156.7, 159.8, 162.2,
    165.5, 167.9, 171.3, 173.8, 177.3, 179.9, 183.5, 186.2, 189.9, 192.8, 196.6, 199.5, 203.5,
    206.5, 210.7, 218.1, 225.7, 229.1, 233.6, 241.8, 250.3, 254.1,
];

pub const DCS_CODES: [u16; 83] = [
    23, 25, 26, 31, 32, 43, 47, 51, 54, 65, 71, 72, 73, 74, 114, 115, 116, 125, 131, 132, 134, 143,
    152, 155, 156, 162, 165, 172, 174, 205, 223, 226, 243, 244, 245, 251, 261, 263, 265, 271, 306,
    311, 315, 331, 343, 346, 351, 364, 365, 371, 411, 412, 413, 423, 431, 432, 445, 464, 465, 466,
    503, 506, 516, 532, 546, 565, 606, 612, 624, 627, 631, 632, 654, 662, 664, 703, 712, 723, 731,
    732, 734, 743, 754,
];

#[must_use]
pub fn is_standard_ctcss(hz: f64) -> bool {
    CTCSS_TONES_HZ.iter().any(|&t| (t - hz).abs() < 0.05)
}

#[must_use]
pub fn is_standard_dcs(code: u16) -> bool {
    DCS_CODES.contains(&code)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Subaudible {
    pub ctcss_hz: Option<f64>,
    pub dcs_code: Option<u16>,
}

pub struct ToneSquelch {
    stage1: RealDecimator,
    stage2: RealDecimator,
    stage1_out: Vec<f32>,
    decimated: Vec<f32>,
    ctcss: CtcssBank,
    dcs: DcsDecoder,
}

impl ToneSquelch {
    #[must_use]
    pub fn new() -> Self {
        Self {
            stage1: RealDecimator::new(
                &design_lowpass(STAGE1_TAPS, TONE_CUTOFF_HZ / INPUT_RATE_HZ),
                STAGE1_DECIM,
            ),
            stage2: RealDecimator::new(
                &design_lowpass(
                    STAGE2_TAPS,
                    TONE_CUTOFF_HZ / (INPUT_RATE_HZ / STAGE1_DECIM as f64),
                ),
                STAGE2_DECIM,
            ),
            stage1_out: Vec::new(),
            decimated: Vec::new(),
            ctcss: CtcssBank::new(),
            dcs: DcsDecoder::new(),
        }
    }

    pub fn process(&mut self, demodulated: &[f32]) -> Subaudible {
        self.stage1.process(demodulated, &mut self.stage1_out);
        self.stage2.process(&self.stage1_out, &mut self.decimated);
        for &s in &self.decimated {
            self.ctcss.push(s);
        }
        self.dcs.process(&mut self.decimated);
        Subaudible {
            ctcss_hz: self.ctcss.detected(),
            dcs_code: self.dcs.detected(),
        }
    }

    pub fn reset(&mut self) {
        self.ctcss = CtcssBank::new();
        self.dcs = DcsDecoder::new();
    }
}

impl Default for ToneSquelch {
    fn default() -> Self {
        Self::new()
    }
}

const CTCSS_WINDOW_S: f64 = 0.5;
const CTCSS_DECISION_SAMPLES: usize = (TONE_RATE * 0.05) as usize;
const CTCSS_MIN_LEVEL: f32 = 0.02;
const CTCSS_MARGIN: f32 = 3.0;
const CTCSS_ACQUIRE: u32 = 2;
const CTCSS_RELEASE: u32 = 4;

struct CtcssBank {
    tones: Vec<ToneCorrelator>,
    levels: Vec<f32>,
    since_decision: usize,
    candidate: Option<usize>,
    agreements: u32,
    held: Option<usize>,
    misses: u32,
}

impl CtcssBank {
    fn new() -> Self {
        let window = (TONE_RATE * CTCSS_WINDOW_S) as usize;
        Self {
            tones: CTCSS_TONES_HZ
                .iter()
                .map(|&hz| ToneCorrelator::new(TONE_RATE, hz, window))
                .collect(),
            levels: vec![0.0; CTCSS_TONES_HZ.len()],
            since_decision: 0,
            candidate: None,
            agreements: 0,
            held: None,
            misses: 0,
        }
    }

    fn push(&mut self, sample: f32) {
        for (tone, level) in self.tones.iter_mut().zip(self.levels.iter_mut()) {
            *level = tone.push(sample);
        }
        self.since_decision += 1;
        if self.since_decision >= CTCSS_DECISION_SAMPLES {
            self.since_decision = 0;
            self.decide();
        }
    }

    fn winner(&self) -> Option<usize> {
        let mut best = (0usize, 0.0f32);
        let mut runner_up = 0.0f32;
        for (index, &level) in self.levels.iter().enumerate() {
            if level > best.1 {
                runner_up = best.1;
                best = (index, level);
            } else if level > runner_up {
                runner_up = level;
            }
        }
        (best.1 >= CTCSS_MIN_LEVEL && best.1 >= runner_up * CTCSS_MARGIN).then_some(best.0)
    }

    fn decide(&mut self) {
        let winner = self.winner();
        if winner.is_some() && winner == self.candidate {
            self.agreements += 1;
        } else {
            self.candidate = winner;
            self.agreements = 1;
        }
        if winner.is_none() || winner != self.held {
            self.misses += 1;
        } else {
            self.misses = 0;
        }
        if self.agreements >= CTCSS_ACQUIRE && winner.is_some() {
            self.held = winner;
            self.misses = 0;
        } else if self.misses >= CTCSS_RELEASE {
            self.held = None;
        }
    }

    fn detected(&self) -> Option<f64> {
        self.held.and_then(|i| CTCSS_TONES_HZ.get(i).copied())
    }
}

const DCS_BAUD: f64 = 134.4;
const DCS_WORD_BITS: u32 = 23;
const DCS_SIGNATURE: u32 = 0b100;
const DCS_CONFIRMATIONS: u32 = 2;
const DCS_TIMEOUT_WORDS: f64 = 3.0;

struct DcsDecoder {
    dc: DcBlocker,
    sync: BitSync,
    register: u32,
    bits: u32,
    pending: Option<u16>,
    confirmations: u32,
    held: Option<u16>,
    quiet: usize,
    timeout: usize,
}

impl DcsDecoder {
    fn new() -> Self {
        Self {
            dc: DcBlocker::new(),
            sync: BitSync::new(TONE_RATE, DCS_BAUD),
            register: 0,
            bits: 0,
            pending: None,
            confirmations: 0,
            held: None,
            quiet: 0,
            timeout: (TONE_RATE * DCS_TIMEOUT_WORDS * f64::from(DCS_WORD_BITS) / DCS_BAUD) as usize,
        }
    }

    fn process(&mut self, decimated: &mut [f32]) {
        self.dc.process(decimated);
        for &s in decimated.iter() {
            let Some(bit) = self.sync.push(s) else {
                self.quiet += 1;
                self.forget_if_stale();
                continue;
            };
            self.register = self.register >> 1 | u32::from(bit) << (DCS_WORD_BITS - 1);
            self.bits = (self.bits + 1).min(DCS_WORD_BITS);
            self.quiet += 1;
            if self.bits >= DCS_WORD_BITS {
                self.inspect();
            }
            self.forget_if_stale();
        }
    }

    fn inspect(&mut self) {
        if !golay23_ok(self.register) || self.register >> 20 != DCS_SIGNATURE {
            return;
        }
        let code = octal_digits(self.register >> 11 & 0x1FF);
        if !is_standard_dcs(code) {
            return;
        }
        self.quiet = 0;
        if self.pending == Some(code) {
            self.confirmations += 1;
        } else {
            self.pending = Some(code);
            self.confirmations = 1;
        }
        if self.confirmations >= DCS_CONFIRMATIONS {
            self.held = Some(code);
        }
    }

    fn forget_if_stale(&mut self) {
        if self.quiet > self.timeout {
            self.quiet = 0;
            self.pending = None;
            self.confirmations = 0;
            self.held = None;
        }
    }

    fn detected(&self) -> Option<u16> {
        self.held
    }
}

fn octal_digits(code: u32) -> u16 {
    ((code >> 6 & 7) * 100 + (code >> 3 & 7) * 10 + (code & 7)) as u16
}

#[cfg(test)]
mod tests {
    use sdrmm_dsp::golay23_encode;

    use super::*;

    #[test]
    fn no_standard_dcs_code_can_be_read_out_of_another_ones_word() {
        for &code in &DCS_CODES {
            assert_eq!(
                readouts(dcs_word(code)),
                vec![code],
                "{code:03} does not read out uniquely"
            );
        }
    }

    #[test]
    fn an_inverted_transmission_reads_as_the_inverse_pair_partner() {
        let mut paired = 0;
        for &code in &DCS_CODES {
            let read = readouts(dcs_word(code) ^ MASK);
            assert!(read.len() <= 1, "{code:03} inverted reads as {read:?}");
            let Some(&partner) = read.first() else {
                continue;
            };
            paired += 1;
            assert_ne!(partner, code, "{code:03} cannot be its own inverse");
            assert_eq!(
                readouts(dcs_word(partner) ^ MASK),
                vec![code],
                "{code:03} <-> {partner:03} is not a pair"
            );
        }
        assert_eq!(paired, 82, "41 inverse pairs, and 172 alone without one");
        assert_eq!(readouts(dcs_word(23) ^ MASK), vec![47]);
    }

    #[test]
    fn no_code_outside_the_standard_set_reads_as_one_inside_it() {
        for raw in 0..512u32 {
            let code = octal_digits(raw);
            if is_standard_dcs(code) {
                continue;
            }
            assert_eq!(
                standard_readout(golay23_encode((DCS_SIGNATURE << 9 | raw) as u16)),
                None,
                "{code:03} masquerades as a standard code"
            );
        }
    }

    #[test]
    fn octal_digits_are_the_digits_a_radio_shows() {
        assert_eq!(octal_digits(0b000_010_011), 23);
        assert_eq!(octal_digits(0b111_101_100), 754);
        assert_eq!(octal_digits(0), 0);
        assert_eq!(octal_digits(0b111_111_111), 777);
    }

    #[test]
    fn the_standard_tables_are_what_they_claim() {
        assert!(is_standard_ctcss(88.5) && is_standard_ctcss(254.1));
        assert!(!is_standard_ctcss(88.0) && !is_standard_ctcss(300.0));
        assert!(is_standard_ctcss(103.500_000_1));
        assert!(is_standard_dcs(23) && is_standard_dcs(754));
        assert!(!is_standard_dcs(24) && !is_standard_dcs(999));
        assert!(CTCSS_TONES_HZ.windows(2).all(|w| w[0] < w[1]));
        assert!(DCS_CODES.windows(2).all(|w| w[0] < w[1]));
    }

    const MASK: u32 = (1 << DCS_WORD_BITS) - 1;

    pub(super) fn dcs_word(code: u16) -> u32 {
        let digits = u32::from(code);
        let raw = (digits / 100 % 10) << 6 | (digits / 10 % 10) << 3 | (digits % 10);
        golay23_encode((DCS_SIGNATURE << 9 | raw) as u16)
    }

    fn rotate(word: u32, by: u32) -> u32 {
        (word << by | word >> (DCS_WORD_BITS - by)) & MASK
    }

    fn standard_readout(word: u32) -> Option<u16> {
        if !golay23_ok(word) || word >> 20 != DCS_SIGNATURE {
            return None;
        }
        let code = octal_digits(word >> 11 & 0x1FF);
        is_standard_dcs(code).then_some(code)
    }

    fn readouts(word: u32) -> Vec<u16> {
        (0..DCS_WORD_BITS)
            .filter_map(|r| standard_readout(rotate(word, r)))
            .collect()
    }
}
