use num_complex::Complex;

use super::{AzimuthSpan, Geometry, LIGHT_SPEED_M_S, SteeringGrid, wavenumber};
use crate::special::norm_deg;

pub const ALIAS_THRESHOLD: f32 = 0.85;

const MAIN_LOBE: f32 = 0.5;
const MAX_POINTS: usize = 180;
const ENDFIRE_SPAN: f64 = 2.0;
const EDGE_STEP: f64 = 1e-3;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AliasReport {
    pub spacing_ratio: f32,
    pub aperture_wavelengths: f32,
    pub ambiguity: f32,
    pub worst_pair_deg: (f32, f32),
    pub aliased: bool,
}

#[must_use]
pub fn alias_check(ring: &SteeringGrid, geometry: &Geometry, freq_hz: f64) -> AliasReport {
    let wavelength = LIGHT_SPEED_M_S / freq_hz;
    let mut report = AliasReport {
        spacing_ratio: (geometry.nearest_spacing_m() / (wavelength / 2.0)) as f32,
        aperture_wavelengths: (geometry.aperture_m() / wavelength) as f32,
        ..AliasReport::default()
    };
    let line = geometry
        .line_axis_deg()
        .map(|axis_deg| (axis_deg, endfires_meet(geometry, axis_deg, freq_hz)));
    let view = RingView::new(ring, line);
    let mut lobe = [false; MAX_POINTS];
    for i in 0..view.count {
        view.main_lobes(i, &mut lobe[..view.count]);
        for j in (0..view.count).filter(|&j| !lobe[j]) {
            let match_ = view.coherence(i, j);
            if match_ > report.ambiguity {
                report.ambiguity = match_;
                report.worst_pair_deg = (view.azimuth(i) as f32, view.azimuth(j) as f32);
            }
        }
    }
    report.aliased = report.ambiguity >= ALIAS_THRESHOLD;
    report
}

fn endfires_meet(geometry: &Geometry, axis_deg: f64, freq_hz: f64) -> bool {
    let k = wavenumber(freq_hz);
    let (sin, cos) = axis_deg.to_radians().sin_cos();
    let coherence = |shift: f64| {
        geometry
            .positions()
            .iter()
            .map(|p| Complex::from_polar(1.0, k * (p.x * sin + p.y * cos) * shift))
            .sum::<Complex<f64>>()
            .norm()
    };
    coherence(ENDFIRE_SPAN) >= coherence(ENDFIRE_SPAN - EDGE_STEP)
}

struct RingView<'a> {
    ring: &'a SteeringGrid,
    stride: usize,
    count: usize,
    line: Option<Line>,
}

#[derive(Clone, Copy)]
struct Line {
    axis_deg: f64,
    endfires: Option<(usize, usize)>,
}

impl<'a> RingView<'a> {
    fn new(ring: &'a SteeringGrid, line: Option<(f64, bool)>) -> Self {
        let stride = ring.azimuths().div_ceil(MAX_POINTS).max(1);
        let mut view = Self {
            ring,
            stride,
            count: ring.azimuths().div_ceil(stride),
            line: None,
        };
        view.line = line.map(|(axis_deg, meet)| Line {
            axis_deg,
            endfires: meet.then(|| view.endfires(axis_deg)).flatten(),
        });
        view
    }

    fn endfires(&self, axis_deg: f64) -> Option<(usize, usize)> {
        Some((self.nearest(axis_deg)?, self.nearest(axis_deg + 180.0)?))
    }

    fn azimuth(&self, index: usize) -> f64 {
        self.ring.direction(index * self.stride).azimuth_deg
    }

    fn vector(&self, index: usize) -> &[Complex<f32>] {
        self.ring.vector(index * self.stride)
    }

    fn coherence(&self, i: usize, j: usize) -> f32 {
        let a = self.vector(i);
        let inner: Complex<f32> = a
            .iter()
            .zip(self.vector(j))
            .map(|(x, y)| x.conj() * y)
            .sum();
        inner.norm() / a.len() as f32
    }

    fn nearest(&self, azimuth_deg: f64) -> Option<usize> {
        let step = self.ring.azimuth_step_deg() * self.stride as f64;
        let start = self.azimuth(0);
        let offset = norm_deg(azimuth_deg - start);
        match self.ring.spec().span {
            AzimuthSpan::Full => Some((offset / step).round() as usize % self.count),
            AzimuthSpan::Half { .. } => {
                let index = (offset / step).round() as usize;
                (index < self.count).then_some(index)
            }
        }
    }

    fn step(&self, index: usize, forward: bool) -> Option<usize> {
        match (forward, self.ring.wraps()) {
            (true, true) => Some((index + 1) % self.count),
            (false, true) => Some((index + self.count - 1) % self.count),
            (true, false) => (index + 1 < self.count).then_some(index + 1),
            (false, false) => index.checked_sub(1),
        }
    }

