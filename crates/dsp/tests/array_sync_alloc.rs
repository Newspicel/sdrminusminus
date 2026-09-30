use num_complex::Complex;
use sdrmm_dsp::array_sync::{Boxcar, FastConvolver, design_correction};
use sdrmm_test_support::{CountingAlloc, assert_no_alloc};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

fn ramp(len: usize) -> Vec<Complex<f32>> {
    (0..len)
        .map(|n| Complex::from_polar(1.0, 0.013 * n as f32))
        .collect()
}

#[test]
fn convolver_push_does_not_allocate() {
    let mut convolver = FastConvolver::new(4_096, 129);
    let mut spectrum = Vec::new();
    design_correction(
        4_096,
        129,
        8.0,
        0.3,
        Complex::from_polar(0.8, 0.4),
        None,
        &mut spectrum,
    );
    assert_eq!(convolver.set_response(&spectrum), Ok(()));
    let input = ramp(16_384);
    let mut out = Vec::with_capacity(input.len() + convolver.hop());
    let mut failed = false;
    assert_no_alloc("convolver", || {
        for size in [1, 17, 3_968, 4_096, 16_384] {
            for chunk in input.chunks(size) {
                out.clear();
                convolver.push(chunk, &mut out);
            }
        }
        failed |= convolver.set_response(&spectrum).is_err();
        convolver.reset();
    });
    assert!(!failed);
}

#[test]
fn boxcar_push_does_not_allocate() {
    let mut boxcar = Boxcar::new(3, 64);
    let input = ramp(16_384);
    let lanes = [input.as_slice(), input.as_slice(), input.as_slice()];
    let mut out: Vec<Vec<Complex<f32>>> = (0..3)
        .map(|_| Vec::with_capacity(4 * input.len() / 64))
        .collect();
    let mut failed = false;
    assert_no_alloc("boxcar", || {
        for size in [1, 63, 1_000, 16_384] {
            for start in (0..input.len()).step_by(size) {
                let end = (start + size).min(input.len());
                let views = [
                    &lanes[0][start..end],
                    &lanes[1][start..end],
                    &lanes[2][start..end],
                ];
                failed |= boxcar.push(&views, &mut out).is_err();
            }
        }
        boxcar.reset();
    });
    assert!(!failed);
    assert!(out.iter().all(|lane| lane.len() == 4 * input.len() / 64));
}
