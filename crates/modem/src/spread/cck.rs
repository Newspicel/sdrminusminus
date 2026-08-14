//! Complementary code keying: eight complex
//! chips carrying eight bits, detected by correlating against the whole codebook at once.
//!
//! **What separates it from the direct-sequence entry beside it.** DSSS spends its chips on one
//! constellation point and gets interference rejection back; CCK spends the same eight chips on a
//! *block code* and gets rate back — 802.11b's 11 Mbit/s is this codebook at the same 11 Mchip/s
//! the 1 Mbit/s Barker rate uses. The two share the chip substrate ([`chip`](super::chip)) and
//! nothing above it, which is exactly the split 802.11b itself makes.
//!
//! **The codebook is generated, not transcribed** (§3.3: point sets are data — so are codeword
//! sets). Every word comes out of the one formula IEEE 802.11-1999 §18.4.6.5 states,
//!
//! ```text
//! c = { e^{j(φ1+φ2+φ3+φ4)}, e^{j(φ1+φ3+φ4)}, e^{j(φ1+φ2+φ4)}, −e^{j(φ1+φ4)},
//!       e^{j(φ1+φ2+φ3)},    e^{j(φ1+φ3)},    −e^{j(φ1+φ2)},    e^{jφ1} }
//! ```
//!
//! with the four phases drawn from a QPSK alphabet at the 8-bit rate and from a reduced one at
//! the 4-bit rate. A table of 64 transcribed words would be a liability no test could check; a
//! generator's properties can be, and they are: constant modulus, the minimum distance the rate
//! rests on, and the fact that φ1 factors out of the word as a pure rotation.
//!
//! **That factorisation is the receiver.** φ1 multiplies every chip, so it does not change which
//! word was sent — the correlator bank runs over the `M/4` words with `φ1 = 0`, and the winner's
//! own phase carries φ1. A 64-word bank decodes 256 candidates.
//!
//! **The differential layer is not here.** 802.11b encodes φ1 differentially; this crate has one
//! differential codec ([`symbolcode`](crate::symbolcode)) and the π/4-DQPSK entry already
//! establishes where it goes — on the symbol *indices*, outside the engine. What the engine does
//! carry is the spec's odd-symbol π rotation, because that is a per-symbol rotation of the
//! waveform exactly as π/2-BPSK's is, and a receiver cannot undo it without knowing it.
//!
//! **A label is four phase indices**, two bits each, φ1 in the low pair. Which of a MAC frame's
//! bits land in which index is 802.11's business and not this entry's (§6 scope decision: the
//! waveform is in, the protocol is out).

use std::f32::consts::FRAC_1_SQRT_2;

use num_complex::Complex;

use super::chip::{ChipShaper, find_burst};
use crate::{linear::PhaseAnchor, soft::Llr};

/// Chips in a CCK codeword — eight, at both rates, which is the point of the code: the rate
/// changes and the chip rate does not.
pub const CHIPS: usize = 8;

/// Words in the largest bank ([`CckMode::Bits8`]'s), and the receive path's scratch size.
pub const MAX_WORDS: usize = 64;

/// How many bits a codeword carries — 802.11b's 5.5 and 11 Mbit/s rates, named by the property
/// that distinguishes them rather than by a data rate this crate has no opinion about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CckMode {
    /// Four bits: φ1 from a QPSK alphabet, φ2 ∈ {π/2, 3π/2}, φ3 = 0, φ4 ∈ {0, π}. 802.11b's
    /// 5.5 Mbit/s.
    Bits4,
    /// Eight bits: all four phases from the QPSK alphabet. 802.11b's 11 Mbit/s.
    Bits8,
}

impl CckMode {
    #[must_use]
    pub fn bits_per_symbol(self) -> usize {
        match self {
            Self::Bits4 => 4,
            Self::Bits8 => 8,
        }
    }

    /// Words in the correlator bank: the alphabet with φ1 divided out.
    #[must_use]
    pub fn words(self) -> usize {
        1 << (self.bits_per_symbol() - 2)
    }

    /// Labels the mode defines.
    #[must_use]
    pub fn alphabet(self) -> u32 {
        1 << self.bits_per_symbol()
    }
}

