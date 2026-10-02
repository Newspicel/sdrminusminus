use super::{
    Net, NetError,
    format::{Binary, ConvSpec, Graph, Op, Unary},
    kernels::{Conv, EinSum, Group, Gru, Operand, Split},
    walk::{MAX_RANK, Walk, contiguous, offsets},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Slot {
    Const(usize),
    Buf(usize),
}

pub(super) enum Kernel {
    Unary(Unary),
    Binary(Binary, Walk<3>),
    SumReduce(Walk<2>),
    Copy(Vec<(Walk<2>, usize, usize)>),
    EinSum(Box<EinSum>),
    Conv(Box<Conv>),
    Gru(Gru),
    RmsNorm(Split, f32),
    Gather(Split, Vec<usize>),
    Table(Vec<usize>),
}

pub(super) struct Step {
    pub(super) kernel: Kernel,
    pub(super) inputs: Vec<Slot>,
    pub(super) outputs: Vec<usize>,
}

impl Step {
    fn reads(&self, index: usize) -> bool {
        match &self.kernel {
            Kernel::EinSum(einsum) => {
                let (a, b) = if einsum.swapped {
                    (&einsum.rhs, &einsum.lhs)
                } else {
                    (&einsum.lhs, &einsum.rhs)
                };
                [a, b].get(index).is_some_and(|operand| operand.packed.is_none())
            }
            Kernel::Gru(_) => !matches!(index, 1 | 2),
            _ => true,
        }
    }
}

fn unsupported(message: impl Into<String>) -> NetError {
    NetError::Unsupported(message.into())
}

fn check(ok: bool, message: impl FnOnce() -> String) -> Result<(), NetError> {
    if ok { Ok(()) } else { Err(unsupported(message())) }
}

pub(super) fn compile(graph: &Graph) -> Result<Net, NetError> {
    let mut builder = Builder {
        graph,
        slots: vec![None; graph.values.len()],
        consts: Vec::new(),
        buffers: Vec::new(),
        scratch: 0,
    };
    for (id, value) in graph.values.iter().enumerate() {
        check(value.shape.len() <= MAX_RANK, || format!("value {id} rank too high"))?;
        if let Some(data) = &value.data {
            check(data.len() == volume(&value.shape), || {
                format!("value {id} holds {} numbers for {:?}", data.len(), value.shape)
            })?;
            builder.slots[id] = Some(Slot::Const(builder.consts.len()));
            builder.consts.push(data.to_f32());
        }
    }
    let inputs = graph
        .inputs
        .iter()
        .map(|&id| builder.new_buffer(id))
        .collect::<Result<_, _>>()?;
    let steps: Vec<Step> = graph
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| builder.node(node).map_err(|e| at_node(e, index)).transpose())
        .collect::<Result<_, _>>()?;
    let outputs: Vec<Slot> = graph
        .outputs
        .iter()
        .map(|&id| builder.slot(id))
        .collect::<Result<_, _>>()?;
    let mut used = vec![false; builder.consts.len()];
    for step in &steps {
        for (index, slot) in step.inputs.iter().enumerate() {
            if let Slot::Const(id) = *slot
                && step.reads(index)
            {
                used[id] = true;
            }
        }
    }
    for slot in &outputs {
        if let Slot::Const(id) = *slot {
            used[id] = true;
        }
    }
    for (data, used) in builder.consts.iter_mut().zip(used) {
        if !used {
            *data = Vec::new();
        }
    }
    Ok(Net {
        vector: sdrmm_dsp::vector::Vector::detect(),
        meta: graph.meta.clone(),
        consts: builder.consts,
        buffers: builder.buffers,
        steps,
        inputs,
        outputs,
        scratch: builder.scratch,
    })
}

fn at_node(error: NetError, index: usize) -> NetError {
    match error {
        NetError::Unsupported(message) => NetError::Unsupported(format!("node {index}: {message}")),
        other => other,
    }
}

fn volume(shape: &[usize]) -> usize {
    shape.iter().product()
}

