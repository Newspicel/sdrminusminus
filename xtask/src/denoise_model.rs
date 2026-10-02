use std::{collections::HashMap, fs, path::Path, process::Command, sync::Arc, time::Instant};

use anyhow::{Context, Result, bail, ensure};
use sdrmm_channels::neural::{
    Binary, ConvSpec, Graph, Net, Node, Op, Session, Unary, Value, Weights, f16_to_f32, f32_to_f16,
};
use sdrmm_server::denoise_models::{MODELS_PREFIX, artifact, file_name};
use sdrmm_wire::DenoiseModel;
use sha2::{Digest, Sha256};
use tract_onnx::prelude::*;
use tract_onnx::tract_core::internal::DimLike as _;
use tract_onnx::tract_core::ops::{
    array::{Gather, Pad, PadMode, Slice, TypedConcat},
    binary::TypedBinOp,
    change_axes::AxisOp,
    cnn::{Conv, KernelFormat, PaddingSpec},
    einsum::EinSum,
    element_wise::ElementWiseOp,
    gru_seq::GruSeq,
    konst::Const,
    nn::{DataFormat, Reduce, Reducer, RmsNorm},
    source::TypedSource,
};

const SOURCE_BASE: &str = "https://huggingface.co/Ceva-IP/DPDFNet/resolve/main/onnx";
const SOURCES: [(DenoiseModel, &str); 8] = [
    (
        DenoiseModel::Baseline,
        "371d26182aff0e1e0d31354e24c81f79cd57458f5a7dd003fc10a7ebf64255e0",
    ),
    (
        DenoiseModel::Dpdfnet2,
        "4f0ee28935b4a32abecc717d745416976565834d839601acf43031094b4dc94c",
    ),
    (
        DenoiseModel::Dpdfnet4,
        "42a386d3601922f474c4309927e1f471cce422e9bd8e3fd3967bb943bac36c01",
    ),
    (
        DenoiseModel::Dpdfnet8,
        "899d4f23f3ff86edbffa8c537e4bcbdc49da1b4e84e0ef390611e0604a3b26cb",
    ),
    (
        DenoiseModel::Dpdfnet2Narrow,
        "6218f1dbd6e4bac5768c63b7d899fe7b84b3788f2a35c4e246d4ab0946165c5d",
    ),
    (
        DenoiseModel::Dpdfnet8Narrow,
        "c061bcc56b803fa2fa97d448a45db6d966f7d17aff1304e464455d748745ea62",
    ),
    (
        DenoiseModel::Dpdfnet2Full,
        "7f0575a5cec0ba4ffd8f8bd657e06d007e4ccdd955d76faab922b9d3291dc14b",
    ),
    (
        DenoiseModel::Dpdfnet8Full,
        "7b3afbb260a08fe9af3d16e3bda992971be1e7e951d1dee7c2d235f5c43f5631",
    ),
];
const FIXTURE: (DenoiseModel, &str) = (
    DenoiseModel::Dpdfnet2,
    "crates/channels/models/dpdfnet2.sdrmmnn",
);
const WEIGHT_MIN_LEN: usize = 256;
const PARITY_FRAMES: usize = 64;
const PARITY_TOLERANCE: f32 = 1e-2;

pub(crate) fn run(root: &Path) -> Result<()> {
    let scratch = root.join("target/denoise-model");
    fs::create_dir_all(&scratch)?;
    let mut stale = Vec::new();
    for (model, sha256) in SOURCES {
        let onnx = scratch.join(format!("{}.onnx", model.name()));
        download(model, sha256, &onnx)?;
        let typed = tract_onnx::onnx()
            .model_for_path(&onnx)?
            .into_typed()?
            .into_decluttered()?;
        let graph = convert(&typed, model)?;
        let bytes = graph.encode();
        let net = Arc::new(Net::load(&bytes)?);
        let report = parity(typed, &net)?;
        let out = scratch.join(file_name(model));
        fs::write(&out, &bytes)?;
        let digest = hex(&Sha256::digest(&bytes));
        let listed = artifact(model);
        if listed.sha256 != digest || listed.bytes != bytes.len() as u64 {
            stale.push(format!(
                "{}: {} bytes, sha256 {digest}",
                model.name(),
                bytes.len()
            ));
        }
        if model == FIXTURE.0 {
            fs::write(root.join(FIXTURE.1), &bytes)?;
        }
        println!(
            "{:<16} {:>9} bytes  sha256 {}  error {:.1e}  {:.0} us/frame (tract {:.0})",
            model.name(),
            bytes.len(),
            digest,
            report.error,
            report.ours_us,
            report.tract_us,
        );
    }
    ensure!(
        stale.is_empty(),
        "update the catalog in crates/server/src/denoise_models.rs:\n{}",
        stale.join("\n")
    );
    println!(
        "upload with: scripts/r2-upload.sh {MODELS_PREFIX} {}/*.sdrmmnn",
        scratch.display()
    );
    Ok(())
}