/// The generated codeword set with φ1 divided out — `words()` unit-modulus words of [`CHIPS`]
/// chips, indexed by the label's upper bits.
#[derive(Clone, Debug, PartialEq)]
pub struct Codebook {
    mode: CckMode,
    words: Vec<[Complex<f32>; CHIPS]>,
}

/// The four QPSK phases, as unit vectors indexed by a dibit (0 → 1, 1 → j, 2 → −1, 3 → −j).
fn quadrant(index: u32) -> Complex<f32> {
    match index & 3 {
        0 => Complex::new(1.0, 0.0),
        1 => Complex::new(0.0, 1.0),
        2 => Complex::new(-1.0, 0.0),
        _ => Complex::new(0.0, -1.0),
    }
}

impl Codebook {
    /// Builds the mode's bank from the §18.4.6.5 formula.
    #[must_use]
    pub fn new(mode: CckMode) -> Self {
        let words = (0..mode.words() as u32)
            .map(|index| Self::word(mode, index))
            .collect();
        Self { mode, words }
    }

    /// One word at `φ1 = 0`, from the base index (the label's bits above the φ1 pair).
    fn word(mode: CckMode, index: u32) -> [Complex<f32>; CHIPS] {
        let (p2, p3, p4) = match mode {
            CckMode::Bits8 => (
                quadrant(index & 3),
                quadrant((index >> 2) & 3),
                quadrant((index >> 4) & 3),
            ),
            // The reduced alphabet: φ2 is a BPSK pair offset a quarter turn, φ3 is fixed at 0,
            // φ4 is a plain BPSK pair. Two bits, four words.
            CckMode::Bits4 => (
                quadrant(1 + 2 * (index & 1)),
                quadrant(0),
                quadrant(2 * ((index >> 1) & 1)),
            ),
        };
        [
            p2 * p3 * p4,
            p3 * p4,
            p2 * p4,
            -p4,
            p2 * p3,
            p3,
            -p2,
            Complex::new(1.0, 0.0),
        ]
    }

    #[must_use]
    pub fn mode(&self) -> CckMode {
        self.mode
    }

    #[must_use]
    pub fn words(&self) -> &[[Complex<f32>; CHIPS]] {
        &self.words
    }

    /// The chips a label transmits at symbol index `symbol`, at the amplitude that makes a
    /// symbol's energy 1: the base word times φ1's rotation, times the spec's odd-symbol π.
    pub fn chips(&self, label: u32, symbol: usize, out: &mut [Complex<f32>; CHIPS]) {
        let word = &self.words[(label >> 2) as usize % self.words.len()];
        let rotation = quadrant(label + 2 * (symbol as u32 & 1)) * (CHIPS as f32).sqrt().recip();
        for (slot, &chip) in out.iter_mut().zip(word) {
            *slot = chip * rotation;
        }
    }

    /// The symbol-domain value a correct correlation produces: the unit phasor φ1 rotated into,
    /// odd-symbol π included. This is what the §3.4 anchor fits against, and the value the
    /// entry's noise variance is measured as a residual from.
    #[must_use]
    pub fn reference(&self, label: u32, symbol: usize) -> Complex<f32> {
        quadrant(label + 2 * (symbol as u32 & 1))
    }

    /// The correlator bank: `r[i] = (1/√8)·Σ_c y_c·conj(w_i[c])`, normalised so the winning
    /// correlation of a clean symbol has unit magnitude and every `r[i]` carries the chip
    /// stream's own N0 — the same normalisation the despreader beside it uses, so the two
    /// entries' noise variances mean one thing.
    ///
    /// # Panics
    /// If `out` is shorter than the bank.
    pub fn correlate(&self, chips: &[Complex<f32>; CHIPS], out: &mut [Complex<f32>]) {
        assert!(
            out.len() >= self.words.len(),
            "the bank has {} words; {} slots cannot hold it",
            self.words.len(),
            out.len()
        );
        let scale = (CHIPS as f64).sqrt().recip();
        for (slot, word) in out.iter_mut().zip(&self.words) {
            let mut re = 0.0f64;
            let mut im = 0.0f64;
            for (&y, &w) in chips.iter().zip(word) {
                re += f64::from(y.re) * f64::from(w.re) + f64::from(y.im) * f64::from(w.im);
                im += f64::from(y.im) * f64::from(w.re) - f64::from(y.re) * f64::from(w.im);
            }
            *slot = Complex::new((re * scale) as f32, (im * scale) as f32);
        }
    }

