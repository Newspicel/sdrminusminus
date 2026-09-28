use num_complex::Complex;

use super::DoaError;
use crate::linalg::{
    CMat, Eigen, GeneralEigen, HermitianEigen, LinalgError, MAX_ORDER, MAX_POLY_DEGREE, Qr, Roots,
};
use crate::manifold::widen;
use crate::special::norm_deg;

pub const MAX_ESPRIT_SOURCES: usize = MAX_ORDER / 2;

const LINE_SLACK: f64 = 0.01;

type C64 = Complex<f64>;

pub fn root_music(
    projector: &CMat,
    roots: &mut Roots,
    coeffs: &mut [C64],
    found: &mut [C64],
    out: &mut [C64],
) -> Result<usize, DoaError> {
    let m = projector.order();
    if m < 2 {
        return Err(LinalgError::Order(m).into());
    }
    let degree = 2 * m - 2;
    if coeffs.len() <= degree || found.len() < degree || out.len() < m - 1 {
        return Err(LinalgError::Order(m).into());
    }
    let coeffs = &mut coeffs[..=degree];
    coeffs.fill(C64::new(0.0, 0.0));
    for p in 0..m {
        for q in 0..m {
            coeffs[q + m - 1 - p] += widen(projector.get(p, q));
        }
    }
    let count = roots.solve(coeffs, found)?;
    let found = &mut found[..count];
    for root in found.iter_mut() {
        let radius = root.norm();
        if radius > 1.0 {
            *root = (*root / radius) / radius;
        }
    }
    Ok(pick_twins(found, m - 1, out))
}

fn pick_twins(inside: &[C64], wanted: usize, out: &mut [C64]) -> usize {
    let mut used = [false; MAX_POLY_DEGREE];
    let mut picked = 0;
    while picked < wanted {
        let Some(best) = (0..inside.len())
            .filter(|&i| !used[i])
            .max_by(|&a, &b| inside[a].norm().total_cmp(&inside[b].norm()))
        else {
            break;
        };
        used[best] = true;
        out[picked] = inside[best];
        picked += 1;
        let twin = (0..inside.len()).filter(|&i| !used[i]).min_by(|&a, &b| {
            (inside[a] - inside[best])
                .norm()
                .total_cmp(&(inside[b] - inside[best]).norm())
        });
        if let Some(twin) = twin {
            used[twin] = true;
        }
    }
    picked
}

pub struct EspritSolvers {
    eigen: HermitianEigen,
    qr: Qr,
    general: GeneralEigen,
}

impl EspritSolvers {
    pub fn new() -> Result<Self, LinalgError> {
        Ok(Self {
            eigen: HermitianEigen::new(2)?,
            qr: Qr::new(1, 1)?,
            general: GeneralEigen::new(MAX_ESPRIT_SOURCES)?,
        })
    }
}

pub fn esprit(
    signal: &[Complex<f32>],
    rows: usize,
    sources: usize,
    row_weights: &[f32],
    solvers: &mut EspritSolvers,
    out: &mut [C64],
) -> Result<usize, DoaError> {
    let d = sources;
    let weighted = !row_weights.is_empty();
    if d == 0
        || d > MAX_ESPRIT_SOURCES
        || rows <= d
        || signal.len() < rows * d
        || out.len() < d
        || (weighted && row_weights.len() < rows - 1)
    {
        return Err(LinalgError::Order(d).into());
    }
    let stacked = shifted_gram(signal, rows, d, row_weights)?;
    let EspritSolvers { eigen, qr, general } = solvers;
    if eigen.order() != 2 * d {
        *eigen = HermitianEigen::new(2 * d)?;
    }
    let mut decomposition = Eigen::new();
    eigen.solve(&stacked, &mut decomposition)?;
    let mut lower = [Complex::new(0.0f32, 0.0); MAX_ESPRIT_SOURCES * MAX_ESPRIT_SOURCES];
    for c in 0..d {
        let vector = decomposition.vector(c);
        for r in 0..d {
            lower[r * d + c] = vector[d + r];
        }
    }
    *qr = Qr::new(d, d)?;
    qr.factor(&lower[..d * d])?;
    let mut psi = [C64::new(0.0, 0.0); MAX_ESPRIT_SOURCES * MAX_ESPRIT_SOURCES];
    let mut rhs = [Complex::new(0.0f32, 0.0); MAX_ESPRIT_SOURCES];
    for c in 0..d {
        let upper = &decomposition.vector(c)[..d];
        for (i, value) in rhs.iter_mut().enumerate().take(d) {
            *value = (0..d).map(|r| qr.q().get(r, i).conj() * upper[r]).sum();
        }
        qr.solve_upper(&mut rhs[..d])?;
        for (i, value) in rhs.iter().enumerate().take(d) {
            psi[i * d + c] = -widen(*value);
        }
    }
    general.eigenvalues(&psi[..d * d], &mut out[..d])?;
    Ok(d)
}

fn shifted_gram(
    signal: &[Complex<f32>],
    rows: usize,
    d: usize,
    row_weights: &[f32],
) -> Result<CMat, LinalgError> {
    let width = 2 * d;
    let entry = |row: usize, column: usize| {
        if column < d {
            signal[column * rows + row]
        } else {
            signal[(column - d) * rows + row + 1]
        }
    };
    let weight = |row: usize| row_weights.get(row).map_or(1.0, |w| w * w);
    let mut gram = CMat::zeros(width)?;
    for a in 0..width {
        for b in a..width {
            let value: Complex<f32> = (0..rows - 1)
                .map(|row| entry(row, a).conj() * entry(row, b) * weight(row))
                .sum();
            gram.set(a, b, value);
            gram.set(b, a, value.conj());
        }
    }
    Ok(gram)
}