fn download(model: DenoiseModel, sha256: &str, to: &Path) -> Result<()> {
    let url = format!("{SOURCE_BASE}/{}.onnx", model.name());
    if !to.exists() {
        println!("fetching {url}");
        let status = Command::new("curl")
            .args(["-fsSL", "--max-time", "600", "-o"])
            .arg(to)
            .arg(&url)
            .status()
            .context("curl not found")?;
        ensure!(status.success(), "curl failed for {url}");
    }
    let digest = hex(&Sha256::digest(fs::read(to)?));
    if digest != sha256 {
        fs::remove_file(to)?;
        bail!("{url} changed upstream: sha256 {digest}");
    }
    Ok(())
}

fn convert(model: &TypedModel, name: DenoiseModel) -> Result<Graph> {
    let mut converter = Converter {
        model,
        values: Vec::new(),
        ids: HashMap::new(),
        nodes: Vec::new(),
    };
    for id in model.eval_order()? {
        converter
            .node(&model.nodes[id])
            .with_context(|| format!("node {}", model.nodes[id].name))?;
    }
    let inputs = model
        .input_outlets()?
        .iter()
        .map(|outlet| converter.id(*outlet))
        .collect::<Result<_>>()?;
    let outputs = model
        .output_outlets()?
        .iter()
        .map(|outlet| converter.id(*outlet))
        .collect::<Result<_>>()?;
    Ok(Graph {
        meta: metadata(model, name)?,
        values: converter.values,
        nodes: converter.nodes,
        inputs,
        outputs,
    })
}

fn metadata(model: &TypedModel, name: DenoiseModel) -> Result<Vec<(String, String)>> {
    let mut meta = vec![("model".to_string(), name.name().to_string())];
    for (key, value) in &model.properties {
        if let Some(key) = key.strip_prefix("onnx.metadata_props.") {
            let text = value.try_as_plain_ram()?.to_scalar::<String>()?.clone();
            meta.push((key.to_string(), text));
        }
    }
    meta.sort();
    Ok(meta)
}

struct Converter<'a> {
    model: &'a TypedModel,
    values: Vec<Value>,
    ids: HashMap<OutletId, usize>,
    nodes: Vec<Node>,
}