    /// The joint maximum-likelihood decision over word *and* φ1, from a correlator bank.
    ///
    /// Not `argmax |r_i|`, which is the tempting shortcut and is wrong: every candidate has the
    /// same energy, so the likelihood is `Re(r_i·e^{−jφ1})`, and a slightly weaker correlation
    /// sitting exactly on a QPSK axis beats a stronger one sitting between two. The difference is
    /// only visible in noise, which is where it matters.
    #[must_use]
    pub fn decide(&self, bank: &[Complex<f32>], symbol: usize) -> u32 {
        let mut best = (0u32, f32::NEG_INFINITY);
        for label in 0..self.mode.alphabet() {
            let metric = self.metric(bank, label, symbol);
            if metric > best.1 {
                best = (label, metric);
            }
        }
        best.0
    }

    /// The log-likelihood of `label`, up to the common scale every candidate shares:
    /// `Re(r_i·e^{−jφ1})` with the odd-symbol rotation folded in. One of `±Re`, `±Im` of the
    /// word's own correlation, since φ1 is a quadrant.
    #[must_use]
    pub fn metric(&self, bank: &[Complex<f32>], label: u32, symbol: usize) -> f32 {
        let r = bank[(label >> 2) as usize % self.words.len()];
        match (label + 2 * (symbol as u32 & 1)) & 3 {
            0 => r.re,
            1 => r.im,
            2 => -r.re,
            _ => -r.im,
        }
    }

    /// Per-bit max-log LLRs over the joint alphabet.
    ///
    /// The crate's one point demapper cannot serve here and the reason is structural rather than
    /// an omission: its observation is a *point* and this one is a vector of eight chips. What is
    /// shared is the form — max-log over an alphabet — and the calibration: all codewords carry
    /// equal energy, so `|y − c|²` reduces to `−2·Re⟨y, c⟩` and the LLR is
    /// `(2/N0)·(max_{bit=1} metric − max_{bit=0} metric)`.
    ///
    /// # Panics
    /// If `out` is not `bits_per_symbol` long, or `noise_var` is not a positive finite number.
    pub fn llrs(&self, bank: &[Complex<f32>], symbol: usize, noise_var: f64, out: &mut [Llr]) {
        let bits = self.mode.bits_per_symbol();
        assert_eq!(out.len(), bits, "one LLR slot per codeword bit");
        assert!(
            noise_var.is_finite() && noise_var > 0.0,
            "noise_var is a measured variance; {noise_var} is not one"
        );
        let mut max0 = [f32::NEG_INFINITY; 8];
        let mut max1 = [f32::NEG_INFINITY; 8];
        for label in 0..self.mode.alphabet() {
            let metric = self.metric(bank, label, symbol);
            for k in 0..bits {
                if (label >> k) & 1 == 1 {
                    max1[k] = max1[k].max(metric);
                } else {
                    max0[k] = max0[k].max(metric);
                }
            }
        }
        let scale = 2.0 / noise_var;
        for (k, slot) in out.iter_mut().enumerate() {
            *slot = Llr((f64::from(max1[k] - max0[k]) * scale) as f32);
        }
    }
}

/// One CCK waveform as data: the rate, the chip pulse, and the burst search's coherent span.
#[derive(Clone, Debug)]
pub struct CckParams {
    codebook: Codebook,
    shaper: ChipShaper,
    search_group_symbols: usize,
}

impl CckParams {
    /// # Panics
    /// If the search group is zero — a burst search integrating over no symbols has nothing to
    /// peak on.
    #[must_use]
    pub fn new(mode: CckMode, shaper: ChipShaper, search_group_symbols: usize) -> Self {
        assert!(search_group_symbols > 0, "a search group spans ≥ 1 symbol");
        Self {
            codebook: Codebook::new(mode),
            shaper,
            search_group_symbols,
        }
    }

