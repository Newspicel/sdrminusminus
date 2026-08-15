//! Stage two of identification: measure the waveform.
//!
//! Everything here runs on the *zoomed* signal — the detected band mixed to DC and decimated to a
//! few times its own width — because the features that separate one modulation from another are
//! ratios, and a ratio measured across a mostly-empty analysis slice is a ratio of noise.

use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_dsp::{Ddc, SpectrumAnalyzer};

use super::detect::Band;

/// Samples kept per Hz of occupied bandwidth. Four leaves room for a matched filter and puts the
/// symbol clock — never above half the occupied width — well inside the searchable range.
const ZOOM_OVERSAMPLE: f64 = 4.0;
/// Floor for the zoom rate. A 200 Hz teleprinter shift would otherwise decimate to a rate with no
/// room left for its own 45 baud clock.
const MIN_ZOOM_RATE_HZ: f64 = 2_000.0;
/// Output samples discarded while a filter chain fills. Its history starts at zero, so the
/// leading samples are a ramp rather than the signal.
const SETTLE: usize = 96;

/// Transform sizes the spectral searches run at, smallest first. A narrow signal zooms down to a
/// few thousand samples and cannot fill the large one; padding it instead would push its data
/// under the window's own taper and smear the line being looked for.
const FFT_SIZES: [usize; 3] = [256, 1_024, 4_096];
/// Below this there is not enough of anything to measure.
const MIN_ANALYSIS_SAMPLES: usize = FFT_SIZES[0];
/// Shortest keyed run the clock search will take. Shorter than this a burst holds too few symbols
/// to say anything about their rate, and joining it to the next one only adds a discontinuity.
const MIN_RUN_SAMPLES: usize = 256;
const MAX_SEGMENTS: usize = 16;

/// Bins skipped at the bottom of a clock search. An edge signal is mostly DC and drift; the clock
/// line is not down there, and the leakage skirt around bin zero would win every time.
const CLOCK_SKIRT_BINS: usize = 8;

/// Bins either side of a candidate that its own local background is estimated from, and the guard
/// band excluded from that estimate so a line cannot raise the floor it is being measured against.
///
/// A clock line does not sit on white noise. Squaring a modulated waveform produces the line
/// *and* a broad self-noise continuum that is strongest near DC, so a candidate measured against
/// the median of the whole spectrum is measured against the wrong thing — the low end scores high
/// everywhere and the real line at a few kilohertz loses to it. Measured against its own
/// neighbourhood, only something that stands out locally can win.
const BACKGROUND_BINS: usize = 32;
const GUARD_BINS: usize = 4;

/// Spread of the instantaneous frequency, as a fraction of the zoom rate, below which nothing is
/// moving and the histogram has no levels to resolve — only the shape of its own rounding. An
/// unmodulated or keyed carrier lands here, and must, because the clock of a keyed carrier is in
/// its envelope and asking the frequency for it would find nothing at all.
const MIN_SHIFT_FRACTION: f64 = 1e-4;

/// Levels resolved in the instantaneous-frequency histogram.
const HIST_BINS: usize = 96;
/// Peaks below this fraction of the tallest are shoulders, not levels.
const PEAK_FLOOR: f32 = 0.25;
/// How far apart two histogram peaks must sit to be two levels, in bins.
const PEAK_SEPARATION: usize = HIST_BINS / 12;

/// A spectral line has to stand this far over its own local background to be a line.
const LINE_THRESHOLD_DB: f32 = 6.0;
/// Segments a clock search must average before its answer is worth anything. A line found in a
/// two-segment periodogram is a noise bin — and a bare carrier, which has no clock at all, zooms
/// down to exactly the short record that produces those.
const MIN_CLOCK_SEGMENTS: usize = 3;

/// Harmonics of the symbol rate a clock search steps back down through.
///
/// An abrupt symbol transition is an impulse, and an impulse train puts equal energy in *every*
/// harmonic of its rate — so on a rectangular waveform the tallest line in the search range is as
/// likely to be the eighth harmonic as the first, and which of them wins is decided by noise. A
/// shaped waveform has no such ambiguity, its fundamental dominating outright.
const MAX_HARMONIC: usize = 8;
/// How far under the tallest line a sub-harmonic may stand and still be the real clock, measured
/// as local contrast. Not raw power: the low end of an edge spectrum carries the waveform's own
/// self-noise, which reaches half the height of a genuine line without being one.
const SUBHARMONIC_FRACTION: f32 = 0.5;

/// Level below the loud end of the envelope at which a sample stops counting as keyed on — nine
/// decibels, which is far under any real modulation trough and far over a gap between bursts.
const KEYED_FRACTION: f32 = 0.35;

/// Envelope spread of noise on its own — the Rayleigh coefficient of variation, `sqrt(4/π − 1)`.
/// No amount of noise moves the measurement past this, so no estimate of noise may claim more.
const RAYLEIGH_VARIATION: f32 = 0.522_723;

