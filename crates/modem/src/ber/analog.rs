//! The analog measurement class ( §5 item 4: *"analog entries use SINAD/THD vs input
//! SNR instead of BER"*) — the same four-part regime as every other entry, with one substitution
//! at its root.
//!
//! **What replaces the bit.** A BER curve counts a discrete outcome, so its x-axis is Eb/N0 and
//! its y-axis is a probability. An analog entry has no outcomes to count: what arrives is a
//! waveform, and the question is how much of it is the message. So the x-axis becomes the
//! *channel* SNR — received power over the noise in one message bandwidth, the reference every
//! closed form in [`theory`](super::theory) is stated against and the one
//! [`Awgn::for_channel_snr`] applies — and the y-axis becomes **SINAD**: total output power over
//! everything in it that is not the fundamental. Distortion is inside the denominator on purpose;
//! a detector that trades noise for harmonics has not improved, and a signal-to-noise ratio that
//! could not see the trade would let it.
//!
//! **Everything else is unchanged.** Seeds name realisations, curves are committed as JSON,
//! comparators are one-sided and loud when they cannot answer, and the acceptance is an oracle
//! wherever a closed form exists — which for the analog entries is everywhere above threshold,
//! since [`theory`](super::theory)'s figures of merit are exact there. What no closed form
//! describes is the *knee*: below its threshold an envelope detector's nonlinearity starts
//! suppressing the message and a discriminator's clicks arrive in bursts, and both fall off the
//! straight line. That knee is the analog entries' one committed-and-guarded quantity, and it is
//! recorded as a number — the SNR at which the measured curve first falls a stated distance
//! below its own oracle.
//!
//! **Why the tone is snapped to a bin.** The fundamental is read by direct correlation rather
//! than by an FFT with a window, which is exact only when the analysis window holds a whole
//! number of tone periods — otherwise the leakage of the fundamental into its neighbours lands
//! in the denominator and reads as distortion that is not there. [`TonePlan`] makes that
//! structural: it snaps the requested frequency to the nearest exact bin of the window it will
//! be analysed in, so no measurement can be taken at a frequency the analysis cannot resolve.

use std::{f64::consts::TAU, fs, io, path::Path};

use num_complex::Complex;
use serde::{Deserialize, Serialize};

use super::{
    impair::{Awgn, ChannelSpec, Impairment},
    rng::Rng,
    sweep::point_seed,
};

// --- The tone and its analysis ----------------------------------------------------------------

/// A test tone and the window it will be analysed in, with the frequency snapped to an exact
/// bin of that window (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TonePlan {
    /// Cycles per sample, exactly `cycles / window`.
    pub freq: f64,
    /// Whole tone cycles inside the window.
    pub cycles: usize,
    pub window: usize,
}

impl TonePlan {
    /// The plan closest to `freq_hint` (cycles/sample) that the window can resolve exactly.
    ///
    /// # Panics
    /// If the window is empty, or the hint does not land on at least one whole cycle inside it.
    #[must_use]
    pub fn new(freq_hint: f64, window: usize) -> Self {
        assert!(window > 1, "an analysis window needs at least two samples");
        let cycles = (freq_hint * window as f64).round() as usize;
        assert!(
            cycles >= 1 && cycles * 2 < window,
            "a {freq_hint} cycles/sample tone does not resolve in a {window}-sample window"
        );
        Self {
            freq: cycles as f64 / window as f64,
            cycles,
            window,
        }
    }
}

/// `amplitude·cos(2π·freq·n)` — the message every analog measurement is taken with, and the one
/// [`theory`](super::theory)'s `message_power = ½` figures of merit are stated for.
#[must_use]
pub fn tone(freq: f64, amplitude: f32, samples: usize) -> Vec<f32> {
    (0..samples)
        .map(|n| amplitude * (TAU * freq * n as f64).cos() as f32)
        .collect()
}

/// What one analysis window says about a recovered tone. Powers are DC-free: the mean is
/// removed first, because an analog detector's DC is its own offset and never the message.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToneAnalysis {
    /// Amplitude of the recovered fundamental — the detector's gain, which no ratio below
    /// depends on but which a loopback wants to see is non-zero.
    pub amplitude: f64,
    /// Mean square of the window with its DC removed.
    pub ac_power: f64,
    pub fundamental_power: f64,
    /// Power in harmonics 2..=[`MAX_HARMONIC`] that fall below Nyquist.
    pub harmonic_power: f64,
}

