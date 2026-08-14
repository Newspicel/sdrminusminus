//! Direct-sequence spread spectrum (MODEM-PLAN §3.1 `spread/`, §6 frameworks table): one
//! constellation point per PN period, recovered by correlating the period back.
//!
//! **A framework, not a mapper** (§3.3). Nothing here knows what a BPSK point is: the modulator
//! takes points and the demodulator returns them, `constellation/` supplies the table and the
//! demapper, and the acceptance is that a spread curve lands on the *same* closed form its
//! unspread twin does. That identity is the entry's whole content under AWGN and it is worth
//! stating as arithmetic rather than as a claim: a chip carries `1/√N` of the symbol's amplitude,
//! so despreading multiplies the wanted term by `N` and the noise term — being incoherent across
//! chips — by `√N`; the ratio is unchanged. **Spreading buys nothing against thermal noise and
//! costs nothing either**, and every dB the committed curve sits from its oracle is framing,
//! never the spreader.
//!
//! **What it does buy is measured on a different axis.** Against interference narrow compared
//! with the chip rate, despreading spreads the interferer across `N` chips' worth of bandwidth
//! while collecting the wanted signal coherently, so the signal-to-interference ratio improves by
//! the processing gain `10·log₁₀(N)` ([`PnSequence::processing_gain_db`]). That number is the
//! entry's headline and `tests/spread.rs` measures it two ways — directly, as the correlator's
//! own input-to-output SIR, and as the C/I threshold two spreading factors fail at.
//!
//! **Acquisition is the correlator again.** A burst is found by sliding the *known* preamble's
//! own chips along the matched-filtered stream and taking the peak
//! ([`super::chip::find_burst`]) — which is why a Barker word is the canonical short
//! code: its aperiodic sidelobes are bounded by 1, so a partial, unaligned window cannot
//! impersonate the peak. The §3.4 hook then fits a complex gain over the same known symbols
//! ([`PhaseAnchor`]), and their residual is the entry's noise-variance estimate.
//!
//! **The anchor is fitted for gain only, and that is a measurement rather than a simplification.**
//! [`PhaseAnchor::fit`] will also fit a frequency slope, and across a preamble of `P` symbols at
//! the SNR an acquisition works at that slope carries an error of order `√(12/(ρ·P³))` radians per
//! symbol — harmless over the word it was fitted on and ruinous extrapolated across a payload
//! tens of times longer, which is the phase-6 pilot-extrapolation finding in this entry's
//! geometry. Measured on this chain, on an *unimpaired* burst at a 32-symbol preamble and a
//! 2048-symbol payload: the fitted slope loses a third of the payload where a constant gain loses
//! only the AWGN floor (`fitting_the_anchor_slope_costs_more_phase_than_it_removes`). So the
//! burst is treated as what
//! it is — short enough for one phase — and the §4.3 CFO row is exactly that statement, landing
//! in family with the linear engine's own coherent rows.

use num_complex::Complex;

use super::{
    chip::{ChipShaper, find_burst},
    pn::PnSequence,
};
use crate::{
    constellation::{Constellation, demap},
    linear::PhaseAnchor,
    soft::Llr,
};

/// Largest PN period the receive path holds scratch for. A period past this is a frame, not a
/// spreading factor, and it would want the chip stream buffered rather than a stack array —
/// which is a different engine, not a bigger constant.
pub const MAX_CHIPS: usize = 4_096;

/// One direct-sequence waveform as data: the spreading code, the chip pulse, and the coherent
/// span the burst search integrates over.
#[derive(Clone, Debug)]
pub struct DsssParams {
    pn: PnSequence,
    shaper: ChipShaper,
    search_group_symbols: usize,
}

