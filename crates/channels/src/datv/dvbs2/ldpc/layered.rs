use super::{
    GROUP,
    lanes::{Kernel, STRIDE},
    layout::{Edge, Layout},
};

const TARGET: f32 = 1_024.0;
const CEILING: f32 = i16::MAX as f32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Sweep {
    pub unsatisfied: u32,
    pub flipped: bool,
}

impl Sweep {
    pub(super) const fn settled(self) -> bool {
        self.unsatisfied == 0 && !self.flipped
    }
}

pub(super) struct Layered {
    pub layout: Layout,
    pub kernel: Kernel,
    pub totals: Vec<i16>,
    messages: Vec<i16>,
    gathered: Vec<i16>,
    extrinsic: Vec<i16>,
}

impl Layered {
    pub(super) fn new(layout: Layout, length: usize) -> Self {
        let scratch = layout.degree * STRIDE;
        Self {
            messages: vec![0; layout.edges.len() * STRIDE],
            gathered: vec![0; scratch],
            extrinsic: vec![0; scratch],
            totals: vec![0; length],
            kernel: Kernel::detect(),
            layout,
        }
    }

    pub(super) fn load(&mut self, llrs: &[f32]) {
        let scale = scale(llrs);
        for (bit, &llr) in llrs.iter().enumerate() {
            self.totals[self.layout.position(bit)] = quantize(llr, scale);
        }
        self.messages.fill(0);
    }

    pub(super) fn run(&mut self, limit: usize) -> Option<usize> {
        (0..=limit).find(|_| self.sweep().settled())
    }

    fn sweep(&mut self) -> Sweep {
        let mut sweep = Sweep::default();
        for layer in 0..self.layout.layers {
            let outcome = self.update(layer);
            sweep.unsatisfied += outcome.unsatisfied;
            sweep.flipped |= outcome.flipped;
        }
        sweep
    }

    fn update(&mut self, layer: usize) -> Sweep {
        let edges = self.layout.layer(layer);
        let rows = edges.len() * STRIDE;
        let first = self.layout.bounds[layer] * STRIDE;
        let messages = &mut self.messages[first..first + rows];
        for (edge, row) in edges.iter().zip(self.gathered.as_chunks_mut::<STRIDE>().0) {
            gather(&self.totals, *edge, row);
        }
        let unsatisfied = self.kernel.update(
            &mut self.gathered[..rows],
            &mut self.extrinsic[..rows],
            messages,
        );
        let mut flips = 0i16;
        for ((edge, delta), message) in edges
            .iter()
            .zip(self.gathered.as_chunks::<STRIDE>().0)
            .zip(messages.as_chunks_mut::<STRIDE>().0)
        {
            flips |= scatter(&mut self.totals, *edge, delta, message);
        }
        Sweep {
            unsatisfied,
            flipped: flips < 0,
        }
    }

    pub(super) fn harden(&self, hard: &mut [bool]) {
        for (bit, &total) in hard.iter_mut().zip(&self.totals) {
            *bit = total < 0;
        }
    }
}

fn scale(llrs: &[f32]) -> f32 {
    let sum: f32 = llrs.iter().map(|llr| llr.abs().min(CEILING)).sum();
    let mean = sum / llrs.len().max(1) as f32;
    if mean.is_normal() { TARGET / mean } else { 1.0 }
}

fn quantize(llr: f32, scale: f32) -> i16 {
    (llr * scale).round().clamp(-CEILING, CEILING) as i16
}

fn gather(totals: &[i16], edge: Edge, row: &mut [i16; STRIDE]) {
    let split = GROUP - edge.shift;
    let source = &totals[edge.base..edge.base + GROUP];
    if edge.open {
        row[..split].fill(i16::MAX);
    } else {
        row[..split].copy_from_slice(&source[edge.shift..]);
    }
    row[split..GROUP].copy_from_slice(&source[..edge.shift]);
    row[GROUP..].fill(i16::MAX);
}

fn scatter(
    totals: &mut [i16],
    edge: Edge,
    delta: &[i16; STRIDE],
    message: &mut [i16; STRIDE],
) -> i16 {
    let split = GROUP - edge.shift;
    let target = &mut totals[edge.base..edge.base + GROUP];
    let (head, tail) = target.split_at_mut(edge.shift);
    let flips = if edge.open {
        message[..split].fill(0);
        0
    } else {
        accumulate(tail, &delta[..split])
    };
    flips | accumulate(head, &delta[split..GROUP])
}

fn accumulate(target: &mut [i16], delta: &[i16]) -> i16 {
    let mut flips = 0i16;
    for (total, &change) in target.iter_mut().zip(delta) {
        let updated = total.saturating_add(change);
        flips |= *total ^ updated;
        *total = updated;
    }
    flips
}