impl Converter<'_> {
    fn id(&self, outlet: OutletId) -> Result<usize> {
        self.ids
            .get(&outlet)
            .copied()
            .with_context(|| format!("{outlet:?} has no value"))
    }

    fn shape(&self, outlet: OutletId) -> Result<Vec<usize>> {
        Ok(self
            .model
            .outlet_fact(outlet)?
            .shape
            .as_concrete()
            .context("symbolic shape")?
            .to_vec())
    }

    fn constant(&self, outlet: OutletId) -> Option<Arc<Tensor>> {
        self.model.nodes[outlet.node]
            .op_as::<Const>()
            .map(|konst| konst.val().clone())
    }

    fn add_value(&mut self, outlet: OutletId, data: Option<Weights>) -> Result<usize> {
        let shape = self.shape(outlet)?;
        self.values.push(Value { shape, data });
        self.ids.insert(outlet, self.values.len() - 1);
        Ok(self.values.len() - 1)
    }

    fn node(&mut self, node: &TypedNode) -> Result<()> {
        if node.op_as::<TypedSource>().is_some() {
            self.add_value(OutletId::new(node.id, 0), None)?;
            return Ok(());
        }
        if let Some(konst) = node.op_as::<Const>() {
            if konst.val().datum_type() == f32::datum_type() {
                let data = weights(konst.val())?;
                self.add_value(OutletId::new(node.id, 0), Some(data))?;
            }
            return Ok(());
        }
        let (op, inputs) = self.op(node)?;
        let inputs = inputs
            .into_iter()
            .map(|outlet| self.id(outlet))
            .collect::<Result<_>>()?;
        let outputs = (0..node.outputs.len())
            .map(|slot| self.add_value(OutletId::new(node.id, slot), None))
            .collect::<Result<_>>()?;
        self.nodes.push(Node {
            op,
            inputs,
            outputs,
        });
        Ok(())
    }

    fn op(&self, node: &TypedNode) -> Result<(Op, Vec<OutletId>)> {
        let all = node.inputs.clone();
        let first = || vec![all[0]];
        if let Some(bin) = node.op_as::<TypedBinOp>() {
            return Ok((Op::Binary(binary(bin.0.name())?), all.clone()));
        }
        if let Some(unary_op) = node.op_as::<ElementWiseOp>() {
            return Ok((Op::Unary(unary(unary_op.0.name())?), first()));
        }
        if let Some(axis) = node.op_as::<AxisOp>() {
            let op = match axis {
                AxisOp::Move(from, to) => Op::Transpose {
                    perm: moved(self.shape(all[0])?.len(), *from, *to),
                },
                _ => Op::Alias,
            };
            return Ok((op, first()));
        }
        if let Some(slice) = node.op_as::<Slice>() {
            let op = Op::Slice {
                axis: slice.axis,
                start: slice.start.to_usize()?,
                end: slice.end.to_usize()?,
            };
            return Ok((op, first()));
        }
        if let Some(concat) = node.op_as::<TypedConcat>() {
            return Ok((Op::Concat { axis: concat.axis }, all.clone()));
        }
        if let Some(reduce) = node.op_as::<Reduce>() {
            ensure!(
                matches!(reduce.reducer, Reducer::Sum),
                "reducer {:?}",
                reduce.reducer
            );
            let axes = reduce.axes.to_vec();
            return Ok((Op::SumReduce { axes }, first()));
        }
        if let Some(einsum) = node.op_as::<EinSum>() {
            return Ok((einsum_op(einsum)?, all.clone()));
        }
        if let Some(conv) = node.op_as::<Conv>() {
            return Ok((Op::Conv(conv_spec(conv)?), all.clone()));
        }
        if let Some(gru) = node.op_as::<GruSeq>() {
            ensure!(gru.has_bias && gru.emit_y, "gru without bias or sequence");
            let op = Op::Gru {
                hidden: gru.hidden,
                backward: gru.chunk < 0,
            };
            return Ok((op, all.clone()));
        }
        if let Some(norm) = node.op_as::<RmsNorm>() {
            let eps = norm.eps.cast_to_scalar::<f32>()?;
            return Ok((
                Op::RmsNorm {
                    axis: norm.axis,
                    eps,
                },
                first(),
            ));
        }
        if let Some(gather) = node.op_as::<Gather>() {
            let indices = self.indices(all[1], self.shape(all[0])?[gather.axis])?;
            let op = Op::Gather {
                axis: gather.axis,
                indices,
            };
            return Ok((op, first()));
        }
        if let Some(pad) = node.op_as::<Pad>() {
            ensure!(
                matches!(pad.mode, PadMode::Reflect),
                "pad mode {:?}",
                pad.mode
            );
            let op = Op::PadReflect {
                before: pad.pads.iter().map(|p| p.0).collect(),
                after: pad.pads.iter().map(|p| p.1).collect(),
            };
            return Ok((op, first()));
        }
        bail!("unsupported op {}", node.op.name())
    }

    fn indices(&self, outlet: OutletId, len: usize) -> Result<Vec<usize>> {
        let tensor = self
            .constant(outlet)
            .context("gather indices are not constant")?;
        let tensor = tensor.cast_to::<i64>()?;
        tensor
            .try_as_plain_ram()?
            .as_slice::<i64>()?
            .iter()
            .map(|&index| {
                let index = if index < 0 { index + len as i64 } else { index };
                usize::try_from(index).context("gather index")
            })
            .collect()
    }
}

