use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

use num_complex::Complex;

use super::{Constellation, ConstellationError};

/// Binary-reflected Gray code of `i`. Consecutive values differ in exactly one bit, which is
/// the whole property the PAM/PSK/QAM labellings are built from.
#[must_use]
pub fn gray(i: u32) -> u32 {
    i ^ (i >> 1)
}

/// Order validity shared by every generator: a power of two in `2..=1024`. The upper bound is
/// the largest square-QAM row in the catalog; nothing here needs to reject a bigger table on
/// principle, but a generator asked for 2^20 points is a caller bug worth naming.
fn check_order(family: &'static str, m: u32, ok: bool) -> Result<(), ConstellationError> {
    if ok && m >= 2 && m.is_power_of_two() && m <= 1024 {
        return Ok(());
    }
    Err(ConstellationError::UnsupportedOrder { family, m })
}

/// Gray-labelled bipolar M-PAM on the real axis: points at the odd integers
/// ±1, ±3, …, ±(M−1), label `gray(i)` on the i-th point counting up from the most negative.
/// Construction normalises to mean Es = 1, so the stored table is the classic ±1/√5, ±3/√5 at
/// M = 4.
///
/// `pam(2)` is the crate's BPSK table (see the module docs on polarity).
///
/// # Errors
/// [`ConstellationError::UnsupportedOrder`] unless M is a power of two in 2..=1024.
pub fn pam(m: u32) -> Result<Constellation, ConstellationError> {
    check_order("PAM", m, true)?;
    let n = m as usize;
    let points = (0..n)
        .map(|i| Complex::new((2 * i) as f32 - (m - 1) as f32, 0.0))
        .collect();
    Constellation::from_points(points, (0..m).map(gray).collect())
}

/// Gray-labelled unipolar M-ASK: amplitudes 0, 1, …, M−1 on the real axis, label `gray(i)` on
/// amplitude i. The *unipolar* family — a transmitter that keys its carrier on and off rather
/// than inverting it — so the mean energy carried into the normalisation is (2M−1)(M−1)/6,
/// not the PAM value, and an ASK entry's Eb/N0 is worse than the PAM of the same order by the
/// familiar amount. That difference is the point of having both.
///
/// # Errors
/// [`ConstellationError::UnsupportedOrder`] unless M is a power of two in 2..=1024.
pub fn ask(m: u32) -> Result<Constellation, ConstellationError> {
    check_order("ASK", m, true)?;
    let points = (0..m).map(|i| Complex::new(i as f32, 0.0)).collect();
    Constellation::from_points(points, (0..m).map(gray).collect())
}

/// The crate's BPSK table, infallibly. [`pam`] returns a `Result` because a caller can ask for an
/// order the family does not define; 2 is not such an order, and the several call sites that want
/// *this* table — the calibration link's polarity, RDS's slicer — should not each carry an
/// unreachable error branch.
#[must_use]
pub fn bpsk() -> Constellation {
    match pam(2) {
        Ok(table) => table,
        // `pam` rejects only non-powers of two and orders outside 2..=1024.
        Err(_) => unreachable!("pam(2) is a valid table by construction"),
    }
}

/// On-off keying: [`ask`] at M = 2 — the off state carries no energy at all, so normalisation
/// puts the on state at √2 and the table's mean Es is still 1. Named because the whole envelope
/// tier and two repo channels (morse, subghz) speak of it by this name.
///
/// # Errors
/// Never in practice; the signature matches the family for uniform call sites.
pub fn ook() -> Result<Constellation, ConstellationError> {
    ask(2)
}

/// Gray-labelled M-PSK on the unit circle: point i at angle 2πi/M, label `gray(i)`. Unit radius
/// is already mean Es = 1, so the normalisation is a no-op here up to f32 rounding.
///
/// # Errors
/// [`ConstellationError::UnsupportedOrder`] unless M is a power of two in 2..=1024.
pub fn psk(m: u32) -> Result<Constellation, ConstellationError> {
    psk_rotated(m, 0.0)
}

/// [`psk`] with every point rotated by `phase_rad` — the offset axis π/2-BPSK (π/2), QPSK in
/// its two-independent-rails orientation (π/4) and π/4-DQPSK's odd-symbol grid all live on.
/// A rotation is a table transform, not a new modulation, which is why it is a parameter here
/// rather than a family below.
///
/// # Errors
/// [`ConstellationError::UnsupportedOrder`] unless M is a power of two in 2..=1024.
pub fn psk_rotated(m: u32, phase_rad: f64) -> Result<Constellation, ConstellationError> {
    check_order("PSK", m, true)?;
    let points = (0..m)
        .map(|i| {
            let theta = phase_rad + TAU * f64::from(i) / f64::from(m);
            Complex::new(theta.cos() as f32, theta.sin() as f32)
        })
        .collect();
    Constellation::from_points(points, (0..m).map(gray).collect())
}

