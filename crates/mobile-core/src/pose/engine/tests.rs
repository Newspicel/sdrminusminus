use super::*;
use crate::records::{AlignState, HeadingMode, Mount};

const T0: i64 = 1_790_000_000_000;

fn settings(heading_mode: HeadingMode, offset: f64) -> PoseSettings {
    PoseSettings {
        heading_mode,
        mount: Mount::Flat,
        mount_offset_deg: offset,
        share_pose: true,
    }
}

fn fix(t: i64, speed: Option<f64>, course: Option<f64>) -> LocationSample {
    LocationSample {
        t_unix_ms: t,
        lat: 52.52,
        lon: 13.405,
        alt_m: Some(40.0),
        h_acc_m: 5.0,
        v_acc_m: None,
        speed_mps: speed,
        speed_acc_mps: None,
        course_deg: course,
        course_acc_deg: course.map(|_| 2.0),
    }
}

fn conj(q: Quat) -> Quat {
    Quat {
        w: q.w,
        x: -q.x,
        y: -q.y,
        z: -q.z,
    }
}

fn about_z(deg: f64) -> Quat {
    Quat::about(Vec3::new(0.0, 0.0, 1.0), deg)
}

fn true_north(heading: f64) -> Quat {
    about_z(-(90.0 + heading))
}

fn enu(heading: f64) -> Quat {
    about_z(-heading)
}

fn motion(
    t: i64,
    frame: MotionFrame,
    q: Quat,
    rate_cw_deg_s: f64,
    accuracy: MagAccuracy,
) -> MotionSample {
    MotionSample {
        t_unix_ms: t,
        frame,
        qw: q.w,
        qx: q.x,
        qy: q.y,
        qz: q.z,
        rot_x: 0.0,
        rot_y: 0.0,
        rot_z: -rate_cw_deg_s.to_radians(),
        grav_x: 0.0,
        grav_y: 0.0,
        grav_z: -1.0,
        heading_deg: None,
        mag_accuracy: accuracy,
    }
}

fn gyro(t: i64, rate_cw_deg_s: f64) -> MotionSample {
    motion(
        t,
        MotionFrame::Arbitrary,
        about_z(0.0),
        rate_cw_deg_s,
        MagAccuracy::Uncalibrated,
    )
}

fn heading_sample(
    t: i64,
    magnetic: f64,
    true_deg: Option<f64>,
    accuracy: Option<f64>,
) -> HeadingSample {
    HeadingSample {
        t_unix_ms: t,
        true_deg,
        magnetic_deg: magnetic,
        accuracy_deg: accuracy,
    }
}

fn heading(engine: &PoseEngine, now: i64) -> Option<f64> {
    engine.view(now).heading_deg
}

fn close(value: Option<f64>, expected: f64, tolerance: f64) -> bool {
    value.is_some_and(|value| wrap_180(value - expected).abs() <= tolerance)
}

fn drive(engine: &mut PoseEngine, from: i64, seconds: i64, speed: f64, course: f64) -> i64 {
    for second in 0..seconds {
        engine.location(fix(from + second * 1_000, Some(speed), Some(course)));
    }
    from + seconds * 1_000
}

#[test]
fn enu_magnetic_quaternion_gives_true_heading_with_declination() {
    let mut engine = PoseEngine::new(settings(HeadingMode::Compass, 0.0));
    engine.heading(heading_sample(T0, 30.0, Some(33.0), None));
    assert_eq!(heading(&engine, T0), None);
    engine.motion(motion(
        T0 + 10,
        MotionFrame::EnuMagnetic,
        enu(30.0),
        0.0,
        MagAccuracy::High,
    ));
    assert!(
        close(heading(&engine, T0 + 10), 33.0, 1e-6),
        "{:?}",
        heading(&engine, T0 + 10)
    );
}