/// Harmonics counted toward THD. Ten is the instrument convention and past any order an audio
/// filter at the message bandwidth leaves standing anyway.
pub const MAX_HARMONIC: usize = 10;

impl ToneAnalysis {
    /// Signal-to-noise-and-distortion in dB: total over everything that is not the
    /// fundamental.
    ///
    /// Both infinities are reachable and they mean opposite things, so neither is saturated
    /// away: `+∞` is a window that is pure tone — real only for a synthetic input — and `−∞`
    /// is a window with no power in it at all, which is a demodulator that returned nothing
    /// and must read as the worst possible outcome rather than the best.
    #[must_use]
    pub fn sinad_db(&self) -> f64 {
        if self.ac_power <= 0.0 {
            return f64::NEG_INFINITY;
        }
        let residual = self.ac_power - self.fundamental_power;
        if residual <= 0.0 {
            return f64::INFINITY;
        }
        10.0 * (self.ac_power / residual).log10()
    }

    /// Total harmonic distortion as a fraction of the fundamental's amplitude — the second
    /// half of the §5 item 4 pair, and the one that separates a detector's own nonlinearity
    /// from the channel's noise.
    #[must_use]
    pub fn thd(&self) -> f64 {
        if self.fundamental_power <= 0.0 {
            return f64::INFINITY;
        }
        (self.harmonic_power / self.fundamental_power).sqrt()
    }
}

/// Reads one window: DC removed, the fundamental and its harmonics correlated out at their
/// exact frequencies, everything else left in the denominator.
///
/// Correlation rather than a windowed transform, and exact for the reason [`TonePlan`] exists:
/// at a whole number of cycles the complex exponentials at `k·freq` are mutually orthogonal
/// over the window, so each amplitude is read without leakage from any other.
#[must_use]
pub fn analyse_tone(audio: &[f32], freq: f64) -> ToneAnalysis {
    let n = audio.len();
    if n == 0 {
        return ToneAnalysis {
            amplitude: 0.0,
            ac_power: 0.0,
            fundamental_power: 0.0,
            harmonic_power: 0.0,
        };
    }
    let mean = audio.iter().map(|&x| f64::from(x)).sum::<f64>() / n as f64;
    let ac_power = audio
        .iter()
        .map(|&x| {
            let v = f64::from(x) - mean;
            v * v
        })
        .sum::<f64>()
        / n as f64;
    let component = |f: f64| {
        let mut acc = Complex::new(0.0, 0.0);
        for (i, &x) in audio.iter().enumerate() {
            acc += Complex::from_polar(f64::from(x) - mean, -TAU * f * i as f64);
        }
        // A real tone splits its power between ±f, so the one-sided amplitude is twice the
        // correlation and its power half that amplitude squared.
        2.0 * acc.norm() / n as f64
    };
    let amplitude = component(freq);
    let harmonic_power = (2..=MAX_HARMONIC)
        .map(|k| k as f64 * freq)
        .filter(|f| *f < 0.5)
        .map(|f| 0.5 * component(f).powi(2))
        .sum();
    ToneAnalysis {
        amplitude,
        ac_power,
        fundamental_power: 0.5 * amplitude * amplitude,
        harmonic_power,
    }
}

// --- The link ---------------------------------------------------------------------------------

/// Audio to complex baseband — the analog counterpart of [`ModulateFn`](super::sweep::ModulateFn).
pub type ModulateAudioFn = Box<dyn Fn(&[f32]) -> Vec<Complex<f32>>>;

/// Complex baseband back to audio, sample for sample.
pub type DemodulateAudioFn = Box<dyn Fn(&[Complex<f32>]) -> Vec<f32>>;

/// One analog chain under test. Both closures construct their own engines per call: an analog
/// receiver holds loop and filter state, and a measurement that inherited the previous point's
/// converged carrier loop would be measuring the sweep's order rather than the entry.
pub struct AnalogLink {
    /// Names the chain in curve labels, e.g. `"AM full carrier, depth 0.8, envelope"`.
    pub label: String,
    /// Message bandwidth in cycles per sample — what the channel SNR is stated in, and what
    /// the entry's own filters cut at.
    pub bandwidth: f64,
    pub tone: TonePlan,
    /// Peak audio amplitude the modulator is driven at.
    pub drive: f32,
    /// Samples discarded ahead of the analysis window: filter group delay, and whatever a
    /// carrier loop needs to acquire.
    pub settle: usize,
    pub modulate: ModulateAudioFn,
    pub demodulate: DemodulateAudioFn,
}