fn weights(tensor: &Tensor) -> Result<Weights> {
    let values = tensor.try_as_plain_ram()?.as_slice::<f32>()?;
    Ok(if values.len() >= WEIGHT_MIN_LEN {
        Weights::F16(values.iter().map(|&x| f32_to_f16(x)).collect())
    } else {
        Weights::F32(values.to_vec())
    })
}

fn binary(name: impl AsRef<str>) -> Result<Binary> {
    Ok(match name.as_ref() {
        "Add" => Binary::Add,
        "Sub" => Binary::Sub,
        "Mul" => Binary::Mul,
        "Max" => Binary::Max,
        "Min" => Binary::Min,
        other => bail!("binary {other}"),
    })
}

fn unary(name: impl AsRef<str>) -> Result<Unary> {
    Ok(match name.as_ref() {
        "Sqrt" => Unary::Sqrt,
        "Rsqrt" => Unary::Rsqrt,
        "Square" => Unary::Square,
        "Ln" => Unary::Ln,
        "Sigmoid" => Unary::Sigmoid,
        "Tanh" => Unary::Tanh,
        other => bail!("unary {other}"),
    })
}

fn moved(rank: usize, from: usize, to: usize) -> Vec<usize> {
    let mut perm: Vec<usize> = (0..rank).collect();
    let axis = perm.remove(from);
    perm.insert(to, axis);
    perm
}

fn einsum_op(einsum: &EinSum) -> Result<Op> {
    ensure!(einsum.q_params.is_none(), "quantized einsum");
    let expression = einsum.axes.to_string();
    let (inputs, out) = expression.split_once("->").context("einsum expression")?;
    let operands: Vec<&str> = inputs.split(',').collect();
    ensure!(operands.len() == 2, "einsum {expression}");
    Ok(Op::EinSum {
        a: operands[0].as_bytes().to_vec(),
        b: operands[1].as_bytes().to_vec(),
        out: out.as_bytes().to_vec(),
    })
}

fn conv_spec(conv: &Conv) -> Result<ConvSpec> {
    ensure!(conv.q_params.is_none(), "quantized conv");
    ensure!(
        matches!(conv.kernel_fmt, KernelFormat::OIHW),
        "kernel format"
    );
    let pool = &conv.pool_spec;
    let rank = pool.kernel_shape.len();
    let PaddingSpec::Explicit(before, after) = &pool.padding else {
        bail!("conv padding {:?}", pool.padding);
    };
    let (channels_last, batched) = match pool.data_format {
        DataFormat::NCHW => (false, true),
        DataFormat::NHWC => (true, true),
        DataFormat::CHW => (false, false),
        DataFormat::HWC => (true, false),
    };
    Ok(ConvSpec {
        channels_last,
        batched,
        group: conv.group,
        strides: pool.strides.clone().map_or(vec![1; rank], |s| s.to_vec()),
        dilations: pool.dilations.clone().map_or(vec![1; rank], |d| d.to_vec()),
        pads_before: before.to_vec(),
        pads_after: after.to_vec(),
    })
}

struct Parity {
    error: f32,
    ours_us: f64,
    tract_us: f64,
}