impl DsssParams {
    /// # Panics
    /// If the PN period exceeds [`MAX_CHIPS`], or if the search group is zero — a burst search
    /// integrating over no symbols has nothing to peak on.
    #[must_use]
    pub fn new(pn: PnSequence, shaper: ChipShaper, search_group_symbols: usize) -> Self {
        assert!(
            pn.len() <= MAX_CHIPS,
            "PN period {} exceeds the receive path's {MAX_CHIPS}-chip scratch",
            pn.len()
        );
        assert!(search_group_symbols > 0, "a search group spans ≥ 1 symbol");
        Self {
            pn,
            shaper,
            search_group_symbols,
        }
    }

    #[must_use]
    pub fn pn(&self) -> &PnSequence {
        &self.pn
    }

    #[must_use]
    pub fn shaper(&self) -> &ChipShaper {
        &self.shaper
    }

    /// Chips per symbol — the spreading factor `N`.
    #[must_use]
    pub fn chips_per_symbol(&self) -> usize {
        self.pn.len()
    }

    /// Samples one symbol occupies.
    #[must_use]
    pub fn symbol_samples(&self) -> usize {
        self.pn.len() * self.shaper.sps()
    }

    /// The processing gain this spreading factor claims, `10·log₁₀(N)`.
    #[must_use]
    pub fn processing_gain_db(&self) -> f64 {
        self.pn.processing_gain_db()
    }

    /// Eb charged to a frame's own framing, in dB: a burst radiates `preamble + payload` symbols
    /// of equal energy and only the payload's carry information, so the sweep runner's
    /// energy-over-information-bits accounting charges `10·log₁₀((P + L)/L)` — a closed form of
    /// the geometry, identical for every constellation because the bits per symbol cancel out of
    /// the ratio.
    #[must_use]
    pub fn framing_overhead_db(preamble: usize, payload: usize) -> f64 {
        10.0 * ((preamble + payload) as f64 / payload as f64).log10()
    }

    /// Chips of `points` spread by the code, at the amplitude that makes a symbol's energy its
    /// point's squared magnitude: `chip = point · pn[c] / √N`.
    fn spread_into(&self, points: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        let scale = (self.pn.len() as f32).sqrt().recip();
        out.reserve(points.len() * self.pn.len());
        for &point in points {
            for &chip in self.pn.chips() {
                out.push(point * (chip * scale));
            }
        }
    }
}

/// The transmitter. Cold path (a signal generator and `tx.rs`'s source): it allocates its chip
/// buffer once and reuses it, and the §4.2 zero-allocation gate binds the receiver.
#[derive(Clone, Debug)]
pub struct DsssMod {
    params: DsssParams,
    chips: Vec<Complex<f32>>,
}

impl DsssMod {
    #[must_use]
    pub fn new(params: DsssParams) -> Self {
        Self {
            params,
            chips: Vec::new(),
        }
    }

    #[must_use]
    pub fn params(&self) -> &DsssParams {
        &self.params
    }

    /// A whole burst: the known preamble's symbols, then the payload's, appended to `out`. The
    /// burst's origin — the grid index [`DsssDemod::acquire`] reports — is `out.len()` as
    /// measured when this is called.
    pub fn frame(
        &mut self,
        preamble: &[Complex<f32>],
        points: &[Complex<f32>],
        out: &mut Vec<Complex<f32>>,
    ) {
        self.chips.clear();
        self.params.spread_into(preamble, &mut self.chips);
        self.params.spread_into(points, &mut self.chips);
        self.params.shaper.render(&self.chips, out);
    }

    /// The preamble's chips alone — what a receiver correlates against, and what the harness
    /// hands [`DsssDemod::acquire`].
    #[must_use]
    pub fn preamble_chips(&self, preamble: &[Complex<f32>]) -> Vec<Complex<f32>> {
        let mut chips = Vec::new();
        self.params.spread_into(preamble, &mut chips);
        chips
    }
}

/// Where the burst was, and everything read off its known symbols (§3.4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Acquisition {
    /// Grid index of the burst's first chip — the same index the modulator's `out.len()` was.
    pub origin: usize,
    /// The complex gain fitted over the preamble — scale and rotation, no slope (see the module
    /// docs for the measurement behind that).
    pub anchor: PhaseAnchor,
    /// Total complex noise variance N0 in the despread symbol domain, measured as the
    /// preamble's own residual — the calibrated variance the demapper needs to produce an
    /// [`Llr`] rather than a confidence.
    pub noise_var: f64,
}

