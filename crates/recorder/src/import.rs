use std::{
    fs::{self, File},
    io::{BufWriter, Read, Write},
    path::{Path, PathBuf},
};

use crate::{
    DATA_SUFFIX, DATATYPE_CF32_LE, META_SUFFIX, SIGMF_VERSION, SigmfError, SigmfMeta, data_path,
    meta_path,
};

mod archive;
mod datatype;

pub use archive::{ARCHIVE_SUFFIX, Member, read_archive};
pub use datatype::Datatype;

pub const MAX_IMPORT_NAME_LEN: usize = 96;

const CHUNK_SAMPLES: usize = 64 * 1024;

#[derive(Debug)]
pub struct Imported {
    pub stem: PathBuf,
    pub samples: u64,
}

pub fn import_pair(
    dir: &Path,
    name: &str,
    meta_json: &[u8],
    data: impl Read,
) -> Result<Imported, SigmfError> {
    let name = sanitize(name)?;
    let mut meta: SigmfMeta = serde_json::from_slice(meta_json)?;
    let datatype = Datatype::parse(&meta.global.datatype)
        .ok_or_else(|| SigmfError::UnsupportedDatatype(meta.global.datatype.clone()))?;
    fs::create_dir_all(dir)?;
    let stem = unique_stem(dir, &name);
    let samples = match convert(&stem, datatype, data) {
        Ok(samples) => samples,
        Err(err) => {
            let _ = fs::remove_file(data_path(&stem));
            return Err(err);
        }
    };

    meta.global.datatype = DATATYPE_CF32_LE.to_owned();
    if meta.global.version.is_empty() {
        meta.global.version = SIGMF_VERSION.to_owned();
    }
    if meta.global.name.is_none() {
        meta.global.name = Some(name.clone());
    }
    if let Err(err) = fs::write(meta_path(&stem), serde_json::to_vec_pretty(&meta)?) {
        let _ = fs::remove_file(data_path(&stem));
        return Err(err.into());
    }
    Ok(Imported { stem, samples })
}

pub fn import_archive(dir: &Path, archive: impl Read) -> Result<Imported, SigmfError> {
    let Member { name, meta, data } = read_archive(archive)?;
    import_pair(dir, &name, &meta, data.as_slice())
}

pub fn sanitize(name: &str) -> Result<String, SigmfError> {
    let trimmed = name.trim();
    let base = trimmed
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(trimmed)
        .trim_end_matches(ARCHIVE_SUFFIX)
        .trim_end_matches(META_SUFFIX)
        .trim_end_matches(DATA_SUFFIX);
    let kept: String = base
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .take(MAX_IMPORT_NAME_LEN)
        .collect();
    let kept = kept.trim_matches('.').to_owned();
    if kept.is_empty() {
        return Err(SigmfError::Malformed(
            "the upload carries no usable file name".to_owned(),
        ));
    }
    Ok(kept)
}

fn unique_stem(dir: &Path, name: &str) -> PathBuf {
    let mut candidate = dir.join(name);
    let mut n = 2;
    while meta_path(&candidate).exists() || data_path(&candidate).exists() {
        candidate = dir.join(format!("{name}-{n}"));
        n += 1;
    }
    candidate
}

