use super::vector::Vec3;
use crate::records::Mount;

pub(crate) const TOP: Vec3 = Vec3::new(0.0, 1.0, 0.0);
pub(crate) const BACK: Vec3 = Vec3::new(0.0, 0.0, -1.0);
const RIGHT: Vec3 = Vec3::new(1.0, 0.0, 0.0);

pub(crate) const fn forward(mount: Mount) -> Vec3 {
    match mount {
        Mount::Flat => TOP,
        Mount::Upright => BACK,
    }
}

pub(crate) fn pitch_roll(forward: Vec3, up: Vec3) -> (f64, f64) {
    let pitch = forward.dot(up).clamp(-1.0, 1.0).asin().to_degrees();
    let roll = -RIGHT.dot(up).clamp(-1.0, 1.0).asin().to_degrees();
    (pitch + 0.0, roll + 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_mount_uses_the_top_and_an_upright_mount_the_back() {
        assert_eq!(forward(Mount::Flat), TOP);
        assert_eq!(forward(Mount::Upright), BACK);
    }

    #[test]
    fn pitch_and_roll_follow_aviation_signs() {
        let flat = Vec3::new(0.0, 0.0, 1.0);
        assert_eq!(pitch_roll(TOP, flat), (0.0, 0.0));
        let tilt = 20f64.to_radians();
        let top_raised = Vec3::new(0.0, tilt.sin(), tilt.cos());
        let (pitch, roll) = pitch_roll(TOP, top_raised);
        assert!((pitch - 20.0).abs() < 1e-9 && roll.abs() < 1e-9);
        let right_down = Vec3::new(-tilt.sin(), 0.0, tilt.cos());
        let (pitch, roll) = pitch_roll(TOP, right_down);
        assert!(pitch.abs() < 1e-9 && (roll - 20.0).abs() < 1e-9);
        let upright_camera_up = Vec3::new(0.0, tilt.cos(), -tilt.sin());
        let (pitch, _) = pitch_roll(BACK, upright_camera_up);
        assert!((pitch - 20.0).abs() < 1e-9);
    }
}