/// The detected band, mixed to DC and decimated to a rate matched to it.
pub(crate) struct Zoom {
    pub(crate) rate: f64,
    pub(crate) iq: Vec<Complex<f32>>,
}

/// Mix `band` to DC and decimate to a few times its own width.
pub(crate) fn zoom(iq: &[Complex<f32>], rate: f64, band: &Band) -> Option<Zoom> {
    let target = (band.bandwidth_hz * ZOOM_OVERSAMPLE).clamp(MIN_ZOOM_RATE_HZ, rate);
    let mut ddc = Ddc::new(rate, target, band.center_hz).ok()?;
    let mut out = Vec::with_capacity((iq.len() as f64 * target / rate) as usize + 1);
    ddc.process(iq, &mut out);
    if out.len() <= SETTLE + MIN_ANALYSIS_SAMPLES {
        return None;
    }
    out.drain(..SETTLE);
    Some(Zoom {
        rate: target,
        iq: out,
    })
}

/// What the waveform measurements came to.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Waveform {
    /// Standard deviation over mean of the envelope, measured while the carrier is *on* — so a
    /// keyed carrier reads near zero here and says what it has to say through `duty` instead.
    pub(crate) envelope_variation: f32,
    /// How much of `envelope_variation` the band's own noise accounts for. A constant-envelope
    /// signal reads whatever the noise puts on it, which at ordinary receive levels is more than
    /// any threshold a modulation could be recognised by — so this is what it has to clear.
    pub(crate) noise_variation: f32,
    /// Fraction of the observation the carrier was up.
    pub(crate) duty: f32,
    /// Keyed-on level over keyed-off level, in dB.
    pub(crate) on_off_db: f32,
    /// Distinct instantaneous-frequency levels resolved; 0 when the distribution is a continuum.
    pub(crate) frequency_levels: u8,
    /// Half the spacing between the outermost levels, or the spread when there are none.
    pub(crate) deviation_hz: f64,
    /// Dwell-weighted standard deviation of the instantaneous frequency, in Hz.
    pub(crate) frequency_spread_hz: f64,
    /// How deep the histogram runs between the outermost levels, as a fraction of the shallower
    /// of the two: 0 when they are separated by empty ground, 1 when nothing separates them.
    pub(crate) level_valley: f32,
    pub(crate) symbol_rate_hz: Option<f64>,
    pub(crate) square_line_db: f32,
    pub(crate) quartic_line_db: f32,
}

pub(crate) struct Meter {
    spectra: Vec<SpectrumAnalyzer>,
    amplitude: Vec<f32>,
    frequency: Vec<f32>,
    keyed: Vec<bool>,
    weight: Vec<f32>,
    edge: Vec<f32>,
    runs: Vec<(usize, usize)>,
    starts: Vec<usize>,
    fft_in: Vec<Complex<f32>>,
    fft_db: Vec<f32>,
    power: Vec<f32>,
    /// Each bin over its own local background — what a line is looked for in.
    contrast: Vec<f32>,
    scratch: Vec<f32>,
    histogram: Vec<f32>,
    smoothed: Vec<f32>,
}

impl Meter {
    pub(crate) fn new() -> Self {
        let largest = FFT_SIZES[FFT_SIZES.len() - 1];
        Self {
            spectra: FFT_SIZES
                .iter()
                .map(|&n| SpectrumAnalyzer::new(n))
                .collect(),
            amplitude: Vec::new(),
            frequency: Vec::new(),
            keyed: Vec::new(),
            weight: Vec::new(),
            edge: Vec::new(),
            runs: Vec::new(),
            starts: Vec::new(),
            fft_in: vec![Complex::new(0.0, 0.0); largest],
            fft_db: vec![0.0; largest],
            power: vec![0.0; largest],
            contrast: vec![0.0; largest],
            scratch: Vec::new(),
            histogram: vec![0.0; HIST_BINS],
            smoothed: vec![0.0; HIST_BINS],
        }
    }

    pub(crate) fn measure(&mut self, zoom: &Zoom, band: &Band) -> Waveform {
        let (duty, on_off_db, gate) = self.envelope(&zoom.iq);
        let envelope_variation = self.envelope_variation(gate);
        self.discriminate(&zoom.iq, zoom.rate, gate);

        let levels = self.levels(zoom.rate);
        let symbol_rate_hz = self.symbol_rate(zoom.rate, levels.count);
        let (square_line_db, quartic_line_db) = self.nonlinearity_lines(&zoom.iq);

        Waveform {
            envelope_variation,
            noise_variation: noise_variation(zoom.rate, band),
            duty,
            on_off_db,
            frequency_levels: levels.count,
            deviation_hz: levels.deviation_hz,
            frequency_spread_hz: levels.spread_hz,
            level_valley: levels.valley,
            symbol_rate_hz,
            square_line_db,
            quartic_line_db,
        }
    }

