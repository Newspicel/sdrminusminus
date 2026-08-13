//! The sweep runner (MODEM-PLAN §3.1): drives any modulate/demodulate pair through the
//! calibrated [`impair`](super::impair) channel across a grid of Eb/N0 points and counts bit
//! errors into a committed [`Curve`]. Every correctness gate in the harness — the BPSK-vs-erfc
//! calibration, every later entry's oracle match, every committed-reference guard — is a
//! comparison between a curve this runner measured and a reference, so the comparators live
//! here beside it.
//!
//! Accounting (crate root, MODEM-PLAN §4.1): Eb is energy per *information* bit. The runner
//! owns the AWGN axis — whatever else the channel template carries, noise is set per point
//! from the waveform's own measured energy and the trial's information-bit count via
//! [`Awgn::for_ebn0`], applied canonically last, so the curve's x-axis is true at the detector
//! by construction rather than by per-link bookkeeping.
//!
//! Determinism: one point's error count is fully determined by `(seed, point index)` — payload
//! bits and channel noise draw from one [`Rng`] seeded from exactly that pair — so any single
//! point of a committed curve can be regenerated without resweeping the rest.

use std::{fs, io, path::Path};

use num_complex::Complex;

use super::{
    Curve, CurvePoint,
    impair::{Awgn, ChannelSpec, Impairment},
    rng::Rng,
};

/// Payload bits to complex-baseband waveform. Boxed `dyn Fn` because phase 0 has no engine
/// types to name — a link captures whatever taps and tables it designed at construction.
pub type ModulateFn = Box<dyn Fn(&[bool]) -> Vec<Complex<f32>>>;

/// Waveform back to payload-aligned bits.
pub type DemodulateFn = Box<dyn Fn(&[Complex<f32>]) -> Vec<bool>>;

/// One payload-to-payload chain under test, at the minimal shape phase 0 needs: the concrete
/// engines come later, and nothing above this closure pair may assume more about them than
/// "bits in, bits out". `demodulate` returns bits already aligned to the transmitted payload —
/// group delay, filter transients and timing are the link's own business, because only the
/// link knows its cascade.
pub struct Link {
    /// Names the chain in curve labels, e.g. `"ideal BPSK, RRC matched filter"`.
    pub label: String,
    /// Information bits per trial block — the payload length handed to `modulate` and the
    /// bit count Eb is accounted against. For an uncoded link the two are the same number;
    /// a coded link must still state *information* bits here or its curve's x-axis lies.
    pub bits_per_trial: usize,
    pub modulate: ModulateFn,
    pub demodulate: DemodulateFn,
}

/// Measures one BER curve: for each Eb/N0 in `points_db` (ascending, as [`Curve`] requires),
/// random payload blocks are modulated, run through `channel_template` with the AWGN axis
/// overridden to the point's Eb/N0, demodulated, and compared bit-for-bit until `min_errors`
/// errors are seen or `max_trial_bits` bits have been tried. Pass
/// [`MIN_ERRORS_PER_POINT`](super::MIN_ERRORS_PER_POINT) as `min_errors` unless a test has a
/// stated reason for another confidence level.
///
/// A demodulator that returns fewer bits than the payload has lost them; the missing positions
/// count as errors, never silently as fewer trials.
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