/// Gray-labelled square M-QAM for M ∈ {4, 16, 64, 256, 1024}: two independent √M-PAM rails,
/// the I rail in the low `k/2` label bits and the Q rail in the high ones. Every
/// nearest-neighbour pair differs in one bit because each rail's labelling does, which is the
/// premise of [`theory::mqam_ber`](crate::ber::theory::mqam_ber).
///
/// M = 4 is Gray QPSK — the (±1, ±1)/√2 orientation, i.e. [`psk_rotated`] at π/4 up to a
/// permutation of the two label bits.
///
/// # Errors
/// [`ConstellationError::UnsupportedOrder`] unless M is a power of two with an even number of
/// bits, in 4..=1024.
pub fn qam_square(m: u32) -> Result<Constellation, ConstellationError> {
    check_order("square QAM", m, m >= 4 && m.ilog2().is_multiple_of(2))?;
    let side = 1u32 << (m.ilog2() / 2);
    let half = m.ilog2() / 2;
    let coord = |i: u32| (2 * i) as f32 - (side - 1) as f32;
    let mut points = Vec::with_capacity(m as usize);
    let mut labels = Vec::with_capacity(m as usize);
    for qi in 0..side {
        for ii in 0..side {
            points.push(Complex::new(coord(ii), coord(qi)));
            labels.push(gray(ii) | (gray(qi) << half));
        }
    }
    Constellation::from_points(points, labels)
}

pub fn qam_cross(m: u32) -> Result<Constellation, ConstellationError> {
    check_order("cross QAM", m, m == 32 || m == 128)?;
    // 32 → 6×6 grid, corner blocks 1 wide; 128 → 12×12, corner blocks 2 wide.
    let (side, corner) = if m == 32 { (6i32, 1i32) } else { (12i32, 2i32) };
    let coord = |i: i32| (2 * i - (side - 1)) as f32;
    let mut points = Vec::with_capacity(m as usize);
    for qi in 0..side {
        for ii in 0..side {
            let in_corner =
                (ii < corner || ii >= side - corner) && (qi < corner || qi >= side - corner);
            if !in_corner {
                points.push(Complex::new(coord(ii), coord(qi)));
            }
        }
    }
    debug_assert_eq!(points.len(), m as usize);
    let labels = label_by_descent(&points);
    Constellation::from_points(points, labels)
}

/// Star QAM: `rings` concentric circles of `points_per_ring` points each, radii taken from
/// `radii` (relative — construction rescales the whole table to mean Es = 1). The classic
/// differentially-detectable geometry: amplitude and phase are separable, so the label is a
/// product — Gray over the ring index in the high bits, Gray over the phase index in the low
/// ones — and a differential detector can carry the phase bits without ever knowing the
/// absolute carrier phase.
///
/// # Errors
/// [`ConstellationError::UnsupportedOrder`] when the ring count or points-per-ring is not a
/// power of two, or their product is outside 2..=1024. Radii must be positive and strictly
/// increasing, or the table is not a star: that is reported as an unsupported order too, since
/// a mis-ordered radius list describes no constellation.
pub fn qam_star(radii: &[f64], points_per_ring: u32) -> Result<Constellation, ConstellationError> {
    let rings = u32::try_from(radii.len()).unwrap_or(u32::MAX);
    let m = rings.saturating_mul(points_per_ring);
    let ordered = radii.first().is_some_and(|&r| r > 0.0)
        && radii.windows(2).all(|w| w[1] > w[0])
        && radii.iter().all(|r| r.is_finite());
    check_order(
        "star QAM",
        m,
        rings.is_power_of_two() && points_per_ring.is_power_of_two() && ordered,
    )?;
    let phase_bits = points_per_ring.ilog2();
    let mut points = Vec::with_capacity(m as usize);
    let mut labels = Vec::with_capacity(m as usize);
    for (ring, &radius) in radii.iter().enumerate() {
        for k in 0..points_per_ring {
            let theta = TAU * f64::from(k) / f64::from(points_per_ring);
            points.push(Complex::new(
                (radius * theta.cos()) as f32,
                (radius * theta.sin()) as f32,
            ));
            labels.push(gray(k) | (gray(ring as u32) << phase_bits));
        }
    }
    Constellation::from_points(points, labels)
}