fn convert(stem: &Path, datatype: Datatype, mut data: impl Read) -> Result<u64, SigmfError> {
    let mut out = BufWriter::new(File::create(data_path(stem))?);
    let stride = datatype.bytes_per_sample();
    let mut input = vec![0u8; CHUNK_SAMPLES * stride];
    let mut encoded = Vec::with_capacity(CHUNK_SAMPLES * 8);
    let mut held = 0usize;
    let mut samples = 0u64;
    loop {
        let read = data.read(&mut input[held..])?;
        if read == 0 {
            break;
        }
        held += read;
        let whole = held - held % stride;
        encoded.clear();
        for chunk in input[..whole].chunks_exact(stride) {
            let (re, im) = datatype.sample(chunk);
            encoded.extend_from_slice(&re.to_le_bytes());
            encoded.extend_from_slice(&im.to_le_bytes());
        }
        out.write_all(&encoded)?;
        samples += (whole / stride) as u64;
        input.copy_within(whole..held, 0);
        held -= whole;
    }
    out.flush()?;
    if samples == 0 {
        return Err(SigmfError::Malformed(
            "the upload carries no samples".to_owned(),
        ));
    }
    Ok(samples)
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;
    use tempfile::TempDir;

    use super::*;
    use crate::{Export, ExportKind, SigmfReader, SigmfWriter};

    fn meta_json(datatype: &str) -> Vec<u8> {
        serde_json::json!({
            "global": {
                "core:datatype": datatype,
                "core:version": "1.2.6",
                "core:sample_rate": 250_000.0,
            },
            "captures": [{ "core:sample_start": 0, "core:frequency": 100e6 }],
        })
        .to_string()
        .into_bytes()
    }

    fn ci16(samples: &[(i16, i16)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for (re, im) in samples {
            bytes.extend_from_slice(&re.to_le_bytes());
            bytes.extend_from_slice(&im.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn a_ci16_upload_is_stored_as_the_cf32_the_player_reads() {
        let dir = TempDir::new().expect("temp");
        let data = ci16(&[(16_384, -16_384), (0, 32_767)]);
        let imported = import_pair(
            dir.path(),
            "airband.sigmf-meta",
            &meta_json("ci16_le"),
            data.as_slice(),
        )
        .expect("imports");
        assert_eq!(imported.samples, 2);
        assert_eq!(
            imported.stem.file_name().and_then(|n| n.to_str()),
            Some("airband")
        );

        let mut reader = SigmfReader::open(&imported.stem).expect("plays back");
        assert_eq!(reader.meta().global.sample_rate, Some(250_000.0));
        assert_eq!(reader.meta().captures[0].frequency, Some(100e6));
        let mut block = [Complex::new(0.0f32, 0.0); 2];
        assert_eq!(reader.read_block(&mut block).expect("read"), 2);
        assert!((block[0].re - 0.5).abs() < 1e-3, "{:?}", block[0]);
        assert!((block[0].im + 0.5).abs() < 1e-3, "{:?}", block[0]);
    }

    #[test]
    fn a_cf32_upload_crosses_over_bit_exact() {
        let dir = TempDir::new().expect("temp");
        let sent = [Complex::new(0.125f32, -0.25), Complex::new(-1.0, 0.75)];
        let mut data = Vec::new();
        for sample in sent {
            data.extend_from_slice(&sample.re.to_le_bytes());
            data.extend_from_slice(&sample.im.to_le_bytes());
        }
        let imported = import_pair(dir.path(), "exact", &meta_json("cf32_le"), data.as_slice())
            .expect("imports");
        let mut reader = SigmfReader::open(&imported.stem).expect("plays back");
        let mut block = [Complex::new(0.0f32, 0.0); 2];
        reader.read_block(&mut block).expect("read");
        assert_eq!(block, sent);
    }

    #[test]
    fn an_upload_never_lands_on_a_recording_that_is_already_there() {
        let dir = TempDir::new().expect("temp");
        let data = ci16(&[(1, 1)]);
        let first =
            import_pair(dir.path(), "same", &meta_json("ci16_le"), data.as_slice()).expect("first");
        let second = import_pair(dir.path(), "same", &meta_json("ci16_le"), data.as_slice())
            .expect("second");
        assert_ne!(first.stem, second.stem);
        assert_eq!(
            second.stem.file_name().and_then(|n| n.to_str()),
            Some("same-2")
        );
    }

    #[test]
    fn a_datatype_no_player_can_read_leaves_nothing_behind() {
        let dir = TempDir::new().expect("temp");
        let err = import_pair(
            dir.path(),
            "real",
            &meta_json("rf32_le"),
            [0u8; 8].as_slice(),
        )
        .expect_err("real samples are not IQ");
        assert!(matches!(err, SigmfError::UnsupportedDatatype(_)));
        assert!(scan_stems_empty(dir.path()));
    }

    #[test]
    fn an_upload_with_no_samples_leaves_nothing_behind() {
        let dir = TempDir::new().expect("temp");
        let err = import_pair(dir.path(), "empty", &meta_json("ci16_le"), [].as_slice())
            .expect_err("no samples");
        assert!(matches!(err, SigmfError::Malformed(_)));
        assert!(scan_stems_empty(dir.path()));
    }

    fn scan_stems_empty(dir: &Path) -> bool {
        crate::scan_stems(dir).expect("scan").is_empty()
            && std::fs::read_dir(dir)
                .expect("read dir")
                .filter_map(Result::ok)
                .all(|entry| entry.path().extension().is_none())
    }

    #[test]
    fn an_archive_lands_in_the_library_under_its_own_name() {
        let source = TempDir::new().expect("temp");
        let stem = source.path().join("from-archive");
        let mut writer =
            SigmfWriter::create(&stem, 48_000.0, 100e6, "upload fixture").expect("create");
        writer
            .write_block(&[Complex::new(0.5f32, -0.5); 64])
            .expect("write");
        writer.finalize().expect("finalize");
        let mut export = Export::open(&stem, ExportKind::SigmfArchive).expect("export");
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut export, &mut bytes).expect("read");

        let library = TempDir::new().expect("temp");
        let imported = import_archive(library.path(), bytes.as_slice()).expect("imports");
        assert_eq!(
            imported.stem.file_name().and_then(|n| n.to_str()),
            Some("from-archive")
        );
        assert_eq!(imported.samples, 64);
    }

    #[test]
    fn a_name_that_reaches_out_of_the_library_is_flattened() {
        assert_eq!(sanitize("../../etc/passwd").expect("kept"), "passwd");
        assert_eq!(sanitize("sub/dir/take.sigmf").expect("kept"), "take");
        assert_eq!(sanitize("odd name!.sigmf-data").expect("kept"), "odd_name_");
        assert_eq!(
            sanitize(&"x".repeat(500)).expect("kept").len(),
            MAX_IMPORT_NAME_LEN
        );
        assert!(sanitize("  ").is_err());
        assert!(sanitize("..").is_err());
    }
}