/// The receiver: matched filter, burst search, despread, anchor.
#[derive(Clone, Debug)]
pub struct DsssDemod {
    params: DsssParams,
    filtered: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    acquisition: Option<Acquisition>,
}

impl DsssDemod {
    #[must_use]
    pub fn new(params: DsssParams) -> Self {
        Self {
            params,
            filtered: Vec::new(),
            scratch: Vec::new(),
            acquisition: None,
        }
    }

    #[must_use]
    pub fn params(&self) -> &DsssParams {
        &self.params
    }

    #[must_use]
    pub fn acquisition(&self) -> Option<Acquisition> {
        self.acquisition
    }

    /// Matched-filters `wave` and finds the burst whose preamble carries `preamble`, searching
    /// origins in `0..search`.
    ///
    /// `None` when the search was impossible or the anchor would not fit — a preamble shorter than
    /// two symbols, an empty origin range, or a degenerate fit. It is *not* a detector: given a
    /// searchable range the correlator always names its best origin, so a `Some` says "this is
    /// where the burst is if there is one", not "there is one".
    ///
    /// Steady state allocates nothing: both buffers reach their capacity on the first call and
    /// are reused, which is what the §4.2 gate asserts.
    pub fn acquire(
        &mut self,
        wave: &[Complex<f32>],
        preamble: &[Complex<f32>],
        search: usize,
    ) -> Option<Acquisition> {
        self.acquisition = None;
        if preamble.len() < 2 {
            return None;
        }
        self.params.shaper.matched(wave, &mut self.filtered);

        self.scratch.clear();
        self.params.spread_into(preamble, &mut self.scratch);
        let group_chips = self.params.search_group_symbols * self.params.chips_per_symbol();
        let origin = find_burst(
            &self.params.shaper,
            &self.filtered,
            &self.scratch,
            group_chips,
            search,
        )?;

        // Despread the preamble in place over the scratch the known chips no longer need.
        self.scratch.clear();
        for symbol in 0..preamble.len() {
            self.scratch.push(self.despread(origin, symbol));
        }
        let anchor = PhaseAnchor::fit_gain_only(&self.scratch, preamble).ok()?;
        anchor.correct_block(0, &mut self.scratch);
        let noise_var = demap::noise_var_from_known(&self.scratch, preamble);
        let acquisition = Acquisition {
            origin,
            anchor,
            noise_var,
        };
        self.acquisition = Some(acquisition);
        Some(acquisition)
    }

    /// Declares the acquisition instead of searching for it — the comparison receiver, and the
    /// entry point a protocol attachment with its own burst detector uses.
    pub fn genie(&mut self, wave: &[Complex<f32>], acquisition: Acquisition) {
        self.params.shaper.matched(wave, &mut self.filtered);
        self.acquisition = Some(acquisition);
    }

    /// `symbols` payload points, appended to `out`, from the acquired burst. Payload symbol 0 is
    /// the one after the preamble, so `preamble_symbols` states where the payload starts.
    ///
    /// Writes nothing when nothing has been acquired: a demodulator with no origin has no
    /// symbols, and inventing them at zero would be a silent failure (§8).
    pub fn demodulate(&self, preamble_symbols: usize, symbols: usize, out: &mut Vec<Complex<f32>>) {
        let Some(acquisition) = self.acquisition else {
            return;
        };
        out.reserve(symbols);
        for k in 0..symbols {
            let index = preamble_symbols + k;
            let raw = self.despread(acquisition.origin, index);
            out.push(acquisition.anchor.correct(index, raw));
        }
    }