    /// Envelope statistics, and the level a sample has to reach to count as keyed on.
    fn envelope(&mut self, iq: &[Complex<f32>]) -> (f32, f32, f32) {
        self.amplitude.clear();
        self.amplitude.extend(iq.iter().map(|s| s.norm()));
        let high = self.percentile(0.90);
        let low = self.percentile(0.10);
        let gate = high * KEYED_FRACTION;
        let on = self.amplitude.iter().filter(|&&a| a >= gate).count();
        let duty = on as f32 / self.amplitude.len().max(1) as f32;
        let on_off_db = 20.0 * (high / low.max(f32::MIN_POSITIVE)).log10();
        (duty, on_off_db, gate)
    }

    fn percentile(&mut self, fraction: f32) -> f32 {
        self.scratch.clear();
        self.scratch.extend_from_slice(&self.amplitude);
        let n = self.scratch.len();
        if n == 0 {
            return 0.0;
        }
        let index = ((n - 1) as f32 * fraction) as usize;
        let (_, value, _) = self.scratch.select_nth_unstable_by(index, f32::total_cmp);
        *value
    }

    /// Amplitude spread over the keyed-on samples only. Measured there because the question it
    /// answers is whether the *carrier* carries information in its amplitude, and the gaps
    /// between bursts would answer it "yes" for every keyed mode there is.
    fn envelope_variation(&self, gate: f32) -> f32 {
        let mut n = 0u32;
        let mut sum = 0.0f64;
        let mut sum_sq = 0.0f64;
        for &a in self.amplitude.iter().filter(|&&a| a >= gate) {
            n += 1;
            sum += f64::from(a);
            sum_sq += f64::from(a) * f64::from(a);
        }
        if n < 2 {
            return 0.0;
        }
        let mean = sum / f64::from(n);
        let variance = (sum_sq / f64::from(n) - mean * mean).max(0.0);
        (variance.sqrt() / mean.max(f64::MIN_POSITIVE)) as f32
    }

    /// Instantaneous frequency in Hz, a keyed-on mask, and a dwell weight per sample.
    ///
    /// The weight is what makes a level histogram work on a shaped waveform: a keyed carrier
    /// spends most of each symbol *at* a level and a fraction of it in transit, so weighting each
    /// sample by how little the frequency is moving under it recovers the levels without needing
    /// symbol timing — which is not known yet, and which the levels help find.
    fn discriminate(&mut self, iq: &[Complex<f32>], rate: f64, gate: f32) {
        let scale = (rate / TAU) as f32;
        self.frequency.clear();
        self.frequency.push(0.0);
        for pair in iq.windows(2) {
            self.frequency
                .push((pair[1] * pair[0].conj()).arg() * scale);
        }

        let mut slope_sq = 0.0f64;
        for pair in self.frequency.windows(2) {
            let d = f64::from(pair[1] - pair[0]);
            slope_sq += d * d;
        }
        let slope_rms = (slope_sq / self.frequency.len().max(1) as f64)
            .sqrt()
            .max(1.0) as f32;

        self.keyed.clear();
        self.keyed.extend(self.amplitude.iter().map(|&a| a >= gate));

        self.weight.clear();
        self.weight.push(0.0);
        for i in 1..self.frequency.len() {
            let carrier = self.keyed[i] && self.keyed[i - 1];
            let slope = (self.frequency[i] - self.frequency[i - 1]) / slope_rms;
            let dwell = 1.0 / (1.0 + slope * slope);
            self.weight.push(if carrier {
                self.amplitude[i] * self.amplitude[i] * dwell
            } else {
                0.0
            });
        }
    }

    /// What the instantaneous-frequency histogram came to.
    fn levels(&mut self, rate: f64) -> Levels {
        let bare = |spread: f64| Levels {
            spread_hz: spread,
            count: if spread > 0.0 { 1 } else { 0 },
            deviation_hz: spread,
            valley: 1.0,
        };
        let total: f64 = self.weight.iter().map(|&w| f64::from(w)).sum();
        if total <= 0.0 {
            return bare(0.0);
        }
        let weighted = |power: i32, mean: f64| -> f64 {
            self.frequency
                .iter()
                .zip(&self.weight)
                .map(|(&f, &w)| f64::from(w) * (f64::from(f) - mean).powi(power))
                .sum::<f64>()
                / total
        };
        let mean = weighted(1, 0.0);
        let spread = weighted(2, mean).max(0.0).sqrt();
        if spread < rate * MIN_SHIFT_FRACTION {
            return bare(spread);
        }
        let span = spread * 3.0;

        let scale = HIST_BINS as f64 / (2.0 * span);
        self.histogram.fill(0.0);
        for (&f, &w) in self.frequency.iter().zip(&self.weight) {
            if w <= 0.0 {
                continue;
            }
            let position = (f64::from(f) - mean + span) * scale;
            if position < 0.0 || position >= HIST_BINS as f64 {
                continue;
            }
            self.histogram[position as usize] += w;
        }
        smooth(&self.histogram, &mut self.smoothed);
        smooth(&self.smoothed, &mut self.histogram);
        std::mem::swap(&mut self.histogram, &mut self.smoothed);

        let peaks = self.peaks();
        let to_hz = |bin: usize| (bin as f64 + 0.5) / scale - span + mean;
        let (deviation, valley) = match peaks.as_slice() {
            [] => (spread, 1.0),
            [only] => ((to_hz(*only) - mean).abs().max(spread), 1.0),
            [first, .., last] => (
                (to_hz(*last) - to_hz(*first)) / 2.0,
                self.valley(*first, *last),
            ),
        };
        Levels {
            spread_hz: spread,
            count: peaks.len().min(u8::MAX as usize) as u8,
            deviation_hz: deviation,
            valley,
        }
    }

