//! The coherent-tier linear demodulator: stagger removal → matched filter → `SymbolSync` →
//! blind power normalisation → de-rotation → carrier loop. One complex soft symbol comes out per
//! symbol period, on the constellation's own scale, ready for
//! [`demap`](crate::constellation::demap) or a hard slice.
//!
//! **What each stage is for, and what it is not.**
//!
//! - *Stagger removal* is the OQPSK axis, and it is one integer delay: the transmitter held Q
//!   back half a symbol, so the receiver holds I back by the same, and what reaches the matched
//!   filter is ordinary QPSK. Delaying rather than advancing keeps the operation causal, and the
//!   even-`sps` rule in [`LinearParams::with_offset`](super::LinearParams::with_offset) is what
//!   makes it exact instead of a fractional-delay filter with a passband of its own. Filtering is
//!   linear, so doing it before the matched filter or after is the same signal; before is where
//!   the delay line is one rail wide.
//! - *Timing* is `sdrmm_dsp::SymbolSync` — the one timing stack in the workspace (MODEM-PLAN
//!   §3.2). This module schedules it and never reimplements it.
//! - *Blind power normalisation* puts the symbol stream at unit mean power, which is the
//!   constellation's own scale. It is what makes a decision-directed carrier loop and a QAM
//!   slice possible before any known symbol has been seen — and it carries a documented bias:
//!   the measured mean power is Es + N0, so the estimate runs high by √(1 + 1/SNR) — 4.6 % at
//!   10 dB, 1.5 % at 15 dB, 0.5 % at 20 dB. A constant-modulus table never notices. A table with
//!   rings does, and the §3.4 answer is the same one the CPM engine reached for its M = 8
//!   boundary: known symbols, through [`anchor`](super::anchor), whose fit has no such bias.
//! - *De-rotation* undoes the entry's own per-symbol rotation schedule before the carrier loop
//!   sees anything, so the loop's detector always faces the plain table.
//! - *The carrier loop* is optional. `None` is not a lesser tier: the differential rows have no
//!   use for absolute phase, and the anchored rows recover it in one block-wide least-squares
//!   fit that beats any loop over a short burst.

use num_complex::Complex;
use sdrmm_dsp::{Decimator, SymbolSync};

use super::{carrier::CarrierLoop, params::LinearParams, timing::FeedforwardTiming};

/// Timing loop bandwidth for continuously-keyed linear entries, in cycles per symbol. The same
/// reasoning — and the same measured number — as the CPM engine's
/// [`TIMING_BW_CONTINUOUS`](crate::cpm::TIMING_BW_CONTINUOUS): a wider loop integrates the
/// Gardner detector's own self-noise into a rate estimate that walks on a signal with zero clock
/// error.
pub const TIMING_BW_CONTINUOUS: f64 = 0.003;

/// Timing loop bandwidth for burst entries, where the loop must acquire inside a preamble and a
/// static sample-clock error must pull in before the payload. The CPM engine's
/// [`TIMING_BW_BURST`](crate::cpm::TIMING_BW_BURST) figure, for the same trade.
pub const TIMING_BW_BURST: f64 = 0.015;

/// The two loops a linear entry has to size: the timing recovery's bandwidth and the blind power
/// estimate's averaging. Both are per-entry *data* with measured operating points, for the same
/// reason the CPM engine's timing bandwidth is — a burst receiver and a continuous one want
/// genuinely different loops, and a single constant would quietly serve one of them badly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinearTiming {
    /// `SymbolSync` loop bandwidth, cycles per symbol.
    pub timing_bw: f64,
    /// One-pole time constant of the mean-power estimate, in symbols. `f64::INFINITY` *holds* the
    /// estimate at its initial unit mean power, which is what a burst chain wants: the scale is
    /// then the §3.4 anchor's business, and the blind estimate cannot drift across the frame.
    pub power_symbols: f64,
}