pub fn qam_hierarchical(m: u32, alpha: f64) -> Result<Constellation, ConstellationError> {
    check_order(
        "hierarchical QAM",
        m,
        (m == 16 || m == 64) && alpha.is_finite() && alpha >= 1.0,
    )?;
    let side = 1u32 << (m.ilog2() / 2);
    let half = m.ilog2() / 2;
    let quadrant = side / 2;
    // Index i counts up from the most negative rail position; |offset from centre| within a
    // quadrant is 0, 1, 2, … so the coordinate magnitude is α + 2·(that).
    let coord = |i: u32| -> f32 {
        let (sign, step) = if i < quadrant {
            (-1.0, f64::from(quadrant - 1 - i))
        } else {
            (1.0, f64::from(i - quadrant))
        };
        (sign * (alpha + 2.0 * step)) as f32
    };
    let mut points = Vec::with_capacity(m as usize);
    let mut labels = Vec::with_capacity(m as usize);
    for qi in 0..side {
        for ii in 0..side {
            points.push(Complex::new(coord(ii), coord(qi)));
            labels.push(gray(ii) | (gray(qi) << half));
        }
    }
    Constellation::from_points(points, labels)
}

/// One ring of an [`apsk`] table: how many points, at what relative radius, starting at what
/// angle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ApskRing {
    pub points: u32,
    pub radius: f64,
    pub phase_rad: f64,
}

pub fn apsk(rings: &[ApskRing]) -> Result<Constellation, ConstellationError> {
    let m = rings
        .iter()
        .map(|r| r.points)
        .fold(0u32, u32::saturating_add);
    let sane = !rings.is_empty()
        && rings
            .iter()
            .all(|r| r.points > 0 && r.radius.is_finite() && r.radius > 0.0);
    check_order("APSK", m, sane)?;
    let mut points = Vec::with_capacity(m as usize);
    for ring in rings {
        for k in 0..ring.points {
            let theta = ring.phase_rad + TAU * f64::from(k) / f64::from(ring.points);
            points.push(Complex::new(
                (ring.radius * theta.cos()) as f32,
                (ring.radius * theta.sin()) as f32,
            ));
        }
    }
    let labels = label_by_descent(&points);
    Constellation::from_points(points, labels)
}

/// DVB-S2 16-APSK: 4 inner points at π/4 + kπ/2 and 12 outer at kπ/6, ring ratio `gamma`
/// = R2/R1. The spec tabulates γ per code rate (EN 302 307-1 Table 9); 3.15 is the rate-3/4
/// value and the catalog's reference configuration.
///
/// # Errors
/// [`ConstellationError::UnsupportedOrder`] when `gamma` is not a finite ratio above 1.
pub fn apsk16_dvbs2(gamma: f64) -> Result<Constellation, ConstellationError> {
    if !(gamma.is_finite() && gamma > 1.0) {
        return Err(ConstellationError::UnsupportedOrder {
            family: "APSK",
            m: 16,
        });
    }
    apsk(&[
        ApskRing {
            points: 4,
            radius: 1.0,
            phase_rad: FRAC_PI_4,
        },
        ApskRing {
            points: 12,
            radius: gamma,
            phase_rad: 0.0,
        },
    ])
}

/// DVB-S2 32-APSK: 4 + 12 + 16 points at ring ratios `gamma1` = R2/R1 and `gamma2` = R3/R1.
/// The rate-3/4 pair from EN 302 307-1 Table 10 is (2.84, 5.27) — the catalog's reference
/// configuration. Ring phases follow the spec's staggering: π/4 + kπ/2 inner, π/12 + kπ/6
/// middle, kπ/8 outer.
///
/// # Errors
/// [`ConstellationError::UnsupportedOrder`] when the ratios are not finite and strictly
/// increasing above 1.
pub fn apsk32_dvbs2(gamma1: f64, gamma2: f64) -> Result<Constellation, ConstellationError> {
    if !(gamma1.is_finite() && gamma2.is_finite() && gamma1 > 1.0 && gamma2 > gamma1) {
        return Err(ConstellationError::UnsupportedOrder {
            family: "APSK",
            m: 32,
        });
    }
    apsk(&[
        ApskRing {
            points: 4,
            radius: 1.0,
            phase_rad: FRAC_PI_4,
        },
        ApskRing {
            points: 12,
            radius: gamma1,
            phase_rad: PI / 12.0,
        },
        ApskRing {
            points: 16,
            radius: gamma2,
            phase_rad: 0.0,
        },
    ])
}

/// π/2-BPSK and π/4-DQPSK carry a per-symbol rotation rather than a rotated table; this is the
/// rotation each uses, exported so an entry states it once. The value is π/M for an M-point
/// PSK: it puts every odd symbol exactly between two even-symbol points, which is what removes
/// the through-origin transitions that make a linear amplifier's job hard.
#[must_use]
pub fn offset_rotation(m: u32) -> f64 {
    PI / f64::from(m.max(2))
}

/// π/2 — [`offset_rotation`] at M = 2, named for the π/2-BPSK row.
pub const PI_2_ROTATION: f64 = FRAC_PI_2;

/// π/4 — [`offset_rotation`] at M = 4, named for the π/4-DQPSK row.
pub const PI_4_ROTATION: f64 = FRAC_PI_4;

/// Passes the descent may take. Every table in the catalog converges in well under ten; the cap
/// exists so a pathological geometry terminates rather than spins.
const MAX_PASSES: usize = 64;