struct Builder<'a> {
    graph: &'a Graph,
    slots: Vec<Option<Slot>>,
    consts: Vec<Vec<f32>>,
    buffers: Vec<usize>,
    scratch: usize,
}

impl Builder<'_> {
    fn shape(&self, id: usize) -> Result<&[usize], NetError> {
        self.graph
            .values
            .get(id)
            .map(|value| value.shape.as_slice())
            .ok_or_else(|| NetError::Format(format!("no value {id}")))
    }

    fn slot(&self, id: usize) -> Result<Slot, NetError> {
        self.slots
            .get(id)
            .copied()
            .flatten()
            .ok_or_else(|| NetError::Format(format!("value {id} is read before it is made")))
    }

    fn new_buffer(&mut self, id: usize) -> Result<usize, NetError> {
        let len = volume(self.shape(id)?);
        match self.slots.get(id) {
            Some(None) => {}
            _ => return Err(NetError::Format(format!("value {id} is made twice"))),
        }
        self.slots[id] = Some(Slot::Buf(self.buffers.len()));
        self.buffers.push(len);
        Ok(self.buffers.len() - 1)
    }

    fn node(&mut self, node: &super::Node) -> Result<Option<Step>, NetError> {
        let shapes = node
            .inputs
            .iter()
            .map(|&id| self.shape(id).map(<[usize]>::to_vec))
            .collect::<Result<Vec<_>, _>>()?;
        let outs = node
            .outputs
            .iter()
            .map(|&id| self.shape(id).map(<[usize]>::to_vec))
            .collect::<Result<Vec<_>, _>>()?;
        let inputs = node
            .inputs
            .iter()
            .map(|&id| self.slot(id))
            .collect::<Result<Vec<_>, _>>()?;
        check(!outs.is_empty(), || "node makes nothing".into())?;
        let arity = expected_inputs(&node.op);
        check(arity.contains(&inputs.len()), || {
            format!("{:?} takes {arity:?} inputs, got {}", node.op, inputs.len())
        })?;
        let expected_outputs = if matches!(node.op, Op::Gru { .. }) { 2 } else { 1 };
        check(outs.len() == expected_outputs, || "wrong output count".into())?;
        if matches!(node.op, Op::Alias) {
            check(volume(&shapes[0]) == volume(&outs[0]), || "alias resizes".into())?;
            match self.slots.get(node.outputs[0]) {
                Some(None) => self.slots[node.outputs[0]] = Some(inputs[0]),
                _ => return Err(NetError::Format("alias output made twice".into())),
            }
            return Ok(None);
        }
        let kernel = self.kernel(&node.op, &shapes, &outs, &inputs)?;
        let outputs = node
            .outputs
            .iter()
            .map(|&id| self.new_buffer(id))
            .collect::<Result<_, _>>()?;
        Ok(Some(Step {
            kernel,
            inputs,
            outputs,
        }))
    }

    fn constant(&self, slot: Slot) -> Option<&[f32]> {
        match slot {
            Slot::Const(id) => self.consts.get(id).map(Vec::as_slice),
            Slot::Buf(_) => None,
        }
    }

    fn kernel(
        &mut self,
        op: &Op,
        ins: &[Vec<usize>],
        outs: &[Vec<usize>],
        slots: &[Slot],
    ) -> Result<Kernel, NetError> {
        let out = &outs[0];
        Ok(match op {
            Op::Alias => return Err(unsupported("alias has no kernel")),
            Op::Unary(kind) => {
                same(&ins[0], out)?;
                Kernel::Unary(*kind)
            }
            Op::Binary(kind) => Kernel::Binary(*kind, broadcast(&ins[0], &ins[1], out)?),
            Op::SumReduce { axes } => Kernel::SumReduce(sum_reduce(&ins[0], axes, out)?),
            Op::Slice { axis, start, end } => Kernel::Copy(vec![slice(&ins[0], *axis, *start, *end, out)?]),
            Op::Concat { axis } => Kernel::Copy(concat(ins, *axis, out)?),
            Op::Transpose { perm } => Kernel::Copy(vec![transpose(&ins[0], perm, out)?]),
            Op::EinSum { a, b, out: labels } => {
                let data = [self.constant(slots[0]), self.constant(slots[1])];
                let einsum = einsum([a, b, labels], [&ins[0], &ins[1], out], data)?;
                self.scratch = self.scratch.max(einsum.scratch_len());
                Kernel::EinSum(Box::new(einsum))
            }
            Op::Conv(spec) => {
                let conv = conv(spec, ins, out)?;
                self.scratch = self.scratch.max(conv.scratch_len());
                Kernel::Conv(Box::new(conv))
            }
            Op::Gru { hidden, backward } => {
                let weights = [self.constant(slots[1]), self.constant(slots[2])];
                let gru = gru(*hidden, *backward, ins, outs, weights)?;
                self.scratch = self.scratch.max(gru.scratch_len());
                Kernel::Gru(gru)
            }
            Op::RmsNorm { axis, eps } => {
                same(&ins[0], out)?;
                Kernel::RmsNorm(split(&ins[0], *axis)?, *eps)
            }
            Op::Gather { axis, indices } => gather(&ins[0], *axis, indices, out)?,
            Op::PadReflect { before, after } => Kernel::Table(pad_reflect(&ins[0], before, after, out)?),
        })
    }
}