impl LinearTiming {
    /// Continuously-keyed operating point.
    ///
    /// What limits the noiseless residual here is the timing loop, not the power estimate, and
    /// that was measured rather than assumed: lengthening the power averaging from 200 symbols to
    /// 1000 moved cross-32's residual from 0.062 to 0.060 RMS, while replacing the recovered
    /// clock with a genie one moved it to 0.0028 — twenty times better. The averaging is 1000
    /// anyway because on a continuous stream it is free; the burst point below is where it costs
    /// something.
    ///
    /// The timing bandwidth is the measured optimum for *this* engine, and it is a compromise
    /// between two failure modes rather than a corner: at 0.0001 the loop is still acquiring
    /// after 40 000 symbols (0.067 RMS on 16-QAM), and at 0.01 Gardner's data-dependent
    /// self-noise — much larger on an amplitude-varying table than on the constant-modulus CPM
    /// alphabets — walks the clock (0.043 on cross-32). Between them, 0.003 settles at 0.012 and
    /// 0.020 respectively, which is 4 % and 9 % of those tables' slicing margins.
    pub const CONTINUOUS: Self = Self {
        timing_bw: TIMING_BW_CONTINUOUS,
        power_symbols: 1_000.0,
    };

    /// Burst operating point: the timing loop fast enough to acquire inside a preamble, and the
    /// power estimate **held**. A burst's transmitter level is constant, so there is nothing for
    /// the estimate to track — and its ripple is not harmless: on 1024-QAM, whose outermost point
    /// sits 1.34 from the origin against a slicing margin of 0.038, a 1000-symbol estimate's own
    /// wander across the frame put an error floor at 1.4e-4 that a *held* estimate plus the §3.4
    /// anchor removes entirely (measured: 5.7e-4 at 25 dB against the estimate's 5.2e-3, and
    /// 1.1e-5 at 28 dB against 3.1e-4).
    pub const BURST: Self = Self {
        timing_bw: TIMING_BW_BURST,
        power_symbols: f64::INFINITY,
    };
}

/// Where the power estimate starts. Unit mean power is what a correctly-levelled front end
/// delivers, so a cold demodulator assumes it and converges from there rather than from zero —
/// which would divide the first symbols by nothing.
const INITIAL_POWER: f32 = 1.0;

/// The stages every timing tier shares: blind power normalisation, de-rotation, carrier loop.
/// Factored out because [`LinearDemod`] and [`LinearBurstDemod`] differ *only* in how they place
/// the symbol instants — a second copy of this would let the two tiers' measurements drift apart
/// on something other than the timing, which is the one thing a tier comparison must not do.
struct SymbolStage {
    carrier: Option<CarrierLoop>,
    params: LinearParams,
    power: f32,
    power_alpha: f32,
    /// Symbols emitted so far — the de-rotation schedule's index. Integer, so a long
    /// transmission's rotation is exact.
    symbols_out: u64,
}

impl SymbolStage {
    fn new(params: &LinearParams, power_symbols: f64, carrier: Option<CarrierLoop>) -> Self {
        Self {
            carrier,
            params: params.clone(),
            power: INITIAL_POWER,
            // An infinite time constant is a held estimate, not a division by infinity that
            // happens to work: it is the burst chains' operating point and it is named.
            power_alpha: if power_symbols.is_finite() {
                (1.0 / power_symbols) as f32
            } else {
                0.0
            },
            symbols_out: 0,
        }
    }

    fn push(&mut self, y: Complex<f32>) -> Complex<f32> {
        // The power estimate leads the scaling: a symbol contributes to the estimate it is then
        // divided by, which is what keeps a level step from passing through unscaled.
        self.power += self.power_alpha * (y.norm_sqr() - self.power);
        let scale = self.power.max(f32::MIN_POSITIVE).sqrt().recip();
        let mut symbol = y * scale;
        if self.params.rotation_rad() != 0.0 {
            let theta =
                -((self.symbols_out as f64 * self.params.rotation_rad()) % std::f64::consts::TAU);
            symbol *= Complex::new(theta.cos() as f32, theta.sin() as f32);
        }
        if let Some(carrier) = &mut self.carrier {
            symbol = carrier.advance(symbol, self.params.constellation());
        }
        self.symbols_out += 1;
        symbol
    }

    fn reset(&mut self) {
        if let Some(carrier) = &mut self.carrier {
            carrier.reset();
        }
        self.power = INITIAL_POWER;
        self.symbols_out = 0;
    }
}

/// The half-symbol delay line the stagger axis needs, on the in-phase rail.
#[derive(Clone, Debug, Default)]
struct Unstagger {
    line: Vec<f32>,
    at: usize,
}

impl Unstagger {
    fn new(len: usize) -> Self {
        Self {
            line: vec![0.0; len],
            at: 0,
        }
    }