    #[must_use]
    pub fn codebook(&self) -> &Codebook {
        &self.codebook
    }

    #[must_use]
    pub fn shaper(&self) -> &ChipShaper {
        &self.shaper
    }

    #[must_use]
    pub fn mode(&self) -> CckMode {
        self.codebook.mode()
    }

    #[must_use]
    pub fn bits_per_symbol(&self) -> usize {
        self.codebook.mode().bits_per_symbol()
    }

    #[must_use]
    pub fn symbol_samples(&self) -> usize {
        CHIPS * self.shaper.sps()
    }

    /// Eb charged to a frame's own framing, in dB — the same closed form of the geometry the
    /// direct-sequence entry's is: `10·log₁₀((P + L)/L)` over symbols, independent of the rate
    /// because the bits per symbol cancel out of the ratio.
    #[must_use]
    pub fn framing_overhead_db(preamble: usize, payload: usize) -> f64 {
        10.0 * ((preamble + payload) as f64 / payload as f64).log10()
    }
}

/// The transmitter. Cold path, like every modulator in this crate.
#[derive(Clone, Debug)]
pub struct CckMod {
    params: CckParams,
    chips: Vec<Complex<f32>>,
}

impl CckMod {
    #[must_use]
    pub fn new(params: CckParams) -> Self {
        Self {
            params,
            chips: Vec::new(),
        }
    }

    #[must_use]
    pub fn params(&self) -> &CckParams {
        &self.params
    }

    /// A whole burst: the known preamble's labels, then the payload's. The burst's origin is
    /// `out.len()` as measured when this is called, and symbol indices — which the odd-symbol
    /// rotation reads — count from the preamble's first symbol.
    pub fn frame(&mut self, preamble: &[u32], labels: &[u32], out: &mut Vec<Complex<f32>>) {
        self.chips.clear();
        self.push_labels(preamble, 0);
        self.push_labels(labels, preamble.len());
        self.params.shaper.render(&self.chips, out);
    }

    fn push_labels(&mut self, labels: &[u32], first_symbol: usize) {
        let mut word = [Complex::new(0.0, 0.0); CHIPS];
        for (k, &label) in labels.iter().enumerate() {
            self.params
                .codebook
                .chips(label, first_symbol + k, &mut word);
            self.chips.extend_from_slice(&word);
        }
    }

    /// The preamble's chips alone — what [`CckDemod::acquire`] correlates against.
    #[must_use]
    pub fn preamble_chips(&self, preamble: &[u32]) -> Vec<Complex<f32>> {
        let mut chips = Vec::with_capacity(preamble.len() * CHIPS);
        let mut word = [Complex::new(0.0, 0.0); CHIPS];
        for (k, &label) in preamble.iter().enumerate() {
            self.params.codebook.chips(label, k, &mut word);
            chips.extend_from_slice(&word);
        }
        chips
    }
}

/// The receiver: matched filter, burst search, correlator bank, anchor.
#[derive(Clone, Debug)]
pub struct CckDemod {
    params: CckParams,
    filtered: Vec<Complex<f32>>,
    known: Vec<Complex<f32>>,
    fitted: Vec<Complex<f32>>,
    expected: Vec<Complex<f32>>,
    acquisition: Option<super::dsss::Acquisition>,
}

impl CckDemod {
    #[must_use]
    pub fn new(params: CckParams) -> Self {
        Self {
            params,
            filtered: Vec::new(),
            known: Vec::new(),
            fitted: Vec::new(),
            expected: Vec::new(),
            acquisition: None,
        }
    }

    #[must_use]
    pub fn params(&self) -> &CckParams {
        &self.params
    }

    #[must_use]
    pub fn acquisition(&self) -> Option<super::dsss::Acquisition> {
        self.acquisition
    }