fn parity(mut model: TypedModel, net: &Arc<Net>) -> Result<Parity> {
    for node in &mut model.nodes {
        if let Some(gru) = node.op_as_mut::<GruSeq>() {
            gru.reset_every_turn = true;
        }
    }
    round_weights(&mut model)?;
    let spec_shape = model
        .outlet_fact(model.input_outlets()?[0])?
        .shape
        .as_concrete()
        .context("spec")?
        .to_vec();
    let plan = model.into_optimized()?.into_runnable()?;
    let mut runner = plan.spawn()?;
    let mut session = Session::new(Arc::clone(net));
    let initial = sdrmm_channels::neural_denoise::initial_state(net)?;
    let mut state = Tensor::from_shape(&[initial.len()], &initial)?;
    let mut ours_state = initial;
    let spec_len = net.input_len(0).context("spec input")?;
    let mut noise = 0x9e37_79b9_7f4a_7c15u64;
    let (mut worst, mut peak) = (0.0f32, 0.0f32);
    let (mut ours_time, mut tract_time) = (0.0, 0.0);
    for _ in 0..PARITY_FRAMES {
        let spec: Vec<f32> = (0..spec_len).map(|_| next_noise(&mut noise)).collect();
        let started = Instant::now();
        let mut outputs = runner.run(tvec!(
            Tensor::from_shape(&spec_shape, &spec)?.into(),
            state.into()
        ))?;
        tract_time += started.elapsed().as_secs_f64();
        state = outputs.remove(1).into_tensor();
        let theirs = outputs.remove(0).into_tensor();
        session.input_mut(0).copy_from_slice(&spec);
        session.input_mut(1).copy_from_slice(&ours_state);
        let started = Instant::now();
        session.run();
        ours_time += started.elapsed().as_secs_f64();
        ours_state.copy_from_slice(session.output(1));
        for (a, b) in theirs
            .try_as_plain_ram()?
            .as_slice::<f32>()?
            .iter()
            .zip(session.output(0))
        {
            worst = worst.max((a - b).abs());
            peak = peak.max(a.abs());
        }
    }
    let error = worst / peak.max(f32::MIN_POSITIVE);
    ensure!(error <= PARITY_TOLERANCE, "differs from tract by {error}");
    let frames = PARITY_FRAMES as f64 / 1e6;
    Ok(Parity {
        error,
        ours_us: ours_time / frames,
        tract_us: tract_time / frames,
    })
}

fn round_weights(model: &mut TypedModel) -> Result<()> {
    for node in &mut model.nodes {
        let Some(konst) = node.op_as::<Const>() else {
            continue;
        };
        let value = konst.val();
        if value.datum_type() != f32::datum_type() || value.len() < WEIGHT_MIN_LEN {
            continue;
        }
        let rounded: Vec<f32> = value
            .try_as_plain_ram()?
            .as_slice::<f32>()?
            .iter()
            .map(|&x| f16_to_f32(f32_to_f16(x)))
            .collect();
        let tensor = Tensor::from_shape(value.shape(), &rounded)?;
        node.op = Box::new(Const::new(tensor.into_arc_tensor())?);
    }
    Ok(())
}

fn next_noise(state: &mut u64) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    ((*state >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0) * 0.05
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tract_onnx::tract_core::ops::math::add;

    fn model_with_weight(weight: Tensor) -> Result<TypedModel> {
        let mut model = TypedModel::default();
        let input = model.add_source("input", f32::fact(weight.shape()))?;
        let konst = model.add_const("weight", weight)?;
        let sum = model.wire_node("sum", add(), &[input, konst])?;
        model.select_output_outlets(&sum)?;
        Ok(model)
    }

    #[test]
    fn a_converted_model_runs_like_tract() -> Result<()> {
        let values: Vec<f32> = (0..WEIGHT_MIN_LEN).map(|i| i as f32 / 7.0).collect();
        let model = model_with_weight(Tensor::from_shape(&[WEIGHT_MIN_LEN], &values)?)?;
        let graph = convert(&model, DenoiseModel::Dpdfnet2)?;
        assert!(matches!(graph.values[1].data, Some(Weights::F16(_))));
        let net = Arc::new(Net::load(&graph.encode())?);
        let mut session = Session::new(net);
        session.input_mut(0).fill(1.0);
        session.run();
        for (got, want) in session.output(0).iter().zip(&values) {
            assert!(
                (got - (want + 1.0)).abs() <= (want + 1.0) / 1024.0,
                "{got} vs {want}"
            );
        }
        Ok(())
    }

    #[test]
    fn small_constants_stay_full_precision() -> Result<()> {
        let weight = Tensor::from_shape(&[3], &[1.0f32 / 3.0, 2.0, 3.0])?;
        let graph = convert(&model_with_weight(weight)?, DenoiseModel::Dpdfnet2)?;
        assert!(matches!(graph.values[1].data, Some(Weights::F32(_))));
        Ok(())
    }

    #[test]
    fn moving_an_axis_is_a_permutation() {
        assert_eq!(moved(4, 2, 0), [2, 0, 1, 3]);
        assert_eq!(moved(4, 0, 3), [1, 2, 3, 0]);
    }
}
