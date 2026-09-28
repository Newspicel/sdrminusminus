use sdrmm_wire::geo::wrap_360;

pub(crate) const MIN_HORIZONTAL: f64 = 0.2;
const MIN_NORM: f64 = 1e-3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Vec3 {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) z: f64,
}

impl Vec3 {
    pub(crate) const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    pub(crate) fn dot(self, other: Self) -> f64 {
        self.x
            .mul_add(other.x, self.y.mul_add(other.y, self.z * other.z))
    }

    pub(crate) fn cross(self, other: Self) -> Self {
        Self::new(
            self.y.mul_add(other.z, -(self.z * other.y)),
            self.z.mul_add(other.x, -(self.x * other.z)),
            self.x.mul_add(other.y, -(self.y * other.x)),
        )
    }

    pub(crate) fn scale(self, factor: f64) -> Self {
        Self::new(self.x * factor, self.y * factor, self.z * factor)
    }

    pub(crate) fn plus(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }

    pub(crate) fn norm(self) -> f64 {
        self.dot(self).sqrt()
    }

    pub(crate) fn finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    pub(crate) fn unit(self) -> Option<Self> {
        let norm = self.norm();
        (self.finite() && norm >= MIN_NORM).then(|| self.scale(1.0 / norm))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Quat {
    pub(crate) w: f64,
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) z: f64,
}

impl Quat {
    pub(crate) fn unit(self) -> Option<Self> {
        let norm = self
            .w
            .mul_add(
                self.w,
                Vec3::new(self.x, self.y, self.z).dot(Vec3::new(self.x, self.y, self.z)),
            )
            .sqrt();
        let finite = self.w.is_finite() && Vec3::new(self.x, self.y, self.z).finite();
        (finite && norm >= MIN_NORM).then(|| Self {
            w: self.w / norm,
            x: self.x / norm,
            y: self.y / norm,
            z: self.z / norm,
        })
    }

    pub(crate) fn rotate(self, v: Vec3) -> Vec3 {
        let u = Vec3::new(self.x, self.y, self.z);
        let uv = u.cross(v);
        v.plus(uv.scale(2.0 * self.w)).plus(u.cross(uv).scale(2.0))
    }

    #[cfg(test)]
    pub(crate) fn about(axis: Vec3, deg: f64) -> Self {
        let half = deg.to_radians() / 2.0;
        let axis = axis.unit().unwrap_or(Vec3::new(0.0, 0.0, 1.0));
        Self {
            w: half.cos(),
            x: axis.x * half.sin(),
            y: axis.y * half.sin(),
            z: axis.z * half.sin(),
        }
    }

    #[cfg(test)]
    pub(crate) fn then(self, next: Self) -> Self {
        Self {
            w: next.w * self.w - next.x * self.x - next.y * self.y - next.z * self.z,
            x: next.w * self.x + next.x * self.w + next.y * self.z - next.z * self.y,
            y: next.w * self.y - next.x * self.z + next.y * self.w + next.z * self.x,
            z: next.w * self.z + next.x * self.y - next.y * self.x + next.z * self.w,
        }
    }
}

pub(crate) fn heading_of(east: f64, north: f64) -> Option<f64> {
    (east.hypot(north) >= MIN_HORIZONTAL).then(|| wrap_360(east.atan2(north).to_degrees()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9 && (a.z - b.z).abs() < 1e-9
    }

    #[test]
    fn quaternion_rotates_device_axes_into_the_world() {
        let quarter = Quat::about(Vec3::new(0.0, 0.0, 1.0), 90.0);
        assert!(close(
            quarter.rotate(Vec3::new(1.0, 0.0, 0.0)),
            Vec3::new(0.0, 1.0, 0.0)
        ));
        assert!(close(
            quarter.rotate(Vec3::new(0.0, 0.0, 1.0)),
            Vec3::new(0.0, 0.0, 1.0)
        ));
        let both = quarter.then(Quat::about(Vec3::new(1.0, 0.0, 0.0), 90.0));
        assert!(close(
            both.rotate(Vec3::new(1.0, 0.0, 0.0)),
            Vec3::new(0.0, 0.0, 1.0)
        ));
        let scaled = Quat {
            w: 2.0,
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        assert_eq!(
            scaled.unit(),
            Some(Quat {
                w: 1.0,
                x: 0.0,
                y: 0.0,
                z: 0.0
            })
        );
        let zero = Quat {
            w: 0.0,
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        assert_eq!(zero.unit(), None);
    }

    #[test]
    fn heading_of_a_vertical_axis_is_undefined() {
        assert_eq!(heading_of(0.0, 0.1), None);
        assert_eq!(heading_of(1.0, 0.0), Some(90.0));
        assert_eq!(heading_of(0.0, 1.0), Some(0.0));
        assert_eq!(heading_of(-1.0, 0.0), Some(270.0));
        assert_eq!(Vec3::new(0.0, 0.0, 0.0).unit(), None);
        assert_eq!(Vec3::new(f64::NAN, 1.0, 0.0).unit(), None);
    }
}
