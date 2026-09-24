use std::{fs, io::Write as _, path::Path, process::Command};

use anyhow::{Context, Result, bail, ensure};
use flate2::{Compression, write::GzEncoder};
use sha2::{Digest, Sha256};
use tract_nnef::internal::*;
use tract_nnef::tract_core::ops::{cast::cast, konst::Const};
use tract_onnx::tract_hir::infer::InferenceModelExt as _;

const SOURCE_URL: &str = "https://huggingface.co/Ceva-IP/DPDFNet/resolve/main/onnx/dpdfnet2.onnx";
const SOURCE_SHA256: &str = "4f0ee28935b4a32abecc717d745416976565834d839601acf43031094b4dc94c";
const OUTPUT: &str = "crates/channels/models/dpdfnet2.nnef.tgz";
const WEIGHT_MIN_LEN: usize = 256;

pub(crate) fn run(root: &Path) -> Result<()> {
    let scratch = root.join("target/denoise-model");
    fs::create_dir_all(&scratch)?;
    let onnx = scratch.join("dpdfnet2.onnx");
    download(&onnx)?;
    let model = tract_onnx::onnx()
        .model_for_path(&onnx)?
        .into_typed()?
        .into_decluttered()?;
    let packed = pack(half_precision_weights(model)?)?;
    let out = root.join(OUTPUT);
    fs::write(&out, &packed).with_context(|| format!("write {}", out.display()))?;
    println!("wrote {} ({} bytes)", out.display(), packed.len());
    Ok(())
}

fn download(to: &Path) -> Result<()> {
    if !to.exists() {
        println!("fetching {SOURCE_URL}");
        let status = Command::new("curl")
            .args(["-fsSL", "--max-time", "600", "-o"])
            .arg(to)
            .arg(SOURCE_URL)
            .status()
            .context("curl not found")?;
        ensure!(status.success(), "curl failed for {SOURCE_URL}");
    }
    let digest = hex(&Sha256::digest(fs::read(to)?));
    if digest != SOURCE_SHA256 {
        fs::remove_file(to)?;
        bail!("{SOURCE_URL} changed upstream: sha256 {digest}");
    }
    Ok(())
}

fn half_precision_weights(mut model: TypedModel) -> Result<TypedModel> {
    let weights: Vec<usize> = model
        .nodes
        .iter()
        .filter(|node| is_weight(node))
        .map(|node| node.id)
        .collect();
    for id in weights {
        let name = format!("{}.f16", model.node(id).name);
        let value = model
            .node(id)
            .op_as::<Const>()
            .context("weight is a constant")?
            .val()
            .clone();
        let stored = value.cast_to::<f16>()?.into_owned().into_arc_tensor();
        let fact = TypedFact::try_from(stored.clone())?;
        let source = model.add_node(name, Const::new(stored)?, tvec!(fact))?;
        let node = model.node_mut(id);
        node.op = Box::new(cast(f32::datum_type()));
        node.outputs[0].fact = f32::fact(value.shape());
        model.add_edge(OutletId::new(source, 0), InletId::new(id, 0))?;
    }
    Ok(model)
}

fn is_weight(node: &TypedNode) -> bool {
    node.op_as::<Const>().is_some_and(|konst| {
        konst.val().datum_type() == f32::datum_type() && konst.val().len() >= WEIGHT_MIN_LEN
    })
}

fn pack(model: TypedModel) -> Result<Vec<u8>> {
    let tar = tract_nnef::nnef().write_to_tar_with_config(&model, Vec::new(), false, true)?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(&tar)?;
    Ok(encoder.finish()?)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tract_nnef::tract_core::ops::math::add;

    fn model_with_weight(weight: Tensor) -> Result<TypedModel> {
        let mut model = TypedModel::default();
        let input = model.add_source("input", f32::fact(weight.shape()))?;
        let konst = model.add_const("weight", weight)?;
        let sum = model.wire_node("sum", add(), &[input, konst])?;
        model.select_output_outlets(&sum)?;
        Ok(model)
    }

    fn run_once(model: TypedModel, input: Tensor) -> Result<Tensor> {
        let outputs = model.into_runnable()?.run(tvec!(input.into()))?;
        Ok(outputs[0].clone().into_tensor())
    }

    #[test]
    fn weights_keep_their_value_to_half_precision() -> Result<()> {
        let values: Vec<f32> = (0..WEIGHT_MIN_LEN).map(|i| i as f32 / 7.0).collect();
        let weight = Tensor::from_shape(&[WEIGHT_MIN_LEN], &values)?;
        let zeros = Tensor::zero::<f32>(&[WEIGHT_MIN_LEN])?;
        let halved = half_precision_weights(model_with_weight(weight)?)?;
        assert!(halved.nodes.iter().any(|node| {
            node.op_as::<Const>()
                .is_some_and(|k| k.val().datum_type() == f16::datum_type())
        }));
        let output = run_once(halved, zeros)?;
        for (got, want) in output.to_plain_array_view::<f32>()?.iter().zip(&values) {
            assert!((got - want).abs() <= want.abs() / 1024.0, "{got} vs {want}");
        }
        Ok(())
    }

    #[test]
    fn small_constants_stay_full_precision() -> Result<()> {
        let weight = Tensor::from_shape(&[3], &[1.0f32 / 3.0, 2.0, 3.0])?;
        let halved = half_precision_weights(model_with_weight(weight)?)?;
        assert!(!halved.nodes.iter().any(|node| {
            node.op_as::<Const>()
                .is_some_and(|k| k.val().datum_type() == f16::datum_type())
        }));
        Ok(())
    }

    #[test]
    fn a_packed_model_loads_back() -> Result<()> {
        let values: Vec<f32> = (0..WEIGHT_MIN_LEN).map(|i| i as f32).collect();
        let weight = Tensor::from_shape(&[WEIGHT_MIN_LEN], &values)?;
        let packed = pack(half_precision_weights(model_with_weight(weight)?)?)?;
        let loaded = tract_nnef::nnef().model_for_read(&mut &packed[..])?;
        let output = run_once(
            loaded.into_optimized()?,
            Tensor::zero::<f32>(&[WEIGHT_MIN_LEN])?,
        )?;
        assert_eq!(
            output
                .to_plain_array_view::<f32>()?
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            values
        );
        Ok(())
    }
}