/// A Gray-like labelling for a table no closed-form Gray code fits (cross-QAM, APSK).
///
/// **The objective.** At high SNR a symbol is mistaken for another with probability falling as
/// `exp(-d^2/N0)`, and each such mistake costs the Hamming distance between the two labels. The
/// cost minimised here is exactly that expectation with the noise scale pinned to the table's
/// own minimum distance:
///
/// ```text
/// cost = sum over i<j of  hamming(label_i, label_j) * exp(-d_ij^2 / d_min^2)
/// ```
///
/// A *nearest-neighbour-only* cost — count the closest shell, ignore everything else — is the
/// textbook statement of the same idea and is what the closed-form Gray families achieve
/// exactly, but it is degenerate on the geometries that need this function: DVB-S2 16-APSK's
/// closest shell is the four inner-ring pairs and nothing else, so 28 of the 32 confusions that
/// actually happen would carry no weight at all. The exponential keeps every pair, ordered by
/// how likely the confusion is.
///
/// **The search.** Each of the [`seed_orders`] starting labellings is refined by a 2-opt
/// descent — scan every label pair in a fixed order, swap when that strictly lowers the cost,
/// repeat until a pass changes nothing — and the cheapest result wins, ties going to the earlier
/// seed. Seeds matter more than the descent does: from natural binary, cross-32 settles at 1.66
/// weighted bits per confusion where a traversal seed reaches 1.30, because no single swap can
/// undo a globally wrong ordering. The seeds are traversals that visit the geometry in
/// near-adjacent order, Gray-coded so consecutive positions already differ in one bit.
///
/// **Determinism.** Fixed seeds, fixed scan order, strict improvement, no RNG and no wall clock,
/// so the table is a property of the geometry alone. The penalty reached for each catalog table
/// is pinned by test: this is a local optimum, not a proven global one, and pinning the number
/// is what turns "good enough" into a fact that cannot silently change.
#[must_use]
pub fn label_by_descent(points: &[Complex<f32>]) -> Vec<u32> {
    let weights = pair_weights(points);
    let mut best: Option<(f64, Vec<u32>)> = None;
    for order in seed_orders(points) {
        let labels = descend(seed_labels(&order), &weights, points.len());
        let cost = total_cost(&labels, &weights);
        if best.as_ref().is_none_or(|(c, _)| cost < *c) {
            best = Some((cost, labels));
        }
    }
    // `seed_orders` always yields at least the table order, so the option is inhabited; the
    // fallback keeps the function total rather than panicking on an empty table.
    best.map_or_else(|| (0..points.len() as u32).collect(), |(_, labels)| labels)
}

/// The committed quality metric: the descent's cost divided by the total confusion weight —
/// *expected bit errors per symbol error*, weighted by how likely each confusion is. A perfect
/// Gray labelling of a regular geometry sits a little above 1 (its second shell contributes two
/// bits at a small weight); a random labelling of a k-bit table tends to k/2.
#[must_use]
pub fn gray_penalty(c: &Constellation) -> f64 {
    let weights = pair_weights(c.points());
    let total: f64 = weights.iter().sum::<f64>() / 2.0;
    total_cost(c.labels(), &weights) / total
}

/// Confusion weights `exp(-d_ij^2 / d_min^2)` as a flat n x n matrix, zero on the diagonal.
/// Distances accumulate in f64, as everywhere the geometry is read.
fn pair_weights(points: &[Complex<f32>]) -> Vec<f64> {
    let n = points.len();
    let d2 = |a: usize, b: usize| {
        let dr = f64::from(points[a].re) - f64::from(points[b].re);
        let di = f64::from(points[a].im) - f64::from(points[b].im);
        dr * dr + di * di
    };
    let mut min = f64::INFINITY;
    for i in 0..n {
        for j in (i + 1)..n {
            min = min.min(d2(i, j));
        }
    }
    let mut w = vec![0.0f64; n * n];
    for i in 0..n {
        for j in (i + 1)..n {
            let v = (-d2(i, j) / min).exp();
            w[i * n + j] = v;
            w[j * n + i] = v;
        }
    }
    w
}