    /// Hold the in-phase rail back so both rails land on the same instants. A zero-length line is
    /// the unstaggered case and copies through.
    fn apply(&mut self, iq: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        out.clear();
        out.reserve(iq.len());
        if self.line.is_empty() {
            out.extend_from_slice(iq);
            return;
        }
        let n = self.line.len();
        for &s in iq {
            let delayed = self.line[self.at];
            self.line[self.at] = s.re;
            self.at = (self.at + 1) % n;
            out.push(Complex::new(delayed, s.im));
        }
    }

    fn reset(&mut self) {
        self.line.fill(0.0);
        self.at = 0;
    }
}

/// Linear demodulator, coherent tier, **tracking timing**. Streaming: filter, timing, level and
/// carrier state carry across calls, so any block split gives the same symbols.
pub struct LinearDemod {
    matched: Decimator,
    sync: SymbolSync,
    stage: SymbolStage,
    stagger: Unstagger,
    aligned: Vec<Complex<f32>>,
    filtered: Vec<Complex<f32>>,
    retimed: Vec<Complex<f32>>,
}

impl LinearDemod {
    /// `receive_filter` is the entry's matched filter — the receive half of a root pair, or the
    /// pulse itself where the entry's pulse is its own match — as unit-energy taps
    /// ([`pulse::Norm::Energy`](crate::pulse::Norm)), because the noise variance the demapper is
    /// handed is the waveform's own only if the filter passes white noise at unit gain.
    /// `timing` sizes the two loops; [`LinearTiming::CONTINUOUS`] and [`LinearTiming::BURST`] are
    /// the measured operating points and the choice is part of the entry's data. `carrier` is the
    /// coherent tier's loop, or `None` for the differential and anchored rows.
    ///
    /// # Panics
    /// If `receive_filter` is empty or not unit-energy.
    #[must_use]
    pub fn new(
        params: &LinearParams,
        receive_filter: &[f32],
        timing: LinearTiming,
        carrier: Option<CarrierLoop>,
    ) -> Self {
        assert_unit_energy(receive_filter);
        Self {
            matched: Decimator::new(receive_filter, 1),
            sync: SymbolSync::new(params.sps() as f64, timing.timing_bw),
            stage: SymbolStage::new(params, timing.power_symbols, carrier),
            stagger: Unstagger::new(params.stagger_samples()),
            aligned: Vec::new(),
            filtered: Vec::new(),
            retimed: Vec::new(),
        }
    }

    #[must_use]
    pub fn params(&self) -> &LinearParams {
        &self.stage.params
    }

    /// The loop's carrier frequency estimate in cycles per symbol, or 0 for an open-loop tier —
    /// what a limits row reads to state what a CFO search actually pulled in.
    #[must_use]
    pub fn carrier_freq_cycles_per_symbol(&self) -> f64 {
        self.stage
            .carrier
            .as_ref()
            .map_or(0.0, CarrierLoop::freq_cycles_per_symbol)
    }

    /// Demodulate a block of complex baseband, appending one soft symbol per recovered symbol
    /// period to `out`.
    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        self.stagger.apply(iq, &mut self.aligned);
        self.matched.process(&self.aligned, &mut self.filtered);
        // `SymbolSync::process` appends where `Decimator::process` replaces; the buffer is the
        // engine's own scratch, so it is cleared here rather than growing across blocks.
        self.retimed.clear();
        self.sync.process(&self.filtered, &mut self.retimed);
        for &y in &self.retimed {
            out.push(self.stage.push(y));
        }
    }

    pub fn reset(&mut self) {
        self.sync.reset();
        self.stage.reset();
        self.stagger.reset();
    }
}

/// Linear demodulator, coherent tier, **feedforward timing**: the burst form. One call is one
/// burst — [`FeedforwardTiming`] needs the whole thing before it can place the first symbol — and
/// everything after the timing is the identical [`SymbolStage`] the tracking demodulator runs, so
/// a comparison between the two reads the timing and nothing else.
///
/// This is the tier the high-order rows are measured on; the numbers behind that are in
/// [`timing`](super::timing).
pub struct LinearBurstDemod {
    timing: FeedforwardTiming,
    stage: SymbolStage,
    stagger: Unstagger,
    aligned: Vec<Complex<f32>>,
    retimed: Vec<Complex<f32>>,
}

impl LinearBurstDemod {
    /// `power_symbols` is the blind level estimate's time constant, as in [`LinearTiming`]; the
    /// timing has no bandwidth to size, which is the point of the tier.
    ///
    /// # Panics
    /// As [`FeedforwardTiming::new`].
    #[must_use]
    pub fn new(
        params: &LinearParams,
        receive_filter: &[f32],
        power_symbols: f64,
        carrier: Option<CarrierLoop>,
    ) -> Self {
        Self {
            timing: FeedforwardTiming::new(params, receive_filter),
            stage: SymbolStage::new(params, power_symbols, carrier),
            stagger: Unstagger::new(params.stagger_samples()),
            aligned: Vec::new(),
            retimed: Vec::new(),
        }
    }

