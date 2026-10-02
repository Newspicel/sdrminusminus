use sdrmm_dsp::vector::{GemmShape, Vector};

use super::{
    format::{Binary, Unary},
    walk::Walk,
};

pub(super) fn unary(vector: Vector, kind: Unary, input: &[f32], out: &mut [f32]) {
    let pairs = out.iter_mut().zip(input);
    match kind {
        Unary::Sqrt => pairs.for_each(|(o, &x)| *o = x.sqrt()),
        Unary::Rsqrt => pairs.for_each(|(o, &x)| *o = 1.0 / x.sqrt()),
        Unary::Square => pairs.for_each(|(o, &x)| *o = x * x),
        Unary::Ln => pairs.for_each(|(o, &x)| *o = x.ln()),
        Unary::Sigmoid => {
            out.copy_from_slice(input);
            vector.sigmoid(out);
        }
        Unary::Tanh => {
            out.copy_from_slice(input);
            vector.tanh(out);
        }
    }
}

pub(super) fn binary(kind: Binary, walk: &Walk<3>, a: &[f32], b: &[f32], out: &mut [f32]) {
    match kind {
        Binary::Add => broadcast(walk, a, b, out, |x, y| x + y),
        Binary::Sub => broadcast(walk, a, b, out, |x, y| x - y),
        Binary::Mul => broadcast(walk, a, b, out, |x, y| x * y),
        Binary::Max => broadcast(walk, a, b, out, f32::max),
        Binary::Min => broadcast(walk, a, b, out, f32::min),
    }
}

fn broadcast(
    walk: &Walk<3>,
    a: &[f32],
    b: &[f32],
    out: &mut [f32],
    f: impl Fn(f32, f32) -> f32 + Copy,
) {
    let len = walk.len;
    let [sa, sb, _] = walk.inner;
    walk.rows(|[ia, ib, io]| {
        let row = &mut out[io..io + len];
        match (sa, sb) {
            (1, 1) => {
                for ((o, &x), &y) in row.iter_mut().zip(&a[ia..ia + len]).zip(&b[ib..ib + len]) {
                    *o = f(x, y);
                }
            }
            (1, 0) => {
                let y = b[ib];
                for (o, &x) in row.iter_mut().zip(&a[ia..ia + len]) {
                    *o = f(x, y);
                }
            }
            (0, 1) => {
                let x = a[ia];
                for (o, &y) in row.iter_mut().zip(&b[ib..ib + len]) {
                    *o = f(x, y);
                }
            }
            _ => {
                for (i, o) in row.iter_mut().enumerate() {
                    *o = f(a[ia + i * sa], b[ib + i * sb]);
                }
            }
        }
    });
}

pub(super) fn sum_reduce(walk: &Walk<2>, input: &[f32], out: &mut [f32]) {
    out.fill(0.0);
    let len = walk.len;
    let [si, so] = walk.inner;
    walk.rows(|[ii, io]| {
        if so == 0 {
            out[io] += (0..len).map(|i| input[ii + i * si]).sum::<f32>();
        } else {
            for i in 0..len {
                out[io + i * so] += input[ii + i * si];
            }
        }
    });
}

pub(super) fn copy(walk: &Walk<2>, src: &[f32], src_base: usize, dst: &mut [f32], dst_base: usize) {
    let len = walk.len;
    let [ss, sd] = walk.inner;
    walk.rows(|[is, id]| {
        let (is, id) = (src_base + is, dst_base + id);
        if ss == 1 && sd == 1 {
            dst[id..id + len].copy_from_slice(&src[is..is + len]);
        } else {
            for i in 0..len {
                dst[id + i * sd] = src[is + i * ss];
            }
        }
    });
}

pub(super) struct Group {
    pub(super) len: usize,
    pub(super) a: Vec<usize>,
    pub(super) b: Vec<usize>,
    pub(super) c: Vec<usize>,
}

pub(super) struct Operand {
    pub(super) packed: Option<Vec<f32>>,
    pub(super) direct: bool,
    pub(super) batch: Vec<usize>,
    pub(super) outer: Vec<usize>,
    pub(super) inner: Vec<usize>,
}

impl Operand {
    fn len(&self) -> usize {
        self.batch.len() * self.outer.len() * self.inner.len()
    }

    pub(super) fn scratch_len(&self) -> usize {
        if self.packed.is_some() || self.direct {
            0
        } else {
            self.len()
        }
    }

    pub(super) fn pack(&self, src: &[f32]) -> Vec<f32> {
        let mut out = vec![0.0; self.len()];
        self.pack_into(src, &mut out);
        out
    }

    fn pack_into(&self, src: &[f32], out: &mut [f32]) {
        let mut at = 0;
        for &b in &self.batch {
            for &o in &self.outer {
                for &i in &self.inner {
                    out[at] = src[b + o + i];
                    at += 1;
                }
            }
        }
    }

