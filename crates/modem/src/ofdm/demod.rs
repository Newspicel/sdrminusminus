//! The OFDM receiver: acquire, estimate, equalise, track, demap.
//!
//! The chain the catalog row names, in the order it runs:
//!
//! 1. [`PreambleSync`] places the frame and measures the carrier offset (`sync`).
//! 2. The known training gives a least-squares channel estimate and — from the same repeats —
//!    the noise variance the LLRs need (`equalize`).
//! 3. Every data symbol is read as: de-rotate by the measured offset, drop the prefix, transform,
//!    divide by the estimate, remove the residual phase the pilots fitted, demap.
//!
//! What is *not* here is as deliberate as what is. There is no channel tracking across symbols
//! beyond the pilots' line — a burst this short sees a static channel, and the limits table's
//! multipath rows say what that assumption costs. There is no second receive path for the DMT
//! domain: the Hermitian flag is a transmitter property, and this receiver reads the map it was
//! given, which is the lower half of the spectrum either way.
//!
//! **The transform window starts at the end of the cyclic prefix.** That places it at the far
//! edge of the interval where the channel's response is still circular, which is the choice that
//! maximises delay-spread tolerance — the whole prefix — at the cost of any *late* sampling
//! slack. It is the right trade here because the sampling instant only ever walks *early* under
//! a positive clock error (a fast receive clock reads the burst sooner and sooner), and because
//! an early window inside the prefix is not an error at all: a cyclic shift is a phase ramp
//! across the band, which is exactly what the pilot tracker's slope term removes. The committed
//! sample-clock row is bounded by the prefix for this reason and not by the pilots.

use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use super::{
    equalize::{ChannelEstimate, ChannelEstimator, PilotFit, PilotTracker, noise_var_from_repeats},
    params::OfdmParams,
    sync::{Acquisition, PreambleSync, rotor},
};
use crate::{
    constellation::{Constellation, demap::max_log_llrs},
    soft::Llr,
};

/// Samples every transform window is moved earlier into the cyclic prefix.
///
/// The window has to sit inside `[delay spread, cp]` relative to the symbol to be ISI-free, and
/// the two ends of that interval buy different things: the far end maximises delay-spread
/// tolerance, an earlier one buys tolerance to a *late* timing estimate — which is not
/// symmetrical, because a window one sample early is a cyclic shift (a phase ramp the estimate
/// itself absorbs, since the training is read through the same backoff) while a window one sample
/// late is inter-symbol interference no equaliser of this shape removes.
///
/// So the default is not zero, and the number is measured rather than chosen: a quarter of the
/// prefix costs nothing on the multipath row that matters (the committed two-ray limit is still
/// the prefix minus the backoff) and takes the acquiring rows' distance from their closed form
/// down by ~0.5 dB, because the long-training correlation's peak in noise is occasionally a
/// sample late.
pub const DEFAULT_BACKOFF: usize = 4;

/// One OFDM receiver over one parameter set. Construction designs the transform, the training
/// references and every buffer; the receive path allocates nothing.
///
/// `Clone` for the reason [`OfdmMod`](super::OfdmMod) is: a chain clones a configured receiver
/// per trial rather than rebuilding one, so no trial inherits the previous trial's acquisition
/// and none of them pays for a transform plan.
#[derive(Clone)]
pub struct OfdmDemod {
    params: OfdmParams,
    fft: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    grid: Vec<Complex<f32>>,
    scale: f32,
    sync: PreambleSync,
    estimator: ChannelEstimator,
    pilot_tracking: bool,
    backoff: usize,
    channel: ChannelEstimate,
    tracker: PilotTracker,
    cfo: f64,
    data_start: usize,
    /// Per-training-bin scratch for the two repeats the estimate and the noise variance are
    /// formed from.
    first: Vec<Complex<f32>>,
    second: Vec<Complex<f32>>,
    pilot_offsets: Vec<i32>,
    pilot_errors: Vec<Complex<f32>>,
    pilot_weights: Vec<f32>,
    /// One symbol's equalised data points — what `demodulate` streams out of.
    points: Vec<Complex<f32>>,
}