fn expected_inputs(op: &Op) -> std::ops::RangeInclusive<usize> {
    match op {
        Op::Binary(_) | Op::EinSum { .. } => 2..=2,
        Op::Conv(_) => 3..=3,
        Op::Gru { .. } => 5..=5,
        Op::Concat { .. } => 1..=usize::MAX,
        _ => 1..=1,
    }
}

fn same(input: &[usize], out: &[usize]) -> Result<(), NetError> {
    check(input == out, || format!("shape {input:?} becomes {out:?}"))
}

fn broadcast(a: &[usize], b: &[usize], out: &[usize]) -> Result<Walk<3>, NetError> {
    check(a.len() == out.len() && b.len() == out.len(), || "ranks differ".into())?;
    let mut strides = [contiguous(a), contiguous(b), contiguous(out)];
    for axis in 0..out.len() {
        for (k, shape) in [a, b].into_iter().enumerate() {
            match shape[axis] {
                len if len == out[axis] => {}
                1 => strides[k][axis] = 0,
                _ => return Err(unsupported(format!("{a:?} and {b:?} do not broadcast to {out:?}"))),
            }
        }
    }
    Ok(Walk::new(out, [&strides[0], &strides[1], &strides[2]]))
}

fn sum_reduce(input: &[usize], axes: &[usize], out: &[usize]) -> Result<Walk<2>, NetError> {
    check(input.len() == out.len(), || "reduce changes rank".into())?;
    let mut out_strides = contiguous(out);
    for axis in 0..input.len() {
        if axes.contains(&axis) {
            check(out[axis] == 1, || "reduced axis kept".into())?;
            out_strides[axis] = 0;
        } else {
            check(out[axis] == input[axis], || "reduce resizes".into())?;
        }
    }
    check(axes.iter().all(|&axis| axis < input.len()), || "bad reduce axis".into())?;
    Ok(Walk::new(input, [&contiguous(input), &out_strides]))
}

fn slice(
    input: &[usize],
    axis: usize,
    start: usize,
    end: usize,
    out: &[usize],
) -> Result<(Walk<2>, usize, usize), NetError> {
    check(axis < input.len() && start <= end && end <= input[axis], || "bad slice".into())?;
    let mut expected = input.to_vec();
    expected[axis] = end - start;
    same(&expected, out)?;
    let strides = contiguous(input);
    Ok((Walk::new(out, [&strides, &contiguous(out)]), start * strides[axis], 0))
}

