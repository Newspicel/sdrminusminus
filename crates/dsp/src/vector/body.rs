use super::GemmShape;

const LANES: usize = 16;
const ROWS: usize = 4;
const WIDE: usize = 64;

#[inline(always)]
fn madd<const FMA: bool>(a: f32, b: f32, sum: f32) -> f32 {
    if FMA { a.mul_add(b, sum) } else { sum + a * b }
}

#[inline(always)]
pub(super) fn dot<const FMA: bool>(a: &[f32], b: &[f32]) -> f32 {
    let (a_blocks, a_rest) = a.as_chunks::<LANES>();
    let (b_blocks, b_rest) = b.as_chunks::<LANES>();
    let mut acc = [0.0f32; LANES];
    for (x, y) in a_blocks.iter().zip(b_blocks) {
        for lane in 0..LANES {
            acc[lane] = madd::<FMA>(x[lane], y[lane], acc[lane]);
        }
    }
    let tail = a_rest
        .iter()
        .zip(b_rest)
        .fold(0.0, |sum, (&x, &y)| madd::<FMA>(x, y, sum));
    acc.iter().sum::<f32>() + tail
}

#[inline(always)]
pub(super) fn axpy<const FMA: bool>(y: &mut [f32], alpha: f32, x: &[f32]) {
    for (out, &value) in y.iter_mut().zip(x) {
        *out = madd::<FMA>(alpha, value, *out);
    }
}

#[inline(always)]
pub(super) fn gemm<const FMA: bool>(shape: GemmShape, lhs: &[f32], rhs: &[f32], out: &mut [f32]) {
    let GemmShape { rows, cols, .. } = shape;
    let mut row = 0;
    while row + ROWS <= rows {
        let mut col = 0;
        while col + LANES <= cols {
            block::<FMA, ROWS, LANES>(shape, row, col, lhs, rhs, out);
            col += LANES;
        }
        tail::<FMA>(shape, row..row + ROWS, col, lhs, rhs, out);
        row += ROWS;
    }
    while row < rows {
        let mut col = 0;
        while col + WIDE <= cols {
            block::<FMA, 1, WIDE>(shape, row, col, lhs, rhs, out);
            col += WIDE;
        }
        while col + LANES <= cols {
            block::<FMA, 1, LANES>(shape, row, col, lhs, rhs, out);
            col += LANES;
        }
        tail::<FMA>(shape, row..row + 1, col, lhs, rhs, out);
        row += 1;
    }
}

fn tail<const FMA: bool>(
    shape: GemmShape,
    rows: std::ops::Range<usize>,
    from: usize,
    lhs: &[f32],
    rhs: &[f32],
    out: &mut [f32],
) {
    let GemmShape { depth, cols, .. } = shape;
    for r in rows {
        let line = &lhs[r * depth..(r + 1) * depth];
        for c in from..cols {
            out[r * cols + c] = line
                .iter()
                .enumerate()
                .fold(0.0, |sum, (k, &a)| madd::<FMA>(a, rhs[k * cols + c], sum));
        }
    }
}

#[inline(always)]
fn block<const FMA: bool, const R: usize, const W: usize>(
    shape: GemmShape,
    row: usize,
    col: usize,
    lhs: &[f32],
    rhs: &[f32],
    out: &mut [f32],
) {
    let GemmShape { depth, cols, .. } = shape;
    let mut acc = [[0.0f32; W]; R];
    let lines: [&[f32]; R] = std::array::from_fn(|i| &lhs[(row + i) * depth..(row + i + 1) * depth]);
    for k in 0..depth {
        let Some(b) = rhs[k * cols + col..].first_chunk::<W>() else {
            return;
        };
        for i in 0..R {
            let a = lines[i][k];
            for lane in 0..W {
                acc[i][lane] = madd::<FMA>(a, b[lane], acc[i][lane]);
            }
        }
    }
    for (i, sums) in acc.iter().enumerate() {
        let at = (row + i) * cols + col;
        out[at..at + W].copy_from_slice(sums);
    }
}

const TANH_CLAMP: f32 = 7.905_311;
const TANH_P: [f32; 7] = [
    -2.760_768_5e-16,
    2.000_188e-13,
    -8.604_672e-11,
    5.122_297e-8,
    1.485_722_4e-5,
    6.372_619_3e-4,
    4.893_524_6e-3,
];
const TANH_Q: [f32; 4] = [1.198_258_4e-6, 1.185_347e-4, 2.268_434_6e-3, 4.893_525e-3];

#[inline(always)]
fn tanh_one<const FMA: bool>(x: f32) -> f32 {
    let x = x.clamp(-TANH_CLAMP, TANH_CLAMP);
    let x2 = x * x;
    let p = TANH_P[1..]
        .iter()
        .fold(TANH_P[0], |acc, &c| madd::<FMA>(acc, x2, c));
    let q = TANH_Q[1..]
        .iter()
        .fold(TANH_Q[0], |acc, &c| madd::<FMA>(acc, x2, c));
    x * p / q
}

#[inline(always)]
pub(super) fn tanh<const FMA: bool>(values: &mut [f32]) {
    for value in values {
        *value = tanh_one::<FMA>(*value);
    }
}

#[inline(always)]
pub(super) fn sigmoid<const FMA: bool>(values: &mut [f32]) {
    for value in values {
        *value = madd::<FMA>(0.5, tanh_one::<FMA>(0.5 * *value), 0.5);
    }
}