#[test]
fn a_magnetic_compass_is_made_true_east_positive() {
    for (magnetic, true_deg, expected) in [(10.0, 13.0, 13.0), (2.0, 357.0, 357.0)] {
        let mut engine = PoseEngine::new(settings(HeadingMode::Compass, 0.0));
        engine.heading(heading_sample(
            T0,
            50.0,
            Some(50.0 + (true_deg - magnetic)),
            None,
        ));
        engine.motion(motion(
            T0 + 10,
            MotionFrame::EnuMagnetic,
            enu(magnetic),
            0.0,
            MagAccuracy::High,
        ));
        assert!(close(heading(&engine, T0 + 10), expected, 1e-6));
    }
}

#[test]
fn a_magnetic_compass_without_declination_is_refused_with_a_notice() {
    let mut engine = PoseEngine::new(settings(HeadingMode::Compass, 0.0));
    let step = engine.motion(motion(
        T0,
        MotionFrame::EnuMagnetic,
        enu(30.0),
        0.0,
        MagAccuracy::High,
    ));
    assert_eq!(step.notices, vec![Notice::warn("No declination")]);
    let again = engine.motion(motion(
        T0 + 20,
        MotionFrame::EnuMagnetic,
        enu(30.0),
        0.0,
        MagAccuracy::High,
    ));
    assert!(again.notices.is_empty());
    assert_eq!(heading(&engine, T0 + 20), None);
    assert_eq!(engine.counters().compass_rejected, 2);
}

#[test]
fn a_compass_without_accuracy_is_refused_with_a_notice() {
    let mut engine = PoseEngine::new(settings(HeadingMode::Compass, 0.0));
    let step = engine.motion(motion(
        T0,
        MotionFrame::TrueNorth,
        true_north(30.0),
        0.0,
        MagAccuracy::Uncalibrated,
    ));
    assert_eq!(step.notices, vec![Notice::warn("No compass accuracy")]);
    assert_eq!(heading(&engine, T0), None);
    let mut flat = PoseEngine::new(settings(HeadingMode::Compass, 0.0));
    let step = flat.heading(heading_sample(T0, 30.0, Some(31.0), None));
    assert_eq!(step.notices, vec![Notice::warn("No compass accuracy")]);
}

#[test]
fn ios_quaternion_turning_clockwise_raises_heading() {
    let mut engine = PoseEngine::new(settings(HeadingMode::Compass, 0.0));
    engine.motion(motion(
        T0,
        MotionFrame::TrueNorth,
        true_north(10.0),
        0.0,
        MagAccuracy::High,
    ));
    assert!(close(heading(&engine, T0), 10.0, 1e-6));
    for step in 1..=30 {
        let t = T0 + step * 100;
        let at = 10.0 + step as f64;
        engine.motion(motion(
            t,
            MotionFrame::TrueNorth,
            true_north(at),
            10.0,
            MagAccuracy::High,
        ));
    }
    assert!(
        close(heading(&engine, T0 + 3_000), 40.0, 0.5),
        "{:?}",
        heading(&engine, T0 + 3_000)
    );
}

#[test]
fn course_is_used_only_after_3_s_above_3_mps() {
    let mut engine = PoseEngine::new(settings(HeadingMode::Auto, 0.0));
    for second in 0..3 {
        engine.location(fix(T0 + second * 1_000, Some(5.0), Some(80.0)));
        assert_eq!(heading(&engine, T0 + second * 1_000), None);
    }
    assert_eq!(engine.movement(), Movement::Unknown);
    engine.location(fix(T0 + 3_000, Some(5.0), Some(80.0)));
    assert_eq!(engine.movement(), Movement::Moving);
    assert!(close(heading(&engine, T0 + 3_000), 80.0, 1e-6));
    assert_eq!(engine.view(T0 + 3_000).source, HeadingSourceKind::Course);
}

#[test]
fn low_speed_holds_the_heading_on_the_gyro() {
    let mut engine = PoseEngine::new(settings(HeadingMode::Auto, 0.0));
    let t = drive(&mut engine, T0, 5, 10.0, 80.0);
    for step in 0..50 {
        let at = t + step * 100;
        engine.motion(gyro(at, 0.0));
        if step % 10 == 0 {
            engine.location(fix(at, Some(1.0), Some(200.0)));
        }
    }
    assert!(close(heading(&engine, t + 5_000), 80.0, 0.5));
    assert_eq!(engine.view(t + 5_000).source, HeadingSourceKind::Fused);
}