    /// How deep the histogram runs between its outermost levels, as a fraction of the shallower
    /// of the two.
    ///
    /// This is what says the levels are levels. A carrier that is being keyed between two
    /// frequencies is *at* one of them at almost every instant, so the ground between them falls
    /// away to nothing; a voice waveform sweeping its passband spends time everywhere in
    /// between, and the bumps its histogram throws up stand on a floor nearly as high as they
    /// are.
    fn valley(&self, first: usize, last: usize) -> f32 {
        let rim = self.smoothed[first].min(self.smoothed[last]);
        if rim <= 0.0 {
            return 1.0;
        }
        let floor = self.smoothed[first..=last]
            .iter()
            .copied()
            .fold(f32::INFINITY, f32::min);
        (floor / rim).clamp(0.0, 1.0)
    }

    fn peaks(&self) -> Vec<usize> {
        let max = self.smoothed.iter().copied().fold(0.0f32, f32::max);
        if max <= 0.0 {
            return Vec::new();
        }
        let floor = max * PEAK_FLOOR;
        let mut peaks: Vec<usize> = Vec::new();
        for i in 1..HIST_BINS - 1 {
            let v = self.smoothed[i];
            if v < floor || v < self.smoothed[i - 1] || v < self.smoothed[i + 1] {
                continue;
            }
            match peaks.last() {
                // Two bins of one broad maximum are one level; the taller wins.
                Some(&previous) if i - previous < PEAK_SEPARATION => {
                    if v > self.smoothed[previous] {
                        let last = peaks.len() - 1;
                        peaks[last] = i;
                    }
                }
                _ => peaks.push(i),
            }
        }
        peaks
    }

    /// The symbol clock, from whichever of the frequency and the envelope shows it.
    ///
    /// Both are searched because which one carries the clock is exactly what is not known yet: a
    /// keyed carrier's edges are in its envelope, a shifted one's are in its frequency, and a
    /// phase-modulated one's show up in both.
    fn symbol_rate(&mut self, rate: f64, levels: u8) -> Option<f64> {
        // A shifted carrier keeps its clock in the frequency. A TDMA radio keeps a *burst*
        // cadence in its envelope as well, an order of magnitude slower than its symbols, so the
        // envelope is only asked when the frequency has no levels to have shifted between.
        let series: &[Series] = if levels >= 2 {
            &[Series::Frequency]
        } else {
            &[Series::Envelope, Series::Frequency]
        };
        let mut best: Option<(f64, f32)> = None;
        for &series in series {
            for &detector in &[Detector::Square, Detector::Difference] {
                self.build_edge(series, detector);
                if let Some((hz, db)) = self.line_search(rate)
                    && best.is_none_or(|(_, previous)| db > previous)
                {
                    best = Some((hz, db));
                }
            }
        }
        best.filter(|&(_, db)| db >= LINE_THRESHOLD_DB)
            .map(|(hz, _)| hz)
    }

    /// A nonlinearity of one of the two series, chosen so its result carries a spectral line at
    /// the symbol rate.
    ///
    /// Both detectors are needed and neither subsumes the other. Squaring is textbook timing
    /// recovery for a pulse-amplitude waveform — which is what the instantaneous frequency of a
    /// shaped keyed carrier is — but it needs excess bandwidth: square a *rectangular* two-level
    /// shift and the result is a constant with no line in it at all. Differencing is the
    /// complementary case, strongest exactly where the waveform steps.
    ///
    /// The frequency series is taken from inside one transmission, never across the gaps between
    /// them. Blanking the dead air instead would multiply the series by the burst pattern, and a
    /// multiplication in time is a convolution in frequency — the clock line smears out and the
    /// burst cadence's own harmonics arrive to be mistaken for it.
    fn build_edge(&mut self, series: Series, detector: Detector) {
        let values: &[f32] = match series {
            Series::Frequency => &self.frequency,
            Series::Envelope => &self.amplitude,
        };
        self.runs.clear();
        match series {
            Series::Frequency => keyed_runs(&self.keyed, MIN_RUN_SAMPLES, &mut self.runs),
            Series::Envelope => self.runs.push((0, values.len())),
        }
        let live: usize = self.runs.iter().map(|&(a, b)| b - a).sum();
        let mean = if live == 0 {
            0.0
        } else {
            (self
                .runs
                .iter()
                .flat_map(|&(a, b)| values[a..b].iter())
                .map(|&v| f64::from(v))
                .sum::<f64>()
                / live as f64) as f32
        };

        // Built over the whole series, keeping index alignment with `self.runs`: the runs are
        // what the transform is *taken inside*, and joining them into one buffer instead would
        // splice unrelated symbol phases together and erase the line being looked for.
        self.edge.clear();
        match detector {
            Detector::Square => self
                .edge
                .extend(values.iter().map(|&v| (v - mean) * (v - mean))),
            Detector::Difference => {
                self.edge.push(0.0);
                self.edge.extend(values.windows(2).map(|p| {
                    let d = p[1] - p[0];
                    d * d
                }));
            }
        }
    }

