use std::sync::Arc;

use num_complex::Complex;
use sdrmm_dsp::manifold::{
    Direction, Geometry, GridSpec, Manifold, ManifoldError, ManifoldTable, SteeringGrid, Winding,
    alias_check, steer,
};
use sdrmm_test_support::{CountingAlloc, assert_no_alloc};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

fn kraken() -> Result<Geometry, ManifoldError> {
    Geometry::uca(0.35, 5, 0.0, Winding::Clockwise)
}

fn table(geometry: &Geometry) -> Result<ManifoldTable, ManifoldError> {
    let freqs = [420e6, 440e6];
    let mut data = vec![Complex::new(0.0f32, 0.0); freqs.len() * 72 * geometry.len()];
    for (row, chunk) in data.chunks_exact_mut(geometry.len()).enumerate() {
        let direction = Direction::horizon((row % 72) as f64 * 5.0);
        steer(geometry.positions(), freqs[row / 72], direction, chunk);
    }
    ManifoldTable::new(geometry.len(), freqs.to_vec(), 5.0, vec![0.0], data)
}

#[test]
fn grid_rebuild_does_not_allocate() {
    let geometry = kraken().unwrap();
    let ideal = Manifold::ideal(geometry.clone());
    let measured =
        Manifold::measured(geometry.clone(), Arc::new(table(&geometry).unwrap())).unwrap();
    let mut grid = SteeringGrid::new(&ideal, GridSpec::ring(1.0), 433.92e6).unwrap();
    let line = Geometry::ula(0.35, 4, 90.0).unwrap();
    let line_ring = SteeringGrid::new(
        &Manifold::ideal(line.clone()),
        GridSpec::ring(1.0),
        433.92e6,
    )
    .unwrap();
    let mut steering = [Complex::new(0.0f32, 0.0); 5];
    let mut rates = [0.0f64; 5];
    let mut sink = 0.0f64;
    assert_no_alloc("grid rebuild and steering", || {
        grid.rebuild(&ideal, 144e6).unwrap();
        grid.rebuild(&measured, 430e6).unwrap();
        grid.rebuild(&ideal, 433.92e6).unwrap();
        let report = alias_check(&grid, &geometry, 433.92e6);
        sink += f64::from(report.ambiguity);
        sink += f64::from(alias_check(&line_ring, &line, 433.92e6).ambiguity);
        measured.steer(430e6, Direction::new(33.3, 10.0), &mut steering);
        measured.phase_rates(430e6, Direction::horizon(33.3), &mut rates);
        sink += rates[1];
        measured.elevation_rates(430e6, Direction::horizon(33.3), &mut rates);
        ideal.phase_rates(430e6, Direction::horizon(33.3), &mut rates);
        sink += rates[2] + f64::from(steering[3].re);
        sink += geometry.antipodes().map_or(0, |pairs| pairs.len()) as f64;
    });
    assert!(sink.is_finite());
    assert_eq!(grid.points(), 360);
}