#[test]
fn fast_turns_skip_the_course() {
    let mut engine = PoseEngine::new(settings(HeadingMode::Auto, 0.0));
    let t = drive(&mut engine, T0, 5, 10.0, 80.0);
    for step in 1..=20 {
        engine.motion(gyro(t + step * 50, 30.0));
    }
    let turned = heading(&engine, t + 1_000);
    engine.location(fix(t + 1_000, Some(10.0), Some(150.0)));
    assert_eq!(heading(&engine, t + 1_000), turned);
    assert!(close(turned, 108.5, 0.1), "{turned:?}");
}

#[test]
fn vehicle_compass_takes_over_only_when_stationary_and_uncertain() {
    let mut engine = PoseEngine::new(settings(HeadingMode::Auto, 0.0));
    let t = drive(&mut engine, T0, 5, 10.0, 80.0);
    for second in 0..3 {
        engine.location(fix(t + second * 1_000, Some(0.0), None));
    }
    let parked = t + 3_000;
    engine.motion(motion(
        parked,
        MotionFrame::TrueNorth,
        true_north(120.0),
        0.0,
        MagAccuracy::High,
    ));
    assert!(close(heading(&engine, parked), 80.0, 0.5));
    let mut blind = PoseEngine::new(settings(HeadingMode::Auto, 0.0));
    let t = drive(&mut blind, T0, 5, 10.0, 80.0);
    for second in 0..3 {
        blind.location(fix(t + second * 1_000, Some(0.0), None));
    }
    blind.tick(t + 20_000);
    blind.location(fix(t + 20_000, Some(0.0), None));
    blind.location(fix(t + 22_000, Some(0.0), None));
    assert!(
        blind
            .view(t + 22_000)
            .accuracy_deg
            .is_some_and(|sigma| sigma > 15.0)
    );
    blind.motion(motion(
        t + 22_000,
        MotionFrame::TrueNorth,
        true_north(120.0),
        0.0,
        MagAccuracy::High,
    ));
    assert!(
        close(heading(&blind, t + 22_000), 120.0, 10.0),
        "{:?}",
        heading(&blind, t + 22_000)
    );
}

#[test]
fn a_compass_heading_claims_no_more_than_the_compass_accuracy() {
    let mut engine = PoseEngine::new(settings(HeadingMode::Compass, 0.0));
    for step in 0..500 {
        engine.motion(motion(
            T0 + step * 20,
            MotionFrame::TrueNorth,
            true_north(40.0),
            0.0,
            MagAccuracy::High,
        ));
    }
    let view = engine.view(T0 + 10_000);
    assert!(close(view.heading_deg, 40.0, 1e-6));
    assert_eq!(view.accuracy_deg, Some(10.0));
    let mut vehicle = PoseEngine::new(settings(HeadingMode::Auto, 0.0));
    let t = drive(&mut vehicle, T0, 10, 10.0, 80.0);
    assert!(
        vehicle
            .view(t)
            .accuracy_deg
            .is_some_and(|sigma| sigma < 3.0)
    );
}

#[test]
fn compass_accuracy_gates_by_mode() {
    let accept = |mode: HeadingMode, accuracy: f64| {
        let mut engine = PoseEngine::new(settings(mode, 0.0));
        engine.heading(heading_sample(T0, 60.0, Some(60.0), Some(accuracy)));
        heading(&engine, T0).is_some()
    };
    assert!(accept(HeadingMode::Compass, 20.0));
    assert!(!accept(HeadingMode::Auto, 20.0));
    assert!(accept(HeadingMode::Auto, 10.0));
    assert!(!accept(HeadingMode::Compass, 30.0));
    assert!(!accept(HeadingMode::Auto, 30.0));
    assert!(!accept(HeadingMode::Course, 5.0));
}