    /// Strongest spectral line in `self.edge`, taken inside `self.runs`, and how far it stands
    /// over its own local background.
    fn line_search(&mut self, rate: f64) -> Option<(f64, f32)> {
        let size = self.spectrum_of_runs()?;

        let bin_hz = rate / size as f64;
        let center = size / 2;
        // A symbol rate cannot exceed the bandwidth its symbols occupy, and the zoom's rate *is*
        // that bandwidth times [`ZOOM_OVERSAMPLE`]. Without the bound the search runs up into the
        // twentieth harmonic of a rectangular waveform's clock, where the background is lowest
        // and no sub-harmonic step reaches back down to the rate itself.
        let first = center + CLOCK_SKIRT_BINS;
        let last = (center + (rate / ZOOM_OVERSAMPLE / bin_hz) as usize).min(size - 2);
        if first >= last {
            return None;
        }
        self.contrast(size);
        let tallest =
            (first..=last).max_by(|&a, &b| self.contrast[a].total_cmp(&self.contrast[b]))?;
        // Strength is read at the line that was actually found, and the frequency at whichever
        // sub-harmonic of it is the real clock: the evidence that there *is* a symbol clock is
        // the tallest line, whether or not it turns out to be the first harmonic.
        let db = 10.0 * self.contrast[tallest].max(f32::MIN_POSITIVE).log10();
        let peak = self.fundamental(tallest, center, first);
        Some((interpolate(&self.contrast, peak, center, bin_hz), db))
    }

    /// Each bin of `self.power[..size]` over the mean of its neighbourhood, guard band excluded.
    ///
    /// The background is floored relative to the spectrum's own mean. Some waveforms make one of
    /// the detectors produce an *exactly* constant series — squaring a rectangular two-level
    /// shift is the case that turns up — and an unfloored ratio there is a division of zero by
    /// zero that beats every real line in the comparison that follows.
    fn contrast(&mut self, size: usize) {
        self.scratch.clear();
        self.scratch.push(0.0);
        let mut running = 0.0f64;
        for &p in &self.power[..size] {
            running += f64::from(p);
            self.scratch.push(running as f32);
        }
        let quiet = (running / size as f64 * 1e-6) as f32;
        if quiet <= 0.0 || !quiet.is_finite() {
            self.contrast[..size].fill(1.0);
            return;
        }
        let sum =
            |prefix: &[f32], lo: usize, hi: usize| prefix[hi.min(size)] - prefix[lo.min(size)];
        for i in 0..size {
            let wide = sum(
                &self.scratch,
                i.saturating_sub(BACKGROUND_BINS),
                i + BACKGROUND_BINS + 1,
            );
            let guarded = sum(
                &self.scratch,
                i.saturating_sub(GUARD_BINS),
                i + GUARD_BINS + 1,
            );
            let bins = (i + BACKGROUND_BINS + 1).min(size)
                - i.saturating_sub(BACKGROUND_BINS)
                - ((i + GUARD_BINS + 1).min(size) - i.saturating_sub(GUARD_BINS));
            let background = (wide - guarded) / bins.max(1) as f32;
            self.contrast[i] = self.power[i] / background.max(quiet);
        }
    }

    /// The lowest sub-harmonic of `tallest` that carries comparable energy and is a line in its
    /// own right — the symbol rate, where the tallest line is one of its harmonics.
    fn fundamental(&self, tallest: usize, center: usize, first: usize) -> usize {
        let strong = self.contrast[tallest] * SUBHARMONIC_FRACTION;
        let offset = tallest - center;
        for divisor in (2..=MAX_HARMONIC).rev() {
            let candidate = center + offset / divisor;
            if candidate <= first + 2 {
                continue;
            }
            // A harmonic of a clock that fell between bins sits a bin either side of where whole
            // division puts it, so the neighbours count as the same line.
            let Some(local) = (candidate - 1..=candidate + 1)
                .max_by(|&a, &b| self.contrast[a].total_cmp(&self.contrast[b]))
            else {
                continue;
            };
            let shoulder = self.contrast[local - 2].max(self.contrast[local + 2]);
            if self.contrast[local] >= strong && self.contrast[local] > shoulder {
                return local;
            }
        }
        tallest
    }