/// Deterministic traversals of the point set, each a permutation giving the visiting order.
/// Table order is first (for APSK that is ring by ring, phase by phase — already adjacent);
/// then a row-major snake and a column-major snake, which are the near-adjacent traversals of a
/// grid geometry like cross-QAM; then an angular sweep, which is the one for a ring geometry
/// whose table order is not already it.
fn seed_orders(points: &[Complex<f32>]) -> Vec<Vec<usize>> {
    let n = points.len();
    let natural: Vec<usize> = (0..n).collect();
    // Coordinates quantised to 1e-4 so points nominally on one row sort as one row despite
    // trigonometric rounding, and so the sort keys are integers — no float comparison decides
    // a traversal.
    let q = |x: f32| (f64::from(x) * 1e4).round() as i64;
    let snake = |horizontal: bool| -> Vec<usize> {
        let key = |i: usize| -> (i64, i64) {
            if horizontal {
                (q(points[i].im), q(points[i].re))
            } else {
                (q(points[i].re), q(points[i].im))
            }
        };
        let mut order = natural.clone();
        order.sort_by_key(|&i| key(i));
        // Reverse alternate rows so the traversal never jumps the full width between steps.
        let mut out: Vec<usize> = Vec::with_capacity(n);
        let (mut row_start, mut rows) = (0usize, 0usize);
        while row_start < n {
            let major = key(order[row_start]).0;
            let mut row_end = row_start;
            while row_end < n && key(order[row_end]).0 == major {
                row_end += 1;
            }
            if rows % 2 == 1 {
                out.extend(order[row_start..row_end].iter().rev());
            } else {
                out.extend(&order[row_start..row_end]);
            }
            row_start = row_end;
            rows += 1;
        }
        out
    };
    let mut angular = natural.clone();
    angular.sort_by_key(|&i| {
        let p = points[i];
        (
            (f64::from(p.im).atan2(f64::from(p.re)) * 1e4).round() as i64,
            (f64::from(p.norm()) * 1e4).round() as i64,
        )
    });
    let (rows, columns) = (snake(true), snake(false));
    vec![natural, rows, columns, angular]
}

/// Gray-coded labels along a traversal: position `order[i]` gets `gray(i)`, so consecutive
/// points on the traversal start exactly one bit apart.
fn seed_labels(order: &[usize]) -> Vec<u32> {
    let mut labels = vec![0u32; order.len()];
    for (i, &pos) in order.iter().enumerate() {
        labels[pos] = gray(i as u32);
    }
    labels
}

/// Improvement a swap must show to be taken. Costs here are sums of a few thousand terms below
/// 32, so 1e-9 is far above any rounding residue and far below any real improvement — it exists
/// so a pair of swaps can never each look like progress and cycle forever.
const MIN_IMPROVEMENT: f64 = 1e-9;

fn descend(mut labels: Vec<u32>, weights: &[f64], n: usize) -> Vec<u32> {
    for _ in 0..MAX_PASSES {
        let mut improved = false;
        for i in 0..n {
            for j in (i + 1)..n {
                let before =
                    point_cost(&labels, weights, n, i) + point_cost(&labels, weights, n, j);
                labels.swap(i, j);
                let after = point_cost(&labels, weights, n, i) + point_cost(&labels, weights, n, j);
                if after + MIN_IMPROVEMENT < before {
                    improved = true;
                } else {
                    labels.swap(i, j);
                }
            }
        }
        if !improved {
            break;
        }
    }
    labels
}

/// Weighted Hamming cost of the edges incident on `i`. A swap of `i` and `j` changes only the
/// edges touching either, and the `i`–`j` edge is counted in both terms before and after, so
/// comparing the sums is exact.
fn point_cost(labels: &[u32], weights: &[f64], n: usize, i: usize) -> f64 {
    (0..n)
        .map(|j| f64::from((labels[i] ^ labels[j]).count_ones()) * weights[i * n + j])
        .sum()
}