fn concat(ins: &[Vec<usize>], axis: usize, out: &[usize]) -> Result<Vec<(Walk<2>, usize, usize)>, NetError> {
    check(axis < out.len(), || "bad concat axis".into())?;
    let out_strides = contiguous(out);
    let mut at = 0;
    let mut parts = Vec::with_capacity(ins.len());
    for input in ins {
        let mut expected = out.to_vec();
        expected[axis] = input.get(axis).copied().unwrap_or(0);
        same(input, &expected)?;
        parts.push((
            Walk::new(input, [&contiguous(input), &out_strides]),
            0,
            at * out_strides[axis],
        ));
        at += input[axis];
    }
    check(at == out[axis], || "concat length".into())?;
    Ok(parts)
}

fn transpose(input: &[usize], perm: &[usize], out: &[usize]) -> Result<(Walk<2>, usize, usize), NetError> {
    let mut seen = vec![false; input.len()];
    check(perm.len() == input.len(), || "bad permutation".into())?;
    for &axis in perm {
        check(axis < input.len() && !std::mem::replace(&mut seen[axis], true), || {
            "bad permutation".into()
        })?;
    }
    let expected: Vec<usize> = perm.iter().map(|&axis| input[axis]).collect();
    same(&expected, out)?;
    let strides = contiguous(input);
    let src: Vec<usize> = perm.iter().map(|&axis| strides[axis]).collect();
    Ok((Walk::new(out, [&src, &contiguous(out)]), 0, 0))
}

#[derive(Default)]
struct Axes {
    shape: Vec<usize>,
    strides: [Vec<usize>; 3],
}

impl Axes {
    fn group(&self) -> Group {
        let [a, b, c] = &self.strides;
        Group {
            len: volume(&self.shape),
            a: offsets(&self.shape, a),
            b: offsets(&self.shape, b),
            c: offsets(&self.shape, c),
        }
    }
}

fn sequential(tables: [&[usize]; 3]) -> bool {
    let mut next = 0;
    for &b in tables[0] {
        for &o in tables[1] {
            for &i in tables[2] {
                if b + o + i != next {
                    return false;
                }
                next += 1;
            }
        }
    }
    true
}

fn operand(tables: [&[usize]; 3], data: Option<&[f32]>) -> Operand {
    let [batch, outer, inner] = tables.map(<[usize]>::to_vec);
    let mut operand = Operand {
        packed: None,
        direct: sequential(tables),
        batch,
        outer,
        inner,
    };
    if let Some(data) = data {
        operand.packed = Some(operand.pack(data));
    }
    operand
}

fn einsum(
    labels: [&[u8]; 3],
    shapes: [&[usize]; 3],
    data: [Option<&[f32]>; 2],
) -> Result<EinSum, NetError> {
    let strides = shapes.map(contiguous);
    for k in 0..3 {
        check(labels[k].len() == shapes[k].len(), || "einsum labels do not match rank".into())?;
    }
    let mut order: Vec<u8> = Vec::new();
    for label in labels.iter().flat_map(|l| l.iter().copied()) {
        if !order.contains(&label) {
            order.push(label);
        }
    }
    let [mut batch, mut m, mut n, mut k] = [(); 4].map(|()| Axes::default());
    for label in order {
        let mut len = 1;
        let mut step = [0usize; 3];
        let mut present = [false; 3];
        for t in 0..3 {
            let mut hits = labels[t].iter().enumerate().filter(|&(_, &l)| l == label);
            if let Some((axis, _)) = hits.next() {
                check(hits.next().is_none(), || "repeated einsum label".into())?;
                check(len == 1 || shapes[t][axis] == len || shapes[t][axis] == 1, || {
                    "einsum sizes disagree".into()
                })?;
                if shapes[t][axis] != 1 {
                    len = shapes[t][axis];
                    step[t] = strides[t][axis];
                    present[t] = true;
                }
            }
        }
        if len == 1 {
            continue;
        }
        check(present[0] || present[1], || "einsum output axis has no source".into())?;
        let target = match present {
            [true, true, true] => &mut batch,
            [true, false, true] => &mut m,
            [false, true, true] => &mut n,
            _ => &mut k,
        };
        target.shape.push(len);
        for t in 0..3 {
            target.strides[t].push(step[t]);
        }
    }
    let (batch, m, n, k) = (batch.group(), m.group(), n.group(), k.group());
    let swapped = n.len < m.len;
    let (rows, cols) = if swapped { (&n, &m) } else { (&m, &n) };
    let pick = |group: &Group, from_a: bool| if from_a { group.a.clone() } else { group.b.clone() };
    let lhs_tables = [pick(&batch, !swapped), pick(rows, !swapped), pick(&k, !swapped)];
    let rhs_tables = [pick(&batch, swapped), pick(&k, swapped), pick(cols, swapped)];
    let (lhs_data, rhs_data) = if swapped { (data[1], data[0]) } else { (data[0], data[1]) };
    let out_tables = [batch.c.as_slice(), rows.c.as_slice(), cols.c.as_slice()];
    Ok(EinSum {
        shape: sdrmm_dsp::vector::GemmShape {
            rows: rows.len,
            depth: k.len,
            cols: cols.len,
        },
        batch: batch.len,
        lhs: operand([&lhs_tables[0], &lhs_tables[1], &lhs_tables[2]], lhs_data),
        rhs: operand([&rhs_tables[0], &rhs_tables[1], &rhs_tables[2]], rhs_data),
        swapped,
        out_direct: sequential(out_tables),
        out_batch: batch.c.clone(),
        out_rows: rows.c.clone(),
        out_cols: cols.c.clone(),
    })
}

