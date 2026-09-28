use super::{MAX_ELEMENTS, MAX_EXTENT_M, ManifoldError, Vec3};
use crate::special::norm_deg;

const MIN_SEPARATION_M: f64 = 1e-4;
const SHAPE_TOLERANCE: f64 = 1e-3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Winding {
    #[default]
    Clockwise,
    Counterclockwise,
}

impl Winding {
    const fn sign(self) -> f64 {
        match self {
            Self::Clockwise => 1.0,
            Self::Counterclockwise => -1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    Uca {
        radius_m: f64,
        first_deg: f64,
        winding: Winding,
    },
    Ula {
        spacing_m: f64,
        axis_deg: f64,
    },
    Explicit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Permutation {
    len: usize,
    map: [u8; MAX_ELEMENTS],
}

impl Permutation {
    #[must_use]
    pub fn identity(len: usize) -> Self {
        Self::from_fn(len, |i| i)
    }

    #[must_use]
    pub fn reversal(len: usize) -> Self {
        let len = len.min(MAX_ELEMENTS);
        Self::from_fn(len, |i| len - 1 - i)
    }

    #[must_use]
    pub fn get(&self, index: usize) -> usize {
        usize::from(self.map[index])
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.map[..self.len].iter().map(|&index| usize::from(index))
    }

    fn from_fn(len: usize, f: impl Fn(usize) -> usize) -> Self {
        let len = len.min(MAX_ELEMENTS);
        let mut map = [0u8; MAX_ELEMENTS];
        for (i, slot) in map.iter_mut().enumerate().take(len) {
            *slot = f(i) as u8;
        }
        Self { len, map }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    shape: Shape,
    count: usize,
    positions: [Vec3; MAX_ELEMENTS],
}

impl Geometry {
    pub fn uca(
        radius_m: f64,
        count: usize,
        first_deg: f64,
        winding: Winding,
    ) -> Result<Self, ManifoldError> {
        check_count(count)?;
        if !(radius_m.is_finite() && radius_m > 0.0) {
            return Err(ManifoldError::Spacing);
        }
        let mut positions = [Vec3::default(); MAX_ELEMENTS];
        for (i, position) in positions.iter_mut().enumerate().take(count) {
            let gamma = first_deg + winding.sign() * 360.0 * i as f64 / count as f64;
            let (sin, cos) = gamma.to_radians().sin_cos();
            *position = Vec3::new(radius_m * sin, radius_m * cos, 0.0);
        }
        let shape = Shape::Uca {
            radius_m,
            first_deg,
            winding,
        };
        Self::checked(shape, count, positions)
    }

    pub fn ula(spacing_m: f64, count: usize, axis_deg: f64) -> Result<Self, ManifoldError> {
        check_count(count)?;
        if !(spacing_m.is_finite() && spacing_m > 0.0) {
            return Err(ManifoldError::Spacing);
        }
        let (sin, cos) = axis_deg.to_radians().sin_cos();
        let middle = (count as f64 - 1.0) / 2.0;
        let mut positions = [Vec3::default(); MAX_ELEMENTS];
        for (i, position) in positions.iter_mut().enumerate().take(count) {
            let along = (i as f64 - middle) * spacing_m;
            *position = Vec3::new(along * sin, along * cos, 0.0);
        }
        let shape = Shape::Ula {
            spacing_m,
            axis_deg,
        };
        Self::checked(shape, count, positions)
    }

    pub fn explicit(positions: &[Vec3]) -> Result<Self, ManifoldError> {
        check_count(positions.len())?;
        let mut stored = [Vec3::default(); MAX_ELEMENTS];
        stored[..positions.len()].copy_from_slice(positions);
        Self::checked(Shape::Explicit, positions.len(), stored)
    }

    #[must_use]
    pub const fn shape(&self) -> Shape {
        self.shape
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.count
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    #[must_use]
    pub fn positions(&self) -> &[Vec3] {
        &self.positions[..self.count]
    }

    #[must_use]
    pub fn centroid(&self) -> Vec3 {
        let n = self.count as f64;
        let sum = self.positions().iter().fold(Vec3::default(), |sum, p| {
            Vec3::new(sum.x + p.x, sum.y + p.y, sum.z + p.z)
        });
        Vec3::new(sum.x / n, sum.y / n, sum.z / n)
    }

    #[must_use]
    pub fn aperture_m(&self) -> f64 {
        self.pair_distances().fold(0.0, f64::max)
    }

    #[must_use]
    pub fn nearest_spacing_m(&self) -> f64 {
        self.pair_distances().fold(f64::INFINITY, f64::min)
    }

    #[must_use]
    pub fn antipodes(&self) -> Option<Permutation> {
        let tol = self.tolerance();
        let centred = self.centred();
        let mut map = [0u8; MAX_ELEMENTS];
        let mut taken = [false; MAX_ELEMENTS];
        for (i, p) in centred[..self.count].iter().enumerate() {
            let (partner, distance) = centred[..self.count]
                .iter()
                .enumerate()
                .map(|(j, q)| (j, Vec3::new(p.x + q.x, p.y + q.y, p.z + q.z).norm()))
                .min_by(|a, b| a.1.total_cmp(&b.1))?;
            if distance > tol || taken[partner] {
                return None;
            }
            taken[partner] = true;
            map[i] = partner as u8;
        }
        Some(Permutation {
            len: self.count,
            map,
        })
    }

    #[must_use]
    pub fn line_axis_deg(&self) -> Option<f64> {
        if !self.is_planar_horizontal() {
            return None;
        }
        let direction = self.line_direction()?;
        Some(norm_deg(direction.x.atan2(direction.y).to_degrees()))
    }

    #[must_use]
    pub fn is_collinear(&self) -> bool {
        self.line_direction().is_some()
    }

    #[must_use]
    pub fn axis_order(&self) -> Option<Permutation> {
        let axis = self.line_axis_deg()?;
        let along = self.projections(axis);
        let mut order = Permutation::identity(self.count);
        order.map[..self.count]
            .sort_by(|&a, &b| along[usize::from(a)].total_cmp(&along[usize::from(b)]));
        Some(order)
    }

    #[must_use]
    pub fn uniform_line_spacing_m(&self) -> Option<f64> {
        let axis = self.line_axis_deg()?;
        let order = self.axis_order()?;
        let along = self.projections(axis);
        let first = along[order.get(0)];
        let last = along[order.get(self.count - 1)];
        let spacing = (last - first) / (self.count - 1) as f64;
        let tol = self.tolerance();
        let uniform = (1..self.count).all(|k| {
            let gap = along[order.get(k)] - along[order.get(k - 1)];
            (gap - spacing).abs() <= tol
        });
        uniform.then_some(spacing)
    }

    #[must_use]
    pub fn is_planar_horizontal(&self) -> bool {
        let tol = self.tolerance();
        let mean_z = self.centroid().z;
        self.positions().iter().all(|p| (p.z - mean_z).abs() <= tol)
    }

    fn checked(
        shape: Shape,
        count: usize,
        positions: [Vec3; MAX_ELEMENTS],
    ) -> Result<Self, ManifoldError> {
        let geometry = Self {
            shape,
            count,
            positions,
        };
        let inside = |v: f64| v.is_finite() && v.abs() <= MAX_EXTENT_M;
        if !geometry
            .positions()
            .iter()
            .all(|p| inside(p.x) && inside(p.y) && inside(p.z))
        {
            return Err(ManifoldError::Position);
        }
        if geometry.nearest_spacing_m() <= MIN_SEPARATION_M {
            return Err(ManifoldError::Coincident);
        }
        Ok(geometry)
    }

    fn pair_distances(&self) -> impl Iterator<Item = f64> + '_ {
        let positions = self.positions();
        positions
            .iter()
            .enumerate()
            .flat_map(move |(i, p)| positions[i + 1..].iter().map(move |q| p.minus(*q).norm()))
    }

    fn tolerance(&self) -> f64 {
        SHAPE_TOLERANCE * self.aperture_m()
    }

    fn centred(&self) -> [Vec3; MAX_ELEMENTS] {
        let centre = self.centroid();
        let mut centred = [Vec3::default(); MAX_ELEMENTS];
        for (out, p) in centred.iter_mut().zip(self.positions()) {
            *out = p.minus(centre);
        }
        centred
    }

    fn line_direction(&self) -> Option<Vec3> {
        let positions = self.positions();
        let first = positions[0];
        let last = positions[self.count - 1];
        let (a, b) = self.farthest_pair();
        let span = positions[b].minus(positions[a]);
        let length = span.norm();
        let unit = Vec3::new(span.x / length, span.y / length, span.z / length);
        let tol = self.tolerance();
        let collinear = positions.iter().all(|p| {
            let offset = p.minus(positions[a]);
            let along = offset.dot(unit);
            Vec3::new(
                offset.x - along * unit.x,
                offset.y - along * unit.y,
                offset.z - along * unit.z,
            )
            .norm()
                <= tol
        });
        if !collinear {
            return None;
        }
        let sign = if last.minus(first).dot(unit) < 0.0 {
            -1.0
        } else {
            1.0
        };
        Some(Vec3::new(sign * unit.x, sign * unit.y, sign * unit.z))
    }

    fn farthest_pair(&self) -> (usize, usize) {
        let positions = self.positions();
        let mut best = (0, 1, 0.0f64);
        for (i, p) in positions.iter().enumerate() {
            for (j, q) in positions.iter().enumerate().skip(i + 1) {
                let distance = p.minus(*q).norm();
                if distance > best.2 {
                    best = (i, j, distance);
                }
            }
        }
        (best.0, best.1)
    }

    fn projections(&self, axis_deg: f64) -> [f64; MAX_ELEMENTS] {
        let (sin, cos) = axis_deg.to_radians().sin_cos();
        let mut along = [0.0; MAX_ELEMENTS];
        for (value, p) in along.iter_mut().zip(self.positions()) {
            *value = p.x * sin + p.y * cos;
        }
        along
    }
}

const fn check_count(count: usize) -> Result<(), ManifoldError> {
    if count < 2 || count > MAX_ELEMENTS {
        return Err(ManifoldError::Count(count));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        a.minus(b).norm() < 1e-12
    }

    #[test]
    fn uca_element_zero_is_forward_and_winds_clockwise() {
        let geometry = Geometry::uca(0.5, 4, 0.0, Winding::Clockwise).unwrap();
        let p = geometry.positions();
        assert!(close(p[0], Vec3::new(0.0, 0.5, 0.0)));
        assert!(close(p[1], Vec3::new(0.5, 0.0, 0.0)));
        assert!(close(p[2], Vec3::new(0.0, -0.5, 0.0)));
        assert!(close(p[3], Vec3::new(-0.5, 0.0, 0.0)));
        assert!(matches!(geometry.shape(), Shape::Uca { .. }));
        assert!((geometry.aperture_m() - 1.0).abs() < 1e-12);
        assert!((geometry.nearest_spacing_m() - 0.5 * 2f64.sqrt()).abs() < 1e-12);
    }

    #[test]
    fn uca_counterclockwise_mirrors_the_x_axis() {
        let clockwise = Geometry::uca(0.3, 5, 20.0, Winding::Clockwise).unwrap();
        let counter = Geometry::uca(0.3, 5, -20.0, Winding::Counterclockwise).unwrap();
        for (a, b) in clockwise.positions().iter().zip(counter.positions()) {
            assert!((a.x + b.x).abs() < 1e-12);
            assert!((a.y - b.y).abs() < 1e-12);
        }
    }

    #[test]
    fn ula_axis_rotates_the_line() {
        let geometry = Geometry::ula(0.5, 3, 0.0).unwrap();
        let p = geometry.positions();
        assert!(close(p[0], Vec3::new(0.0, -0.5, 0.0)));
        assert!(close(p[1], Vec3::new(0.0, 0.0, 0.0)));
        assert!(close(p[2], Vec3::new(0.0, 0.5, 0.0)));
        assert!(close(geometry.centroid(), Vec3::default()));
        let east_west = Geometry::ula(0.5, 2, 90.0).unwrap();
        assert!(close(east_west.positions()[0], Vec3::new(-0.25, 0.0, 0.0)));
        assert!(close(east_west.positions()[1], Vec3::new(0.25, 0.0, 0.0)));
    }

    #[test]
    fn antipodes_exist_for_even_uca_and_ula_not_for_odd_uca() {
        for count in [4, 6] {
            let geometry = Geometry::uca(0.4, count, 0.0, Winding::Clockwise).unwrap();
            let pairs = geometry.antipodes().unwrap();
            for i in 0..count {
                assert_eq!(pairs.get(i), (i + count / 2) % count);
            }
        }
        let odd = Geometry::uca(0.4, 5, 0.0, Winding::Clockwise).unwrap();
        assert_eq!(odd.antipodes(), None);
        for count in [4, 5] {
            let line = Geometry::ula(0.5, count, 90.0).unwrap();
            assert_eq!(line.antipodes(), Some(Permutation::reversal(count)));
        }
    }

    #[test]
    fn collinear_detection_for_ula_and_explicit_line() {
        let ula = Geometry::ula(0.5, 4, 90.0).unwrap();
        assert!((ula.line_axis_deg().unwrap() - 90.0).abs() < 1e-9);
        assert_eq!(ula.axis_order(), Some(Permutation::identity(4)));
        assert!((ula.uniform_line_spacing_m().unwrap() - 0.5).abs() < 1e-9);
        let backwards = Geometry::ula(0.5, 8, 180.0).unwrap();
        assert!((backwards.line_axis_deg().unwrap() - 180.0).abs() < 1e-9);
        assert_eq!(backwards.axis_order(), Some(Permutation::identity(8)));
        let typed = [
            Vec3::new(1.0, 1.0, 0.2),
            Vec3::new(0.0, 0.0, 0.2),
            Vec3::new(2.0, 2.0, 0.2),
            Vec3::new(0.5, 0.5, 0.2),
        ];
        let line = Geometry::explicit(&typed).unwrap();
        assert!((line.line_axis_deg().unwrap() - 225.0).abs() < 1e-9);
        let order = line.axis_order().unwrap();
        assert_eq!(order.iter().collect::<Vec<_>>(), vec![2, 0, 3, 1]);
        assert_eq!(line.uniform_line_spacing_m(), None);
        let uca = Geometry::uca(0.35, 5, 0.0, Winding::Clockwise).unwrap();
        assert_eq!(uca.line_axis_deg(), None);
        assert_eq!(uca.axis_order(), None);
        let tilted =
            Geometry::explicit(&[Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 1.0)]).unwrap();
        assert_eq!(tilted.line_axis_deg(), None);
        assert!(!tilted.is_planar_horizontal());
        assert!(tilted.is_collinear());
        assert!(ula.is_collinear());
        assert!(!uca.is_collinear());
    }

    #[test]
    fn invalid_geometries_are_refused() {
        assert_eq!(
            Geometry::ula(0.5, 1, 90.0).unwrap_err(),
            ManifoldError::Count(1)
        );
        assert_eq!(
            Geometry::ula(0.5, 17, 90.0).unwrap_err(),
            ManifoldError::Count(17)
        );
        assert_eq!(
            Geometry::ula(0.0, 3, 90.0).unwrap_err(),
            ManifoldError::Spacing
        );
        assert_eq!(
            Geometry::uca(f64::NAN, 3, 0.0, Winding::Clockwise).unwrap_err(),
            ManifoldError::Spacing
        );
        assert_eq!(
            Geometry::ula(60.0, 5, 90.0).unwrap_err(),
            ManifoldError::Position
        );
        assert_eq!(
            Geometry::explicit(&[Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)]).unwrap_err(),
            ManifoldError::Coincident
        );
        assert_eq!(
            Geometry::explicit(&[Vec3::new(f64::NAN, 0.0, 0.0), Vec3::default()]).unwrap_err(),
            ManifoldError::Position
        );
    }
}