    #[must_use]
    pub fn params(&self) -> &LinearParams {
        &self.stage.params
    }

    /// Demodulate one burst, appending its symbols to `out`. Returns the measured timing offset
    /// in samples — a number a limits row records rather than a diagnostic.
    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<Complex<f32>>) -> f64 {
        self.stagger.apply(iq, &mut self.aligned);
        self.retimed.clear();
        let offset = self.timing.process(&self.aligned, &mut self.retimed);
        for &y in &self.retimed {
            out.push(self.stage.push(y));
        }
        offset
    }

    #[must_use]
    pub fn carrier_freq_cycles_per_symbol(&self) -> f64 {
        self.stage
            .carrier
            .as_ref()
            .map_or(0.0, CarrierLoop::freq_cycles_per_symbol)
    }
}

/// The receive-filter contract, checked once for both tiers.
fn assert_unit_energy(receive_filter: &[f32]) {
    assert!(!receive_filter.is_empty(), "receive filter must have taps");
    let energy: f64 = receive_filter
        .iter()
        .map(|&h| f64::from(h) * f64::from(h))
        .sum();
    assert!(
        (energy - 1.0).abs() < 1e-3,
        "receive filter must be unit-energy (pulse::Norm::Energy), got Σh² = {energy}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ber::{perf::assert_no_alloc, rng::Rng},
        constellation::{Constellation, tables},
        linear::{LinearMod, PhaseDetector},
        pulse::{self, Norm},
    };

    const SPS: usize = 8;

    fn rrc() -> Vec<f32> {
        pulse::root_raised_cosine(SPS as f64, 0.35, 8, Norm::Energy)
    }

    /// The table's minimum point separation — the slicing margin is half of it.
    fn min_distance(table: &Constellation) -> f64 {
        let p = table.points();
        let mut min = f64::INFINITY;
        for i in 0..p.len() {
            for j in (i + 1)..p.len() {
                min = min.min(f64::from((p[i] - p[j]).norm()));
            }
        }
        min
    }

    fn labels(n: usize, m: u32, seed: u64) -> Vec<u32> {
        let mut rng = Rng::new(seed);
        (0..n)
            .map(|_| (rng.next_u64() % u64::from(m)) as u32)
            .collect()
    }

    /// Demodulate a whole transmission and return the symbols after the chain has settled —
    /// the timing loop and the power estimate both need a run-up, and reading a cold chain's
    /// first symbols would measure the acquisition, not the detector.
    fn settled(
        params: &LinearParams,
        wave: &[Complex<f32>],
        carrier: Option<CarrierLoop>,
    ) -> Vec<Complex<f32>> {
        let mut demod = LinearDemod::new(params, &rrc(), LinearTiming::CONTINUOUS, carrier);
        let mut out = Vec::new();
        demod.process(wave, &mut out);
        // The run-up is dropped: at a 0.003 timing bandwidth the loop is still tightening for
        // thousands of symbols (measured in `LinearTiming::CONTINUOUS`'s docs), and reading a
        // cold chain would measure the acquisition rather than the detector.
        let settled_from = out.len() * 3 / 4;
        out.split_off(settled_from)
    }

    /// A noiseless loopback must land on the table: this is the whole chain's correctness
    /// statement before any impairment, and a failure in it means the delay, the scale or the
    /// rotation is wrong — each of which would poison every curve measured through it.
    ///
    /// The bound is the RMS residual as a fraction of the table's own slicing margin (half its
    /// minimum distance), which is the quantity that means the same thing on BPSK and on
    /// cross-32. RMS and not worst-case: at a 0.003 timing bandwidth the loop is still tightening
    /// thousands of symbols in — measured on 16-QAM, the residual falls from 0.069 RMS over
    /// symbols 1000–2000 to 0.023 over 6000–7900 — so the extreme of any finite window is an
    /// acquisition artefact, while the RMS is the chain's own ISI and jitter. A wrong delay,
    /// scale or rotation moves the RMS by far more than the tenth of a margin allowed here.
    #[test]
    fn a_noiseless_loopback_lands_on_the_table() {
        for (name, table) in [
            ("bpsk", tables::pam(2).unwrap()),
            ("qpsk", tables::qam_square(4).unwrap()),
            ("16-qam", tables::qam_square(16).unwrap()),
            ("8-psk", tables::psk(8).unwrap()),
            ("cross-32", tables::qam_cross(32).unwrap()),
        ] {
            let m = table.len() as u32;
            let p = LinearParams::new(table.clone(), rrc(), SPS).unwrap();
            let sent = labels(12_000, m, 0xd0d0);
            let wave = LinearMod::transmission(&p, &sent);
            let got = settled(&p, &wave, None);
            let rms = (got
                .iter()
                .map(|&y| {
                    let l = table.hard_slice(y);
                    let i = table.labels().iter().position(|&x| x == l).unwrap();
                    f64::from((y - table.points()[i]).norm_sqr())
                })
                .sum::<f64>()
                / got.len() as f64)
                .sqrt();
            let margin = min_distance(&table) / 2.0;
            assert!(
                rms < 0.1 * margin,
                "{name}: residual {rms} RMS, slicing margin {margin}"
            );
        }
    }

    /// End to end at the label level, which is what a link actually reads: every symbol of a
    /// noiseless transmission slices back to the label that was sent, once the chain is settled.
    #[test]
    fn every_noiseless_symbol_slices_back_to_its_label() {
        let table = tables::qam_square(16).unwrap();
        let p = LinearParams::new(table.clone(), rrc(), SPS).unwrap();
        let sent = labels(4_000, 16, 0x51ce);
        let wave = LinearMod::transmission(&p, &sent);
        let got = settled(&p, &wave, None);
        let offset = sent.len() - got.len();
        let wrong = got
            .iter()
            .zip(&sent[offset..])
            .filter(|&(&y, &want)| table.hard_slice(y) != want)
            .count();
        assert_eq!(wrong, 0, "{wrong} of {} symbols mis-sliced", got.len());
    }

    /// The stagger axis, end to end: an OQPSK transmission demodulates through the delay-line
    /// path exactly as its unstaggered twin does through the plain one — same labels, same
    /// count. A receiver that skipped the delay would read a constellation smeared across the
    /// half-symbol.
    #[test]
    fn oqpsk_recovers_through_the_unstagger() {
        let table = tables::qam_square(4).unwrap();
        let sent = labels(3_000, 4, 0x0954);
        let plain = LinearParams::new(table.clone(), rrc(), SPS).unwrap();
        let offset = plain.clone().with_offset(true).unwrap();
        for (name, p) in [("qpsk", plain), ("oqpsk", offset)] {
            let wave = LinearMod::transmission(&p, &sent);
            let got = settled(&p, &wave, None);
            let start = sent.len() - got.len();
            let wrong = got
                .iter()
                .zip(&sent[start..])
                .filter(|&(&y, &want)| table.hard_slice(y) != want)
                .count();
            assert_eq!(wrong, 0, "{name}: {wrong} mis-sliced");
        }
    }

    /// Forgetting to undo the stagger is the failure this axis exists to prevent, so it is
    /// measured rather than trusted: the same waveform through a demodulator built without the
    /// offset flag mis-slices a large fraction of its symbols.
    #[test]
    fn ignoring_the_stagger_wrecks_the_constellation() {
        let table = tables::qam_square(4).unwrap();
        let sent = labels(2_000, 4, 0x0954);
        let staggered = LinearParams::new(table.clone(), rrc(), SPS)
            .unwrap()
            .with_offset(true)
            .unwrap();
        let wave = LinearMod::transmission(&staggered, &sent);
        // Demodulated as if it were plain QPSK.
        let plain = LinearParams::new(table.clone(), rrc(), SPS).unwrap();
        let got = settled(&plain, &wave, None);
        let start = sent.len() - got.len();
        let wrong = got
            .iter()
            .zip(&sent[start..])
            .filter(|&(&y, &want)| table.hard_slice(y) != want)
            .count();
        assert!(
            wrong > got.len() / 5,
            "only {wrong} of {} mis-sliced without the unstagger",
            got.len()
        );
    }

    /// The rotation axis: a π/2-BPSK transmission comes back as plain BPSK because the receiver
    /// runs the same schedule in reverse.
    #[test]
    fn the_rotation_schedule_is_undone_at_the_receiver() {
        let table = tables::pam(2).unwrap();
        let p = LinearParams::new(table.clone(), rrc(), SPS)
            .unwrap()
            .with_rotation(tables::PI_2_ROTATION)
            .unwrap();
        let sent = labels(3_000, 2, 0x9020);
        let wave = LinearMod::transmission(&p, &sent);
        let got = settled(&p, &wave, None);
        let start = sent.len() - got.len();
        let wrong = got
            .iter()
            .zip(&sent[start..])
            .filter(|&(&y, &want)| table.hard_slice(y) != want)
            .count();
        assert_eq!(wrong, 0, "{wrong} mis-sliced");
    }

    /// The coherent tier's reason to exist, measured through the whole chain: a static carrier
    /// offset that leaves the open-loop chain reading noise is recovered by the loop.
    #[test]
    fn the_carrier_loop_recovers_an_offset_the_open_chain_cannot() {
        let table = tables::psk(4).unwrap();
        let p = LinearParams::new(table.clone(), rrc(), SPS).unwrap();
        let sent = labels(6_000, 4, 0x0ff5);
        let mut wave = LinearMod::transmission(&p, &sent);
        // 1e-4 cycles/sample is 8e-4 cycles/symbol — a slow rotation, but over 6000 symbols it
        // is nearly five whole turns.
        for (n, s) in wave.iter_mut().enumerate() {
            let theta = std::f64::consts::TAU * 1e-4 * n as f64;
            *s *= Complex::new(theta.cos() as f32, theta.sin() as f32);
        }
        let spread = |symbols: &[Complex<f32>]| -> f64 {
            // Mean distance to the nearest table point, over the settled tail: a locked chain
            // sits on the table, a rotating one averages across it.
            symbols
                .iter()
                .map(|&y| {
                    let l = table.hard_slice(y);
                    let i = table.labels().iter().position(|&x| x == l).unwrap();
                    f64::from((y - table.points()[i]).norm())
                })
                .sum::<f64>()
                / symbols.len() as f64
        };
        let open = settled(&p, &wave, None);
        let locked = settled(
            &p,
            &wave,
            Some(CarrierLoop::new(PhaseDetector::MthPower { m: 4 }, 0.01)),
        );
        assert!(spread(&locked) < 0.05, "locked spread {}", spread(&locked));
        assert!(spread(&open) > 0.3, "open-loop spread {}", spread(&open));
    }

    /// Streaming must be a pure refactor: any block split, the same symbols.
    #[test]
    fn any_block_split_gives_the_same_symbols() {
        let table = tables::qam_square(16).unwrap();
        let p = LinearParams::new(table, rrc(), SPS)
            .unwrap()
            .with_offset(true)
            .unwrap();
        let wave = LinearMod::transmission(&p, &labels(1_500, 16, 0x5b17));
        let carrier = || Some(CarrierLoop::new(PhaseDetector::DecisionDirected, 0.01));
        let mut whole = LinearDemod::new(&p, &rrc(), LinearTiming::CONTINUOUS, carrier());
        let mut a = Vec::new();
        whole.process(&wave, &mut a);
        let mut split = LinearDemod::new(&p, &rrc(), LinearTiming::CONTINUOUS, carrier());
        let mut b = Vec::new();
        for chunk in wave.chunks(311) {
            split.process(chunk, &mut b);
        }
        assert_eq!(a, b);
    }

    /// §4.2: the steady-state path allocates nothing, on both the plain and the staggered
    /// chain — the stagger's delay line is a fixed ring, not a growing buffer.
    #[test]
    fn steady_state_allocates_nothing() {
        for (name, offset) in [("plain", false), ("staggered", true)] {
            let p = LinearParams::new(tables::qam_square(16).unwrap(), rrc(), SPS)
                .unwrap()
                .with_offset(offset)
                .unwrap();
            let wave = LinearMod::transmission(&p, &labels(2_048, 16, 0x0a11));
            let mut demod = LinearDemod::new(
                &p,
                &rrc(),
                LinearTiming::CONTINUOUS,
                Some(CarrierLoop::new(PhaseDetector::DecisionDirected, 0.01)),
            );
            let mut out = Vec::with_capacity(wave.len());
            // Two warm-up blocks: streaming stages carry an inter-block remainder, so the
            // second block is the first whose buffers must fit remainder plus block.
            demod.process(&wave, &mut out);
            out.clear();
            demod.process(&wave, &mut out);
            out.clear();
            assert_no_alloc(&format!("LinearDemod::process ({name})"), || {
                demod.process(&wave, &mut out);
            });
            assert!(
                !out.is_empty(),
                "{name}: the measured call recovered nothing"
            );
        }
    }
}