fn conv(spec: &ConvSpec, ins: &[Vec<usize>], out: &[usize]) -> Result<Conv, NetError> {
    let (x, weights, bias) = (&ins[0], &ins[1], &ins[2]);
    let spatial = weights.len().checked_sub(2).filter(|s| (1..=2).contains(s));
    let spatial = spatial.ok_or_else(|| unsupported("conv kernel rank"))?;
    let lead = usize::from(spec.batched);
    check(x.len() == lead + 1 + spatial && out.len() == x.len(), || "conv input rank".into())?;
    check(!spec.batched || (x[0] == 1 && out[0] == 1), || "conv batch above one".into())?;
    for list in [&spec.strides, &spec.dilations, &spec.pads_before, &spec.pads_after] {
        check(list.len() == spatial, || "conv geometry rank".into())?;
    }
    let channel_axis = if spec.channels_last { x.len() - 1 } else { lead };
    let space: Vec<usize> = (0..x.len()).filter(|&a| a >= lead && a != channel_axis).collect();
    let group = spec.group;
    let in_channels = x[channel_axis];
    let out_channels = weights[0];
    check(
        group > 0 && in_channels == weights[1] * group && out_channels.is_multiple_of(group),
        || "conv channels".into(),
    )?;
    check(out[channel_axis] == out_channels, || "conv output channels".into())?;
    check(
        volume(bias) == out_channels || volume(bias) <= 1,
        || "conv bias".into(),
    )?;
    let (xs, os) = (contiguous(x), contiguous(out));
    let pad = spatial_pad(spatial);
    let mut conv = Conv {
        group,
        in_per_group: weights[1],
        out_per_group: out_channels / group,
        in_channel_stride: xs[channel_axis],
        out_channel_stride: os[channel_axis],
        in_size: [1; 2],
        in_strides: [0; 2],
        out_size: [1; 2],
        out_strides: [0; 2],
        kernel: [1; 2],
        stride: [1; 2],
        dilation: [1; 2],
        pad: [0; 2],
        bias_per_channel: volume(bias) == out_channels && out_channels > 1,
    };
    for (i, &axis) in space.iter().enumerate() {
        let d = pad + i;
        let kernel = weights[2 + i];
        let (stride, dilation) = (spec.strides[i], spec.dilations[i]);
        check(stride > 0 && dilation > 0 && kernel > 0, || "conv geometry".into())?;
        let padded = x[axis] + spec.pads_before[i] + spec.pads_after[i];
        let reach = dilation * (kernel - 1) + 1;
        let expected = padded.checked_sub(reach).map(|room| room / stride + 1);
        check(expected == Some(out[axis]), || "conv output size".into())?;
        conv.in_size[d] = x[axis];
        conv.in_strides[d] = xs[axis];
        conv.out_size[d] = out[axis];
        conv.out_strides[d] = os[axis];
        conv.kernel[d] = kernel;
        conv.stride[d] = stride;
        conv.dilation[d] = dilation;
        conv.pad[d] = spec.pads_before[i];
    }
    Ok(conv)
}