#[test]
fn mount_offset_turns_phone_heading_into_vehicle_heading() {
    let mut engine = PoseEngine::new(settings(HeadingMode::Compass, 10.0));
    engine.motion(motion(
        T0,
        MotionFrame::TrueNorth,
        true_north(100.0),
        0.0,
        MagAccuracy::High,
    ));
    assert!(close(heading(&engine, T0), 90.0, 1e-6));
    engine.settings(settings(HeadingMode::Compass, 0.0));
    assert!(close(heading(&engine, T0), 90.0, 1e-6));
}

#[test]
fn handheld_ignores_course_and_reports_pitch_and_roll() {
    let mut engine = PoseEngine::new(settings(HeadingMode::Compass, 0.0));
    engine.demand(true, true, T0);
    engine.motion(motion(
        T0,
        MotionFrame::TrueNorth,
        true_north(40.0),
        0.0,
        MagAccuracy::High,
    ));
    let t = drive(&mut engine, T0 + 100, 5, 10.0, 200.0);
    assert!(close(heading(&engine, t), 40.0, 1e-6));
    let step = engine.location(fix(t + 100, Some(10.0), Some(200.0)));
    let published = step.publish.and_then(|out| out.fix).expect("fix");
    assert_eq!(published.attitude.pitch_deg, Some(0.0));
    assert_eq!(published.attitude.roll_deg, Some(0.0));
    let mut vehicle = PoseEngine::new(settings(HeadingMode::Auto, 0.0));
    vehicle.demand(true, true, T0);
    vehicle.motion(gyro(T0, 0.0));
    let step = vehicle.location(fix(T0 + 100, Some(10.0), Some(200.0)));
    let published = step.publish.and_then(|out| out.fix).expect("fix");
    assert_eq!(published.attitude.pitch_deg, None);
}

#[test]
fn without_gyro_the_measurement_source_is_reported() {
    let mut engine = PoseEngine::new(settings(HeadingMode::Auto, 0.0));
    let t = drive(&mut engine, T0, 5, 10.0, 80.0);
    assert_eq!(engine.view(t).source, HeadingSourceKind::Course);
    engine.motion(gyro(t, 0.0));
    assert_eq!(engine.view(t).source, HeadingSourceKind::Fused);
    assert_eq!(engine.view(t + 2_000).source, HeadingSourceKind::Course);
    assert_eq!(
        PoseEngine::new(DEFAULT).view(T0).source,
        HeadingSourceKind::None
    );
}

const DEFAULT: PoseSettings = PoseSettings {
    heading_mode: HeadingMode::Auto,
    mount: Mount::Flat,
    mount_offset_deg: 0.0,
    share_pose: false,
};

#[test]
fn a_gyro_gap_inflates_sigma_and_is_counted() {
    let mut engine = PoseEngine::new(settings(HeadingMode::Auto, 0.0));
    let t = drive(&mut engine, T0, 5, 10.0, 80.0);
    engine.motion(gyro(t, 0.0));
    let before = engine.view(t).accuracy_deg.unwrap_or_default();
    engine.motion(gyro(t + 1_000, 0.0));
    let after = engine.view(t + 1_000).accuracy_deg.unwrap_or_default();
    assert_eq!(engine.counters().gyro_gaps, 1);
    assert!(after > 29.0 && after > before, "{before} {after}");
    let stale = engine.motion(gyro(t + 900, 0.0));
    assert_eq!(stale.notices, vec![Notice::warn("Bad sensor data")]);
    assert_eq!(engine.counters().invalid_samples, 1);
}

