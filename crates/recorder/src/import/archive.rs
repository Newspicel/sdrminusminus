use std::io::Read;

use crate::{DATA_SUFFIX, META_SUFFIX, SigmfError};

pub const ARCHIVE_SUFFIX: &str = ".sigmf";

const BLOCK: usize = 512;
const MAX_META_BYTES: u64 = 8 * 1024 * 1024;

pub struct Member {
    pub name: String,
    pub meta: Vec<u8>,
    pub data: Vec<u8>,
}

pub fn read_archive(mut archive: impl Read) -> Result<Member, SigmfError> {
    let mut name = None;
    let mut meta = None;
    let mut data = None;
    let mut header = [0u8; BLOCK];
    loop {
        if !fill(&mut archive, &mut header)? {
            break;
        }
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let entry = entry_name(&header)?;
        let size = entry_size(&header)?;
        let padded = size.div_ceil(BLOCK as u64) * BLOCK as u64;
        let member = member_suffix(&entry);
        match member {
            Some(META_SUFFIX) if size > MAX_META_BYTES => {
                return Err(SigmfError::Malformed(
                    "the archive's metadata is larger than any SigMF metadata".to_owned(),
                ));
            }
            Some(suffix) => {
                let mut body = vec![0u8; size as usize];
                archive.read_exact(&mut body)?;
                skip(&mut archive, padded - size)?;
                name.get_or_insert_with(|| stem_of(&entry, suffix));
                if suffix == META_SUFFIX {
                    meta = Some(body);
                } else {
                    data = Some(body);
                }
            }
            None => skip(&mut archive, padded)?,
        }
        if meta.is_some() && data.is_some() {
            break;
        }
    }
    match (name, meta, data) {
        (Some(name), Some(meta), Some(data)) => Ok(Member { name, meta, data }),
        _ => Err(SigmfError::Malformed(
            "a .sigmf archive must hold one .sigmf-meta and one .sigmf-data".to_owned(),
        )),
    }
}

fn member_suffix(entry: &str) -> Option<&'static str> {
    if entry.ends_with(META_SUFFIX) {
        Some(META_SUFFIX)
    } else if entry.ends_with(DATA_SUFFIX) {
        Some(DATA_SUFFIX)
    } else {
        None
    }
}

fn stem_of(entry: &str, suffix: &str) -> String {
    entry
        .trim_end_matches(suffix)
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(entry)
        .to_owned()
}

fn fill(source: &mut impl Read, block: &mut [u8; BLOCK]) -> Result<bool, SigmfError> {
    let mut filled = 0;
    while filled < BLOCK {
        let read = source.read(&mut block[filled..])?;
        if read == 0 {
            return Ok(false);
        }
        filled += read;
    }
    Ok(true)
}

fn skip(source: &mut impl Read, mut bytes: u64) -> Result<(), SigmfError> {
    let mut sink = [0u8; BLOCK];
    while bytes > 0 {
        let want = bytes.min(BLOCK as u64) as usize;
        source.read_exact(&mut sink[..want])?;
        bytes -= want as u64;
    }
    Ok(())
}

fn entry_name(header: &[u8; BLOCK]) -> Result<String, SigmfError> {
    let prefix = trimmed(&header[345..500]);
    let name = trimmed(&header[..100]);
    if name.is_empty() {
        return Err(SigmfError::Malformed(
            "the archive holds an entry with no name".to_owned(),
        ));
    }
    Ok(if prefix.is_empty() {
        name
    } else {
        format!("{prefix}/{name}")
    })
}

fn entry_size(header: &[u8; BLOCK]) -> Result<u64, SigmfError> {
    let raw = &header[124..136];
    if raw[0] & 0x80 != 0 {
        let start = raw.len() - size_of::<u64>();
        let mut bytes = [0u8; size_of::<u64>()];
        bytes.copy_from_slice(&raw[start..]);
        return Ok(u64::from_be_bytes(bytes));
    }
    u64::from_str_radix(&trimmed(raw), 8).map_err(|_| {
        SigmfError::Malformed("the archive holds an entry with no readable size".to_owned())
    })
}

fn trimmed(field: &[u8]) -> String {
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).trim().to_owned()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use num_complex::Complex;
    use tempfile::TempDir;

    use super::*;
    use crate::{Export, ExportKind, SigmfWriter};

    fn recorded(dir: &Path, name: &str) -> std::path::PathBuf {
        let stem = dir.join(name);
        let mut writer =
            SigmfWriter::create(&stem, 48_000.0, 100e6, "archive fixture").expect("create");
        writer
            .write_block(&[Complex::new(0.25f32, -0.5); 128])
            .expect("write");
        writer.finalize().expect("finalize");
        stem
    }

    #[test]
    fn an_archive_this_build_wrote_reads_back_whole() {
        let dir = TempDir::new().expect("temp");
        let stem = recorded(dir.path(), "round-trip");
        let mut export = Export::open(&stem, ExportKind::SigmfArchive).expect("export");
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut export, &mut bytes).expect("read");

        let member = read_archive(bytes.as_slice()).expect("reads back");
        assert_eq!(member.name, "round-trip");
        assert_eq!(member.data.len(), 128 * 8);
        let meta: crate::SigmfMeta = serde_json::from_slice(&member.meta).expect("meta");
        assert_eq!(meta.global.sample_rate, Some(48_000.0));
    }

    #[test]
    fn a_member_too_large_for_the_octal_size_field_is_still_read() {
        let mut header = [0u8; BLOCK];
        header[..12].copy_from_slice(b"huge.sigmf-d");
        header[12..17].copy_from_slice(b"ata\0\0");
        let size = 9_000_000_000u64;
        header[124] = 0x80;
        header[128..136].copy_from_slice(&size.to_be_bytes());
        assert_eq!(entry_size(&header).expect("base 256 size"), size);
    }

    #[test]
    fn an_archive_without_both_members_is_refused() {
        assert!(matches!(
            read_archive([0u8; BLOCK * 2].as_slice()),
            Err(SigmfError::Malformed(_))
        ));
        assert!(matches!(
            read_archive(b"not a tar at all".as_slice()),
            Err(SigmfError::Malformed(_)) | Err(SigmfError::Io(_))
        ));
    }
}
