use num_complex::Complex;

use super::Nco;

const LANES: usize = 4;
pub(super) const GROUP: usize = 128;

type Block = [Complex<f32>; LANES];
type Quad = [f32; LANES];

#[derive(Clone)]
pub(super) struct Steps {
    re: [Quad; GROUP / LANES],
    im: [Quad; GROUP / LANES],
}

impl Steps {
    pub(super) fn new(step: u64) -> Self {
        let angle = std::f64::consts::TAU * step as i64 as f64 / super::PHASE_SCALE;
        let (sin, cos) = angle.sin_cos();
        let turn = Complex::new(cos, sin);
        let mut wave = Complex::new(1.0f64, 0.0);
        let mut steps = Self {
            re: [[0.0; LANES]; GROUP / LANES],
            im: [[0.0; LANES]; GROUP / LANES],
        };
        for lane in 0..GROUP {
            steps.re[lane / LANES][lane % LANES] = wave.re as f32;
            steps.im[lane / LANES][lane % LANES] = wave.im as f32;
            wave *= turn;
        }
        steps
    }

    #[inline(always)]
    pub(super) fn at(&self, lane: usize) -> Complex<f32> {
        Complex::new(
            self.re[lane / LANES][lane % LANES],
            self.im[lane / LANES][lane % LANES],
        )
    }
}

#[inline(always)]
pub(super) fn product(x: Complex<f32>, w: Complex<f32>) -> Complex<f32> {
    if cfg!(target_arch = "aarch64") {
        Complex::new(
            (-x.im).mul_add(w.im, x.re * w.re),
            x.im.mul_add(w.re, x.re * w.im),
        )
    } else {
        x * w
    }
}

fn lead(nco: &Nco, len: usize) -> usize {
    ((LANES - nco.lane % LANES) % LANES).min(len)
}

fn segments(
    nco: &mut Nco,
    blocks: usize,
    mut each: impl FnMut(usize, &[Quad], &[Quad], Complex<f32>),
) {
    let mut done = 0;
    while done < blocks {
        let first = nco.lane / LANES;
        let count = (GROUP / LANES - first).min(blocks - done);
        let span = first..first + count;
        each(
            done,
            &nco.steps.re[span.clone()],
            &nco.steps.im[span],
            nco.anchor,
        );
        nco.advance(count * LANES);
        done += count;
    }
}

pub(super) fn mix_into(nco: &mut Nco, input: &[Complex<f32>], out: &mut [Complex<f32>]) {
    let (head, input) = input.split_at(lead(nco, input.len()));
    let (head_out, out) = out.split_at_mut(head.len());
    for (target, source) in head_out.iter_mut().zip(head) {
        *target = product(*source, nco.wave());
    }
    let (sources, source_tail) = input.as_chunks::<LANES>();
    let (targets, target_tail) = out.as_chunks_mut::<LANES>();
    segments(nco, sources.len(), |at, re, im, anchor| {
        let sources = sources[at..].iter().zip(&mut targets[at..]);
        for ((source, target), (re, im)) in sources.zip(re.iter().zip(im)) {
            *target = rotate(anchor, re, im, source);
        }
    });
    for (target, source) in target_tail.iter_mut().zip(source_tail) {
        *target = product(*source, nco.wave());
    }
}

pub(super) fn mix(nco: &mut Nco, samples: &mut [Complex<f32>]) {
    let (head, samples) = samples.split_at_mut(lead(nco, samples.len()));
    for sample in head {
        *sample = product(*sample, nco.wave());
    }
    let (blocks, tail) = samples.as_chunks_mut::<LANES>();
    segments(nco, blocks.len(), |at, re, im, anchor| {
        for (block, (re, im)) in blocks[at..].iter_mut().zip(re.iter().zip(im)) {
            *block = rotate(anchor, re, im, block);
        }
    });
    for sample in tail {
        *sample = product(*sample, nco.wave());
    }
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn rotate(anchor: Complex<f32>, step_re: &Quad, step_im: &Quad, source: &Block) -> Block {
    use std::arch::aarch64::*;
    let mut target = [Complex::new(0.0, 0.0); LANES];
    unsafe {
        let step_re = vld1q_f32(step_re.as_ptr());
        let step_im = vld1q_f32(step_im.as_ptr());
        let wave_re = vfmsq_n_f32(vmulq_n_f32(step_re, anchor.re), step_im, anchor.im);
        let wave_im = vfmaq_n_f32(vmulq_n_f32(step_re, anchor.im), step_im, anchor.re);
        let x = vld2q_f32(source.as_ptr().cast());
        let re = vfmsq_f32(vmulq_f32(x.0, wave_re), x.1, wave_im);
        let im = vfmaq_f32(vmulq_f32(x.0, wave_im), x.1, wave_re);
        vst2q_f32(target.as_mut_ptr().cast(), float32x4x2_t(re, im));
    }
    target
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn rotate(anchor: Complex<f32>, step_re: &Quad, step_im: &Quad, source: &Block) -> Block {
    use std::arch::x86_64::*;
    let mut target = [Complex::new(0.0, 0.0); LANES];
    unsafe {
        let step_re = _mm_loadu_ps(step_re.as_ptr());
        let step_im = _mm_loadu_ps(step_im.as_ptr());
        let (anchor_re, anchor_im) = (_mm_set1_ps(anchor.re), _mm_set1_ps(anchor.im));
        let wave_re = _mm_sub_ps(
            _mm_mul_ps(step_re, anchor_re),
            _mm_mul_ps(step_im, anchor_im),
        );
        let wave_im = _mm_add_ps(
            _mm_mul_ps(step_im, anchor_re),
            _mm_mul_ps(step_re, anchor_im),
        );
        let front = _mm_loadu_ps(source.as_ptr().cast());
        let back = _mm_loadu_ps(source.as_ptr().add(2).cast());
        let x_re = _mm_shuffle_ps::<0x88>(front, back);
        let x_im = _mm_shuffle_ps::<0xDD>(front, back);
        let re = _mm_sub_ps(_mm_mul_ps(x_re, wave_re), _mm_mul_ps(x_im, wave_im));
        let im = _mm_add_ps(_mm_mul_ps(x_re, wave_im), _mm_mul_ps(x_im, wave_re));
        _mm_storeu_ps(target.as_mut_ptr().cast(), _mm_unpacklo_ps(re, im));
        _mm_storeu_ps(target.as_mut_ptr().add(2).cast(), _mm_unpackhi_ps(re, im));
    }
    target
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
#[inline(always)]
fn rotate(anchor: Complex<f32>, step_re: &Quad, step_im: &Quad, source: &Block) -> Block {
    std::array::from_fn(|lane| source[lane] * (anchor * Complex::new(step_re[lane], step_im[lane])))
}
