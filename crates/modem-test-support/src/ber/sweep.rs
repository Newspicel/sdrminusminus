use std::{fs, io, path::Path};

use num_complex::Complex;

use super::{
    Curve, CurvePoint,
    impair::{Awgn, ChannelSpec, Impairment},
    rng::Rng,
};

pub type ModulateFn = Box<dyn Fn(&[bool]) -> Vec<Complex<f32>>>;

pub type DemodulateFn = Box<dyn Fn(&[Complex<f32>]) -> Vec<bool>>;

pub struct Link {
    pub label: String,
    pub bits_per_trial: usize,
    pub modulate: ModulateFn,
    pub demodulate: DemodulateFn,
}

pub fn sweep_ber(
    link: &Link,
    channel_template: &ChannelSpec,
    points_db: &[f64],
    seed: u64,
    min_errors: u64,
    max_trial_bits: u64,
) -> Curve {
    let mut points = Vec::with_capacity(points_db.len());
    for (index, &ebn0_db) in points_db.iter().enumerate() {
        let mut rng = Rng::new(point_seed(seed, index));
        let channel = channel_template
            .awgn(Awgn::for_ebn0(ebn0_db, link.bits_per_trial as u64))
            .build();
        let mut errors = 0u64;
        let mut trials = 0u64;
        while errors < min_errors && trials < max_trial_bits {
            let payload = random_bits(&mut rng, link.bits_per_trial);
            let mut wave = (link.modulate)(&payload);
            channel.apply(&mut wave, &mut rng);
            let decoded = (link.demodulate)(&wave);
            for (i, &sent) in payload.iter().enumerate() {
                trials += 1;
                if decoded.get(i) != Some(&sent) {
                    errors += 1;
                }
            }
        }
        points.push(CurvePoint {
            ebn0_db,
            errors,
            trials,
        });
    }
    Curve {
        label: format!("{}, uncoded BER, seed {seed:#x}", link.label),
        points,
    }
}