    /// How strongly the squared and fourth-power signals ring — the classic phase-modulation
    /// tests, run on the unit-modulus signal so amplitude cannot decide the answer.
    fn nonlinearity_lines(&mut self, iq: &[Complex<f32>]) -> (f32, f32) {
        if iq.len() < MIN_ANALYSIS_SAMPLES {
            return (0.0, 0.0);
        }
        let unit = |x: Complex<f32>| {
            let n = x.norm();
            if n > f32::MIN_POSITIVE {
                x / n
            } else {
                Complex::new(0.0, 0.0)
            }
        };
        let square = self.line_strength(iq.len(), |i| {
            let u = unit(iq[i]);
            u * u
        });
        let quartic = self.line_strength(iq.len(), |i| {
            let u = unit(iq[i]);
            let s = u * u;
            s * s
        });
        (square, quartic)
    }

    fn line_strength(&mut self, len: usize, sample: impl Fn(usize) -> Complex<f32>) -> f32 {
        let size = self.average_spectrum(len, sample);
        let Some(peak) = (0..size).max_by(|&a, &b| self.power[a].total_cmp(&self.power[b])) else {
            return 0.0;
        };
        let median = self.median_power(0, size - 1);
        10.0 * (self.power[peak] / median.max(f32::MIN_POSITIVE)).log10()
    }

    /// Welch-average `self.edge` into the head of `self.power`, taking every segment that fits
    /// wholly inside one of `self.runs`. Returns the transform size used.
    fn spectrum_of_runs(&mut self) -> Option<usize> {
        // The largest transform that still gets averaged. Size and averaging pull in opposite
        // directions and size wins: a symbol clock is a *line*, so its power lands in one bin
        // whatever the resolution while the background under it falls as the bins get finer —
        // quadrupling the transform buys six decibels of contrast, which no amount of extra
        // averaging matches. Averaging only has to be enough that the answer is not one bin's
        // luck, which is what a burst-mode signal's short runs bound it to.
        let mut index = 0;
        let mut size = 0;
        for (candidate, &n) in FFT_SIZES.iter().enumerate().rev() {
            self.plan_segments(n);
            if self.starts.len() >= MIN_CLOCK_SEGMENTS {
                (index, size) = (candidate, n);
                break;
            }
        }
        if size == 0 {
            return None;
        }
        // Spread the segments taken across everything available rather than taking the first few:
        // a burst-mode transmission's first bursts are not more representative than its last.
        let stride = self.starts.len().div_ceil(MAX_SEGMENTS);

        self.power[..size].fill(0.0);
        let mut segments = 0u32;
        for k in (0..self.starts.len()).step_by(stride) {
            let start = self.starts[k];
            for (slot, &v) in self.fft_in[..size]
                .iter_mut()
                .zip(&self.edge[start..start + size])
            {
                *slot = Complex::new(v, 0.0);
            }
            let (input, output) = (&self.fft_in[..size], &mut self.fft_db[..size]);
            self.spectra[index].power_db(input, output);
            for (acc, &db) in self.power[..size].iter_mut().zip(&self.fft_db[..size]) {
                *acc += (db * 0.332_192_8).exp2();
            }
            segments += 1;
        }
        let scale = 1.0 / f32::from(u16::try_from(segments).unwrap_or(u16::MAX)).max(1.0);
        for p in &mut self.power[..size] {
            *p *= scale;
        }
        Some(size)
    }

    /// Every `size`-sample segment that fits wholly inside one of `self.runs`, half-overlapped.
    fn plan_segments(&mut self, size: usize) {
        self.starts.clear();
        for &(a, b) in &self.runs {
            let mut start = a;
            while start + size <= b {
                self.starts.push(start);
                start += size / 2;
            }
        }
    }

    /// Welch-average `len` samples drawn through `sample` into the head of `self.power`, at the
    /// largest transform size the data can fill. Returns that size.
    fn average_spectrum(&mut self, len: usize, sample: impl Fn(usize) -> Complex<f32>) -> usize {
        let index = FFT_SIZES.iter().rposition(|&n| n <= len).unwrap_or(0);
        let size = FFT_SIZES[index];
        let spare = len - size;
        let segments = (spare / (size / 2) + 1).min(MAX_SEGMENTS);
        let hop = if segments > 1 {
            spare / (segments - 1)
        } else {
            0
        };

        self.power[..size].fill(0.0);
        for s in 0..segments {
            let start = s * hop;
            for (k, slot) in self.fft_in[..size].iter_mut().enumerate() {
                *slot = sample(start + k);
            }
            let (input, output) = (&self.fft_in[..size], &mut self.fft_db[..size]);
            self.spectra[index].power_db(input, output);
            for (acc, &db) in self.power[..size].iter_mut().zip(&self.fft_db[..size]) {
                *acc += (db * 0.332_192_8).exp2();
            }
        }
        let scale = 1.0 / segments as f32;
        for p in &mut self.power[..size] {
            *p *= scale;
        }
        size
    }

