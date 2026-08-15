use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

use num_complex::Complex;

use super::{Constellation, ConstellationError};

#[must_use]
pub fn gray(i: u32) -> u32 {
    i ^ (i >> 1)
}

fn check_order(family: &'static str, m: u32, ok: bool) -> Result<(), ConstellationError> {
    if ok && m >= 2 && m.is_power_of_two() && m <= 1024 {
        return Ok(());
    }
    Err(ConstellationError::UnsupportedOrder { family, m })
}

pub fn pam(m: u32) -> Result<Constellation, ConstellationError> {
    check_order("PAM", m, true)?;
    let n = m as usize;
    let points = (0..n)
        .map(|i| Complex::new((2 * i) as f32 - (m - 1) as f32, 0.0))
        .collect();
    Constellation::from_points(points, (0..m).map(gray).collect())
}

pub fn ask(m: u32) -> Result<Constellation, ConstellationError> {
    check_order("ASK", m, true)?;
    let points = (0..m).map(|i| Complex::new(i as f32, 0.0)).collect();
    Constellation::from_points(points, (0..m).map(gray).collect())
}

#[must_use]
pub fn bpsk() -> Constellation {
    match pam(2) {
        Ok(table) => table,
        Err(_) => unreachable!("pam(2) is a valid table by construction"),
    }
}

pub fn ook() -> Result<Constellation, ConstellationError> {
    ask(2)
}

pub fn psk(m: u32) -> Result<Constellation, ConstellationError> {
    psk_rotated(m, 0.0)
}

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

#[must_use]
pub fn offset_rotation(m: u32) -> f64 {
    PI / f64::from(m.max(2))
}

pub const PI_2_ROTATION: f64 = FRAC_PI_2;

pub const PI_4_ROTATION: f64 = FRAC_PI_4;

const MAX_PASSES: usize = 64;

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
    best.map_or_else(|| (0..points.len() as u32).collect(), |(_, labels)| labels)
}

#[must_use]
pub fn gray_penalty(c: &Constellation) -> f64 {
    let weights = pair_weights(c.points());
    let total: f64 = weights.iter().sum::<f64>() / 2.0;
    total_cost(c.labels(), &weights) / total
}

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

fn seed_orders(points: &[Complex<f32>]) -> Vec<Vec<usize>> {
    let n = points.len();
    let natural: Vec<usize> = (0..n).collect();
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

fn seed_labels(order: &[usize]) -> Vec<u32> {
    let mut labels = vec![0u32; order.len()];
    for (i, &pos) in order.iter().enumerate() {
        labels[pos] = gray(i as u32);
    }
    labels
}

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

fn point_cost(labels: &[u32], weights: &[f64], n: usize, i: usize) -> f64 {
    (0..n)
        .map(|j| f64::from((labels[i] ^ labels[j]).count_ones()) * weights[i * n + j])
        .sum()
}

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