    /// One symbol's despread value: the PN correlation over the symbol's chips, normalised by
    /// `√N` so the result is on the constellation's own scale and its noise variance is the
    /// chip stream's N0 unchanged (the module docs' arithmetic).
    #[must_use]
    pub fn despread(&self, origin: usize, symbol: usize) -> Complex<f32> {
        let n = self.params.chips_per_symbol();
        let sps = self.params.shaper.sps();
        let chips = self.params.pn.chips();
        let mut re = 0.0f64;
        let mut im = 0.0f64;
        for (c, &code) in chips.iter().enumerate() {
            let at = origin + (symbol * n + c) * sps;
            let Some(&y) = self.filtered.get(at) else {
                continue;
            };
            re += f64::from(code) * f64::from(y.re);
            im += f64::from(code) * f64::from(y.im);
        }
        let scale = (n as f64).sqrt().recip();
        Complex::new((re * scale) as f32, (im * scale) as f32)
    }

    /// Per-bit LLRs of despread points through the crate's one demapper, at the noise variance
    /// the acquisition measured. Appends `points.len() · bits_per_symbol` values.
    ///
    /// Writes zeros — the erasure a decoder reads as "no information" — when nothing has been
    /// acquired, for the reason [`Self::demodulate`] writes nothing: an LLR at an invented
    /// variance looks calibrated and is not, which is exactly the silent failure §8 forbids.
    ///
    /// # Panics
    /// If `out` is not exactly `points.len() · table.bits_per_symbol()` long.
    pub fn llrs(&self, points: &[Complex<f32>], table: &Constellation, out: &mut [Llr]) {
        let bits = table.bits_per_symbol();
        assert_eq!(
            out.len(),
            points.len() * bits,
            "one LLR slot per payload bit"
        );
        let Some(acquisition) = self.acquisition else {
            out.fill(Llr(0.0));
            return;
        };
        let noise_var = acquisition.noise_var.max(f64::MIN_POSITIVE);
        for (point, slot) in points.iter().zip(out.chunks_exact_mut(bits)) {
            demap::max_log_llrs(*point, table, noise_var, slot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ber::rng::Rng, constellation::tables, spread::pn::PnSequence};

    const SPS: usize = 4;
    const LEAD: usize = 53;
    const PREAMBLE: usize = 32;

    fn params(pn: PnSequence) -> DsssParams {
        DsssParams::new(pn, ChipShaper::root_raised_cosine(SPS, 0.35, 8), 8)
    }

    /// Deterministic points drawn from a table — the shape every chain in this module's tests
    /// uses, so a failure is never the generator's.
    fn points(table: &Constellation, count: usize, seed: u32) -> Vec<Complex<f32>> {
        let mut state = seed | 1;
        (0..count)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                table.points()[state as usize % table.len()]
            })
            .collect()
    }

    fn transmit(
        p: &DsssParams,
        preamble: &[Complex<f32>],
        payload: &[Complex<f32>],
    ) -> Vec<Complex<f32>> {
        let mut wave = vec![Complex::new(0.0, 0.0); LEAD];
        DsssMod::new(p.clone()).frame(preamble, payload, &mut wave);
        wave.resize(wave.len() + 128, Complex::new(0.0, 0.0));
        wave
    }

    /// A genie acquisition at a known origin with no correction at all — what a test uses when
    /// the thing under measurement is the correlator and an estimated gain would fold its own
    /// error into the reading.
    fn identity(origin: usize, any: &[Complex<f32>]) -> Acquisition {
        Acquisition {
            origin,
            anchor: PhaseAnchor::fit_gain_only(any, any).unwrap(),
            noise_var: 1.0,
        }
    }

    fn add_noise(wave: &mut [Complex<f32>], seed: u64, noise_var: f64) {
        let mut rng = Rng::new(seed);
        let sigma = (noise_var / 2.0).sqrt();
        for s in wave.iter_mut() {
            *s += Complex::new((rng.normal() * sigma) as f32, (rng.normal() * sigma) as f32);
        }
    }

