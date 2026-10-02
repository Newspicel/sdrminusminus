pub(super) const MAX_RANK: usize = 8;

#[must_use]
pub(super) fn contiguous(shape: &[usize]) -> Vec<usize> {
    let mut strides = vec![1; shape.len()];
    for axis in (0..shape.len().saturating_sub(1)).rev() {
        strides[axis] = strides[axis + 1] * shape[axis + 1];
    }
    strides
}

#[derive(Clone, Debug)]
pub(super) struct Walk<const K: usize> {
    outer: Vec<usize>,
    outer_strides: [Vec<usize>; K],
    pub(super) len: usize,
    pub(super) inner: [usize; K],
}

impl<const K: usize> Walk<K> {
    pub(super) fn new(shape: &[usize], strides: [&[usize]; K]) -> Self {
        let mut dims: Vec<(usize, [usize; K])> = shape
            .iter()
            .enumerate()
            .filter(|&(_, &len)| len != 1)
            .map(|(axis, &len)| (len, strides.map(|s| s[axis])))
            .collect();
        let mut merged: Vec<(usize, [usize; K])> = Vec::with_capacity(dims.len());
        for (len, step) in dims.drain(..) {
            match merged.last_mut() {
                Some((last_len, last_step))
                    if (0..K).all(|k| last_step[k] == step[k] * len) =>
                {
                    *last_len *= len;
                    *last_step = step;
                }
                _ => merged.push((len, step)),
            }
        }
        let (len, inner) = merged.pop().unwrap_or((1, [0; K]));
        let outer = merged.iter().map(|&(len, _)| len).collect();
        let outer_strides = std::array::from_fn(|k| merged.iter().map(|(_, s)| s[k]).collect());
        Self {
            outer,
            outer_strides,
            len,
            inner,
        }
    }

    pub(super) fn rows(&self, mut row: impl FnMut([usize; K])) {
        let mut index = [0usize; MAX_RANK];
        let mut offsets = [0usize; K];
        loop {
            row(offsets);
            let mut axis = self.outer.len();
            loop {
                if axis == 0 {
                    return;
                }
                axis -= 1;
                index[axis] += 1;
                for (k, offset) in offsets.iter_mut().enumerate() {
                    *offset += self.outer_strides[k][axis];
                }
                if index[axis] < self.outer[axis] {
                    break;
                }
                for (k, offset) in offsets.iter_mut().enumerate() {
                    *offset -= self.outer_strides[k][axis] * self.outer[axis];
                }
                index[axis] = 0;
            }
        }
    }
}

#[must_use]
pub(super) fn offsets(shape: &[usize], strides: &[usize]) -> Vec<usize> {
    let mut table = vec![0];
    for (&len, &stride) in shape.iter().zip(strides) {
        table = table
            .iter()
            .flat_map(|&base| (0..len).map(move |i| base + i * stride))
            .collect();
    }
    table
}