impl AnalogLink {
    /// Audio samples one trial generates — settle plus window.
    #[must_use]
    pub fn samples(&self) -> usize {
        self.settle + self.tone.window
    }
}

/// One trial: tone in, waveform through `channel`, audio out, one window analysed.
///
/// **The modulator is primed and its own transient discarded before the channel sees the
/// waveform**, which is not tidiness but accounting: [`Awgn::for_channel_snr`] sets the noise
/// from the waveform's *measured* power, and a filter ramping up over its first few hundred
/// samples lowers that mean — so a curve taken on an unprimed waveform reads a channel SNR
/// higher than the one it claims, uniformly and in the flattering direction. `settle` samples of
/// lead-in are generated and dropped, after which every sample handed to the channel is steady
/// state. The receiver's own transient is what the `settle` at the *other* end covers.
///
/// A demodulator returning fewer samples than the window needs has lost them, and the analysis
/// runs on what arrived — which reads as a collapsed SINAD rather than as a silent success,
/// the same doctrine the sweep runner applies to lost bits.
pub fn measure_tone(
    link: &AnalogLink,
    channel: &dyn Impairment,
    rng: &mut Rng,
) -> (ToneAnalysis, usize) {
    let audio = tone(link.tone.freq, link.drive, link.settle + link.samples());
    let mut wave = (link.modulate)(&audio);
    wave.drain(..link.settle.min(wave.len()));
    channel.apply(&mut wave, rng);
    let out = (link.demodulate)(&wave);
    let end = out.len().min(link.samples());
    let start = link.settle.min(end);
    (analyse_tone(&out[start..end], link.tone.freq), end - start)
}

// --- The committed curve ----------------------------------------------------------------------

/// One measured point of a SINAD curve. `thd_percent` rides along because §5 item 4 asks for
/// both and because a point whose SINAD is distortion-limited rather than noise-limited is
/// unreadable without it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SinadPoint {
    pub snr_db: f64,
    pub sinad_db: f64,
    pub thd_percent: f64,
}

/// A measured SINAD curve — the committed artifact behind an analog entry's §4.1 correctness.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SinadCurve {
    pub label: String,
    pub points: Vec<SinadPoint>,
}

/// Measures one SINAD curve: at each channel SNR in `points_db` (ascending), `trials`
/// independent realisations are modulated, noised, demodulated and analysed, and their powers
/// summed before the ratio is taken — an average of ratios would be dominated by whichever
/// trial happened to come out cleanest.
///
/// Determinism follows the BER sweep exactly: `(seed, point index)` names a point's whole
/// realisation, so any single point regenerates without resweeping the rest.
pub fn sweep_sinad(
    link: &AnalogLink,
    channel_template: &ChannelSpec,
    points_db: &[f64],
    seed: u64,
    trials: usize,
) -> SinadCurve {
    let mut points = Vec::with_capacity(points_db.len());
    for (index, &snr_db) in points_db.iter().enumerate() {
        let mut rng = Rng::new(point_seed(seed, index));
        let channel = channel_template
            .awgn(Awgn::for_channel_snr(snr_db, link.bandwidth))
            .build();
        let (mut ac, mut fundamental, mut harmonic) = (0.0, 0.0, 0.0);
        for _ in 0..trials.max(1) {
            let (analysis, _) = measure_tone(link, &channel, &mut rng);
            ac += analysis.ac_power;
            fundamental += analysis.fundamental_power;
            harmonic += analysis.harmonic_power;
        }
        let summed = ToneAnalysis {
            amplitude: (2.0 * fundamental / trials.max(1) as f64).sqrt(),
            ac_power: ac,
            fundamental_power: fundamental,
            harmonic_power: harmonic,
        };
        points.push(SinadPoint {
            snr_db,
            sinad_db: summed.sinad_db(),
            thd_percent: 100.0 * summed.thd(),
        });
    }
    SinadCurve {
        label: format!("{}, SINAD vs channel SNR, seed {seed:#x}", link.label),
        points,
    }
}