    fn view<'a>(&'a self, src: &'a [f32], scratch: &'a mut [f32]) -> &'a [f32] {
        match &self.packed {
            Some(packed) => packed,
            None if self.direct => src,
            None => {
                self.pack_into(src, scratch);
                scratch
            }
        }
    }
}

pub(super) struct EinSum {
    pub(super) shape: GemmShape,
    pub(super) batch: usize,
    pub(super) lhs: Operand,
    pub(super) rhs: Operand,
    pub(super) swapped: bool,
    pub(super) out_direct: bool,
    pub(super) out_batch: Vec<usize>,
    pub(super) out_rows: Vec<usize>,
    pub(super) out_cols: Vec<usize>,
}

impl EinSum {
    pub(super) fn scratch_len(&self) -> usize {
        let out = if self.out_direct {
            0
        } else {
            self.batch * self.shape.rows * self.shape.cols
        };
        self.lhs.scratch_len() + self.rhs.scratch_len() + out
    }

    pub(super) fn run(
        &self,
        vector: Vector,
        a: &[f32],
        b: &[f32],
        c: &mut [f32],
        scratch: &mut [f32],
    ) {
        let (lhs_src, rhs_src) = if self.swapped { (b, a) } else { (a, b) };
        let (lhs_scratch, rest) = scratch.split_at_mut(self.lhs.scratch_len());
        let (rhs_scratch, out_scratch) = rest.split_at_mut(self.rhs.scratch_len());
        let lhs = self.lhs.view(lhs_src, lhs_scratch);
        let rhs = self.rhs.view(rhs_src, rhs_scratch);
        let GemmShape { rows, depth, cols } = self.shape;
        let target: &mut [f32] = if self.out_direct { c } else { out_scratch };
        for batch in 0..self.batch {
            vector.gemm(
                self.shape,
                &lhs[batch * rows * depth..],
                &rhs[batch * depth * cols..],
                &mut target[batch * rows * cols..],
            );
        }
        if !self.out_direct {
            let mut at = 0;
            for &ob in &self.out_batch {
                for &or in &self.out_rows {
                    for &oc in &self.out_cols {
                        c[ob + or + oc] = out_scratch[at];
                        at += 1;
                    }
                }
            }
        }
    }
}

pub(super) struct Conv {
    pub(super) group: usize,
    pub(super) in_per_group: usize,
    pub(super) out_per_group: usize,
    pub(super) in_channel_stride: usize,
    pub(super) out_channel_stride: usize,
    pub(super) in_size: [usize; 2],
    pub(super) in_strides: [usize; 2],
    pub(super) out_size: [usize; 2],
    pub(super) out_strides: [usize; 2],
    pub(super) kernel: [usize; 2],
    pub(super) stride: [usize; 2],
    pub(super) dilation: [usize; 2],
    pub(super) pad: [usize; 2],
    pub(super) bias_per_channel: bool,
}

impl Conv {
    fn taps(&self) -> usize {
        self.in_per_group * self.kernel[0] * self.kernel[1]
    }

    fn points(&self) -> usize {
        self.out_size[0] * self.out_size[1]
    }

    pub(super) fn scratch_len(&self) -> usize {
        (self.taps() + self.out_per_group) * self.points()
    }

    pub(super) fn run(
        &self,
        vector: Vector,
        x: &[f32],
        weights: &[f32],
        bias: &[f32],
        out: &mut [f32],
        scratch: &mut [f32],
    ) {
        let (taps, points) = (self.taps(), self.points());
        let (columns, product) = scratch.split_at_mut(taps * points);
        for g in 0..self.group {
            self.unfold(g, x, columns);
            let first = g * self.out_per_group;
            vector.gemm(
                GemmShape {
                    rows: self.out_per_group,
                    depth: taps,
                    cols: points,
                },
                &weights[first * taps..],
                columns,
                &mut product[..],
            );
            let product = &product[..self.out_per_group * points];
            for (row, sums) in product.chunks_exact(points).enumerate() {
                let oc = first + row;
                let offset = match (self.bias_per_channel, bias.first()) {
                    (true, _) => bias[oc],
                    (false, Some(&b)) => b,
                    (false, None) => 0.0,
                };
                for (point, &sum) in sums.iter().enumerate() {
                    let (oy, ox) = (point / self.out_size[1], point % self.out_size[1]);
                    out[oc * self.out_channel_stride
                        + oy * self.out_strides[0]
                        + ox * self.out_strides[1]] = sum + offset;
                }
            }
        }
    }