    /// Matched-filters `wave`, finds the burst whose first symbols carry `preamble`, and fits the
    /// §3.4 anchor over them.
    ///
    /// `None` when the search was impossible or the anchor would not fit — a preamble shorter than
    /// two symbols, an empty origin range, or a degenerate fit. It is *not* a detector: given a
    /// searchable range the correlator always names its best origin, so a `Some` says "this is
    /// where the burst is if there is one", not "there is one".
    ///
    /// Steady state allocates nothing: every buffer reaches its capacity on the first call and is
    /// reused, which is what the §4.2 gate asserts.
    pub fn acquire(
        &mut self,
        wave: &[Complex<f32>],
        preamble: &[u32],
        search: usize,
    ) -> Option<super::dsss::Acquisition> {
        self.acquisition = None;
        if preamble.len() < 2 {
            return None;
        }
        self.params.shaper.matched(wave, &mut self.filtered);

        self.known.clear();
        let mut word = [Complex::new(0.0, 0.0); CHIPS];
        for (k, &label) in preamble.iter().enumerate() {
            self.params.codebook.chips(label, k, &mut word);
            self.known.extend_from_slice(&word);
        }
        let group_chips = self.params.search_group_symbols * CHIPS;
        let origin = find_burst(
            &self.params.shaper,
            &self.filtered,
            &self.known,
            group_chips,
            search,
        )?;

        // The anchor reads the *known* word's correlation, not the winning one: at the SNRs an
        // acquisition has to work at, a decision-directed reference would occasionally fit the
        // rotation to the wrong codeword and take the whole burst with it.
        self.fitted.clear();
        self.expected.clear();
        let mut bank = [Complex::new(0.0, 0.0); MAX_WORDS];
        let bank = &mut bank[..self.params.codebook.words().len()];
        for (k, &label) in preamble.iter().enumerate() {
            self.bank_at(origin, k, bank);
            self.fitted.push(bank[(label >> 2) as usize % bank.len()]);
            self.expected.push(self.params.codebook.reference(label, k));
        }
        // Gain only, no slope: the direct-sequence entry beside this one records the measurement
        // (`dsss`'s module docs) — a slope fitted on a short preamble and extrapolated across a
        // long payload is worth more phase error than it removes, and both entries frame bursts
        // the same way.
        let anchor = PhaseAnchor::fit_gain_only(&self.fitted, &self.expected).ok()?;
        anchor.correct_block(0, &mut self.fitted);
        let noise_var =
            crate::constellation::demap::noise_var_from_known(&self.fitted, &self.expected);
        let acquisition = super::dsss::Acquisition {
            origin,
            anchor,
            noise_var,
        };
        self.acquisition = Some(acquisition);
        Some(acquisition)
    }

    /// The correlator bank of symbol `symbol` (counted from the burst's first), with the
    /// acquisition's gain and residual frequency removed.
    ///
    /// # Panics
    /// If `out` is not the bank's length.
    pub fn bank(&self, symbol: usize, out: &mut [Complex<f32>]) {
        let Some(acquisition) = self.acquisition else {
            out.fill(Complex::new(0.0, 0.0));
            return;
        };
        self.bank_at(acquisition.origin, symbol, out);
        for slot in out.iter_mut() {
            *slot = acquisition.anchor.correct(symbol, *slot);
        }
    }

    /// The raw bank, before any correction — the form acquisition needs, since the correction is
    /// what it is still fitting.
    fn bank_at(&self, origin: usize, symbol: usize, out: &mut [Complex<f32>]) {
        let mut chips = [Complex::new(0.0, 0.0); CHIPS];
        self.params
            .shaper
            .block(&self.filtered, origin, symbol * CHIPS, &mut chips);
        self.params.codebook.correlate(&chips, out);
    }

    /// `symbols` payload labels, appended to `out`. Writes nothing without an acquisition (§8).
    pub fn demodulate(&self, preamble_symbols: usize, symbols: usize, out: &mut Vec<u32>) {
        if self.acquisition.is_none() {
            return;
        }
        let mut bank = [Complex::new(0.0, 0.0); MAX_WORDS];
        let bank = &mut bank[..self.params.codebook.words().len()];
        out.reserve(symbols);
        for k in 0..symbols {
            let index = preamble_symbols + k;
            self.bank(index, bank);
            out.push(self.params.codebook.decide(bank, index));
        }
    }