/// One seeded SINAD measurement at one operating point, reported as the limits runner's cost:
/// **negated SINAD in dB**, so that a floor criterion becomes the same ceiling every other axis
/// row is judged by (see [`limits`](super::limits)). The intended body of an analog axis-search
/// closure, and the analog counterpart of [`measure_ber`](super::limits::measure_ber).
///
/// The same `seed` is passed at every axis value on purpose — common random numbers, so probes
/// differ only in the impairment level and the search's boundary is a property of the axis. An
/// impossible empty sweep reads as `+∞`: certain failure, never a silent pass.
pub fn sinad_metric(
    link: &AnalogLink,
    spec: &ChannelSpec,
    snr_db: f64,
    seed: u64,
    trials: usize,
) -> f64 {
    sweep_sinad(link, spec, &[snr_db], seed, trials)
        .points
        .first()
        .map_or(f64::INFINITY, |p| -p.sinad_db)
}

/// Writes the curve as pretty JSON — the committed-artifact format, same as the BER curves.
pub fn save_json(curve: &SinadCurve, path: &Path) -> io::Result<()> {
    let mut text = serde_json::to_string_pretty(curve).map_err(io::Error::other)?;
    text.push('\n');
    fs::write(path, text)
}

pub fn load_json(path: &Path) -> io::Result<SinadCurve> {
    serde_json::from_str(&fs::read_to_string(path)?).map_err(io::Error::other)
}

/// Writes `snr_db,sinad_db,thd_percent` rows — the plotting/export format.
pub fn save_csv(curve: &SinadCurve, path: &Path) -> io::Result<()> {
    let mut text = String::from("snr_db,sinad_db,thd_percent\n");
    for p in &curve.points {
        text.push_str(&format!("{},{},{}\n", p.snr_db, p.sinad_db, p.thd_percent));
    }
    fs::write(path, text)
}

// --- Comparators ------------------------------------------------------------------------------
//
// Vertical, in dB, and that is not a departure from the BER comparators' horizontal rule but
// the same rule where the curve is a straight line of unit slope: above threshold SINAD is the
// channel SNR plus a constant, so a dB of vertical shortfall *is* a dB of horizontal one. Below
// threshold the curve turns over and the horizontal distance stops existing at all, while the
// vertical one still reads exactly what was lost — which is the quantity an analog entry's knee
// has to be stated in.
//
// Failure stays loud: a comparison that cannot be made returns +∞ and fails any `< tolerance`
// gate rather than passing it vacuously.

/// Worst amount by which a measured curve falls below a closed-form oracle over `[lo, hi]`, in
/// dB. Positive is a loss; a measurement *above* its oracle past the noise is a harness defect,
/// so the sign is kept and gates take `.abs()` where they mean both directions.
pub fn worst_shortfall_db(
    measured: &SinadCurve,
    oracle: impl Fn(f64) -> f64,
    lo: f64,
    hi: f64,
) -> f64 {
    let mut worst = f64::INFINITY;
    let mut any = false;
    for p in &measured.points {
        if p.snr_db < lo || p.snr_db > hi {
            continue;
        }
        if !p.sinad_db.is_finite() {
            return f64::INFINITY;
        }
        let gap = oracle(p.snr_db) - p.sinad_db;
        if !any || gap.abs() > worst.abs() {
            worst = gap;
            any = true;
        }
    }
    worst
}

/// [`worst_shortfall_db`] against a committed curve instead of a closed form — the guard every
/// analog row carries, including the below-threshold points no oracle describes. Points are
/// matched by their own SNR: a committed grid and its reproduction share it exactly, and a
/// point the reference does not carry is a grid that moved, which must fail rather than
/// interpolate.
///
/// The grid is checked in *both* directions over `[lo, hi]`. A measured curve that simply omits
/// the SNRs it would have regressed at is the same defect as one that moved the grid, and only
/// the reverse check sees it.
pub fn worst_shortfall_db_vs_curve(
    measured: &SinadCurve,
    reference: &SinadCurve,
    lo: f64,
    hi: f64,
) -> f64 {
    let in_span = |p: &&SinadPoint| p.snr_db >= lo && p.snr_db <= hi;
    let same_snr = |a: f64, b: f64| (a - b).abs() < 1e-9;
    let missing = reference
        .points
        .iter()
        .filter(in_span)
        .any(|r| !measured.points.iter().any(|p| same_snr(p.snr_db, r.snr_db)));
    if missing {
        return f64::INFINITY;
    }
    let mut worst = f64::INFINITY;
    let mut any = false;
    for p in &measured.points {
        if p.snr_db < lo || p.snr_db > hi {
            continue;
        }
        let Some(r) = reference
            .points
            .iter()
            .find(|q| same_snr(q.snr_db, p.snr_db))
        else {
            return f64::INFINITY;
        };
        if !p.sinad_db.is_finite() || !r.sinad_db.is_finite() {
            return f64::INFINITY;
        }
        let gap = r.sinad_db - p.sinad_db;
        if !any || gap.abs() > worst.abs() {
            worst = gap;
            any = true;
        }
    }
    worst
}