/// The whole objective, each pair counted once.
fn total_cost(labels: &[u32], weights: &[f64]) -> f64 {
    let n = labels.len();
    (0..n)
        .map(|i| point_cost(labels, weights, n, i))
        .sum::<f64>()
        / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ber::perf::assert_no_alloc;

    /// Mean symbol energy of a table, in f64 — the invariant construction promises.
    fn mean_energy(c: &Constellation) -> f64 {
        c.points()
            .iter()
            .map(|p| f64::from(p.re).powi(2) + f64::from(p.im).powi(2))
            .sum::<f64>()
            / c.len() as f64
    }

    fn min_distance(c: &Constellation) -> f64 {
        let p = c.points();
        let mut min = f64::INFINITY;
        for i in 0..p.len() {
            for j in (i + 1)..p.len() {
                let d = f64::from((p[i] - p[j]).norm());
                min = min.min(d);
            }
        }
        min
    }

    /// Mean Hamming distance over the strict nearest-neighbour shell — the number a perfect
    /// Gray labelling drives to exactly 1. Test-local, and deliberately *not* the descent's
    /// objective: this is the textbook statement, which the closed-form families satisfy
    /// exactly and the exotic geometries cannot satisfy at all.
    fn mean_neighbour_hamming(c: &Constellation) -> f64 {
        let p = c.points();
        let d2 = |a: usize, b: usize| f64::from((p[a] - p[b]).norm_sqr());
        let mut min = f64::INFINITY;
        for i in 0..p.len() {
            for j in (i + 1)..p.len() {
                min = min.min(d2(i, j));
            }
        }
        let limit = min * 1.002;
        let (mut sum, mut edges) = (0u32, 0u32);
        for i in 0..p.len() {
            for j in (i + 1)..p.len() {
                if d2(i, j) <= limit {
                    sum += (c.labels()[i] ^ c.labels()[j]).count_ones();
                    edges += 1;
                }
            }
        }
        f64::from(sum) / f64::from(edges)
    }

    #[test]
    fn gray_code_steps_one_bit_at_a_time() {
        for i in 0..1023u32 {
            assert_eq!((gray(i) ^ gray(i + 1)).count_ones(), 1, "at {i}");
        }
        assert_eq!(gray(0), 0);
        assert_eq!(
            (0..8).map(gray).collect::<Vec<_>>(),
            [0, 1, 3, 2, 6, 7, 5, 4]
        );
    }

    /// Every generated table is a valid constellation at unit mean energy — the invariant the
    /// whole Eb/N0 accounting rests on, checked once across the catalog rather than per family.
    #[test]
    fn every_catalog_table_is_normalised_and_valid() {
        let tables: Vec<(String, Constellation)> = catalog_tables();
        assert!(tables.len() >= 18, "only {} tables", tables.len());
        for (name, c) in tables {
            let e = mean_energy(&c);
            assert!((e - 1.0).abs() < 1e-5, "{name}: mean Es {e}");
            assert_eq!(c.len(), 1 << c.bits_per_symbol(), "{name}");
            let mut seen: Vec<u32> = c.labels().to_vec();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), c.len(), "{name}: labels are not a permutation");
        }
    }

    /// Every table the catalog's linear rows are measured on, built once for the invariants
    /// above and the descent pin below.
    fn catalog_tables() -> Vec<(String, Constellation)> {
        let mut v = vec![
            ("ook".to_string(), ook().unwrap()),
            ("ask4".to_string(), ask(4).unwrap()),
            ("pam2 (bpsk)".to_string(), pam(2).unwrap()),
            ("pam4".to_string(), pam(4).unwrap()),
            ("pam8".to_string(), pam(8).unwrap()),
            ("psk2".to_string(), psk(2).unwrap()),
            ("qpsk".to_string(), psk_rotated(4, FRAC_PI_4).unwrap()),
            ("psk8".to_string(), psk(8).unwrap()),
            ("psk16".to_string(), psk(16).unwrap()),
            ("cross32".to_string(), qam_cross(32).unwrap()),
            ("cross128".to_string(), qam_cross(128).unwrap()),
            ("star16".to_string(), qam_star(&[1.0, 2.0], 8).unwrap()),
            ("hier16 a=2".to_string(), qam_hierarchical(16, 2.0).unwrap()),
            ("hier64 a=2".to_string(), qam_hierarchical(64, 2.0).unwrap()),
            ("apsk16".to_string(), apsk16_dvbs2(3.15).unwrap()),
            ("apsk32".to_string(), apsk32_dvbs2(2.84, 5.27).unwrap()),
        ];
        for m in [4u32, 16, 64, 256, 1024] {
            v.push((format!("qam{m}"), qam_square(m).unwrap()));
        }
        v
    }

    /// The infallible BPSK constructor is the same table `pam(2)` builds, which is what makes it a
    /// convenience rather than a second definition.
    #[test]
    fn the_bpsk_shortcut_is_pam_2() {
        assert_eq!(bpsk(), pam(2).unwrap());
    }

    #[test]
    fn pam_is_the_odd_integer_grid_gray_labelled() {
        let c = pam(4).unwrap();
        let a = 5f64.sqrt().recip();
        for (i, want) in [-3.0, -1.0, 1.0, 3.0].into_iter().enumerate() {
            assert!((f64::from(c.points()[i].re) - want * a).abs() < 1e-7);
            assert_eq!(c.points()[i].im, 0.0);
        }
        assert_eq!(c.labels(), [0b00, 0b01, 0b11, 0b10]);
        let bpsk = pam(2).unwrap();
        assert_eq!(bpsk.labels(), [0, 1]);
        assert!(bpsk.points()[1].re > 0.0);
    }

    /// OOK's off state is genuinely off, and the on state carries the whole table's energy —
    /// the 3 dB an on-off keyer gives away against antipodal signalling, visible in the table.
    #[test]
    fn ook_keys_one_point_to_the_origin() {
        let c = ook().unwrap();
        assert_eq!(c.points()[0], Complex::new(0.0, 0.0));
        assert!((f64::from(c.points()[1].re) - 2f64.sqrt()).abs() < 1e-6);
        assert_eq!(c.labels(), [0, 1]);
    }

    #[test]
    fn psk_sits_on_the_unit_circle_with_the_stated_rotation() {
        let c = psk(8).unwrap();
        for p in c.points() {
            assert!((f64::from(p.norm()) - 1.0).abs() < 1e-6);
        }
        assert!((f64::from(c.points()[0].re) - 1.0).abs() < 1e-6);
        let q = psk_rotated(4, FRAC_PI_4).unwrap();
        let x = std::f32::consts::FRAC_1_SQRT_2;
        assert!((q.points()[0].re - x).abs() < 1e-6 && (q.points()[0].im - x).abs() < 1e-6);
        assert!(psk(2).unwrap().points()[0].re > 0.0);
        assert!(pam(2).unwrap().points()[0].re < 0.0);
    }

    /// Square QAM at M = 4 is Gray QPSK: the same four points as π/4-rotated 4-PSK, and a
    /// labelling that is Gray in both readings.
    #[test]
    fn square_qam_4_is_gray_qpsk() {
        let a = qam_square(4).unwrap();
        let b = psk_rotated(4, FRAC_PI_4).unwrap();
        for p in a.points() {
            assert!(
                b.points().iter().any(|q| (p - q).norm() < 1e-6),
                "point {p} missing from the PSK reading"
            );
        }
        assert!((mean_neighbour_hamming(&a) - 1.0).abs() < 1e-12);
    }

    /// The closed-form families are exactly Gray: every nearest-neighbour pair differs in one
    /// bit. This is the premise of the `SER/log2(M)` oracles, so it is asserted, not assumed.
    #[test]
    fn closed_form_families_are_exactly_gray() {
        let exact: Vec<(String, Constellation)> = vec![
            ("pam4".into(), pam(4).unwrap()),
            ("pam8".into(), pam(8).unwrap()),
            ("ask4".into(), ask(4).unwrap()),
            ("psk8".into(), psk(8).unwrap()),
            ("psk16".into(), psk(16).unwrap()),
            ("qam16".into(), qam_square(16).unwrap()),
            ("qam64".into(), qam_square(64).unwrap()),
            ("qam256".into(), qam_square(256).unwrap()),
            ("star16".into(), qam_star(&[1.0, 2.0], 8).unwrap()),
            ("hier16".into(), qam_hierarchical(16, 2.0).unwrap()),
            ("hier64".into(), qam_hierarchical(64, 2.0).unwrap()),
        ];
        for (name, c) in exact {
            let mean = mean_neighbour_hamming(&c);
            assert!(
                (mean - 1.0).abs() < 1e-12,
                "{name}: neighbour Hamming {mean}"
            );
        }
    }

    /// α = 1 is the spec's uniform case, so the hierarchical generator must reproduce square
    /// QAM point-for-point and label-for-label; α > 1 must open the quadrant gap and shrink the
    /// within-quadrant spacing (normalisation keeps the mean energy fixed, so one grows only at
    /// the other's expense).
    #[test]
    fn hierarchical_qam_reduces_to_uniform_at_alpha_one() {
        let uniform = qam_hierarchical(16, 1.0).unwrap();
        assert_eq!(uniform, qam_square(16).unwrap());
        let warped = qam_hierarchical(16, 3.0).unwrap();
        let mut rails: Vec<f64> = warped
            .points()
            .iter()
            .take(4)
            .map(|p| f64::from(p.re))
            .collect();
        rails.sort_by(f64::total_cmp);
        let gap_across_origin = rails[2] - rails[1];
        let gap_within = rails[1] - rails[0];
        assert!(
            (gap_across_origin / gap_within - 3.0).abs() < 1e-5,
            "α should be the gap ratio, got {}",
            gap_across_origin / gap_within
        );
        assert!(min_distance(&warped) < min_distance(&uniform));
    }

    /// Cross-QAM's defining geometry, read back off the normalised table: the points sit on the
    /// odd-integer grid (in units of half the minimum distance), and every corner block of the
    /// enclosing square is empty.
    #[test]
    fn cross_qam_removes_exactly_the_corner_blocks() {
        for (m, side, corner) in [(32u32, 6i32, 1i32), (128, 12, 2)] {
            let c = qam_cross(m).unwrap();
            assert_eq!(c.len(), m as usize);
            let unit = min_distance(&c) / 2.0;
            let edge = f64::from(side - 1);
            let inner_edge = edge - 2.0 * f64::from(corner);
            for p in c.points() {
                let (i, q) = (f64::from(p.re) / unit, f64::from(p.im) / unit);
                assert!((i - i.round()).abs() < 1e-3, "off grid: {p}");
                assert!(
                    i.round().abs() as i64 % 2 == 1,
                    "not an odd coordinate: {p}"
                );
                assert!(
                    i.abs() <= edge + 1e-3 && q.abs() <= edge + 1e-3,
                    "outside: {p}"
                );
                assert!(
                    !(i.abs() > inner_edge + 1e-3 && q.abs() > inner_edge + 1e-3),
                    "corner point survived: {p}"
                );
            }
        }
    }

    /// DVB-S2's ring populations and ratios, read back off the built table.
    #[test]
    fn dvbs2_apsk_carries_its_ring_structure() {
        let c = apsk16_dvbs2(3.15).unwrap();
        let mut radii: Vec<f64> = c.points().iter().map(|p| f64::from(p.norm())).collect();
        radii.sort_by(f64::total_cmp);
        assert_eq!(radii.iter().filter(|r| **r < radii[15] / 2.0).count(), 4);
        assert!((radii[15] / radii[0] - 3.15).abs() < 1e-4);
        let c32 = apsk32_dvbs2(2.84, 5.27).unwrap();
        let mut r32: Vec<f64> = c32.points().iter().map(|p| f64::from(p.norm())).collect();
        r32.sort_by(f64::total_cmp);
        assert_eq!(c32.len(), 32);
        assert!((r32[31] / r32[0] - 5.27).abs() < 1e-4);
        assert!((r32[10] / r32[0] - 2.84).abs() < 1e-4);
    }

    /// The descent's reached cost, pinned. These are local optima of a deterministic search:
    /// the numbers cannot drift without something in the geometry or the search changing, and
    /// a labelling that got *worse* would quietly cost every affected curve a fraction of a dB.
    #[test]
    fn descent_labellings_hold_their_committed_penalty() {
        for (name, want) in [
            ("cross32", 1.359_285_886),
            ("cross128", 1.598_622_405),
            ("apsk16", 1.165_917_943),
            ("apsk32", 1.341_774_498),
        ] {
            let c = catalog_tables()
                .into_iter()
                .find(|(n, _)| n == name)
                .map(|(_, c)| c)
                .unwrap();
            let penalty = gray_penalty(&c);
            assert!(
                (penalty - want).abs() < 1e-6,
                "{name}: Gray penalty {penalty:.9}, committed {want}"
            );
        }
    }

    /// The descent must beat what it started from and stay well under a random labelling — the
    /// two bounds that say the search did something, independently of the pinned numbers above.
    #[test]
    fn descent_beats_its_seeds_on_every_exotic_table() {
        for (name, c) in [
            ("cross32", qam_cross(32).unwrap()),
            ("cross128", qam_cross(128).unwrap()),
            ("apsk16", apsk16_dvbs2(3.15).unwrap()),
            ("apsk32", apsk32_dvbs2(2.84, 5.27).unwrap()),
        ] {
            let weights = pair_weights(c.points());
            let total: f64 = weights.iter().sum::<f64>() / 2.0;
            let seeds: Vec<f64> = seed_orders(c.points())
                .iter()
                .map(|o| total_cost(&seed_labels(o), &weights) / total)
                .collect();
            let penalty = gray_penalty(&c);
            let best_seed = seeds.iter().copied().fold(f64::INFINITY, f64::min);
            assert!(
                penalty <= best_seed,
                "{name}: descent {penalty} worse than its best seed {best_seed}"
            );
            let random = f64::from(c.bits_per_symbol() as u32) / 2.0;
            assert!(penalty < random, "{name}: {penalty} vs random {random}");
        }
    }

    /// Two builds of the same table are the same table — the property that makes a committed
    /// curve reproducible at all, and the one a randomised labeller would break.
    #[test]
    fn tables_are_reproducible() {
        assert_eq!(qam_cross(32).unwrap(), qam_cross(32).unwrap());
        assert_eq!(
            apsk32_dvbs2(2.84, 5.27).unwrap(),
            apsk32_dvbs2(2.84, 5.27).unwrap()
        );
    }

    #[test]
    fn unsupported_orders_are_rejected_by_name() {
        assert_eq!(
            qam_square(8).unwrap_err(),
            ConstellationError::UnsupportedOrder {
                family: "square QAM",
                m: 8
            }
        );
        assert!(qam_cross(64).is_err());
        assert!(qam_hierarchical(32, 2.0).is_err());
        assert!(qam_hierarchical(16, 0.5).is_err());
        assert!(pam(6).is_err());
        assert!(psk(2048).is_err());
        assert!(qam_star(&[2.0, 1.0], 8).is_err());
        assert!(qam_star(&[1.0, 2.0], 6).is_err());
        assert!(apsk16_dvbs2(0.5).is_err());
        assert!(apsk32_dvbs2(5.0, 2.0).is_err());
        assert!(apsk(&[]).is_err());
    }

    #[test]
    fn offset_rotations_are_the_half_step() {
        assert!((offset_rotation(2) - PI_2_ROTATION).abs() < 1e-12);
        assert!((offset_rotation(4) - PI_4_ROTATION).abs() < 1e-12);
        assert!((offset_rotation(8) - PI / 8.0).abs() < 1e-12);
    }

    #[test]
    fn slicing_a_large_table_allocates_nothing() {
        let c = qam_square(1024).unwrap();
        let y = Complex::new(0.31f32, -0.12);
        assert_no_alloc("hard_slice qam1024", || {
            std::hint::black_box(c.hard_slice(y));
        });
    }
}