/// Golden-ratio stride keeps the map `index → seed` injective, and [`Rng::new`]'s SplitMix64
/// expansion makes any two distinct u64 seeds unrelated streams — so points are independent,
/// and reproducing point `i` alone needs only `(seed, i)`.
pub(crate) fn point_seed(seed: u64, index: usize) -> u64 {
    seed.wrapping_add((index as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

/// Payload bits drawn 64 at a time — one `next_u64` per word rather than per bit, because the
/// high-SNR points of a sweep chew through 1e7-bit payloads and the generator should stay
/// invisible in the profile.
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

// --- Curve I/O -------------------------------------------------------------------------------

/// Writes the curve as pretty JSON — the committed-artifact format, kept human-diffable so a
/// regression review can read exactly which point moved.
pub fn save_json(curve: &Curve, path: &Path) -> io::Result<()> {
    let mut text = serde_json::to_string_pretty(curve).map_err(io::Error::other)?;
    text.push('\n');
    fs::write(path, text)
}

pub fn load_json(path: &Path) -> io::Result<Curve> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(io::Error::other)
}

/// Writes `ebn0_db,rate,errors,trials` rows — the plotting/export format. The raw counts ride
/// along so confidence intervals stay recomputable from either format.
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

/// Reads a [`save_csv`] file back. The label is not carried by CSV, so the file stem stands in
/// for it — round-tripping identity lives with JSON; CSV exists for external tooling.
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

// --- Comparators -----------------------------------------------------------------------------
//
// Every gate compares curves *horizontally* — dB distance at equal BER — because that is the
// unit tolerances are stated in (§4.1 "within 0.2 dB") and because vertical distance at equal
// dB explodes wherever the curve is steep. Interpolation between measured points is linear in
// (dB, log10 BER): error-rate curves are near-exponential in dB, so the log domain is where a
// straight segment approximates them honestly.
//
// Failure is loud, not silent: a comparison that cannot be made — the target BER outside the
// measured span, no usable points in the range — returns +∞, which fails any `< tolerance`
// gate instead of vacuously passing it.

/// Horizontal distance in dB between a measured curve and a closed-form oracle at one BER:
/// positive means the measurement needs that many dB more than theory (a loss), negative would
/// mean beating theory — which past counting noise is a harness bug, not a triumph.
pub fn penalty_db(measured: &Curve, oracle: impl Fn(f64) -> f64, at_ber: f64) -> f64 {
    let Some(db_measured) = db_at_ber(measured, at_ber, false) else {
        return f64::INFINITY;
    };
    let Some(db_oracle) = invert_oracle(&oracle, at_ber) else {
        return f64::INFINITY;
    };
    db_measured - db_oracle
}

/// The worst horizontal penalty vs an oracle over `[db_lo, db_hi]`: each measured point in the
/// range (with at least one error — an errorless point states only a bound, not a BER) is
/// compared at its *own* measured rate against the oracle's dB for that rate. Point-wise
/// distance rather than double interpolation, so the number reads the raw counts. Returns the
/// signed penalty of largest magnitude; gates take `.abs()`.
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

/// [`penalty_db`] against a committed reference curve instead of a closed form — the guard for
/// entries whose reference is commit-and-review (§4.1).
pub fn penalty_db_vs_curve(measured: &Curve, reference: &Curve, at_ber: f64) -> f64 {
    let Some(db_measured) = db_at_ber(measured, at_ber, false) else {
        return f64::INFINITY;
    };
    let Some(db_reference) = db_at_ber(reference, at_ber, false) else {
        return f64::INFINITY;
    };
    db_measured - db_reference
}

/// [`worst_penalty_db`] against a committed reference curve. The reference is interpolated
/// with terminal-segment extrapolation: the two curves' endpoint rates differ by counting
/// noise, and refusing to compare the endpoints would exempt exactly the highest-SNR point —
/// the one regressions hit first — from the guard.
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

/// The dB at which `curve` crosses `ber`, by linear interpolation in (dB, log10 BER) over the
/// first bracketing pair of usable points. Points with no trials are unusable; points with
/// trials but no errors enter as the rate bound `0.5 / trials` — they carry real "at most
/// this" information, and dropping them would instead invent a hole in the curve. With
/// `extrapolate`, a target beyond either end is projected along the terminal segment
/// (committed-reference guarding needs the endpoints; see [`worst_penalty_db_vs_curve`]).
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
        // Rates descend with dB, so a target above the first point's rate projects off the
        // low-dB end and one below the last point's rate off the high-dB end.
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

/// Inverts a strictly decreasing oracle by bisection over a bracket wide enough for every
/// curve in the catalog (−30 dB is past any sensitivity, +50 dB past any error floor a f64
/// oracle resolves). `None` when the target BER lies outside the oracle's range there.
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

    /// One sample per bit, ±1 on the real rail: the simplest chain whose measured BER has a
    /// closed form, so sweep-runner defects are not hidden behind filter effects.
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

    /// Even the toy link must sit on the closed form — this is the sweep runner's own Eb/N0
    /// accounting under test, before any pulse shaping is involved. 5000 errors per point
    /// rather than the 100 floor: at the shallow low-SNR log-slope, a 100-error point's
    /// horizontal confidence interval is wider than the 0.2 dB being asserted.
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
        // At 0 dB (BER ~0.079) 100 errors arrive inside one or two blocks.
        assert!(curve.points[0].errors >= 100);
        assert!(curve.points[0].trials <= 4096);
        // At 20 dB errors are essentially unreachable; the cap must bound the point.
        assert!(curve.points[1].trials >= 50_000);
        assert!(curve.points[1].trials < 50_000 + link.bits_per_trial as u64);
    }

    /// A synthetic curve read straight off the oracle must compare at ~0 penalty, and the same
    /// curve shifted 0.5 dB right must read back as a 0.5 dB penalty — the comparator's own
    /// calibration.
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
        // Segment interpolation on a 1 dB grid carries the curve's log-domain curvature,
        // ~0.02 dB at the knee — the bound here. The recovered *shift* cancels most of it
        // (the two curves' grids sample the curvature at offset positions, so the residual
        // is second-order), hence the tighter bound on the difference.
        let pen_exact = penalty_db(&exact, theory::bpsk_ber, 1e-3);
        let pen_shifted = penalty_db(&shifted, theory::bpsk_ber, 1e-3);
        assert!(pen_exact.abs() < 0.03, "exact-curve penalty {pen_exact}");
        assert!((pen_shifted - pen_exact - 0.5).abs() < 0.02);
        // The worst-penalty comparator reads point rates directly — no interpolation of the
        // measured curve — so on synthetic exact points it is limited only by count rounding.
        assert!(worst_penalty_db(&exact, theory::bpsk_ber, 0.0, 10.0).abs() < 0.01);
        let worst = worst_penalty_db(&shifted, theory::bpsk_ber, 0.0, 10.0);
        assert!((worst - 0.5).abs() < 0.01, "worst {worst}");
        // Curve-vs-curve sees the same distance, endpoints included via extrapolation; the
        // reference is interpolated, so the curvature bound applies again.
        assert!((penalty_db_vs_curve(&shifted, &exact, 1e-3) - 0.5).abs() < 0.03);
        let worst = worst_penalty_db_vs_curve(&shifted, &exact, 0.0, 10.0);
        assert!((worst - 0.5).abs() < 0.05, "worst vs curve {worst}");
    }

    /// A comparison that cannot be made must fail a gate, not pass it.
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