#[test]
fn an_inverted_attitude_quaternion_is_reported() {
    let mut engine = PoseEngine::new(settings(HeadingMode::Compass, 0.0));
    let tilt = Quat::about(Vec3::new(1.0, 0.0, 0.0), 40.0).then(true_north(270.0));
    let up = conj(tilt).rotate(Vec3::new(0.0, 0.0, 1.0));
    let inverse = conj(tilt);
    let mut notices = Vec::new();
    for step in 0..12 {
        let mut sample = motion(
            T0 + step * 20,
            MotionFrame::TrueNorth,
            inverse,
            0.0,
            MagAccuracy::High,
        );
        sample.grav_x = -up.x;
        sample.grav_y = -up.y;
        sample.grav_z = -up.z;
        notices.extend(engine.motion(sample).notices);
    }
    assert_eq!(notices, vec![Notice::error("Phone attitude inverted")]);
    assert_eq!(engine.counters().attitude_mismatch, 12);
    assert_eq!(heading(&engine, T0 + 240), None);
    let mut right = motion(
        T0 + 300,
        MotionFrame::TrueNorth,
        tilt,
        0.0,
        MagAccuracy::High,
    );
    right.grav_x = -up.x;
    right.grav_y = -up.y;
    right.grav_z = -up.z;
    engine.motion(right);
    assert!(
        close(heading(&engine, T0 + 300), 270.0, 1e-6),
        "{:?}",
        heading(&engine, T0 + 300)
    );
}

#[test]
fn invalid_samples_are_counted_not_used() {
    let mut engine = PoseEngine::new(settings(HeadingMode::Auto, 0.0));
    let mut bad = fix(T0, Some(5.0), Some(10.0));
    bad.lat = f64::NAN;
    let step = engine.location(bad);
    assert_eq!(step.notices, vec![Notice::warn("Bad sensor data")]);
    let mut zero = gyro(T0, 0.0);
    zero.qw = 0.0;
    engine.motion(zero);
    let mut odd = heading_sample(T0, f64::INFINITY, None, None);
    odd.true_deg = Some(1.0);
    engine.heading(odd);
    assert_eq!(engine.counters().invalid_samples, 3);
    assert_eq!(engine.view(T0).fix_age_ms, None);
    let mut negative = fix(T0, Some(-1.0), Some(-1.0));
    negative.speed_acc_mps = Some(f64::NAN);
    engine.location(negative);
    assert_eq!(engine.view(T0).fix_age_ms, Some(0));
}

#[test]
fn derived_speed_is_used_when_the_os_gives_none() {
    let mut engine = PoseEngine::new(settings(HeadingMode::Auto, 0.0));
    for second in 0..5 {
        let mut sample = fix(T0 + second * 1_000, None, Some(0.0));
        sample.lat = geo::offset_m(
            geo::LatLon {
                lat: 52.52,
                lon: 13.405,
            },
            0.0,
            10.0 * second as f64,
        )
        .lat;
        engine.location(sample);
    }
    assert_eq!(engine.movement(), Movement::Moving);
    assert!(close(heading(&engine, T0 + 4_000), 0.0, 1e-6));
}

#[test]
fn nothing_is_published_while_sharing_is_off_or_no_one_listens() {
    let mut off = PoseEngine::new(DEFAULT);
    off.demand(true, true, T0);
    assert_eq!(off.location(fix(T0, Some(1.0), None)).publish, None);
    let mut unwanted = PoseEngine::new(settings(HeadingMode::Auto, 0.0));
    assert_eq!(unwanted.location(fix(T0, Some(1.0), None)).publish, None);
    assert!(!unwanted.view(T0).sending);
    unwanted.demand(true, false, T0);
    assert!(!unwanted.view(T0).sending);
    assert_eq!(
        unwanted.location(fix(T0 + 10, Some(1.0), None)).publish,
        None
    );
    let step = unwanted.location(fix(T0 + 100, Some(1.0), None));
    assert!(step.publish.is_some_and(|out| out.fix.is_some()));
    unwanted.demand(true, true, T0 + 20);
    assert!(unwanted.view(T0 + 20).sending);
}

