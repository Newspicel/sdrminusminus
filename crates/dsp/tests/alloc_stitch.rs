use num_complex::Complex;
use sdrmm_dsp::stitch::{STITCH_FFT, StitchOptions, Stitcher, auto_offsets};
use sdrmm_test_support::{CountingAlloc, assert_no_alloc};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

const LANES: usize = 3;
const RATE: f64 = 2_048_000.0;
const LEN: usize = 40_000;
const BLOCK: usize = 3_001;

fn noise(len: usize, seed: u32) -> Vec<Complex<f32>> {
    let mut state = seed;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state as f32 / u32::MAX as f32 - 0.5
    };
    (0..len).map(|_| Complex::new(next(), next())).collect()
}

#[test]
fn stitch_steady_state_does_not_allocate() {
    let lanes: Vec<Vec<Complex<f32>>> =
        (0..LANES).map(|lane| noise(LEN, 3 + lane as u32)).collect();
    let mut stitcher = Stitcher::new(LANES, RATE).expect("stitcher");
    let mut out = Vec::with_capacity(LANES * (LEN + STITCH_FFT));
    for _ in 0..4 {
        out.clear();
        let views = [&lanes[0][..], &lanes[1][..], &lanes[2][..]];
        stitcher.process(&views, &mut out).expect("lanes");
    }
    let layout = auto_offsets(LANES, RATE);
    let spread = [layout[0] * 1.01, layout[1], layout[2] * 1.01];
    assert_no_alloc("stitch", || {
        for _ in 0..4 {
            out.clear();
            for start in (0..LEN).step_by(BLOCK) {
                let end = (start + BLOCK).min(LEN);
                let views = [
                    &lanes[0][start..end],
                    &lanes[1][start..end],
                    &lanes[2][start..end],
                ];
                stitcher.process(&views, &mut out).expect("lanes");
            }
        }
        stitcher.retune(&spread).expect("spread");
        stitcher.retune(&layout).expect("layout");
        stitcher.set_options(StitchOptions {
            snr_blend: false,
            ..StitchOptions::default()
        });
        stitcher.set_options(StitchOptions::default());
        stitcher.reset();
        std::hint::black_box(stitcher.lane_state(1));
    });
}