    /// Every spreading factor round-trips, and the burst search finds the origin it was actually
    /// sent at — the whole chain on a clean channel, before any statistics.
    #[test]
    fn every_spreading_factor_round_trips_and_finds_its_own_origin() {
        let table = tables::qam_square(4).unwrap();
        for pn in [
            PnSequence::barker(11).unwrap(),
            PnSequence::barker(13).unwrap(),
            PnSequence::maximal_length(5).unwrap(),
            PnSequence::maximal_length(6).unwrap(),
        ] {
            let n = pn.len();
            let p = params(pn);
            let preamble = points(&table, PREAMBLE, 0x5eed);
            let payload = points(&table, 200, 0xbeef);
            let wave = transmit(&p, &preamble, &payload);
            let mut demod = DsssDemod::new(p);
            let acquisition = demod
                .acquire(&wave, &preamble, LEAD + 64)
                .unwrap_or_else(|| panic!("N = {n}: no acquisition"));
            assert_eq!(acquisition.origin, LEAD, "N = {n}");
            let mut got = Vec::new();
            demod.demodulate(PREAMBLE, payload.len(), &mut got);
            for (k, (&g, &s)) in got.iter().zip(&payload).enumerate() {
                assert!((g - s).norm() < 5e-3, "N = {n}, symbol {k}: {g} vs {s}");
            }
        }
    }

    /// The entry's acceptance in its purest form, before any curve: **despreading leaves Es/N0
    /// exactly where it found it**. The despread symbols' error variance must equal the chip
    /// stream's N0 at every spreading factor — if the correlator's normalisation were wrong the
    /// committed curve would sit a spreading-factor-dependent distance from its oracle, and
    /// nothing downstream would say which.
    #[test]
    fn despreading_leaves_the_symbol_snr_unchanged_at_every_factor() {
        let table = tables::pam(2).unwrap();
        let noise_var = 0.25;
        for pn in [
            PnSequence::barker(11).unwrap(),
            PnSequence::maximal_length(5).unwrap(),
            PnSequence::maximal_length(7).unwrap(),
        ] {
            let n = pn.len();
            let p = params(pn);
            let preamble = points(&table, PREAMBLE, 0x0d55);
            let payload = points(&table, 4_000, 0x0d56);
            let mut wave = transmit(&p, &preamble, &payload);
            add_noise(&mut wave, 0xd555 + n as u64, noise_var);
            // Origin and gain given: what is under test is the correlator's normalisation, and a
            // fitted gain would fold its own estimation error into the residual being read.
            let mut demod = DsssDemod::new(p);
            demod.genie(&wave, identity(LEAD, &payload));
            let mut got = Vec::new();
            demod.demodulate(PREAMBLE, payload.len(), &mut got);
            let measured = demap::noise_var_from_known(&got, &payload);
            assert!(
                (measured / noise_var - 1.0).abs() < 0.05,
                "N = {n}: despread N0 {measured} vs chip-stream {noise_var}"
            );
        }
    }