/// The channel SNR at which `curve` first reaches `sinad_db`, by linear interpolation — the
/// analog sensitivity (§4.3 row one under its documented override). Both axes are dB and the
/// relation above threshold is a straight line, so linear interpolation is the honest one here
/// exactly as log-BER interpolation is there. `None` when the swept span never reaches it.
#[must_use]
pub fn snr_at_sinad(curve: &SinadCurve, sinad_db: f64) -> Option<f64> {
    for pair in curve.points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if !a.sinad_db.is_finite() || !b.sinad_db.is_finite() {
            continue;
        }
        if (a.sinad_db - sinad_db) * (b.sinad_db - sinad_db) <= 0.0 {
            if (b.sinad_db - a.sinad_db).abs() < 1e-12 {
                return Some(a.snr_db);
            }
            let t = (sinad_db - a.sinad_db) / (b.sinad_db - a.sinad_db);
            return Some(a.snr_db + t * (b.snr_db - a.snr_db));
        }
    }
    None
}

/// The **knee**: the highest channel SNR at which the measured curve still sits `drop_db` or
/// more below its own oracle — the one quantity an analog entry has that no closed form
/// describes, and the number its threshold behaviour is committed as.
///
/// Read from the top down, so a curve that dips inside its linear region (counting noise, a
/// distortion-limited point) does not read as a threshold. `None` when the whole swept span is
/// within `drop_db` of theory — the entry's threshold is below the grid, and widening the grid
/// is the fix rather than a projected number.
#[must_use]
pub fn threshold_db(curve: &SinadCurve, oracle: impl Fn(f64) -> f64, drop_db: f64) -> Option<f64> {
    curve
        .points
        .iter()
        .rev()
        .find(|p| !p.sinad_db.is_finite() || oracle(p.snr_db) - p.sinad_db >= drop_db)
        .map(|p| p.snr_db)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ber::theory;

    /// The analyser on signals whose content is known exactly: a pure tone is all fundamental,
    /// a tone plus a known second harmonic reads that harmonic back, and a tone plus known
    /// noise reads the SINAD the powers imply.
    #[test]
    fn the_analyser_reads_known_content() {
        let plan = TonePlan::new(0.021, 4_096);
        let pure = tone(plan.freq, 0.5, plan.window);
        let analysis = analyse_tone(&pure, plan.freq);
        assert!((analysis.amplitude - 0.5).abs() < 1e-4);
        assert!((analysis.ac_power - 0.125).abs() < 1e-6);
        assert!(analysis.thd() < 1e-5, "thd {}", analysis.thd());
        assert!(analysis.sinad_db() > 90.0, "sinad {}", analysis.sinad_db());

        // A 10 % second harmonic is 10 % THD, and −20 dB of distortion is 20.04 dB of SINAD.
        let second = tone(2.0 * plan.freq, 0.05, plan.window);
        let distorted: Vec<f32> = pure.iter().zip(&second).map(|(a, b)| a + b).collect();
        let analysis = analyse_tone(&distorted, plan.freq);
        assert!(
            (analysis.thd() - 0.1).abs() < 1e-3,
            "thd {}",
            analysis.thd()
        );
        assert!(
            (analysis.sinad_db() - 20.043).abs() < 0.05,
            "sinad {}",
            analysis.sinad_db()
        );

        // DC is the detector's own offset and must not enter any power.
        let offset: Vec<f32> = pure.iter().map(|x| x + 3.0).collect();
        let analysis = analyse_tone(&offset, plan.freq);
        assert!((analysis.ac_power - 0.125).abs() < 1e-6);
        assert!(analysis.sinad_db() > 90.0);
    }

    /// Snapping is what makes the correlation exact, and orthogonality is the property that
    /// exactness rests on: at a whole number of cycles, a neighbouring bin reads nothing of the
    /// tone at all — which is what keeps a harmonic read out of the fundamental's estimate and
    /// the fundamental out of the residual that becomes the denominator.
    #[test]
    fn snapping_makes_neighbouring_bins_orthogonal() {
        let window = 4_096;
        let plan = TonePlan::new(0.021, window);
        assert_eq!(plan.cycles, 86);
        assert!((plan.freq - 86.0 / 4_096.0).abs() < 1e-15);
        let signal = tone(plan.freq, 0.5, window);
        assert!((analyse_tone(&signal, plan.freq).amplitude - 0.5).abs() < 1e-4);
        for neighbour in [85.0, 87.0, 172.0] {
            let read = analyse_tone(&signal, neighbour / window as f64).amplitude;
            assert!(read < 1e-4, "bin {neighbour} reads {read}");
        }
    }

    fn synthetic(fom: f64, points: &[f64]) -> SinadCurve {
        SinadCurve {
            label: "synthetic".to_string(),
            points: points
                .iter()
                .map(|&snr_db| SinadPoint {
                    snr_db,
                    sinad_db: theory::analog_sinad_db(fom, snr_db),
                    thd_percent: 0.0,
                })
                .collect(),
        }
    }

    #[test]
    fn comparators_read_a_known_shift_and_refuse_what_they_cannot_answer() {
        let grid = [0.0, 5.0, 10.0, 15.0, 20.0];
        let exact = synthetic(1.0, &grid);
        let oracle = |snr| theory::analog_sinad_db(1.0, snr);
        assert!(worst_shortfall_db(&exact, oracle, 0.0, 20.0).abs() < 1e-9);

        // A curve 0.5 dB down reads exactly 0.5 dB of shortfall, both against the oracle and
        // against the unshifted curve.
        let mut down = exact.clone();
        for p in &mut down.points {
            p.sinad_db -= 0.5;
        }
        assert!((worst_shortfall_db(&down, oracle, 0.0, 20.0) - 0.5).abs() < 1e-9);
        assert!((worst_shortfall_db_vs_curve(&down, &exact, 0.0, 20.0) - 0.5).abs() < 1e-9);

        // A grid that moved is not a comparison to interpolate.
        let moved = synthetic(1.0, &[0.0, 6.0, 12.0]);
        assert!(worst_shortfall_db_vs_curve(&moved, &exact, 0.0, 20.0).is_infinite());
        // Nor is a subset of the committed grid: every point it drops is a point it cannot
        // regress at, so the omission has to fail as loudly as a shift.
        let subset = synthetic(1.0, &[0.0, 10.0, 20.0]);
        assert!(worst_shortfall_db_vs_curve(&subset, &exact, 0.0, 20.0).is_infinite());
        // Narrowing the span to the points it does carry is how a partial curve is compared.
        assert!(worst_shortfall_db_vs_curve(&subset, &exact, 10.0, 10.0).abs() < 1e-9);
        // Nor is an empty span.
        assert!(worst_shortfall_db(&exact, oracle, 40.0, 50.0).is_infinite());
    }

    /// The two §4.3 reads on an analog curve: the SNR for a stated SINAD, and the knee where
    /// the measurement leaves its oracle.
    #[test]
    fn sensitivity_and_threshold_are_read_off_the_curve() {
        let grid = [0.0, 2.0, 4.0, 6.0, 8.0, 10.0, 12.0];
        let mut curve = synthetic(1.0, &grid);
        // 12 dB SINAD on a unity figure of merit is 12 dB of channel SNR.
        let snr = snr_at_sinad(&curve, 12.0).unwrap();
        assert!((snr - 12.0).abs() < 1e-9, "sensitivity {snr}");
        assert!(snr_at_sinad(&curve, 30.0).is_none());

        let oracle = |snr| theory::analog_sinad_db(1.0, snr);
        assert!(threshold_db(&curve, oracle, 1.0).is_none());
        // Bend the three lowest points down: the knee is the highest of them, read top-down.
        for p in curve.points.iter_mut().take(3) {
            p.sinad_db -= 4.0;
        }
        assert_eq!(threshold_db(&curve, oracle, 1.0), Some(4.0));
    }

    #[test]
    fn curves_round_trip_through_json_and_csv_keeps_both_columns() {
        let dir = std::env::temp_dir().join(format!("sdrmm-modem-sinad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let curve = synthetic(0.5, &[0.0, 10.0]);
        let json = dir.join("curve.json");
        save_json(&curve, &json).unwrap();
        assert_eq!(load_json(&json).unwrap(), curve);
        let csv = dir.join("curve.csv");
        save_csv(&curve, &csv).unwrap();
        let text = std::fs::read_to_string(&csv).unwrap();
        assert!(text.starts_with("snr_db,sinad_db,thd_percent\n"));
        assert_eq!(text.lines().count(), 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