    fn median_power(&mut self, lo: usize, hi: usize) -> f32 {
        self.scratch.clear();
        self.scratch.extend_from_slice(&self.power[lo..=hi]);
        let mid = self.scratch.len() / 2;
        let (_, median, _) = self.scratch.select_nth_unstable_by(mid, f32::total_cmp);
        *median
    }
}

/// The envelope spread the band's noise produces on its own, at the rate the envelope was
/// measured at.
///
/// Noise of power `N` under a signal of power `S` puts a spread of `sqrt(N / 2S)` on the
/// envelope. The band's ratio is quoted over the occupied width alone, while the envelope is
/// measured across the whole zoomed slice — so every hertz by which the zoom is wider than the
/// signal brings in noise the band's own figure never counted.
fn noise_variation(zoom_rate: f64, band: &Band) -> f32 {
    let signal_to_noise = 10.0_f64.powf(f64::from(band.snr_db) / 10.0);
    if !signal_to_noise.is_finite() || signal_to_noise <= 0.0 {
        return RAYLEIGH_VARIATION;
    }
    let widening = (zoom_rate / band.bandwidth_hz.max(1.0)).max(1.0);
    ((widening / (2.0 * signal_to_noise)).sqrt() as f32).min(RAYLEIGH_VARIATION)
}

/// What the instantaneous-frequency histogram came to.
struct Levels {
    spread_hz: f64,
    count: u8,
    deviation_hz: f64,
    valley: f32,
}

/// Which measured series a clock is looked for in.
#[derive(Clone, Copy)]
enum Series {
    Frequency,
    Envelope,
}

/// The nonlinearity applied to it.
#[derive(Clone, Copy)]
enum Detector {
    Square,
    Difference,
}

/// The runs of consecutive keyed-on samples at least `minimum` long, as half-open ranges.
///
/// Every one of them is measured, not just the longest: a TDMA radio's bursts are 30 ms each, and
/// one of those holds too few symbol periods to resolve their rate. What must not happen is the
/// *gaps* being measured too — blanking them instead of skipping them multiplies the series by
/// the burst pattern, and that convolution puts the burst cadence's harmonics into the search.
fn keyed_runs(keyed: &[bool], minimum: usize, out: &mut Vec<(usize, usize)>) {
    let mut start = 0;
    for i in 0..=keyed.len() {
        if i < keyed.len() && keyed[i] {
            continue;
        }
        if i - start >= minimum {
            out.push((start, i));
        }
        start = i + 1;
    }
}

fn smooth(input: &[f32], out: &mut [f32]) {
    debug_assert_eq!(input.len(), out.len());
    for i in 0..input.len() {
        let left = input[i.saturating_sub(1)];
        let right = input[(i + 1).min(input.len() - 1)];
        out[i] = (left + 2.0 * input[i] + right) * 0.25;
    }
}