#[must_use]
pub fn line_azimuths(phase_rad: f64, k_d: f64, axis_deg: f64) -> Option<(f64, f64)> {
    if !(k_d.is_finite() && k_d > 0.0 && phase_rad.is_finite() && axis_deg.is_finite()) {
        return None;
    }
    let ratio = phase_rad / k_d;
    if ratio.abs() > 1.0 + LINE_SLACK {
        return None;
    }
    let offset = ratio.clamp(-1.0, 1.0).acos().to_degrees();
    Some((norm_deg(axis_deg + offset), norm_deg(axis_deg - offset)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vandermonde(m: usize, phase: f64) -> Vec<Complex<f32>> {
        (0..m)
            .map(|i| {
                let turn = C64::from_polar(1.0, phase * i as f64);
                Complex::new(turn.re as f32, turn.im as f32)
            })
            .collect()
    }

    fn covariance(m: usize, phases: &[f64], noise: f32) -> CMat {
        let mut r = CMat::identity(m).unwrap();
        r.scale(noise);
        for &phase in phases {
            let a = vandermonde(m, phase);
            for i in 0..m {
                for j in 0..m {
                    r.add(i, j, a[i] * a[j].conj());
                }
            }
        }
        r
    }

    fn decompose(r: &CMat) -> Eigen {
        let mut solver = HermitianEigen::new(r.order()).unwrap();
        let mut eigen = Eigen::new();
        solver.solve(r, &mut eigen).unwrap();
        eigen
    }

    fn closest_phase(values: &[C64], phase: f64) -> f64 {
        values
            .iter()
            .map(|z| crate::special::wrap_deg((z.arg() - phase).to_degrees()).abs())
            .fold(f64::INFINITY, f64::min)
    }

    #[test]
    fn root_music_places_roots_on_the_source_phases() {
        let m = 6;
        let phases = [0.7, -1.9];
        let eigen = decompose(&covariance(m, &phases, 1e-3));
        let mut projector = CMat::zeros(m).unwrap();
        for k in 0..m - 2 {
            let e = eigen.vector(k);
            for i in 0..m {
                for j in 0..m {
                    projector.add(i, j, e[i] * e[j].conj());
                }
            }
        }
        let mut roots = Roots::new(MAX_POLY_DEGREE).unwrap();
        let mut coeffs = [C64::new(0.0, 0.0); MAX_POLY_DEGREE + 1];
        let mut found = [C64::new(0.0, 0.0); MAX_POLY_DEGREE];
        let mut out = [C64::new(0.0, 0.0); MAX_ORDER];
        let count = root_music(&projector, &mut roots, &mut coeffs, &mut found, &mut out).unwrap();
        assert_eq!(count, m - 1);
        for phase in phases {
            assert!(closest_phase(&out[..2], phase) < 0.05, "{:?}", &out[..2]);
        }
        assert!(out[..count].iter().all(|z| z.norm() <= 1.0 + 1e-6));
        let tiny = CMat::identity(1).unwrap();
        assert!(root_music(&tiny, &mut roots, &mut coeffs, &mut found, &mut out).is_err());
    }

    #[test]
    fn esprit_rotates_the_signal_subspace_onto_the_phases() {
        let m = 8;
        let phases = [1.1, -0.4];
        let eigen = decompose(&covariance(m, &phases, 1e-3));
        let mut signal = vec![Complex::new(0.0f32, 0.0); 2 * m];
        for (slot, k) in [m - 1, m - 2].into_iter().enumerate() {
            signal[slot * m..(slot + 1) * m].copy_from_slice(eigen.vector(k));
        }
        let mut solvers = EspritSolvers::new().unwrap();
        let mut out = [C64::new(0.0, 0.0); MAX_ORDER];
        let count = esprit(&signal, m, 2, &[], &mut solvers, &mut out).unwrap();
        assert_eq!(count, 2);
        for phase in phases {
            assert!(closest_phase(&out[..2], phase) < 0.05, "{:?}", &out[..2]);
        }
        assert!(out[..2].iter().all(|z| (z.norm() - 1.0).abs() < 1e-2));
        let weights = [0.5, 1.0, 1.0, 1.0, 1.0, 1.0, 0.5];
        esprit(&signal, m, 2, &weights, &mut solvers, &mut out).unwrap();
        for phase in phases {
            assert!(closest_phase(&out[..2], phase) < 0.05, "{:?}", &out[..2]);
        }
        assert!(esprit(&signal, m, 2, &weights[..3], &mut solvers, &mut out).is_err());
        assert!(esprit(&signal, 2, 2, &[], &mut solvers, &mut out).is_err());
        assert!(esprit(&signal, m, 0, &[], &mut solvers, &mut out).is_err());
    }

    #[test]
    fn line_azimuths_give_both_sides_of_the_axis() {
        let (a, b) =
            line_azimuths(std::f64::consts::FRAC_PI_2 * 0.5, std::f64::consts::PI, 0.0).unwrap();
        assert!((a - 75.522).abs() < 1e-3, "{a}");
        assert!((b - 284.478).abs() < 1e-3, "{b}");
        assert!(line_azimuths(3.3, std::f64::consts::PI, 0.0).is_none());
        let (edge, _) =
            line_azimuths(std::f64::consts::PI * 1.005, std::f64::consts::PI, 90.0).unwrap();
        assert!((edge - 90.0).abs() < 1e-9);
        assert!(line_azimuths(1.0, 0.0, 0.0).is_none());
    }
}