#[test]
fn a_stale_fix_publishes_an_error_not_old_coordinates() {
    let mut engine = PoseEngine::new(settings(HeadingMode::Auto, 0.0));
    let first = engine.demand(true, true, T0);
    assert_eq!(engine.tick(T0 + 1).publish, None);
    assert_eq!(
        first.publish,
        Some(PoseOut {
            fix: None,
            error: Some(NO_FIX.to_owned())
        })
    );
    engine.location(fix(T0 + 2_000, Some(1.0), None));
    let step = engine.tick(T0 + 2_000 + STALE_FIX_MS + 1);
    assert_eq!(
        step.publish.and_then(|out| out.error).as_deref(),
        Some(NO_FIX)
    );
    assert_eq!(engine.tick(T0 + 2_000 + STALE_FIX_MS + 500).publish, None);
    assert!(
        engine
            .tick(T0 + 2_000 + STALE_FIX_MS + 1_001)
            .publish
            .is_some()
    );
}

#[test]
fn a_late_fix_is_published_at_the_current_instant() {
    let mut engine = PoseEngine::new(settings(HeadingMode::Compass, 0.0));
    engine.demand(true, true, T0);
    let mut stamps = Vec::new();
    for step in 1..=30 {
        let t = T0 + step * 100;
        let mut outs = Vec::new();
        if step % 10 == 0 {
            outs.push(engine.location(fix(t - 400, Some(10.0), None)).publish);
        }
        outs.push(engine.motion(gyro(t, 0.0)).publish);
        stamps.extend(
            outs.into_iter()
                .flatten()
                .filter_map(|out| out.fix)
                .map(|fix| (fix.time.parse::<jiff::Timestamp>().ok(), fix.accuracy_m)),
        );
    }
    let times: Vec<i64> = stamps
        .iter()
        .filter_map(|(time, _)| time.map(|time| time.as_millisecond()))
        .collect();
    assert_eq!(times.len(), stamps.len());
    assert!(times.windows(2).all(|pair| pair[0] <= pair[1]), "{times:?}");
    assert_eq!(times.first(), Some(&(T0 + 900)));
    let accuracy = stamps.first().and_then(|(_, accuracy)| *accuracy);
    assert!(
        accuracy.is_some_and(|metres| (metres - 8.0).abs() < 1e-9),
        "{accuracy:?}"
    );
}

#[test]
fn heading_is_dropped_above_45_deg_sigma() {
    let mut engine = PoseEngine::new(settings(HeadingMode::Auto, 0.0));
    let t = drive(&mut engine, T0, 5, 10.0, 80.0);
    assert!(heading(&engine, t).is_some());
    engine.tick(t + 90_000);
    assert_eq!(heading(&engine, t + 90_000), None);
    assert_eq!(engine.view(t + 90_000).source, HeadingSourceKind::None);
}

#[test]
fn align_applies_the_offset_it_finds() {
    let mut engine = PoseEngine::new(settings(HeadingMode::Auto, 0.0));
    engine.start_align(T0);
    assert!(matches!(
        engine.view(T0).align,
        AlignState::Collecting { .. }
    ));
    let mut done = None;
    for second in 0..40 {
        let t = T0 + second * 1_000;
        engine.motion(motion(
            t - 100,
            MotionFrame::TrueNorth,
            true_north(97.0),
            0.0,
            MagAccuracy::High,
        ));
        engine.location(fix(t, Some(12.0), Some(90.0)));
        if let AlignState::Done { offset_deg } = engine.view(t).align {
            done = Some((t, offset_deg));
            break;
        }
    }
    let (t, offset) = done.expect("aligned");
    assert!((offset - 7.0).abs() < 0.2, "{offset}");
    engine.location(fix(t + 1_000, Some(12.0), Some(90.0)));
    assert!(close(heading(&engine, t + 1_000), 90.0, 1.0));
    engine.start_align(t + 2_000);
    engine.cancel_align();
    assert_eq!(engine.view(t + 2_000).align, AlignState::Idle);
    engine.start_align(t + 3_000);
    engine.tick(t + 3_000 + 120_000);
    assert!(matches!(engine.view(t).align, AlignState::Failed { .. }));
}
