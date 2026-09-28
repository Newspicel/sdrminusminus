use num_complex::Complex;
use sdrmm_dsp::doa::{Doa, DoaConfig, DoaError, DoaReport, Estimator, Reconfigure};
use sdrmm_dsp::linalg::{CMat, LinalgError};
use sdrmm_dsp::manifold::{Direction, Geometry, Manifold, Winding, wavenumber};
use sdrmm_test_support::{CountingAlloc, assert_no_alloc};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

const FREQ: f64 = 433.92e6;

fn covariance(manifold: &Manifold, sources: &[(f64, f32)]) -> Result<CMat, LinalgError> {
    let n = manifold.len();
    let mut r = CMat::identity(n)?;
    let mut a = [Complex::new(0.0f32, 0.0); 16];
    for &(azimuth, power) in sources {
        manifold.steer(FREQ, Direction::horizon(azimuth), &mut a[..n]);
        for i in 0..n {
            for j in 0..n {
                r.add(i, j, a[i] * a[j].conj() * power);
            }
        }
    }
    Ok(r)
}

fn configs() -> [DoaConfig; 5] {
    let root = DoaConfig {
        estimator: Estimator::RootMusic,
        ..DoaConfig::default()
    };
    let esprit = DoaConfig {
        estimator: Estimator::Esprit,
        ..DoaConfig::default()
    };
    [
        root,
        esprit,
        DoaConfig {
            smoothing: 2,
            forward_backward: true,
            ..root
        },
        DoaConfig {
            smoothing: 1,
            ..esprit
        },
        DoaConfig {
            smoothing: 2,
            ..DoaConfig::default()
        },
    ]
}

fn cycle(
    doa: &mut Doa,
    manifold: &Manifold,
    r: &CMat,
    report: &mut DoaReport,
    changes: &mut Vec<Reconfigure>,
) -> Result<f64, DoaError> {
    let mut sink = 0.0;
    for config in &configs() {
        changes.push(doa.configure(manifold, config)?);
        doa.estimate(manifold, r, 4096.0, 3.0, report)?;
        sink += report.peaks[0].azimuth_deg;
    }
    Ok(sink)
}

fn run(manifold: &Manifold, r: &CMat, label: &str) -> Result<Vec<Reconfigure>, DoaError> {
    let mut doa = Doa::new(manifold, &configs()[0], FREQ)?;
    let mut report = DoaReport::default();
    let mut changes = Vec::with_capacity(16);
    cycle(&mut doa, manifold, r, &mut report, &mut changes)?;
    changes.clear();
    let mut outcome = Ok(0.0);
    assert_no_alloc(label, || {
        outcome = cycle(&mut doa, manifold, r, &mut report, &mut changes);
    });
    outcome?;
    Ok(changes)
}

#[test]
fn root_music_and_esprit_on_a_line_do_not_allocate() {
    let manifold = Manifold::ideal(Geometry::ula(0.3, 8, 180.0).unwrap());
    let r = covariance(&manifold, &[(60.0, 10.0), (110.0, 5.0)]).unwrap();
    let changes = run(&manifold, &r, "subspace on a line").unwrap();
    assert!(changes.iter().all(|&change| change == Reconfigure::InPlace));
}

#[test]
fn root_music_and_esprit_on_a_circle_do_not_allocate() {
    let radius = 2.0 / wavenumber(FREQ);
    let manifold = Manifold::ideal(Geometry::uca(radius, 8, 0.0, Winding::Clockwise).unwrap());
    let r = covariance(&manifold, &[(40.0, 10.0), (150.0, 5.0)]).unwrap();
    let changes = run(&manifold, &r, "subspace on a circle").unwrap();
    assert!(changes.iter().all(|&change| change == Reconfigure::InPlace));
}