impl OfdmDemod {
    /// A receiver at the default tier: the long training's estimate with pilot tracking on.
    ///
    /// # Panics
    /// As [`PreambleSync::new`].
    #[must_use]
    pub fn new(params: OfdmParams) -> Self {
        let fft_size = params.fft();
        let fft = FftPlanner::<f32>::new().plan_fft_forward(fft_size);
        let scratch = vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len()];
        let occupied = params.map().occupied().len();
        let pilots = params.map().pilots().len();
        let pilot_offsets = params.map().pilots().iter().map(|c| c.offset).collect();
        let data = params.data_subcarriers();
        Self {
            fft,
            scratch,
            grid: vec![Complex::new(0.0, 0.0); fft_size],
            scale: (fft_size as f64).sqrt().recip() as f32,
            sync: PreambleSync::new(&params),
            estimator: ChannelEstimator::LongTraining,
            pilot_tracking: true,
            backoff: DEFAULT_BACKOFF,
            channel: ChannelEstimate::new(fft_size),
            tracker: PilotTracker::new(),
            cfo: 0.0,
            data_start: params.data_offset(),
            first: vec![Complex::new(0.0, 0.0); occupied],
            second: vec![Complex::new(0.0, 0.0); occupied],
            pilot_offsets,
            pilot_errors: vec![Complex::new(0.0, 0.0); pilots],
            pilot_weights: vec![0.0; pilots],
            points: vec![Complex::new(0.0, 0.0); data],
            params,
        }
    }

    /// Selects the channel estimator (§5 item 2: the comb tier is a separate merge, measured
    /// against the long-training tier).
    ///
    /// # Panics
    /// If [`ChannelEstimator::ShortComb`] is asked of a preamble whose short training is shorter
    /// than two transform lengths — there would be nothing to average, and an estimator that
    /// silently reused one window would read its own noise twice.
    #[must_use]
    pub fn with_estimator(mut self, estimator: ChannelEstimator) -> Self {
        if estimator == ChannelEstimator::ShortComb {
            let preamble = self.params.preamble();
            assert!(
                preamble.short_repeats * preamble.short_period(self.params.fft())
                    >= 2 * self.params.fft(),
                "the comb estimator needs two transform lengths of short training"
            );
        }
        self.estimator = estimator;
        self
    }

    /// Moves every transform window this many samples earlier into the prefix (see
    /// [`DEFAULT_BACKOFF`]).
    #[must_use]
    pub fn with_window_backoff(mut self, samples: usize) -> Self {
        self.backoff = samples.min(self.params.cp());
        self
    }

    /// Turns the per-symbol pilot fit on or off. Off is the genie comparison's configuration and
    /// nothing else: a chain that acquires needs it, because a residual carrier offset of a
    /// millionth of a cycle per sample is still 4° of common phase by the end of a frame.
    #[must_use]
    pub fn with_pilot_tracking(mut self, on: bool) -> Self {
        self.pilot_tracking = on;
        self
    }

    #[must_use]
    pub fn params(&self) -> &OfdmParams {
        &self.params
    }

    #[must_use]
    pub fn channel(&self) -> &ChannelEstimate {
        &self.channel
    }

    #[must_use]
    pub fn cfo(&self) -> f64 {
        self.cfo
    }

    #[must_use]
    pub fn data_start(&self) -> usize {
        self.data_start
    }

    /// Finds the frame in `x` (searching the first `search` samples for its start), measures the
    /// carrier offset, and forms the channel estimate and noise variance. Every later call reads
    /// the state this leaves behind.
    ///
    /// `None` only when the buffer is too short to hold a preamble — the honest "nothing to
    /// answer with". A *bad* acquisition is never reported as a failure: the crate's standing
    /// rule is that a chain too degraded to place its frame scores its garbage as bit errors
    /// rather than declining, so a threshold here would only hide the same errors behind a
    /// shorter output.
    pub fn acquire(&mut self, x: &[Complex<f32>], search: usize) -> Option<Acquisition> {
        let acquisition = self.sync.acquire(x, search)?;
        self.cfo = acquisition.cfo;
        self.data_start = acquisition.data_start;
        self.tracker.reset();
        match self.estimator {
            ChannelEstimator::LongTraining => self.estimate_long(x, acquisition.long_start),
            ChannelEstimator::ShortComb => self.estimate_comb(x, acquisition.long_start),
        }
        Some(acquisition)
    }

    /// The comparison receiver: frame origin, carrier offset, channel and residual phase all
    /// *given*. `channel` is one gain per occupied subcarrier in the map's own ascending order.
    /// What separates a curve measured through this from one measured through
    /// [`acquire`](Self::acquire) is the cost of acquisition, which is the number the catalog's
    /// genie row exists to commit.
    ///
    /// # Panics
    /// If `channel` is not one value per occupied subcarrier.
    pub fn genie(&mut self, data_start: usize, channel: &[Complex<f32>], noise_var: f64) {
        assert_eq!(
            channel.len(),
            self.params.map().occupied().len(),
            "one channel gain per occupied subcarrier"
        );
        self.cfo = 0.0;
        self.data_start = data_start;
        self.tracker.reset();
        self.channel.clear();
        for (c, &h) in self.params.map().occupied().iter().zip(channel) {
            self.channel.set(c.bin, h);
        }
        self.channel.finish(self.params.map(), noise_var, 0.0);
    }

    /// One data symbol's equalised points, written into `out` (one per data subcarrier). The hot
    /// path: no allocation, no branch on the configuration beyond the pilot fit.
    ///
    /// # Panics
    /// If `out.len()` is not the data-subcarrier count.
    pub fn symbol(&mut self, x: &[Complex<f32>], symbol: usize, out: &mut [Complex<f32>]) {
        assert_eq!(
            out.len(),
            self.params.data_subcarriers(),
            "one point per data subcarrier"
        );
        self.read_symbol(x, symbol);
        out.copy_from_slice(&self.points);
    }

    /// `symbols` consecutive data symbols, appended to `out`.
    pub fn demodulate(&mut self, x: &[Complex<f32>], symbols: usize, out: &mut Vec<Complex<f32>>) {
        out.reserve(symbols * self.params.data_subcarriers());
        for symbol in 0..symbols {
            self.read_symbol(x, symbol);
            out.extend_from_slice(&self.points);
        }
    }

    /// Per-bit LLRs of one symbol's equalised points, through the crate's one demapper.
    ///
    /// The per-subcarrier noise variance is what makes these true LLRs rather than confidences:
    /// after the one tap a faded bin carries `N0/|H|²`, so its bits arrive *less* believable, and
    /// a FEC stage below is told so. Handing one flat variance to the whole band would be the
    /// mis-scale the harness's genie-LLR bound measures at +0.23 dB on a 10× error.
    ///
    /// # Panics
    /// If `points` is not one symbol's worth, or `out` is not `points.len() ·
    /// bits_per_symbol` long.
    pub fn llrs(&self, points: &[Complex<f32>], table: &Constellation, out: &mut [Llr]) {
        let bits = table.bits_per_symbol();
        assert_eq!(points.len(), self.params.data_subcarriers());
        assert_eq!(out.len(), points.len() * bits);
        for (index, (&point, slot)) in points.iter().zip(out.chunks_exact_mut(bits)).enumerate() {
            let bin = self.params.map().data()[index].bin;
            max_log_llrs(point, table, self.channel.bin_noise_var(bin), slot);
        }
    }

    /// Reads one data symbol into [`Self::points`].
    fn read_symbol(&mut self, x: &[Complex<f32>], symbol: usize) {
        let start = self.data_start + symbol * self.params.symbol_samples() + self.params.cp()
            - self.backoff;
        self.transform_window(x, start);
        let fit = if self.pilot_tracking {
            self.fit_pilots(symbol)
        } else {
            PilotFit::default()
        };
        // The correction is a line in subcarrier offset, so it is stepped rather than evaluated:
        // one `sin_cos` per symbol instead of one per subcarrier.
        let data = self.params.map().data();
        let mut offset = data[0].offset;
        let (mut rot, step) = phase_line(fit.common_rad, fit.slope_rad_per_bin, offset);
        for (index, c) in data.iter().enumerate() {
            while offset < c.offset {
                rot *= step;
                offset += 1;
            }
            let z = self.channel.equalize(c.bin, self.grid[c.bin]);
            let z = Complex::new(f64::from(z.re), f64::from(z.im)) * rot;
            self.points[index] = Complex::new(z.re as f32, z.im as f32);
        }
    }

    /// Equalises the pilots of one symbol and fits their residual phases.
    fn fit_pilots(&mut self, symbol: usize) -> PilotFit {
        for (index, c) in self.params.map().pilots().iter().enumerate() {
            let z = self.channel.equalize(c.bin, self.grid[c.bin]);
            let expect = self.params.pilot_pattern().value(index, symbol);
            self.pilot_errors[index] = z * expect.conj();
            self.pilot_weights[index] = self.channel.gain(c.bin);
        }
        self.tracker
            .fit(&self.pilot_offsets, &self.pilot_errors, &self.pilot_weights)
    }

    /// De-rotates one transform length by the measured carrier offset and transforms it. Samples
    /// past the end of `x` read as zero: a truncated burst scores low rather than panicking,
    /// which is the sweep runner's rule that lost bits are errors and never fewer trials.
    fn transform_window(&mut self, x: &[Complex<f32>], start: usize) {
        let (mut rot, step) = rotor(self.cfo, start);
        for n in 0..self.params.fft() {
            let s = x.get(start + n).copied().unwrap_or(Complex::new(0.0, 0.0));
            let y = Complex::new(f64::from(s.re), f64::from(s.im)) * rot;
            self.grid[n] = Complex::new(y.re as f32, y.im as f32);
            rot *= step;
        }
        self.fft
            .process_with_scratch(&mut self.grid, &mut self.scratch);
        for bin in &mut self.grid {
            *bin *= self.scale;
        }
    }

    /// Tier 1: least squares over the long training's two repeats.
    fn estimate_long(&mut self, x: &[Complex<f32>], long_start: usize) {
        let fft = self.params.fft();
        // The same backoff the data windows take, so the cyclic shift it introduces is common to
        // the estimate and to every symbol equalised with it — and therefore cancels exactly
        // rather than becoming a phase ramp for the pilots to chase.
        let long_start = long_start - self.backoff.min(self.params.preamble().long_guard);
        self.read_training(x, long_start, long_start + fft, |params, index| {
            (
                params.map().occupied()[index].bin,
                params.long_training(index),
            )
        });
    }

    /// Tier 2: least squares over the short training's comb, then interpolation across the band.
    /// The windows are taken at the *end* of the short training so that at least one repeat
    /// period precedes each — which is what makes the channel's response circular over them, the
    /// same argument the cyclic prefix makes for a data symbol.
    fn estimate_comb(&mut self, x: &[Complex<f32>], long_start: usize) {
        let fft = self.params.fft();
        let preamble = self.params.preamble();
        let short_samples = preamble.short_repeats * preamble.short_period(fft);
        // Saturating, because a badly placed acquisition may put the long training earlier than a
        // whole preamble into the buffer; reading from sample 0 then measures the wrong thing
        // loudly (as bit errors) instead of panicking on a subtraction.
        let short_start = long_start
            .saturating_sub(preamble.long_guard)
            .saturating_sub(short_samples);
        // Backed off exactly as the long training and the data windows are: the short training
        // is periodic, so the shift is a phase ramp common to the estimate and to every symbol
        // equalised with it, and it cancels instead of accumulating.
        let base = short_start + (short_samples - 2 * fft) - self.backoff;
        self.read_training(x, base, base + fft, |params, index| {
            (params.short_bins()[index].bin, params.short_training(index))
        });
    }

    /// Two repeats of a known symbol into a channel estimate and a noise variance. `known` maps
    /// a training index to its bin and the value transmitted there.
    fn read_training(
        &mut self,
        x: &[Complex<f32>],
        first: usize,
        second: usize,
        known: fn(&OfdmParams, usize) -> (usize, Complex<f32>),
    ) {
        let count = match self.estimator {
            ChannelEstimator::LongTraining => self.params.map().occupied().len(),
            ChannelEstimator::ShortComb => self.params.short_bins().len(),
        };
        self.transform_window(x, first);
        for index in 0..count {
            self.first[index] = self.grid[known(&self.params, index).0];
        }
        self.transform_window(x, second);
        for index in 0..count {
            self.second[index] = self.grid[known(&self.params, index).0];
        }
        let noise_var = noise_var_from_repeats(&self.first[..count], &self.second[..count]);
        self.channel.clear();
        for index in 0..count {
            let (bin, value) = known(&self.params, index);
            let mean = (self.first[index] + self.second[index]) * 0.5;
            self.channel
                .set(bin, mean * value.conj() / value.norm_sqr());
        }
        // The backoff's own ramp is known, so the interpolation is told about it rather than
        // asked to follow it.
        let ramp = -(self.backoff as f64) / self.params.fft() as f64;
        self.channel.finish(self.params.map(), noise_var, ramp);
    }
}

