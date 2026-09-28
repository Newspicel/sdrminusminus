use num_complex::Complex;
use sdrmm_dsp::doa::{Doa, DoaConfig, DoaReport, Estimator, LIKELIHOOD_POINTS, Reconfigure};
use sdrmm_dsp::linalg::{CMat, LinalgError};
use sdrmm_dsp::manifold::{
    Direction, ElevationSpan, Geometry, Manifold, ManifoldError, Vec3, Winding,
};
use sdrmm_test_support::{CountingAlloc, assert_no_alloc};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

const FREQ: f64 = 433.92e6;

fn covariance(manifold: &Manifold, sources: &[(Direction, f32)]) -> Result<CMat, LinalgError> {
    let n = manifold.len();
    let mut r = CMat::identity(n)?;
    let mut a = [Complex::new(0.0f32, 0.0); 16];
    for &(direction, power) in sources {
        manifold.steer(FREQ, direction, &mut a[..n]);
        for i in 0..n {
            for j in 0..n {
                r.add(i, j, a[i] * a[j].conj() * power);
            }
        }
    }
    Ok(r)
}

fn kraken() -> Result<Manifold, ManifoldError> {
    Ok(Manifold::ideal(Geometry::uca(
        0.2939,
        5,
        0.0,
        Winding::Clockwise,
    )?))
}

#[test]
fn estimate_does_not_allocate() {
    let manifold = kraken().unwrap();
    let r = covariance(
        &manifold,
        &[
            (Direction::horizon(40.0), 10.0),
            (Direction::horizon(200.0), 3.0),
        ],
    )
    .unwrap();
    let mut report = DoaReport::default();
    let mut likelihood = [0.0f32; LIKELIHOOD_POINTS];
    for estimator in [Estimator::Bartlett, Estimator::Capon, Estimator::Music] {
        let config = DoaConfig {
            estimator,
            ..DoaConfig::default()
        };
        let mut doa = Doa::new(&manifold, &config, FREQ).unwrap();
        doa.estimate(&manifold, &r, 4096.0, 3.0, &mut report)
            .unwrap();
        let mut sink = 0.0f64;
        assert_no_alloc("doa estimate", || {
            for _ in 0..3 {
                doa.estimate(&manifold, &r, 4096.0, 3.0, &mut report)
                    .unwrap();
                sink += report.peaks[0].azimuth_deg;
                assert!(
                    doa.likelihood(&manifold, &r, &report, &mut likelihood)
                        .unwrap()
                );
                sink += f64::from(likelihood[40]);
                sink += f64::from(doa.spectrum()[40]);
            }
        });
        assert!(sink.is_finite());
        assert!(report.peak_count >= 1, "{estimator:?}");
    }
}

#[test]
fn configure_and_retune_stay_in_place() {
    let manifold = kraken().unwrap();
    let r = covariance(&manifold, &[(Direction::horizon(137.0), 10.0)]).unwrap();
    let mut doa = Doa::new(&manifold, &DoaConfig::default(), FREQ).unwrap();
    let mut report = DoaReport::default();
    let capon = DoaConfig {
        estimator: Estimator::Capon,
        ..DoaConfig::default()
    };
    let mut changes = Vec::with_capacity(8);
    assert_no_alloc("doa configure and retune", || {
        changes.push(doa.configure(&manifold, &capon).unwrap());
        doa.estimate(&manifold, &r, 4096.0, 3.0, &mut report)
            .unwrap();
        changes.push(doa.configure(&manifold, &DoaConfig::default()).unwrap());
        doa.retune(&manifold, 430e6).unwrap();
        doa.retune(&manifold, FREQ).unwrap();
        doa.estimate(&manifold, &r, 4096.0, 3.0, &mut report)
            .unwrap();
    });
    assert!(changes.iter().all(|&change| change == Reconfigure::InPlace));
    assert!((report.peaks[0].azimuth_deg - 137.0).abs() < 0.5);
}

#[test]
fn elevation_and_forward_backward_do_not_allocate() {
    let geometry = Geometry::uca(0.3, 6, 0.0, Winding::Clockwise).unwrap();
    let mut positions: Vec<Vec3> = geometry.positions().to_vec();
    positions.push(Vec3::new(0.0, 0.0, 0.2));
    let raised = Manifold::ideal(Geometry::explicit(&positions).unwrap());
    let even = Manifold::ideal(geometry);
    let tilted = covariance(&raised, &[(Direction::new(60.0, 30.0), 10.0)]).unwrap();
    let flat = covariance(&even, &[(Direction::horizon(60.0), 10.0)]).unwrap();
    let elevation = DoaConfig {
        elevation: Some(ElevationSpan::default()),
        ..DoaConfig::default()
    };
    let fb = DoaConfig {
        forward_backward: true,
        ..DoaConfig::default()
    };
    let mut up = Doa::new(&raised, &elevation, FREQ).unwrap();
    let mut mirrored = Doa::new(&even, &fb, FREQ).unwrap();
    let mut report = DoaReport::default();
    let mut likelihood = [0.0f32; LIKELIHOOD_POINTS];
    let mut sink = 0.0f64;
    assert_no_alloc("doa elevation and fb", || {
        up.estimate(&raised, &tilted, 4096.0, 3.0, &mut report)
            .unwrap();
        sink += report.peaks[0].elevation_deg;
        assert!(
            up.likelihood(&raised, &tilted, &report, &mut likelihood)
                .unwrap()
        );
        mirrored
            .estimate(&even, &flat, 4096.0, 3.0, &mut report)
            .unwrap();
        sink += report.peaks[0].azimuth_deg;
    });
    assert!(sink.is_finite());
}