    /// **The entry's headline, measured on the correlator itself rather than through a BER.**
    ///
    /// Processing gain is defined against an *unspread system of the same data rate*, and that
    /// comparison is the whole content of the measurement — so the chains here all carry the same
    /// symbol duration (44 samples) and differ only in how it is divided into chips: 1 chip of 44
    /// samples, 4 of 11, 11 of 4. A narrowband interferer swept across each receiver's *own* chip
    /// band then lands at the despreader with power `A²·T_s/N`, and the ratio to the unspread
    /// chain's is `N`.
    ///
    /// Two details are the measurement rather than incidental to it. The sweep is over a *band*
    /// and not a value because a single tone reads the sequence's own spectrum at one point —
    /// and at DC it reads the sequence's *balance*, where a balanced code rejects by `N²` and
    /// flatters itself. And the interferer is swept over the chip band rather than the data band
    /// because that is where a spread receiver is exposed: outside it the chip filter has already
    /// removed the jammer, and rejection there is filtering, not spreading.
    #[test]
    fn the_correlator_rejects_a_narrowband_interferer_by_its_processing_gain() {
        const SYMBOL_SAMPLES: usize = 44;
        let table = tables::pam(2).unwrap();
        let symbols = 64;

        // Jammer power collected per despread symbol, averaged over where the jammer sits in this
        // chain's chip band: `sps` samples per chip put the chip rate at 1/sps of the sample rate,
        // so the band is ±1/(2·sps) cycles per sample.
        let collected = |pn: PnSequence, sps: usize| {
            let p = DsssParams::new(pn, ChipShaper::root_raised_cosine(sps, 0.35, 8), 4);
            assert_eq!(p.symbol_samples(), SYMBOL_SAMPLES);
            let preamble = points(&table, PREAMBLE, 0x1c1);
            let payload = vec![table.points()[0]; symbols];
            let clean = transmit(&p, &preamble, &payload);
            let mut demod = DsssDemod::new(p);
            let mut total = 0.0f64;
            let steps = 128;
            for step in 0..steps {
                let offset = (f64::from(step) / f64::from(steps) - 0.5) / sps as f64;
                let mut wave = clean.clone();
                for (index, s) in wave.iter_mut().enumerate() {
                    let phase = std::f64::consts::TAU * offset * index as f64;
                    *s += Complex::new((0.1 * phase.cos()) as f32, (0.1 * phase.sin()) as f32);
                }
                demod.genie(&wave, identity(LEAD, &payload));
                let mut got = Vec::new();
                demod.demodulate(PREAMBLE, symbols, &mut got);
                total += demap::noise_var_from_known(&got, &payload) / f64::from(steps);
            }
            total
        };

        let unspread = collected(PnSequence::from_chips(&[1]).unwrap(), SYMBOL_SAMPLES);
        for (pn, sps) in [
            (PnSequence::barker(4).unwrap(), 11),
            (PnSequence::barker(11).unwrap(), 4),
        ] {
            let n = pn.len();
            let expected = pn.processing_gain_db();
            let measured = 10.0 * (unspread / collected(pn, sps)).log10();
            assert!(
                (measured - expected).abs() < 1.0,
                "N = {n}: measured gain {measured:.2} dB vs 10·log10(N) = {expected:.2} dB"
            );
        }
    }

    /// The same law between the *committed* codes, and the one clarification the entry needs to
    /// state plainly: **processing gain is chip rate over data rate, so it appears only when the
    /// interferer is referenced to the carrier.**
    ///
    /// Handed a jammer of fixed *absolute* power, two codes at the same chip rate collect the same
    /// interference — measured here at 0.1 dB apart — because what a despreader lets through is
    /// set by the chip band it looks at and nothing else. Referenced to each chain's own carrier
    /// power, which is where a C/I is quoted and what a longer code lowers by spending more chips
    /// per bit, the length-31 code collects 10·log10(31/11) less. Both halves are asserted, since
    /// quoting the second without the first is how "spreading gain" turns into folklore.
    #[test]
    fn the_two_committed_codes_differ_by_their_spreading_factor_ratio() {
        let table = tables::pam(2).unwrap();
        let symbols = 64;
        // (interference collected at fixed absolute jammer power, the chain's own carrier power)
        let collected = |pn: PnSequence| -> (f64, f64) {
            let p = params(pn);
            let preamble = points(&table, PREAMBLE, 0x1c2);
            let payload = vec![table.points()[0]; symbols];
            let clean = transmit(&p, &preamble, &payload);
            let carrier =
                clean.iter().map(|s| f64::from(s.norm_sqr())).sum::<f64>() / clean.len() as f64;
            let mut demod = DsssDemod::new(p);
            let mut total = 0.0f64;
            let steps = 128;
            for step in 0..steps {
                let offset = (f64::from(step) / f64::from(steps) - 0.5) / SPS as f64;
                let mut wave = clean.clone();
                for (index, s) in wave.iter_mut().enumerate() {
                    let phase = std::f64::consts::TAU * offset * index as f64;
                    *s += Complex::new((0.1 * phase.cos()) as f32, (0.1 * phase.sin()) as f32);
                }
                demod.genie(&wave, identity(LEAD, &payload));
                let mut got = Vec::new();
                demod.demodulate(PREAMBLE, symbols, &mut got);
                total += demap::noise_var_from_known(&got, &payload) / f64::from(steps);
            }
            (total, carrier)
        };
        let (barker, barker_carrier) = collected(PnSequence::barker(11).unwrap());
        let (m31, m31_carrier) = collected(PnSequence::maximal_length(5).unwrap());

        let absolute = 10.0 * (barker / m31).log10();
        assert!(
            absolute.abs() < 0.5,
            "at equal absolute jammer power the two codes collect {absolute:.2} dB apart, where a \
             despreader's own chip band says they should collect the same"
        );
        // Referenced to each chain's carrier — the quantity a C/I is stated in.
        // At a stated C/I the jammer's absolute power is the chain's own carrier power times a
        // constant, so the collected interference scales by that carrier — which is the whole of
        // the gain, and why it reads as a ratio of carrier powers.
        let relative = 10.0 * ((barker * barker_carrier) / (m31 * m31_carrier)).log10();
        let predicted = 10.0 * (31.0f64 / 11.0).log10();
        assert!(
            (relative - predicted).abs() < 0.5,
            "at equal C/I the length-31 code collects {relative:.2} dB less interference than \
             Barker-11, where 10·log10(31/11) predicts {predicted:.2} dB"
        );
    }