/// The rotor for `e^{-j(a + b·k)}` at `k = first`, and the step per unit of `k`.
fn phase_line(common: f64, slope: f64, first: i32) -> (Complex<f64>, Complex<f64>) {
    (
        Complex::from_polar(1.0, -(common + slope * f64::from(first))),
        Complex::from_polar(1.0, -slope),
    )
}

impl std::fmt::Debug for OfdmDemod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OfdmDemod")
            .field("params", &self.params)
            .field("estimator", &self.estimator)
            .field("pilot_tracking", &self.pilot_tracking)
            .field("cfo", &self.cfo)
            .field("data_start", &self.data_start)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ber::{
            impair::{Awgn, Cfo, ChannelSpec, ClockError, Impairment, Multipath, MultipathProfile},
            rng::Rng,
        },
        constellation::tables,
        ofdm::modulator::OfdmMod,
    };

    const SYMBOLS: usize = 16;

    fn table() -> Constellation {
        match tables::qam_square(16) {
            Ok(t) => t,
            Err(why) => panic!("16-QAM: {why}"),
        }
    }

    fn payload(params: &OfdmParams, seed: u32) -> Vec<Complex<f32>> {
        let table = table();
        let mut state = seed | 1;
        (0..params.data_subcarriers() * SYMBOLS)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                table.points()[(state % 16) as usize]
            })
            .collect()
    }

    fn burst(
        params: &OfdmParams,
        lead: usize,
        seed: u32,
    ) -> (Vec<Complex<f32>>, Vec<Complex<f32>>) {
        let points = payload(params, seed);
        let mut wave = vec![Complex::new(0.0, 0.0); lead];
        OfdmMod::new(params.clone()).frame(&points, &mut wave);
        // A frame's worth of trailing silence: the delaying impairments shift the burst right and
        // a receiver reading past the end would otherwise be reading the harness, not the signal.
        wave.resize(wave.len() + 128, Complex::new(0.0, 0.0));
        (points, wave)
    }

    fn errors(sent: &[Complex<f32>], got: &[Complex<f32>]) -> usize {
        let table = table();
        sent.iter()
            .zip(got)
            .filter(|(a, b)| table.hard_slice(**a) != table.hard_slice(**b))
            .count()
    }

    fn decode(demod: &mut OfdmDemod, wave: &[Complex<f32>]) -> Vec<Complex<f32>> {
        assert!(demod.acquire(wave, 200).is_some(), "no preamble to acquire");
        let mut out = Vec::new();
        demod.demodulate(wave, SYMBOLS, &mut out);
        out
    }

    /// The round trip, both estimators: what the modulator mapped is what the argmin reads, at
    /// every lead-in the search covers.
    #[test]
    fn every_estimator_round_trips_a_clean_frame() {
        let params = OfdmParams::wifi_like();
        for estimator in [ChannelEstimator::LongTraining, ChannelEstimator::ShortComb] {
            for lead in [0usize, 5, 63, 140] {
                let (sent, wave) = burst(&params, lead, 0x0f_d1);
                let mut demod = OfdmDemod::new(params.clone()).with_estimator(estimator);
                let got = decode(&mut demod, &wave);
                assert_eq!(
                    errors(&sent, &got),
                    0,
                    "{estimator:?} at lead {lead}: {} of {} points wrong",
                    errors(&sent, &got),
                    sent.len()
                );
            }
        }
    }

    /// The DMT configuration through the same receiver, unchanged — the Hermitian flag's claim
    /// that it is a transmitter property and not a second chain.
    #[test]
    fn the_dmt_configuration_decodes_through_the_same_receiver() {
        let params = OfdmParams::dmt_like();
        let (sent, wave) = burst(&params, 17, 0x0d_47);
        let mut demod = OfdmDemod::new(params);
        let got = decode(&mut demod, &wave);
        assert_eq!(errors(&sent, &got), 0);
    }

    /// A carrier offset the preamble measures and the receiver removes — through a whole frame,
    /// where an offset left uncorrected would rotate the last symbol far past any slicing margin.
    #[test]
    fn a_carrier_offset_is_removed_across_a_whole_frame() {
        let params = OfdmParams::wifi_like();
        for cfo in [-0.02, -0.003, 0.003, 0.02] {
            let (sent, mut wave) = burst(&params, 31, 0x0f_c0);
            Cfo::from_cycles_per_sample(cfo).apply(&mut wave, &mut Rng::new(0));
            let mut demod = OfdmDemod::new(params.clone());
            let got = decode(&mut demod, &wave);
            assert_eq!(errors(&sent, &got), 0, "cfo {cfo}");
            assert!((demod.cfo() - cfo).abs() < 1e-5, "cfo {cfo}");
        }
    }

    /// The prefix's whole point, as a threshold: an echo inside it is a per-bin gain the one tap
    /// removes, and an echo past it is inter-symbol interference no equaliser of this shape can.
    /// The measured boundary is the prefix length, which is the claim the multipath limits row
    /// commits.
    #[test]
    fn multipath_inside_the_prefix_is_equalised_and_past_it_is_not() {
        let params = OfdmParams::wifi_like();
        let (sent, clean) = burst(&params, 23, 0x0f_e0);
        let echo = |delay: usize| {
            let mut wave = clean.clone();
            Multipath::new(MultipathProfile::TwoRay {
                delay_samples: delay,
                relative_db: -3.0,
                phase_rad: 0.7,
            })
            .apply(&mut wave, &mut Rng::new(0x0e));
            let mut demod = OfdmDemod::new(params.clone());
            errors(&sent, &decode(&mut demod, &wave))
        };
        // The window sits `cp - DEFAULT_BACKOFF` into the symbol, so that — not the prefix
        // itself — is the interval an echo has to land inside. The backoff is bought from this
        // budget deliberately (see `DEFAULT_BACKOFF`), and the committed multipath row reads the
        // remainder.
        for delay in [1usize, 4, 8, 12] {
            assert_eq!(
                echo(delay),
                0,
                "an echo at {delay} samples is inside the window"
            );
        }
        assert!(
            echo(20) > sent.len() / 100,
            "an echo past the window's own start must break the one-tap model"
        );
    }

    /// The comb tier's own limit, and the reason both tiers are committed rather than one: what
    /// tier 2 buys in the noise (its twelve bins carry a whole symbol's energy, repeated ten
    /// times) it gives back in frequency resolution. A delay of `d` samples turns the channel's
    /// phase by `2πd/fft` per bin, so a line drawn between anchors four bins apart is faithful
    /// only while `d` is a small fraction of `fft/stride` — measured here as: two samples of echo
    /// is error-free and twelve is not, where the long training equalises both.
    ///
    /// QPSK rather than the 16-QAM the other tests use, deliberately: what limits the tier is the
    /// interpolation's *gain* error near the band's midpoints, and a table with a third of the
    /// slicing margin reports the same finding at a third of the delay.
    #[test]
    fn the_comb_tier_trades_delay_spread_for_estimator_noise() {
        let params = OfdmParams::wifi_like();
        let table = match tables::qam_square(4) {
            Ok(t) => t,
            Err(why) => panic!("QPSK: {why}"),
        };
        let mut state = 0x0f_c8u32;
        let sent: Vec<Complex<f32>> = (0..params.data_subcarriers() * SYMBOLS)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                table.points()[(state % 4) as usize]
            })
            .collect();
        let mut clean = vec![Complex::new(0.0, 0.0); 19];
        OfdmMod::new(params.clone()).frame(&sent, &mut clean);
        clean.resize(clean.len() + 128, Complex::new(0.0, 0.0));

        let with = |estimator, delay| {
            let mut wave = clean.clone();
            Multipath::new(MultipathProfile::TwoRay {
                delay_samples: delay,
                relative_db: -3.0,
                phase_rad: 2.1,
            })
            .apply(&mut wave, &mut Rng::new(0x0e));
            let mut demod = OfdmDemod::new(params.clone()).with_estimator(estimator);
            let got = decode(&mut demod, &wave);
            sent.iter()
                .zip(&got)
                .filter(|(a, b)| table.hard_slice(**a) != table.hard_slice(**b))
                .count()
        };
        assert_eq!(with(ChannelEstimator::LongTraining, 2), 0);
        assert_eq!(with(ChannelEstimator::ShortComb, 2), 0);
        assert_eq!(with(ChannelEstimator::LongTraining, 12), 0);
        assert!(
            with(ChannelEstimator::ShortComb, 12) > sent.len() / 10,
            "the comb estimate resolved a delay its anchors cannot represent"
        );
    }

    /// Pilot tracking earning its keep on the axis it exists for: a sampling clock walks the
    /// transform window through the prefix, which is a phase ramp across the band, and the
    /// fitted slope is what removes it.
    #[test]
    fn pilot_tracking_is_what_survives_a_sampling_clock_error() {
        let params = OfdmParams::wifi_like();
        let (sent, clean) = burst(&params, 11, 0x0f_c1);
        let run = |tracking: bool| {
            let mut wave = clean.clone();
            ClockError::new(500.0).apply(&mut wave, &mut Rng::new(0));
            let mut demod = OfdmDemod::new(params.clone()).with_pilot_tracking(tracking);
            errors(&sent, &decode(&mut demod, &wave))
        };
        assert_eq!(run(true), 0, "500 ppm is well inside the prefix's budget");
        assert!(
            run(false) > sent.len() / 20,
            "without the pilot fit the same clock error must be visible"
        );
    }

    /// Soft output: the sign is the hard decision's, and the magnitude follows the *bin's* own
    /// post-equalisation noise — the same point on a faded subcarrier comes back less believable
    /// than on a strong one, which is the whole reason the demapper is handed a per-bin variance
    /// rather than the band's average.
    #[test]
    fn llr_confidence_follows_the_per_bin_channel() {
        let params = OfdmParams::wifi_like();
        let table = table();
        let mut demod = OfdmDemod::new(params.clone());
        // A hand-set channel: the first data subcarrier faded 12 dB, every other one unity.
        let mut truth = vec![Complex::new(1.0f32, 0.0); params.map().occupied().len()];
        let faded = params
            .map()
            .occupied()
            .iter()
            .position(|c| c.bin == params.map().data()[0].bin)
            .unwrap();
        truth[faded] = Complex::new(0.25, 0.0);
        demod.genie(params.data_offset(), &truth, 0.01);

        // The identical point on every subcarrier, so the only thing that can separate two
        // subcarriers' LLRs is the channel.
        let points = vec![table.points()[6]; params.data_subcarriers()];
        let bits = table.bits_per_symbol();
        let mut llrs = vec![Llr(0.0); points.len() * bits];
        demod.llrs(&points, &table, &mut llrs);
        let confidence = |index: usize| {
            llrs[index * bits..(index + 1) * bits]
                .iter()
                .map(|l| f64::from(l.0.abs()))
                .sum::<f64>()
        };
        assert!(
            confidence(0) < confidence(1),
            "the faded bin's bits are as believable as the strong bin's: {} vs {}",
            confidence(0),
            confidence(1)
        );
        // Sign convention (crate root): positive is a logical 1, matching the hard slice.
        let label = table.hard_slice(points[1]);
        for bit in 0..bits {
            let llr = llrs[bits + bit].0;
            assert_eq!(
                (label >> bit) & 1 == 1,
                llr > 0.0,
                "bit {bit}: label {label:b}, llr {llr}"
            );
        }
    }

    /// The genie entry point is the *same* receiver told the answer: on a clean channel it must
    /// agree with the acquiring one point for point, or the comparison the catalog's genie row
    /// makes would be measuring two receivers instead of one acquisition.
    #[test]
    fn the_genie_receiver_agrees_with_the_acquiring_one_on_a_clean_frame() {
        let params = OfdmParams::wifi_like();
        let (sent, mut wave) = burst(&params, 0, 0x0f_9e);
        ChannelSpec::default()
            .awgn(Awgn::with_sigma(0.02))
            .build()
            .apply(&mut wave, &mut Rng::new(0x9e));
        let mut acquiring = OfdmDemod::new(params.clone());
        let a = decode(&mut acquiring, &wave);

        // Told the timing exactly, so it takes no backoff: with the channel given there is
        // nothing to absorb the cyclic shift one would introduce.
        let mut genie = OfdmDemod::new(params.clone())
            .with_pilot_tracking(false)
            .with_window_backoff(0);
        genie.genie(
            params.data_offset(),
            &vec![Complex::new(1.0, 0.0); params.map().occupied().len()],
            1.0,
        );
        let mut b = Vec::new();
        genie.demodulate(&wave, SYMBOLS, &mut b);

        assert_eq!(errors(&sent, &a), 0);
        assert_eq!(errors(&sent, &b), 0);
        // Not merely both correct — the same points, to the noise the estimate carries.
        // The two differ by the channel estimate's own noise and nothing else: two averaged
        // repeats at σ = 0.02 leave σ/√2 per bin, so the worst of 768 points sits near 4σ/√2.
        let worst = a
            .iter()
            .zip(&b)
            .map(|(x, y)| f64::from((x - y).norm()))
            .fold(0.0f64, f64::max);
        assert!(worst < 0.1, "worst point disagreement {worst}");
    }
}
