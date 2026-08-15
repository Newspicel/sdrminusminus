use std::{f64::consts::TAU, fs, io, path::Path};

use num_complex::Complex;
use serde::{Deserialize, Serialize};

use super::{
    impair::{Awgn, ChannelSpec, Impairment},
    rng::Rng,
    sweep::point_seed,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TonePlan {
    pub freq: f64,
    pub cycles: usize,
    pub window: usize,
}

impl TonePlan {
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

#[must_use]
pub fn tone(freq: f64, amplitude: f32, samples: usize) -> Vec<f32> {
    (0..samples)
        .map(|n| amplitude * (TAU * freq * n as f64).cos() as f32)
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToneAnalysis {
    pub amplitude: f64,
    pub ac_power: f64,
    pub fundamental_power: f64,
    pub harmonic_power: f64,
}

pub const MAX_HARMONIC: usize = 10;

impl ToneAnalysis {
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

    #[must_use]
    pub fn thd(&self) -> f64 {
        if self.fundamental_power <= 0.0 {
            return f64::INFINITY;
        }
        (self.harmonic_power / self.fundamental_power).sqrt()
    }
}

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

pub type ModulateAudioFn = Box<dyn Fn(&[f32]) -> Vec<Complex<f32>>>;

pub type DemodulateAudioFn = Box<dyn Fn(&[Complex<f32>]) -> Vec<f32>>;

pub struct AnalogLink {
    pub label: String,
    pub bandwidth: f64,
    pub tone: TonePlan,
    pub drive: f32,
    pub settle: usize,
    pub modulate: ModulateAudioFn,
    pub demodulate: DemodulateAudioFn,
}

impl AnalogLink {
    #[must_use]
    pub fn samples(&self) -> usize {
        self.settle + self.tone.window
    }
}

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

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SinadPoint {
    pub snr_db: f64,
    pub sinad_db: f64,
    pub thd_percent: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SinadCurve {
    pub label: String,
    pub points: Vec<SinadPoint>,
}

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

pub fn save_json(curve: &SinadCurve, path: &Path) -> io::Result<()> {
    let mut text = serde_json::to_string_pretty(curve).map_err(io::Error::other)?;
    text.push('\n');
    fs::write(path, text)
}

pub fn load_json(path: &Path) -> io::Result<SinadCurve> {
    serde_json::from_str(&fs::read_to_string(path)?).map_err(io::Error::other)
}

pub fn save_csv(curve: &SinadCurve, path: &Path) -> io::Result<()> {
    let mut text = String::from("snr_db,sinad_db,thd_percent\n");
    for p in &curve.points {
        text.push_str(&format!("{},{},{}\n", p.snr_db, p.sinad_db, p.thd_percent));
    }
    fs::write(path, text)
}

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

    #[test]
    fn the_analyser_reads_known_content() {
        let plan = TonePlan::new(0.021, 4_096);
        let pure = tone(plan.freq, 0.5, plan.window);
        let analysis = analyse_tone(&pure, plan.freq);
        assert!((analysis.amplitude - 0.5).abs() < 1e-4);
        assert!((analysis.ac_power - 0.125).abs() < 1e-6);
        assert!(analysis.thd() < 1e-5, "thd {}", analysis.thd());
        assert!(analysis.sinad_db() > 90.0, "sinad {}", analysis.sinad_db());

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

        let offset: Vec<f32> = pure.iter().map(|x| x + 3.0).collect();
        let analysis = analyse_tone(&offset, plan.freq);
        assert!((analysis.ac_power - 0.125).abs() < 1e-6);
        assert!(analysis.sinad_db() > 90.0);
    }

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

        let mut down = exact.clone();
        for p in &mut down.points {
            p.sinad_db -= 0.5;
        }
        assert!((worst_shortfall_db(&down, oracle, 0.0, 20.0) - 0.5).abs() < 1e-9);
        assert!((worst_shortfall_db_vs_curve(&down, &exact, 0.0, 20.0) - 0.5).abs() < 1e-9);

        let moved = synthetic(1.0, &[0.0, 6.0, 12.0]);
        assert!(worst_shortfall_db_vs_curve(&moved, &exact, 0.0, 20.0).is_infinite());
        let subset = synthetic(1.0, &[0.0, 10.0, 20.0]);
        assert!(worst_shortfall_db_vs_curve(&subset, &exact, 0.0, 20.0).is_infinite());
        assert!(worst_shortfall_db_vs_curve(&subset, &exact, 10.0, 10.0).abs() < 1e-9);
        assert!(worst_shortfall_db(&exact, oracle, 40.0, 50.0).is_infinite());
    }

    #[test]
    fn sensitivity_and_threshold_are_read_off_the_curve() {
        let grid = [0.0, 2.0, 4.0, 6.0, 8.0, 10.0, 12.0];
        let mut curve = synthetic(1.0, &grid);
        let snr = snr_at_sinad(&curve, 12.0).unwrap();
        assert!((snr - 12.0).abs() < 1e-9, "sensitivity {snr}");
        assert!(snr_at_sinad(&curve, 30.0).is_none());

        let oracle = |snr| theory::analog_sinad_db(1.0, snr);
        assert!(threshold_db(&curve, oracle, 1.0).is_none());
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