    fn main_lobes(&self, i: usize, lobe: &mut [bool]) {
        lobe.fill(false);
        self.flood(i, i, lobe);
        let Some(line) = self.line else {
            return;
        };
        if let Some(mirror) = self.nearest(2.0 * line.axis_deg - self.azimuth(i)) {
            self.flood(i, mirror, lobe);
        }
        if let Some((front, back)) = line.endfires {
            if lobe[front] {
                self.flood(i, back, lobe);
            }
            if lobe[back] {
                self.flood(i, front, lobe);
            }
        }
    }

    fn flood(&self, i: usize, seed: usize, lobe: &mut [bool]) {
        if self.coherence(i, seed) < MAIN_LOBE {
            return;
        }
        lobe[seed] = true;
        for forward in [true, false] {
            let mut at = seed;
            while let Some(next) = self.step(at, forward) {
                if lobe[next] || self.coherence(i, next) < MAIN_LOBE {
                    break;
                }
                lobe[next] = true;
                at = next;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifold::{GridSpec, Manifold, Winding};

    fn check(geometry: &Geometry, freq: f64) -> AliasReport {
        let manifold = Manifold::ideal(geometry.clone());
        let ring = SteeringGrid::new(&manifold, GridSpec::ring(1.0), freq).unwrap();
        alias_check(&ring, geometry, freq)
    }

    fn wavelength(freq: f64) -> f64 {
        LIGHT_SPEED_M_S / freq
    }

    #[test]
    fn alias_check_flags_a_ula_spaced_one_wavelength() {
        let freq = 300e6;
        let geometry = Geometry::ula(wavelength(freq), 4, 90.0).unwrap();
        let report = check(&geometry, freq);
        assert!(report.aliased);
        assert!(report.ambiguity >= 0.9, "{report:?}");
        assert!((report.spacing_ratio - 2.0).abs() < 1e-6);
    }

    #[test]
    fn alias_check_passes_half_wavelength_ula_despite_its_mirror() {
        let freq = 300e6;
        for count in [2, 4, 8] {
            let geometry = Geometry::ula(wavelength(freq) / 2.0, count, 90.0).unwrap();
            let report = check(&geometry, freq);
            assert!(!report.aliased, "{count}: {report:?}");
            assert!((report.spacing_ratio - 1.0).abs() < 1e-6);
        }
        let wide = Geometry::ula(0.75 * wavelength(300e6), 2, 90.0).unwrap();
        assert!(check(&wide, 300e6).aliased);
    }

    #[test]
    fn alias_check_follows_the_half_wavelength_line_rule() {
        let freq = 300e6;
        for count in [2, 4, 8, 16] {
            for ratio in [0.3, 0.45, 0.49, 0.5] {
                let geometry = Geometry::ula(ratio * wavelength(freq), count, 30.0).unwrap();
                let report = check(&geometry, freq);
                assert!(!report.aliased, "{count} at {ratio}: {report:?}");
            }
            for ratio in [0.51, 0.55] {
                let geometry = Geometry::ula(ratio * wavelength(freq), count, 30.0).unwrap();
                let report = check(&geometry, freq);
                assert!(report.aliased, "{count} at {ratio}: {report:?}");
            }
        }
    }

    #[test]
    fn alias_check_kraken_uca_clean_at_design_and_aliased_at_two_and_three_times() {
        let design = 433.92e6;
        let chord = 0.5 * wavelength(design);
        let radius = chord / (2.0 * (std::f64::consts::PI / 5.0).sin());
        let geometry = Geometry::uca(radius, 5, 0.0, Winding::Clockwise).unwrap();
        let clean = check(&geometry, design);
        assert!(!clean.aliased, "{clean:?}");
        assert!((clean.ambiguity - 0.61).abs() < 0.02, "{clean:?}");
        let double = check(&geometry, 2.0 * design);
        let triple = check(&geometry, 3.0 * design);
        for report in [double, triple] {
            assert!(report.aliased, "{report:?}");
            assert!((report.ambiguity - 0.913).abs() < 0.01, "{report:?}");
        }
        assert!((clean.aperture_wavelengths - 0.809).abs() < 0.01);
    }

    #[test]
    fn a_half_plane_ring_is_checked_without_wrapping() {
        let freq = 300e6;
        let geometry = Geometry::ula(wavelength(freq) / 2.0, 4, 90.0).unwrap();
        let manifold = Manifold::ideal(geometry.clone());
        let spec = GridSpec {
            azimuth_step_deg: 1.0,
            span: AzimuthSpan::Half { centre_deg: 0.0 },
            elevation: None,
        };
        let ring = SteeringGrid::new(&manifold, spec, freq).unwrap();
        assert!(!alias_check(&ring, &geometry, freq).aliased);
        let wide = Geometry::ula(wavelength(freq), 4, 90.0).unwrap();
        let ring = SteeringGrid::new(&Manifold::ideal(wide.clone()), spec, freq).unwrap();
        assert!(alias_check(&ring, &wide, freq).aliased);
    }
}