    /// The §3.4 hook doing its job: a burst arriving rotated still decodes, because the fit reads
    /// the rotation off the known symbols. This is the whole correction the entry applies — see
    /// the test below for why there is no second term.
    #[test]
    fn the_anchor_removes_a_rotation() {
        let table = tables::qam_square(4).unwrap();
        let p = params(PnSequence::barker(11).unwrap());
        let preamble = points(&table, PREAMBLE, 0x0a0c);
        let payload = points(&table, 2_048, 0x0a0d);
        let mut wave = transmit(&p, &preamble, &payload);
        let rotation = 0.9f64;
        let gain = 0.35f64;
        let (sin, cos) = rotation.sin_cos();
        for s in wave.iter_mut() {
            *s = Complex::new(
                (gain * (f64::from(s.re) * cos - f64::from(s.im) * sin)) as f32,
                (gain * (f64::from(s.re) * sin + f64::from(s.im) * cos)) as f32,
            );
        }
        add_noise(&mut wave, 0x0a0e, 0.002);
        let mut demod = DsssDemod::new(p);
        let acquisition = demod.acquire(&wave, &preamble, LEAD + 64).unwrap();
        assert_eq!(acquisition.origin, LEAD);
        assert!((acquisition.anchor.gain.norm() - gain).abs() < 0.05 * gain);
        assert!((acquisition.anchor.gain.arg() - rotation).abs() < 0.05);
        let mut got = Vec::new();
        demod.demodulate(PREAMBLE, payload.len(), &mut got);
        let errors = got
            .iter()
            .zip(&payload)
            .filter(|&(&g, &s)| table.hard_slice(g) != table.hard_slice(s))
            .count();
        assert_eq!(errors, 0, "{errors} of {} symbols", payload.len());
    }