pub(crate) fn point_seed(seed: u64, index: usize) -> u64 {
    seed.wrapping_add((index as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

fn random_bits(rng: &mut Rng, n: usize) -> Vec<bool> {
    let mut bits = Vec::with_capacity(n);
    while bits.len() < n {
        let mut word = rng.next_u64();
        let take = 64.min(n - bits.len());
        for _ in 0..take {
            bits.push(word & 1 == 1);
            word >>= 1;
        }
    }
    bits
}

pub fn save_json(curve: &Curve, path: &Path) -> io::Result<()> {
    let mut text = serde_json::to_string_pretty(curve).map_err(io::Error::other)?;
    text.push('\n');
    fs::write(path, text)
}

pub fn load_json(path: &Path) -> io::Result<Curve> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(io::Error::other)
}

pub fn save_csv(curve: &Curve, path: &Path) -> io::Result<()> {
    let mut text = String::from("ebn0_db,rate,errors,trials\n");
    for p in &curve.points {
        text.push_str(&format!(
            "{},{},{},{}\n",
            p.ebn0_db,
            p.rate(),
            p.errors,
            p.trials
        ));
    }
    fs::write(path, text)
}

pub fn load_csv(path: &Path) -> io::Result<Curve> {
    let text = fs::read_to_string(path)?;
    let mut lines = text.lines();
    let header = lines.next().unwrap_or("");
    if header != "ebn0_db,rate,errors,trials" {
        return Err(io::Error::other(format!(
            "unexpected CSV header {header:?}"
        )));
    }
    let mut points = Vec::new();
    for line in lines.filter(|l| !l.is_empty()) {
        let mut fields = line.split(',');
        let (Some(db), _rate, Some(errors), Some(trials)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(io::Error::other(format!("short CSV row {line:?}")));
        };
        points.push(CurvePoint {
            ebn0_db: db.parse().map_err(io::Error::other)?,
            errors: errors.parse().map_err(io::Error::other)?,
            trials: trials.parse().map_err(io::Error::other)?,
        });
    }
    let label = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(Curve { label, points })
}

pub fn penalty_db(measured: &Curve, oracle: impl Fn(f64) -> f64, at_ber: f64) -> f64 {
    let Some(db_measured) = db_at_ber(measured, at_ber, false) else {
        return f64::INFINITY;
    };
    let Some(db_oracle) = invert_oracle(&oracle, at_ber) else {
        return f64::INFINITY;
    };
    db_measured - db_oracle
}

pub fn worst_penalty_db(
    measured: &Curve,
    oracle: impl Fn(f64) -> f64,
    db_lo: f64,
    db_hi: f64,
) -> f64 {
    let mut worst = f64::INFINITY;
    let mut any = false;
    for p in &measured.points {
        if p.ebn0_db < db_lo || p.ebn0_db > db_hi || p.errors == 0 {
            continue;
        }
        let Some(db_oracle) = invert_oracle(&oracle, p.rate()) else {
            return f64::INFINITY;
        };
        let pen = p.ebn0_db - db_oracle;
        if !any || pen.abs() > worst.abs() {
            worst = pen;
            any = true;
        }
    }
    worst
}

pub fn penalty_db_vs_curve(measured: &Curve, reference: &Curve, at_ber: f64) -> f64 {
    let Some(db_measured) = db_at_ber(measured, at_ber, false) else {
        return f64::INFINITY;
    };
    let Some(db_reference) = db_at_ber(reference, at_ber, false) else {
        return f64::INFINITY;
    };
    db_measured - db_reference
}

pub fn worst_penalty_db_vs_curve(
    measured: &Curve,
    reference: &Curve,
    db_lo: f64,
    db_hi: f64,
) -> f64 {
    let mut worst = f64::INFINITY;
    let mut any = false;
    for p in &measured.points {
        if p.ebn0_db < db_lo || p.ebn0_db > db_hi || p.errors == 0 {
            continue;
        }
        let Some(db_reference) = db_at_ber(reference, p.rate(), true) else {
            return f64::INFINITY;
        };
        let pen = p.ebn0_db - db_reference;
        if !any || pen.abs() > worst.abs() {
            worst = pen;
            any = true;
        }
    }
    worst
}

fn db_at_ber(curve: &Curve, ber: f64, extrapolate: bool) -> Option<f64> {
    if ber <= 0.0 || !ber.is_finite() {
        return None;
    }
    let pts: Vec<(f64, f64)> = curve
        .points
        .iter()
        .filter(|p| p.trials > 0)
        .map(|p| {
            let rate = if p.errors == 0 {
                0.5 / p.trials as f64
            } else {
                p.rate()
            };
            (p.ebn0_db, rate.log10())
        })
        .collect();
    let target = ber.log10();
    for pair in pts.windows(2) {
        let (db_a, la) = pair[0];
        let (db_b, lb) = pair[1];
        if (la - target) * (lb - target) <= 0.0 {
            if (la - lb).abs() < 1e-12 {
                return Some(db_a);
            }
            return Some(db_a + (la - target) / (la - lb) * (db_b - db_a));
        }
    }
    if extrapolate && pts.len() >= 2 {
        let ((db_a, la), (db_b, lb)) = if target > pts[0].1 {
            (pts[0], pts[1])
        } else {
            (pts[pts.len() - 2], pts[pts.len() - 1])
        };
        if (la - lb).abs() < 1e-12 {
            return None;
        }
        return Some(db_a + (la - target) / (la - lb) * (db_b - db_a));
    }
    None
}

fn invert_oracle(oracle: &impl Fn(f64) -> f64, ber: f64) -> Option<f64> {
    let (mut lo, mut hi) = (-30.0f64, 50.0f64);
    if !(oracle(lo) >= ber && oracle(hi) <= ber) {
        return None;
    }
    while hi - lo > 1e-9 {
        let mid = 0.5 * (lo + hi);
        if oracle(mid) >= ber {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some(0.5 * (lo + hi))
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;

    use super::*;
    use crate::ber::theory;

    fn toy_bpsk() -> Link {
        Link {
            label: "toy BPSK, 1 sample/bit".to_string(),
            bits_per_trial: 2048,
            modulate: Box::new(|bits| {
                bits.iter()
                    .map(|&b| Complex::new(if b { 1.0 } else { -1.0 }, 0.0))
                    .collect()
            }),
            demodulate: Box::new(|wave| wave.iter().map(|s| s.re > 0.0).collect()),
        }
    }

    #[test]
    fn same_seed_gives_a_byte_identical_curve() {
        let link = toy_bpsk();
        let points = [0.0, 2.0, 4.0];
        let spec = ChannelSpec::default();
        let a = sweep_ber(&link, &spec, &points, 0x5eed, 50, 1_000_000);
        let b = sweep_ber(&link, &spec, &points, 0x5eed, 50, 1_000_000);
        assert_eq!(a, b);
        let c = sweep_ber(&link, &spec, &points, 0x5eee, 50, 1_000_000);
        assert_ne!(a, c, "a different seed must give a different realisation");
    }

    #[test]
    fn toy_bpsk_matches_theory() {
        let link = toy_bpsk();
        let points = [0.0, 2.0, 4.0, 6.0];
        let curve = sweep_ber(&link, &ChannelSpec::default(), &points, 1, 5000, 10_000_000);
        let worst = worst_penalty_db(&curve, theory::bpsk_ber, 0.0, 6.0);
        assert!(worst.abs() < 0.2, "worst penalty {worst} dB");
    }

    #[test]
    fn point_stops_at_min_errors_or_trial_cap() {
        let link = toy_bpsk();
        let curve = sweep_ber(&link, &ChannelSpec::default(), &[0.0, 20.0], 7, 100, 50_000);
        assert!(curve.points[0].errors >= 100);
        assert!(curve.points[0].trials <= 4096);
        assert!(curve.points[1].trials >= 50_000);
        assert!(curve.points[1].trials < 50_000 + link.bits_per_trial as u64);
    }

    #[test]
    fn penalty_reads_a_known_shift() {
        let synth = |shift_db: f64| Curve {
            label: format!("synthetic bpsk shifted {shift_db} dB"),
            points: (0..=10)
                .map(|db| {
                    let rate = theory::bpsk_ber(f64::from(db) - shift_db);
                    CurvePoint {
                        ebn0_db: f64::from(db),
                        errors: (rate * 1e12) as u64,
                        trials: 1_000_000_000_000,
                    }
                })
                .collect(),
        };
        let exact = synth(0.0);
        let shifted = synth(0.5);
        let pen_exact = penalty_db(&exact, theory::bpsk_ber, 1e-3);
        let pen_shifted = penalty_db(&shifted, theory::bpsk_ber, 1e-3);
        assert!(pen_exact.abs() < 0.03, "exact-curve penalty {pen_exact}");
        assert!((pen_shifted - pen_exact - 0.5).abs() < 0.02);
        assert!(worst_penalty_db(&exact, theory::bpsk_ber, 0.0, 10.0).abs() < 0.01);
        let worst = worst_penalty_db(&shifted, theory::bpsk_ber, 0.0, 10.0);
        assert!((worst - 0.5).abs() < 0.01, "worst {worst}");
        assert!((penalty_db_vs_curve(&shifted, &exact, 1e-3) - 0.5).abs() < 0.03);
        let worst = worst_penalty_db_vs_curve(&shifted, &exact, 0.0, 10.0);
        assert!((worst - 0.5).abs() < 0.05, "worst vs curve {worst}");
    }

    #[test]
    fn out_of_span_comparisons_are_infinite() {
        let curve = Curve {
            label: "one point".to_string(),
            points: vec![CurvePoint {
                ebn0_db: 4.0,
                errors: 1000,
                trials: 100_000,
            }],
        };
        assert!(penalty_db(&curve, theory::bpsk_ber, 1e-9).is_infinite());
        assert!(worst_penalty_db(&curve, theory::bpsk_ber, 10.0, 20.0).is_infinite());
        let empty = Curve {
            label: "empty".to_string(),
            points: vec![],
        };
        assert!(penalty_db_vs_curve(&curve, &empty, 1e-2).is_infinite());
    }

    #[test]
    fn json_round_trips_and_csv_preserves_counts() {
        let dir = std::env::temp_dir().join(format!("sdrmm-modem-sweep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let curve = Curve {
            label: "io round trip".to_string(),
            points: vec![
                CurvePoint {
                    ebn0_db: 0.0,
                    errors: 123,
                    trials: 4567,
                },
                CurvePoint {
                    ebn0_db: 2.5,
                    errors: 100,
                    trials: 987_654_321,
                },
            ],
        };
        let json = dir.join("curve.json");
        save_json(&curve, &json).unwrap();
        assert_eq!(load_json(&json).unwrap(), curve);
        let csv = dir.join("curve.csv");
        save_csv(&curve, &csv).unwrap();
        let text = std::fs::read_to_string(&csv).unwrap();
        assert!(text.starts_with("ebn0_db,rate,errors,trials\n"));
        let back = load_csv(&csv).unwrap();
        assert_eq!(back.points, curve.points);
        assert_eq!(back.label, "curve");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
