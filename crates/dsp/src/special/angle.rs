#[must_use]
pub fn wrap_deg(angle: f64) -> f64 {
    let turned = norm_deg(angle);
    if turned > 180.0 {
        turned - 360.0
    } else {
        turned
    }
}

#[must_use]
pub fn norm_deg(angle: f64) -> f64 {
    let turned = angle.rem_euclid(360.0);
    if turned >= 360.0 { 0.0 } else { turned }
}

#[must_use]
pub fn circular_mean_deg(sum_sin: f64, sum_cos: f64) -> f64 {
    norm_deg(sum_sin.atan2(sum_cos).to_degrees())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_and_norm_cover_the_edges() {
        assert_eq!(wrap_deg(180.0), 180.0);
        assert_eq!(wrap_deg(-180.0), 180.0);
        assert_eq!(norm_deg(-0.5), 359.5);
        assert_eq!(wrap_deg(540.0), 180.0);
        assert_eq!(wrap_deg(-190.0), 170.0);
        assert_eq!(wrap_deg(359.0), -1.0);
        assert_eq!(norm_deg(720.0), 0.0);
        assert_eq!(norm_deg(-1e-20), 0.0);
        assert_eq!(wrap_deg(-1e-20), 0.0);
        assert!(norm_deg(f64::NAN).is_nan());
    }

    #[test]
    fn circular_mean_straddles_north() {
        let bearings = [350.0f64, 10.0, 0.0];
        let (sin, cos) = bearings.iter().fold((0.0, 0.0), |(s, c), b: &f64| {
            (s + b.to_radians().sin(), c + b.to_radians().cos())
        });
        assert!(wrap_deg(circular_mean_deg(sin, cos)).abs() < 1e-9);
        assert!((circular_mean_deg(-1.0, 0.0) - 270.0).abs() < 1e-12);
        assert_eq!(circular_mean_deg(0.0, 0.0), 0.0);
    }
}
