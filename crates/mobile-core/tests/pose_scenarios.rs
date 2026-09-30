use sdrmm_mobile_core::{
    HeadingMode, LocationSample, MagAccuracy, MotionFrame, MotionSample, Mount, PoseEngine,
    PoseSettings,
};

const T0: i64 = 1_790_000_000_000;
const MOTION_MS: i64 = 100;
const CAR_DEVIATION_DEG: f64 = 8.0;

struct Noise {
    state: u64,
}

impl Noise {
    fn new(seed: u64) -> Self {
        Self { state: seed.max(1) }
    }

    fn unit(&mut self) -> f64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        ((x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn gauss(&mut self, sigma: f64) -> f64 {
        let (a, b) = (self.unit(), self.unit());
        sigma * (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
    }
}

fn wrap_180(deg: f64) -> f64 {
    let wrapped = deg.rem_euclid(360.0);
    if wrapped > 180.0 {
        wrapped - 360.0
    } else {
        wrapped
    }
}

#[derive(Clone, Copy)]
struct Moment {
    heading: f64,
    rate: f64,
    speed: f64,
    reversing: bool,
}

struct Drive {
    engine: PoseEngine,
    noise: Noise,
    bias: f64,
    t: i64,
    outputs: Vec<(i64, f64, Option<f64>, Option<f64>)>,
}

impl Drive {
    fn new(offset: f64, bias: f64, seed: u64) -> Self {
        let mut engine = PoseEngine::new(PoseSettings {
            heading_mode: HeadingMode::Auto,
            mount: Mount::Flat,
            mount_offset_deg: offset,
            share_pose: false,
        });
        engine.demand(false, false, T0);
        Self {
            engine,
            noise: Noise::new(seed),
            bias,
            t: T0,
            outputs: Vec::new(),
        }
    }

    fn step(&mut self, moment: Moment) {
        self.t += MOTION_MS;
        let compass = moment.heading + CAR_DEVIATION_DEG + self.noise.gauss(5.0);
        let half = -(90.0 + compass).to_radians() / 2.0;
        let rate = moment.rate + self.bias + self.noise.gauss(0.2);
        self.engine.motion(MotionSample {
            t_unix_ms: self.t,
            frame: MotionFrame::TrueNorth,
            qw: half.cos(),
            qx: 0.0,
            qy: 0.0,
            qz: half.sin(),
            rot_x: 0.0,
            rot_y: 0.0,
            rot_z: -rate.to_radians(),
            grav_x: 0.0,
            grav_y: 0.0,
            grav_z: -1.0,
            heading_deg: None,
            mag_accuracy: MagAccuracy::High,
        });
        if (self.t - T0) % 1_000 == 0 {
            let moving = moment.speed > 0.5;
            let course = if moment.reversing {
                moment.heading + 180.0
            } else {
                moment.heading
            };
            self.engine.location(LocationSample {
                t_unix_ms: self.t,
                lat: 52.52,
                lon: 13.405,
                alt_m: None,
                h_acc_m: 5.0,
                v_acc_m: None,
                speed_mps: Some(moment.speed),
                speed_acc_mps: None,
                course_deg: moving.then(|| (course + self.noise.gauss(2.0)).rem_euclid(360.0)),
                course_acc_deg: moving.then_some(2.0),
            });
        }
        let view = self.engine.view(self.t);
        self.outputs
            .push((self.t, moment.heading, view.heading_deg, view.accuracy_deg));
    }

    fn hold(&mut self, seconds: i64, heading: f64, speed: f64) -> f64 {
        for _ in 0..seconds * 10 {
            self.step(Moment {
                heading,
                rate: 0.0,
                speed,
                reversing: false,
            });
        }
        heading
    }

    fn turn(&mut self, from: f64, rate: f64, seconds: f64, speed: f64) -> f64 {
        let steps = (seconds * 10.0).round() as i64;
        let mut heading = from;
        for _ in 0..steps {
            heading += rate * MOTION_MS as f64 / 1_000.0;
            self.step(Moment {
                heading,
                rate,
                speed,
                reversing: false,
            });
        }
        heading
    }

    fn errors_after(&self, from: i64) -> Vec<f64> {
        self.outputs
            .iter()
            .filter(|(t, ..)| *t >= from)
            .map(|(_, truth, out, _)| out.map_or(f64::INFINITY, |out| wrap_180(out - truth).abs()))
            .collect()
    }
}

mod pose {
    use super::*;

    #[test]
    fn a_square_block_stays_within_5_deg() {
        let mut drive = Drive::new(0.0, 0.2, 11);
        let mut heading = 0.0;
        for _ in 0..4 {
            heading = drive.hold(30, heading, 10.0);
            heading = drive.turn(heading, 15.0, 6.0, 10.0);
        }
        let worst = drive
            .errors_after(T0 + 30_000)
            .into_iter()
            .fold(0.0, f64::max);
        assert!(worst <= 5.0, "{worst}");
    }

    #[test]
    fn a_minute_at_the_lights_holds_the_heading() {
        let mut drive = Drive::new(0.0, 0.05, 23);
        drive.hold(30, 45.0, 10.0);
        drive.hold(60, 45.0, 0.0);
        let &(_, truth, out, sigma) = drive.outputs.last().expect("outputs");
        let error = wrap_180(out.expect("heading") - truth).abs();
        assert!(error <= 4.0, "{error}");
        assert!(sigma.expect("sigma") >= error, "{sigma:?} {error}");
    }

    #[test]
    fn driving_through_north_never_jumps() {
        let mut drive = Drive::new(0.0, 0.1, 37);
        let heading = drive.hold(15, 340.0, 10.0);
        let heading = drive.turn(heading, 2.0, 20.0, 10.0);
        drive.hold(10, heading, 10.0);
        let settled: Vec<f64> = drive
            .outputs
            .iter()
            .filter(|(t, ..)| *t >= T0 + 10_000)
            .filter_map(|(_, _, out, _)| *out)
            .collect();
        let jump = settled
            .windows(2)
            .map(|pair| wrap_180(pair[1] - pair[0]).abs())
            .fold(0.0, f64::max);
        assert!(jump <= 2.0, "{jump}");
        let worst = drive
            .errors_after(T0 + 10_000)
            .into_iter()
            .fold(0.0, f64::max);
        assert!(worst <= 5.0, "{worst}");
    }

    #[test]
    fn parked_start_initialises_from_the_compass() {
        let mut drive = Drive::new(CAR_DEVIATION_DEG, 0.05, 41);
        drive.hold(10, 200.0, 0.0);
        let parked = drive.errors_after(T0 + 9_000);
        assert!(parked.iter().all(|error| *error <= 2.0), "{parked:?}");
        drive.hold(30, 200.0, 10.0);
        let driving = drive.errors_after(T0 + 40_000);
        assert!(driving.iter().all(|error| *error <= 2.0), "{driving:?}");
    }

    #[test]
    fn reversing_does_not_flip_the_heading() {
        let mut drive = Drive::new(0.0, 0.05, 53);
        drive.hold(20, 120.0, 10.0);
        drive.hold(3, 120.0, 0.0);
        for _ in 0..100 {
            drive.step(Moment {
                heading: 120.0,
                rate: 0.0,
                speed: 3.5,
                reversing: true,
            });
        }
        let worst = drive
            .errors_after(T0 + 20_000)
            .into_iter()
            .fold(0.0, f64::max);
        assert!(worst <= 5.0, "{worst}");
    }
}