    /// The measurement behind the module docs' gain-only choice, and the reason it is a choice
    /// rather than an omission: on an *unimpaired* burst — nothing to track at all — the slope
    /// [`PhaseAnchor::fit`] reads out of the preamble's own noise, extrapolated across a payload
    /// 64 times longer, costs far more than it removes. The constant gain the entry does fit
    /// leaves only what the channel's own noise puts there.
    #[test]
    fn fitting_the_anchor_slope_costs_more_phase_than_it_removes() {
        let table = tables::qam_square(4).unwrap();
        let p = params(PnSequence::barker(11).unwrap());
        let preamble = points(&table, PREAMBLE, 0x51_0e);
        let payload = points(&table, 2_048, 0x51_0f);
        let mut wave = transmit(&p, &preamble, &payload);
        // Es/N0 = 6 dB at the preamble: a working acquisition point, and the SNR the slope's
        // own variance is quoted at.
        add_noise(&mut wave, 0x51_10, 0.25);
        let mut demod = DsssDemod::new(p);
        demod.acquire(&wave, &preamble, LEAD + 64).unwrap();

        let despread: Vec<Complex<f32>> = (0..preamble.len())
            .map(|k| demod.despread(LEAD, k))
            .collect();
        let indices: Vec<usize> = (0..preamble.len()).collect();
        let sloped = PhaseAnchor::fit(&indices, &despread, &preamble).unwrap();

        let errors_with = |anchor: PhaseAnchor| {
            (0..payload.len())
                .filter(|&k| {
                    let index = PREAMBLE + k;
                    let corrected = anchor.correct(index, demod.despread(LEAD, index));
                    table.hard_slice(corrected) != table.hard_slice(payload[k])
                })
                .count()
        };
        let gain_only = errors_with(demod.acquisition().unwrap().anchor);
        let with_slope = errors_with(sloped);
        // The AWGN floor at Es/N0 = 6 dB is ~4.5 % of symbols; the committed correction must add
        // nothing measurable to it.
        assert!(
            gain_only < payload.len() / 12,
            "the committed correction lost {gain_only} of {}, past the channel's own floor",
            payload.len()
        );
        assert!(
            with_slope > payload.len() / 4,
            "the fitted slope lost only {with_slope} of {} symbols against the constant gain's \
             {gain_only}; if extrapolation has stopped hurting, the entry should fit it",
            payload.len()
        );
    }

    /// A demodulator that never acquired must write nothing rather than decode noise from
    /// origin zero (§8: no silent failure).
    #[test]
    fn a_demodulator_with_no_acquisition_produces_no_symbols() {
        let p = params(PnSequence::barker(11).unwrap());
        let demod = DsssDemod::new(p);
        let mut out = Vec::new();
        demod.demodulate(PREAMBLE, 100, &mut out);
        assert!(out.is_empty());
    }

    /// The framing overhead is a closed form of the geometry, not a fitted number — the same
    /// property the OFDM entry's is, and the reason the committed curve can be held to an
    /// unshifted oracle plus one constant.
    #[test]
    fn the_framing_overhead_is_the_symbol_count_ratio() {
        let overhead = DsssParams::framing_overhead_db(32, 2_048);
        assert!((overhead - 10.0 * (2080.0f64 / 2048.0).log10()).abs() < 1e-12);
        assert!((overhead - 0.0673).abs() < 1e-3, "{overhead}");
        // A free preamble costs nothing, and doubling the burst for the same payload costs 3 dB.
        assert!(DsssParams::framing_overhead_db(0, 100).abs() < 1e-12);
        assert!((DsssParams::framing_overhead_db(100, 100) - 3.0103).abs() < 1e-3);
    }

    /// LLRs come out calibrated: among bits reported at confidence |llr|, the fraction wrong
    /// must track `1/(1+e^|llr|)` — the claim the [`Llr`] type makes, measured rather than
    /// asserted, and the reason the noise variance is estimated from the preamble at all.
    #[test]
    fn llr_magnitudes_predict_their_own_error_rate() {
        let table = tables::qam_square(4).unwrap();
        let p = params(PnSequence::barker(11).unwrap());
        let preamble = points(&table, 128, 0x11c0);
        let payload = points(&table, 20_000, 0x11c1);
        let mut wave = transmit(&p, &preamble, &payload);
        add_noise(&mut wave, 0x11c2, 0.6);
        let mut demod = DsssDemod::new(p);
        demod.acquire(&wave, &preamble, LEAD + 64).unwrap();
        let mut got = Vec::new();
        demod.demodulate(preamble.len(), payload.len(), &mut got);
        let bits = table.bits_per_symbol();
        let mut llrs = vec![Llr(0.0); got.len() * bits];
        demod.llrs(&got, &table, &mut llrs);

        let mut bands = [(0u32, 0u32); 3];
        for (i, &llr) in llrs.iter().enumerate() {
            let label = table.hard_slice(payload[i / bits]);
            let sent = (label >> (i % bits)) & 1 == 1;
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
}
