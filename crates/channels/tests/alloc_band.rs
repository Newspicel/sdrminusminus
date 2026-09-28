use num_complex::Complex;
use sdrmm_channels::array_processor::{ArrayBlock, CalView, CorrectionView, MAX_LANES, Pose};
use sdrmm_channels::band::LaneBand;
use sdrmm_test_support::{CountingAlloc, assert_no_alloc};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

const LANES: usize = 4;
const RATE: f64 = 2_400_000.0;
const MAX_BLOCK: usize = 16_384;

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

fn block<'a>(
    lanes: &'a [&'a [Complex<f32>]],
    corrected: bool,
    correction: CorrectionView<'a>,
) -> ArrayBlock<'a> {
    ArrayBlock {
        lanes,
        corrected,
        correction,
        first_index: 0,
        unix_ns: 0,
        generation: correction.generation,
        gap_before: false,
        centers_hz: &[],
        cal: CalView::default(),
        pose: Pose::default(),
    }
}

fn run(
    band: &mut LaneBand,
    lanes: &[&[Complex<f32>]],
    corrected: bool,
    correction: CorrectionView<'_>,
) -> usize {
    let mut views: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
    band.process(&block(lanes, corrected, correction), &mut views)
}

#[test]
fn lane_band_views_do_not_allocate() {
    let owned: Vec<_> = (0..LANES)
        .map(|lane| noise(MAX_BLOCK, lane as u32 + 3))
        .collect();
    let full: Vec<&[Complex<f32>]> = owned.iter().map(Vec::as_slice).collect();
    let short: Vec<&[Complex<f32>]> = owned.iter().map(|lane| &lane[..3_001]).collect();
    let spectra: Vec<Vec<Complex<f32>>> = (0..LANES)
        .map(|lane| vec![Complex::from_polar(1.0, lane as f32 * 0.3); 4_096])
        .collect();
    let first = CorrectionView::new(1, RATE, &spectra);
    let second = CorrectionView::new(2, RATE, &spectra);
    let mut narrow =
        LaneBand::new(LANES, RATE, 150_000.0, Some(20_000.0), MAX_BLOCK).expect("narrow band");
    let mut wide = LaneBand::new(LANES, RATE, 0.0, None, MAX_BLOCK).expect("wide band");
    run(&mut narrow, &full, false, first);
    assert_no_alloc("lane band", || {
        for _ in 0..4 {
            std::hint::black_box(run(&mut narrow, &full, false, first));
            std::hint::black_box(run(&mut narrow, &short, false, second));
            std::hint::black_box(run(&mut narrow, &full, true, second));
            std::hint::black_box(run(&mut wide, &full, true, first));
            std::hint::black_box(run(&mut wide, &short, false, second));
        }
        narrow.set_offset(-200_000.0);
        std::hint::black_box(run(&mut narrow, &full, false, second));
        narrow.reset();
        wide.reset();
    });
}
