mod format;
mod kernels;
mod plan;
mod walk;

#[cfg(test)]
mod tests;

use std::sync::Arc;

pub use format::{
    Binary, ConvSpec, Graph, Node, Op, Unary, Value, Weights, f16_to_f32, f32_to_f16,
};

use plan::{Kernel, Slot, Step};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NetError {
    #[error("bad network file: {0}")]
    Format(String),
    #[error("unsupported network: {0}")]
    Unsupported(String),
}

pub struct Net {
    vector: sdrmm_dsp::vector::Vector,
    meta: Vec<(String, String)>,
    consts: Vec<Vec<f32>>,
    buffers: Vec<usize>,
    steps: Vec<Step>,
    inputs: Vec<usize>,
    outputs: Vec<Slot>,
    scratch: usize,
}

impl Net {
    pub fn load(bytes: &[u8]) -> Result<Self, NetError> {
        plan::compile(&Graph::decode(bytes)?)
    }

    pub fn from_graph(graph: &Graph) -> Result<Self, NetError> {
        plan::compile(graph)
    }

    #[must_use]
    pub fn meta(&self, key: &str) -> Option<&str> {
        self.meta
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    #[must_use]
    pub fn input_len(&self, index: usize) -> Option<usize> {
        self.inputs.get(index).map(|&buf| self.buffers[buf])
    }

    #[must_use]
    pub fn output_len(&self, index: usize) -> Option<usize> {
        self.outputs.get(index).map(|slot| match *slot {
            Slot::Const(id) => self.consts[id].len(),
            Slot::Buf(id) => self.buffers[id],
        })
    }
}

pub struct Session {
    net: Arc<Net>,
    buffers: Vec<Vec<f32>>,
    scratch: Vec<f32>,
}

impl Session {
    #[must_use]
    pub fn new(net: Arc<Net>) -> Self {
        let buffers = net.buffers.iter().map(|&len| vec![0.0; len]).collect();
        let scratch = vec![0.0; net.scratch];
        Self {
            net,
            buffers,
            scratch,
        }
    }

    #[must_use]
    pub fn net(&self) -> &Arc<Net> {
        &self.net
    }

    pub fn input_mut(&mut self, index: usize) -> &mut [f32] {
        match self.net.inputs.get(index) {
            Some(&buf) => &mut self.buffers[buf],
            None => &mut [],
        }
    }

    #[must_use]
    pub fn output(&self, index: usize) -> &[f32] {
        match self.net.outputs.get(index) {
            Some(&Slot::Const(id)) => &self.net.consts[id],
            Some(&Slot::Buf(id)) => &self.buffers[id],
            None => &[],
        }
    }

    pub fn carry(&mut self, output: usize, input: usize) {
        let (Some(&source), Some(&target)) =
            (self.net.outputs.get(output), self.net.inputs.get(input))
        else {
            return;
        };
        let mut into = std::mem::take(&mut self.buffers[target]);
        let from = match source {
            Slot::Const(id) => self.net.consts[id].as_slice(),
            Slot::Buf(id) => self.buffers[id].as_slice(),
        };
        for (slot, &value) in into.iter_mut().zip(from) {
            *slot = value;
        }
        self.buffers[target] = into;
    }

    pub fn run(&mut self) {
        let net = Arc::clone(&self.net);
        for step in &net.steps {
            self.step(&net, step);
        }
    }

    fn step(&mut self, net: &Net, step: &Step) {
        let mut first = std::mem::take(&mut self.buffers[step.outputs[0]]);
        let mut second = step
            .outputs
            .get(1)
            .map(|&id| std::mem::take(&mut self.buffers[id]))
            .unwrap_or_default();
        let read = |index: usize| match step.inputs[index] {
            Slot::Const(id) => net.consts[id].as_slice(),
            Slot::Buf(id) => self.buffers[id].as_slice(),
        };
        match &step.kernel {
            Kernel::Unary(kind) => kernels::unary(net.vector, *kind, read(0), &mut first),
            Kernel::Binary(kind, walk) => {
                kernels::binary(*kind, walk, read(0), read(1), &mut first)
            }
            Kernel::SumReduce(walk) => kernels::sum_reduce(walk, read(0), &mut first),
            Kernel::Copy(parts) => {
                for (index, (walk, src_base, dst_base)) in parts.iter().enumerate() {
                    kernels::copy(walk, read(index), *src_base, &mut first, *dst_base);
                }
            }
            Kernel::EinSum(einsum) => {
                einsum.run(net.vector, read(0), read(1), &mut first, &mut self.scratch);
            }
            Kernel::Conv(conv) => {
                conv.run(
                    net.vector,
                    read(0),
                    read(1),
                    read(2),
                    &mut first,
                    &mut self.scratch,
                );
            }
            Kernel::Gru(gru) => gru.run(
                net.vector,
                read(0),
                read(3),
                read(4),
                &mut self.scratch,
                &mut first,
                &mut second,
            ),
            Kernel::RmsNorm(split, eps) => kernels::rms_norm(split, *eps, read(0), &mut first),
            Kernel::Gather(split, indices) => kernels::gather(split, indices, read(0), &mut first),
            Kernel::Table(table) => kernels::gather_by_table(table, read(0), &mut first),
        }
        self.buffers[step.outputs[0]] = first;
        if let Some(&id) = step.outputs.get(1) {
            self.buffers[id] = second;
        }
    }
}