/// Parabolic interpolation around a spectral peak, so a clock that falls between bins is reported
/// where it is rather than rounded to the nearest bin.
fn interpolate(power: &[f32], peak: usize, center: usize, bin_hz: f64) -> f64 {
    let left = f64::from(power[peak - 1]);
    let mid = f64::from(power[peak]);
    let right = f64::from(power[peak + 1]);
    let denominator = left - 2.0 * mid + right;
    let shift = if denominator.abs() > f64::MIN_POSITIVE {
        (0.5 * (left - right) / denominator).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    (peak as f64 - center as f64 + shift) * bin_hz
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;

    /// Continuous-phase FSK at `baud`, one of `levels` per symbol, scaled by `deviation_hz`.
    fn fsk(
        levels: &[f64],
        baud: f64,
        deviation_hz: f64,
        symbols: usize,
        seed: u32,
    ) -> Vec<Complex<f32>> {
        let sps = (RATE / baud) as usize;
        let mut state = seed | 1;
        let mut phase = 0.0f64;
        let mut out = Vec::with_capacity(symbols * sps);
        for _ in 0..symbols {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let level = levels[state as usize % levels.len()];
            for _ in 0..sps {
                phase += TAU * level * deviation_hz / RATE;
                out.push(Complex::from_polar(1.0, phase as f32));
            }
        }
        out
    }

    /// The band the generated signals stand in: clean, and filling the quarter of the zoom the
    /// oversample factor gives it.
    fn band() -> Band {
        Band {
            center_hz: 0.0,
            bandwidth_hz: RATE / ZOOM_OVERSAMPLE,
            snr_db: 40.0,
            carrier_db: 3.0,
            flatness: 0.3,
            skew: 0.0,
            peak_hz: 0.0,
        }
    }

    fn measure(iq: Vec<Complex<f32>>) -> Waveform {
        Meter::new().measure(&Zoom { rate: RATE, iq }, &band())
    }

    #[test]
    fn two_level_keying_reads_two_levels_and_its_shift() {
        let w = measure(fsk(&[-1.0, 1.0], 2_400.0, 2_000.0, 4_000, 0x1234));
        assert_eq!(w.frequency_levels, 2, "levels");
        assert!(w.level_valley < 0.2, "valley {}", w.level_valley);
        assert!(
            (w.deviation_hz - 2_000.0).abs() < 400.0,
            "deviation {} Hz",
            w.deviation_hz
        );
        let baud = w
            .symbol_rate_hz
            .expect("a keyed carrier has a symbol clock");
        assert!((baud - 2_400.0).abs() < 120.0, "baud {baud}");
        assert!(w.envelope_variation < 0.05, "constant envelope");
        assert!(w.duty > 0.99, "continuous");
    }

    #[test]
    fn four_level_keying_reads_four_levels_and_its_outer_deviation() {
        let w = measure(fsk(&[-3.0, -1.0, 1.0, 3.0], 4_800.0, 648.0, 6_000, 0x5678));
        assert_eq!(w.frequency_levels, 4, "levels");
        assert!(
            (w.deviation_hz - 1_944.0).abs() < 400.0,
            "deviation {} Hz",
            w.deviation_hz
        );
        let baud = w
            .symbol_rate_hz
            .expect("a keyed carrier has a symbol clock");
        assert!((baud - 4_800.0).abs() < 240.0, "baud {baud}");
    }

    #[test]
    fn an_unmodulated_carrier_has_one_level_and_no_spread() {
        let iq: Vec<Complex<f32>> = (0..24_000)
            .map(|k| Complex::from_polar(1.0, (TAU * 500.0 * f64::from(k) / RATE) as f32))
            .collect();
        let w = measure(iq);
        assert!(
            w.frequency_spread_hz < 50.0,
            "spread {}",
            w.frequency_spread_hz
        );
        assert!(w.square_line_db > 20.0, "square line {}", w.square_line_db);
    }

    /// A carrier swept by band-limited noise — an analog voice waveform, in shape if not in
    /// content. Whatever the histogram makes of it, the ground between its bumps stands nearly
    /// as high as they do, which is what says they are not levels.
    #[test]
    fn a_swept_carrier_has_no_ground_between_its_levels() {
        let mut state = 0x51f3u32;
        let mut smoothed = 0.0f64;
        let mut phase = 0.0f64;
        let iq: Vec<Complex<f32>> = (0..48_000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let noise = f64::from(state) / f64::from(u32::MAX) - 0.5;
                smoothed += 0.02 * (noise - smoothed);
                phase += TAU * smoothed * 12.0 * 3_000.0 / RATE;
                Complex::from_polar(1.0, phase as f32)
            })
            .collect();
        let w = measure(iq);
        assert!(w.level_valley > 0.5, "valley {}", w.level_valley);
    }

    /// What the envelope of a *constant*-envelope signal reads at a given level out of the
    /// noise. Every keyed mode measures this and nothing more, so it is the bar amplitude
    /// modulation has to clear rather than a correction to it.
    #[test]
    fn the_noise_alone_accounts_for_more_envelope_spread_as_the_signal_weakens() {
        let quiet = Band {
            snr_db: 40.0,
            ..band()
        };
        let weak = Band {
            snr_db: 12.0,
            ..band()
        };
        assert!(noise_variation(RATE, &quiet) < 0.05);
        assert!(noise_variation(RATE, &weak) > 0.2);
        // No signal at all is noise, whose own envelope spread is the ceiling on the estimate.
        let none = Band {
            snr_db: -40.0,
            ..band()
        };
        assert_eq!(noise_variation(RATE, &none), RAYLEIGH_VARIATION);
    }

    #[test]
    fn a_keyed_carrier_reports_its_duty_and_depth() {
        let mut iq = Vec::new();
        let mut state = 0x2c9fu32;
        for symbol in 0..4_800i32 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            // Pseudorandom keying, never a repeating pattern: a periodic one puts its own period
            // into the spectrum, and the test would pass on the wrong line.
            let on = !state.is_multiple_of(3);
            for k in 0..10i32 {
                let phase = (TAU * 300.0 * f64::from(symbol * 10 + k) / RATE) as f32;
                iq.push(Complex::from_polar(if on { 1.0 } else { 0.001 }, phase));
            }
        }
        let w = measure(iq);
        assert!(w.duty > 0.55 && w.duty < 0.8, "duty {}", w.duty);
        assert!(w.on_off_db > 30.0, "depth {} dB", w.on_off_db);
        assert!(w.envelope_variation < 0.05, "on-state is flat");
        let baud = w.symbol_rate_hz.expect("keying has a clock");
        assert!((baud - 4_800.0).abs() < 250.0, "baud {baud}");
    }
}
