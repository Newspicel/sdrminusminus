use std::io::Read;

use crate::{COLLECTION_SUFFIX, DATA_SUFFIX, META_SUFFIX, SigmfError};

pub const ARCHIVE_SUFFIX: &str = ".sigmf";

const BLOCK: usize = 512;
const MAX_META_BYTES: u64 = 8 * 1024 * 1024;
const ONE_RECORDING: &str =
    "a .sigmf archive must hold one .sigmf-meta and one .sigmf-data, or a collection and its lanes";

pub struct Member {
    pub name: String,
    pub meta: Vec<u8>,
    pub data: Vec<u8>,
}

pub struct ArchivedCollection {
    pub name: String,
    pub collection: Vec<u8>,
    pub lanes: Vec<Member>,
}

pub enum Archive {
    Pair(Member),
    Collection(ArchivedCollection),
}

struct Found {
    name: String,
    meta: Option<Vec<u8>>,
    data: Option<Vec<u8>>,
}

#[derive(Default)]
struct Parts {
    collection: Option<(String, Vec<u8>)>,
    pairs: Vec<Found>,
}

impl Parts {
    fn take(&mut self, name: String, suffix: &str, body: Vec<u8>) -> Result<(), SigmfError> {
        if suffix == COLLECTION_SUFFIX {
            return match self.collection.replace((name, body)) {
                None => Ok(()),
                Some(_) => Err(SigmfError::Malformed(ONE_RECORDING.to_owned())),
            };
        }
        let index = match self.pairs.iter().position(|found| found.name == name) {
            Some(index) => index,
            None => {
                self.pairs.push(Found {
                    name,
                    meta: None,
                    data: None,
                });
                self.pairs.len() - 1
            }
        };
        let found = &mut self.pairs[index];
        let slot = if suffix == META_SUFFIX {
            &mut found.meta
        } else {
            &mut found.data
        };
        if slot.replace(body).is_some() {
            return Err(SigmfError::Malformed(format!(
                "the archive holds `{}{suffix}` twice",
                found.name
            )));
        }
        Ok(())
    }

    fn finish(self) -> Result<Archive, SigmfError> {
        let mut lanes = Vec::with_capacity(self.pairs.len());
        for found in self.pairs {
            match (found.meta, found.data) {
                (Some(meta), Some(data)) => lanes.push(Member {
                    name: found.name,
                    meta,
                    data,
                }),
                _ => return Err(SigmfError::Malformed(ONE_RECORDING.to_owned())),
            }
        }
        match (self.collection, lanes.len()) {
            (Some((name, collection)), _) => Ok(Archive::Collection(ArchivedCollection {
                name,
                collection,
                lanes,
            })),
            (None, 1) => lanes
                .pop()
                .map(Archive::Pair)
                .ok_or_else(|| SigmfError::Malformed(ONE_RECORDING.to_owned())),
            (None, _) => Err(SigmfError::Malformed(ONE_RECORDING.to_owned())),
        }
    }
}

pub fn read_archive(mut archive: impl Read) -> Result<Archive, SigmfError> {
    let mut parts = Parts::default();
    let mut header = [0u8; BLOCK];
    while fill(&mut archive, &mut header)? && header.iter().any(|byte| *byte != 0) {
        let entry = entry_name(&header)?;
        let size = entry_size(&header)?;
        let padded = size.div_ceil(BLOCK as u64) * BLOCK as u64;
        let Some(suffix) = member_suffix(&entry) else {
            skip(&mut archive, padded)?;
            continue;
        };
        if suffix != DATA_SUFFIX && size > MAX_META_BYTES {
            return Err(SigmfError::Malformed(
                "the archive's metadata is larger than any SigMF metadata".to_owned(),
            ));
        }
        let mut body = vec![0u8; size as usize];
        archive.read_exact(&mut body)?;
        skip(&mut archive, padded - size)?;
        parts.take(stem_of(&entry, suffix), suffix, body)?;
    }
    parts.finish()
}

fn member_suffix(entry: &str) -> Option<&'static str> {
    if entry.ends_with(COLLECTION_SUFFIX) {
        Some(COLLECTION_SUFFIX)
    } else if entry.ends_with(META_SUFFIX) {
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

    const TRAILER: usize = 2 * BLOCK;

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

        let Archive::Pair(member) = read_archive(bytes.as_slice()).expect("reads back") else {
            panic!("one recording");
        };
        assert_eq!(member.name, "round-trip");
        assert_eq!(member.data.len(), 128 * 8);
        let meta: crate::SigmfMeta = serde_json::from_slice(&member.meta).expect("meta");
        assert_eq!(meta.global.sample_rate, Some(48_000.0));
    }

    #[test]
    fn a_collection_archive_reads_back_with_every_lane() {
        let dir = TempDir::new().expect("temp");
        let stem = crate::library::tests::collection(dir.path(), "bank", 3, 40);
        let mut export = Export::open(&stem, ExportKind::SigmfArchive).expect("export");
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut export, &mut bytes).expect("read");

        let Archive::Collection(collection) = read_archive(bytes.as_slice()).expect("reads back")
        else {
            panic!("a collection");
        };
        assert_eq!(collection.name, "bank");
        assert_eq!(
            collection.collection,
            std::fs::read(crate::collection_path(&stem)).expect("collection")
        );
        let lanes: Vec<(&str, usize)> = collection
            .lanes
            .iter()
            .map(|lane| (lane.name.as_str(), lane.data.len()))
            .collect();
        assert_eq!(
            lanes,
            [
                ("bank-lane0", 320),
                ("bank-lane1", 320),
                ("bank-lane2", 320)
            ]
        );
    }

    #[test]
    fn two_recordings_without_a_collection_are_refused() {
        let dir = TempDir::new().expect("temp");
        let stem = crate::library::tests::collection(dir.path(), "bank", 2, 8);
        std::fs::remove_file(crate::collection_path(&stem)).expect("drop the collection");
        let mut bytes = Vec::new();
        for lane in 0..2 {
            let lane = crate::lane_stem(&stem, lane);
            let mut export = Export::open(&lane, ExportKind::SigmfArchive).expect("export");
            std::io::Read::read_to_end(&mut export, &mut bytes).expect("read");
            bytes.truncate(bytes.len() - TRAILER);
        }
        bytes.extend_from_slice(&[0u8; TRAILER]);
        assert!(matches!(
            read_archive(bytes.as_slice()),
            Err(SigmfError::Malformed(_))
        ));
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