    /// Per-bit LLRs of `symbols` payload symbols, at the noise variance the acquisition
    /// measured. Appends `symbols · bits_per_symbol` values in transmission order.
    pub fn llrs(&self, preamble_symbols: usize, symbols: usize, out: &mut Vec<Llr>) {
        let Some(acquisition) = self.acquisition else {
            return;
        };
        let bits = self.params.bits_per_symbol();
        let mut bank = [Complex::new(0.0, 0.0); MAX_WORDS];
        let bank = &mut bank[..self.params.codebook.words().len()];
        let mut symbol_llrs = [Llr(0.0); 8];
        let symbol_llrs = &mut symbol_llrs[..bits];
        let noise_var = acquisition.noise_var.max(f64::MIN_POSITIVE);
        out.reserve(symbols * bits);
        for k in 0..symbols {
            let index = preamble_symbols + k;
            self.bank(index, bank);
            self.params
                .codebook
                .llrs(bank, index, noise_var, symbol_llrs);
            out.extend_from_slice(symbol_llrs);
        }
    }
}

/// The 8-chip codeword's amplitude per chip at unit symbol energy — `1/√8`, written once so the
/// modulator and the tests read the same constant.
pub const CHIP_AMPLITUDE: f32 = FRAC_1_SQRT_2 * 0.5;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ber::rng::Rng;

    const SPS: usize = 4;
    const LEAD: usize = 41;
    const PREAMBLE: usize = 32;

    fn params(mode: CckMode) -> CckParams {
        CckParams::new(mode, ChipShaper::root_raised_cosine(SPS, 0.35, 8), 8)
    }

    fn labels(mode: CckMode, count: usize, seed: u32) -> Vec<u32> {
        let mut state = seed | 1;
        (0..count)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state % mode.alphabet()
            })
            .collect()
    }

    fn transmit(p: &CckParams, preamble: &[u32], payload: &[u32]) -> Vec<Complex<f32>> {
        let mut wave = vec![Complex::new(0.0, 0.0); LEAD];
        CckMod::new(p.clone()).frame(preamble, payload, &mut wave);
        wave.resize(wave.len() + 128, Complex::new(0.0, 0.0));
        wave
    }

    /// The codebook's three structural properties, which is what a *generated* table can be held
    /// to where a transcribed one could not: every chip has unit modulus (so the transmitter is
    /// constant-envelope), no two words coincide, and φ1 is a pure rotation — the property the
    /// bank's `M/4` size rests on.
    #[test]
    fn the_generated_codebook_is_constant_modulus_distinct_and_factors_phi1_out() {
        for mode in [CckMode::Bits4, CckMode::Bits8] {
            let book = Codebook::new(mode);
            assert_eq!(book.words().len(), mode.words());
            for (i, word) in book.words().iter().enumerate() {
                for (c, chip) in word.iter().enumerate() {
                    assert!(
                        (chip.norm() - 1.0).abs() < 1e-6,
                        "{mode:?} word {i} chip {c} modulus {}",
                        chip.norm()
                    );
                }
                for (j, other) in book.words().iter().enumerate().skip(i + 1) {
                    let same = word.iter().zip(other).all(|(a, b)| (a - b).norm() < 1e-6);
                    assert!(!same, "{mode:?} words {i} and {j} coincide");
                }
            }
            // φ1 rotates the whole word and nothing else: the chips of `label` and of the same
            // label with a different φ1 differ by one constant phasor.
            let mut a = [Complex::new(0.0, 0.0); CHIPS];
            let mut b = [Complex::new(0.0, 0.0); CHIPS];
            let base = 2u32 << 2;
            book.chips(base, 0, &mut a);
            book.chips(base + 1, 0, &mut b);
            let ratio = b[0] / a[0];
            for c in 1..CHIPS {
                assert!((b[c] / a[c] - ratio).norm() < 1e-5, "{mode:?} chip {c}");
            }
            assert!((ratio - Complex::new(0.0, 1.0)).norm() < 1e-5);
        }
    }

    /// **CCK is a block code, and this is the number that says so.** Eight complex chips are
    /// sixteen real dimensions carrying eight bits — the same half-bit-per-dimension a bare BPSK
    /// symbol carries — so the 256-word set is free to be *better* packed than 256 independent
    /// binary decisions, and it is: its minimum squared distance is 1 against an antipodal pair's
    /// 4 at the same symbol energy, which at eight bits a symbol against one is 3 dB in CCK's
    /// favour per Eb asymptotically. The committed curve measures 1.4 dB of that at 1e-3, and the
    /// rest is what the *24* nearest neighbours below cost through the union bound — a dense shell
    /// is the price a good packing pays.
    ///
    /// The minimum is reached two ways, and both are a quarter turn: one of φ2..φ4 alone (which
    /// changes four of the eight chips), or φ1 against one of them (which changes the other four).
    /// φ1's own quarter turn changes all eight and sits twice as far, which is why the correlator
    /// bank can afford to decide the word first and the phase second.
    #[test]
    fn the_codebook_is_a_block_code_with_the_distance_that_earns_its_rate() {
        let book = Codebook::new(CckMode::Bits8);
        let chips_of = |label: u32| {
            let mut out = [Complex::new(0.0, 0.0); CHIPS];
            book.chips(label, 0, &mut out);
            out
        };
        let d2 = |x: u32, y: u32| -> f64 {
            chips_of(x)
                .iter()
                .zip(&chips_of(y))
                .map(|(&p, &q)| f64::from((p - q).norm_sqr()))
                .sum()
        };
        let mut d2_min = f64::INFINITY;
        for x in 0..256u32 {
            for y in 0..256u32 {
                if x != y {
                    d2_min = d2_min.min(d2(x, y));
                }
            }
        }
        assert!((d2_min - 1.0).abs() < 1e-5, "d²_min {d2_min}");

        // The shell is the same size around every word — the geometry is a group, so a union
        // bound read off one word is the bound for all of them.
        for x in 0..256u32 {
            let neighbours = (0..256u32)
                .filter(|&y| y != x && (d2(x, y) - d2_min).abs() < 1e-5)
                .count();
            assert_eq!(
                neighbours, 24,
                "word {x} has {neighbours} nearest neighbours"
            );
        }

        // φ1's quarter turn is the *other* distance, twice the minimum — the separation the
        // decision's two stages rest on.
        assert!(
            (d2(0, 1) - 2.0).abs() < 1e-5,
            "φ1 quarter turn d² {}",
            d2(0, 1)
        );
    }

    /// A codeword's energy is 1, which is what puts CCK's Eb/N0 on the same axis as every other
    /// entry's (crate-root convention).
    #[test]
    fn a_codeword_carries_unit_energy() {
        let book = Codebook::new(CckMode::Bits8);
        let mut word = [Complex::new(0.0, 0.0); CHIPS];
        for label in [0u32, 1, 37, 200, 255] {
            book.chips(label, 0, &mut word);
            let energy: f64 = word.iter().map(|c| f64::from(c.norm_sqr())).sum();
            assert!((energy - 1.0).abs() < 1e-6, "label {label} energy {energy}");
            assert!((word[0].norm() - CHIP_AMPLITUDE).abs() < 1e-6);
        }
    }

    /// The bank recovers the sent codeword exactly on a clean channel, and its winning
    /// correlation is the unit phasor the anchor fits against.
    #[test]
    fn the_bank_recovers_a_clean_codeword_and_its_phase() {
        for mode in [CckMode::Bits4, CckMode::Bits8] {
            let book = Codebook::new(mode);
            let mut word = [Complex::new(0.0, 0.0); CHIPS];
            let mut bank = vec![Complex::new(0.0, 0.0); book.words().len()];
            for label in 0..mode.alphabet() {
                for symbol in 0..2 {
                    book.chips(label, symbol, &mut word);
                    book.correlate(&word, &mut bank);
                    assert_eq!(book.decide(&bank, symbol), label, "{mode:?} label {label}");
                    let winner = bank[(label >> 2) as usize % bank.len()];
                    let want = book.reference(label, symbol);
                    assert!((winner - want).norm() < 1e-5, "{mode:?} label {label}");
                }
            }
        }
    }

    /// Both rates round-trip through the whole chain, origin searched rather than known.
    #[test]
    fn both_rates_round_trip_and_find_their_own_origin() {
        for mode in [CckMode::Bits4, CckMode::Bits8] {
            let p = params(mode);
            let preamble = labels(mode, PREAMBLE, 0x0cc4);
            let payload = labels(mode, 400, 0x0cc8);
            let wave = transmit(&p, &preamble, &payload);
            let mut demod = CckDemod::new(p);
            let acquisition = demod
                .acquire(&wave, &preamble, LEAD + 48)
                .unwrap_or_else(|| panic!("{mode:?}: no acquisition"));
            assert_eq!(acquisition.origin, LEAD, "{mode:?}");
            let mut got = Vec::new();
            demod.demodulate(PREAMBLE, payload.len(), &mut got);
            assert_eq!(got, payload, "{mode:?}");
        }
    }

    /// The joint decision is not `argmax |r|`, and the difference is measurable: a bank in which
    /// the strongest correlation sits between two QPSK axes while a weaker one sits on an axis
    /// must decode to the weaker word.
    #[test]
    fn the_decision_is_joint_over_word_and_phase() {
        let book = Codebook::new(CckMode::Bits8);
        let mut bank = vec![Complex::new(0.0, 0.0); book.words().len()];
        // Word 3 at magnitude 1 exactly between two axes; word 5 at 0.95 on the real axis.
        bank[3] = Complex::new(FRAC_1_SQRT_2, FRAC_1_SQRT_2);
        bank[5] = Complex::new(0.95, 0.0);
        assert!(bank[3].norm() > bank[5].norm());
        let decided = book.decide(&bank, 0);
        assert_eq!(decided >> 2, 5, "decided word {}", decided >> 2);
        assert_eq!(decided & 3, 0);
    }

    /// LLR calibration on the joint alphabet: among bits reported at confidence |llr|, the
    /// fraction wrong must track `1/(1+e^|llr|)`. This is what makes the entry's soft output an
    /// [`Llr`] rather than a confidence, and it exercises the noise variance the acquisition
    /// measured end to end.
    #[test]
    fn llr_magnitudes_predict_their_own_error_rate() {
        let mode = CckMode::Bits8;
        let p = params(mode);
        let preamble = labels(mode, 128, 0x11c4);
        let payload = labels(mode, 12_000, 0x11c5);
        let mut wave = transmit(&p, &preamble, &payload);
        let mut rng = Rng::new(0x11c6);
        // Es/N0 = 11 dB: 8 bits a symbol, so this is the operating point the entry's committed
        // curve sits near, and the band the LLR calibration has to hold across.
        let sigma = (0.08f64 / 2.0).sqrt();
        for s in wave.iter_mut() {
            *s += Complex::new((rng.normal() * sigma) as f32, (rng.normal() * sigma) as f32);
        }
        let mut demod = CckDemod::new(p);
        demod.acquire(&wave, &preamble, LEAD + 48).unwrap();
        let mut llrs = Vec::new();
        demod.llrs(preamble.len(), payload.len(), &mut llrs);

        let bits = mode.bits_per_symbol();
        let mut bands = [(0u32, 0u32); 3];
        for (i, &llr) in llrs.iter().enumerate() {
            let sent = (payload[i / bits] >> (i % bits)) & 1 == 1;
            let band = match llr.0.abs() {
                x if x < 1.0 => 0,
                x if x < 3.0 => 1,
                _ => 2,
            };
            bands[band].0 += 1;
            if llr.bit() != sent {
                bands[band].1 += 1;
            }
        }
        for (band, &(count, wrong)) in bands.iter().enumerate() {
            assert!(count > 300, "band {band} saw only {count} bits");
            let measured = f64::from(wrong) / f64::from(count);
            let predicted = 1.0 / (1.0 + [0.5f64, 2.0, 4.0][band].exp());
            assert!(
                measured < predicted * 3.0 + 0.02,
                "band {band}: measured {measured}, predicted {predicted}"
            );
        }
    }

    /// No acquisition, no symbols (§8: no silent failure).
    #[test]
    fn a_demodulator_with_no_acquisition_produces_nothing() {
        let demod = CckDemod::new(params(CckMode::Bits8));
        let mut labels = Vec::new();
        let mut llrs = Vec::new();
        demod.demodulate(PREAMBLE, 50, &mut labels);
        demod.llrs(PREAMBLE, 50, &mut llrs);
        assert!(labels.is_empty() && llrs.is_empty());
    }
}