fn spatial_pad(spatial: usize) -> usize {
    2 - spatial
}

fn transposed(data: &[f32], rows: usize, cols: usize) -> Vec<f32> {
    let mut out = vec![0.0; rows * cols];
    for r in 0..rows {
        for c in 0..cols {
            out[c * rows + r] = data[r * cols + c];
        }
    }
    out
}

fn gru(
    hidden: usize,
    backward: bool,
    ins: &[Vec<usize>],
    outs: &[Vec<usize>],
    weights: [Option<&[f32]>; 2],
) -> Result<Gru, NetError> {
    let [x, w, r, bias, h0] = [&ins[0], &ins[1], &ins[2], &ins[3], &ins[4]];
    check(x.len() == 3 && hidden > 0, || "gru input rank".into())?;
    let (batch, steps, input) = (x[0], x[1], x[2]);
    check(w.as_slice() == [3 * hidden, input], || "gru input weights".into())?;
    check(r.as_slice() == [3 * hidden, hidden], || "gru recurrent weights".into())?;
    check(volume(bias) == 6 * hidden, || "gru bias".into())?;
    check(volume(h0) == batch * hidden, || "gru initial state".into())?;
    same(&outs[0], &[batch, steps, hidden])?;
    same(&outs[1], &[batch, 1, hidden])?;
    let [Some(w_data), Some(r_data)] = weights else {
        return Err(unsupported("gru weights are not constant"));
    };
    Ok(Gru {
        hidden,
        backward,
        batch,
        steps,
        input,
        input_weights: transposed(w_data, 3 * hidden, input),
        recurrent_weights: transposed(r_data, 3 * hidden, hidden),
    })
}

fn split(shape: &[usize], axis: usize) -> Result<Split, NetError> {
    check(axis < shape.len(), || "bad axis".into())?;
    Ok(Split {
        outer: volume(&shape[..axis]),
        len: shape[axis],
        inner: volume(&shape[axis + 1..]),
    })
}

fn gather(input: &[usize], axis: usize, indices: &[usize], out: &[usize]) -> Result<Kernel, NetError> {
    let split = split(input, axis)?;
    check(indices.iter().all(|&i| i < split.len), || "gather index out of range".into())?;
    check(volume(out) == split.outer * indices.len() * split.inner, || "gather output size".into())?;
    Ok(Kernel::Gather(split, indices.to_vec()))
}

fn pad_reflect(input: &[usize], before: &[usize], after: &[usize], out: &[usize]) -> Result<Vec<usize>, NetError> {
    check(before.len() == input.len() && after.len() == input.len(), || "pad rank".into())?;
    let strides = contiguous(input);
    let mut maps = Vec::with_capacity(input.len());
    for axis in 0..input.len() {
        let len = input[axis];
        check(before[axis] < len && after[axis] < len, || "reflect pad wider than input".into())?;
        check(out[axis] == len + before[axis] + after[axis], || "pad output size".into())?;
        let map: Vec<usize> = (0..out[axis])
            .map(|j| {
                let at = j.abs_diff(before[axis]);
                let at = if at < len { at } else { 2 * (len - 1) - at };
                at * strides[axis]
            })
            .collect();
        maps.push(map);
    }
    let mut table = vec![0];
    for map in &maps {
        table = table
            .iter()
            .flat_map(|&base| map.iter().map(move |&offset| base + offset))
            .collect();
    }
    Ok(table)
}