    fn unfold(&self, g: usize, x: &[f32], columns: &mut [f32]) {
        let [kh, kw] = self.kernel;
        let mut at = 0;
        for ic in 0..self.in_per_group {
            let channel = (g * self.in_per_group + ic) * self.in_channel_stride;
            for ky in 0..kh {
                for kx in 0..kw {
                    for oy in 0..self.out_size[0] {
                        for ox in 0..self.out_size[1] {
                            columns[at] = match (self.source(0, oy, ky), self.source(1, ox, kx)) {
                                (Some(iy), Some(ix)) => {
                                    x[channel + iy * self.in_strides[0] + ix * self.in_strides[1]]
                                }
                                _ => 0.0,
                            };
                            at += 1;
                        }
                    }
                }
            }
        }
    }

    #[inline]
    fn source(&self, axis: usize, out: usize, tap: usize) -> Option<usize> {
        (out * self.stride[axis] + tap * self.dilation[axis])
            .checked_sub(self.pad[axis])
            .filter(|&at| at < self.in_size[axis])
    }
}

pub(super) struct Gru {
    pub(super) hidden: usize,
    pub(super) backward: bool,
    pub(super) batch: usize,
    pub(super) steps: usize,
    pub(super) input: usize,
    pub(super) input_weights: Vec<f32>,
    pub(super) recurrent_weights: Vec<f32>,
}

impl Gru {
    pub(super) fn scratch_len(&self) -> usize {
        self.steps * 3 * self.hidden + 7 * self.hidden
    }

    #[expect(clippy::too_many_arguments)]
    pub(super) fn run(
        &self,
        vector: Vector,
        x: &[f32],
        bias: &[f32],
        h0: &[f32],
        scratch: &mut [f32],
        y: &mut [f32],
        last: &mut [f32],
    ) {
        let h = self.hidden;
        let (projected, rest) = scratch.split_at_mut(self.steps * 3 * h);
        let (recurrent, rest) = rest.split_at_mut(3 * h);
        let (pre, rest) = rest.split_at_mut(3 * h);
        let state = &mut rest[..h];
        let (input_bias, recurrent_bias) = bias.split_at(3 * h);
        for batch in 0..self.batch {
            vector.gemm(
                GemmShape {
                    rows: self.steps,
                    depth: self.input,
                    cols: 3 * h,
                },
                &x[batch * self.steps * self.input..],
                &self.input_weights,
                projected,
            );
            for row in projected.chunks_exact_mut(3 * h) {
                vector.axpy(row, 1.0, input_bias);
            }
            state.copy_from_slice(&h0[batch * h..(batch + 1) * h]);
            for step in 0..self.steps {
                let t = if self.backward {
                    self.steps - 1 - step
                } else {
                    step
                };
                vector.gemm(
                    GemmShape {
                        rows: 1,
                        depth: h,
                        cols: 3 * h,
                    },
                    state,
                    &self.recurrent_weights,
                    recurrent,
                );
                vector.axpy(recurrent, 1.0, recurrent_bias);
                let gates = &projected[t * 3 * h..(t + 1) * 3 * h];
                for ((p, &g), &r) in pre[..2 * h].iter_mut().zip(gates).zip(&*recurrent) {
                    *p = g + r;
                }
                vector.sigmoid(&mut pre[..2 * h]);
                let (gate, candidate) = pre.split_at_mut(2 * h);
                for j in 0..h {
                    candidate[j] = gates[2 * h + j] + gate[h + j] * recurrent[2 * h + j];
                }
                vector.tanh(candidate);
                for j in 0..h {
                    state[j] = candidate[j] + gate[j] * (state[j] - candidate[j]);
                }
                let out = (batch * self.steps + t) * h;
                y[out..out + h].copy_from_slice(state);
            }
            last[batch * h..(batch + 1) * h].copy_from_slice(state);
        }
    }
}

pub(super) struct Split {
    pub(super) outer: usize,
    pub(super) len: usize,
    pub(super) inner: usize,
}

pub(super) fn rms_norm(split: &Split, eps: f32, input: &[f32], out: &mut [f32]) {
    let Split { outer, len, inner } = *split;
    for o in 0..outer {
        for i in 0..inner {
            let base = o * len * inner + i;
            let mean = (0..len)
                .map(|j| input[base + j * inner].powi(2))
                .sum::<f32>()
                / len as f32;
            let scale = 1.0 / (mean + eps).sqrt();
            for j in 0..len {
                out[base + j * inner] = input[base + j * inner] * scale;
            }
        }
    }
}

pub(super) fn gather(split: &Split, indices: &[usize], input: &[f32], out: &mut [f32]) {
    let Split { outer, len, inner } = *split;
    let mut at = 0;
    for o in 0..outer {
        for &index in indices {
            let src = (o * len + index) * inner;
            out[at..at + inner].copy_from_slice(&input[src..src + inner]);
            at += inner;
        }
    }
}

pub(super) fn gather_by_table(table: &[usize], input: &[f32], out: &mut [f32]) {
    for (o, &src) in out.iter_mut().zip(table) {
        *o = input[src];
    }
}
