//! SigMF Archive: a recording's pair packed into one `.sigmf` file.
//!
//! SigMF v1.2.6 "Rules for SigMF Archive files" fixes only the outside — POSIX.1-2001 `tar`,
//! the `.sigmf` extension, and members that still satisfy the per-file rules once extracted —
//! so the layout written here is the one the other SigMF tools use: a directory named after
//! the recording holding its `.sigmf-meta` and `.sigmf-data`.

use std::{collections::VecDeque, fs::File, path::Path};

use super::{ExportKind, Part, open_pinned};
use crate::{DATA_SUFFIX, META_SUFFIX, SigmfError, data_path, meta_path};

/// Tar block size: a header is one block, and every payload is padded up to a whole number.
const BLOCK: u64 = 512;
/// Two zero blocks mark end-of-archive (POSIX.1-2001).
const TRAILER: usize = 2 * BLOCK as usize;

/// Longest recording name the header fields can carry. Members are named
/// `<name>/<name>.sigmf-data`, split across `prefix` (155 bytes) and `name` (100); the file
/// part binds first, so the limit is 100 less the longest suffix.
const MAX_NAME: usize = 100 - DATA_SUFFIX.len();

const TYPE_FILE: u8 = b'0';
const TYPE_DIR: u8 = b'5';

pub(super) fn parts(stem: &Path, name: &str) -> Result<VecDeque<Part>, SigmfError> {
    if name.len() > MAX_NAME {
        return Err(SigmfError::Unexportable {
            stem: stem.to_path_buf(),
            format: ExportKind::SigmfArchive.extension(),
            reason: "the name is longer than a tar header can hold",
        });
    }
    let (meta_file, meta_len, mtime) = open_pinned(&meta_path(stem))?;
    let (data_file, data_len, _) = open_pinned(&data_path(stem))?;

    let mut parts = VecDeque::new();
    parts.push_back(Part::bytes(header(
        &format!("{name}/"),
        "",
        0,
        mtime,
        0o755,
        TYPE_DIR,
    )));
    push_member(&mut parts, name, META_SUFFIX, meta_file, meta_len, mtime);
    push_member(&mut parts, name, DATA_SUFFIX, data_file, data_len, mtime);
    parts.push_back(Part::bytes(vec![0; TRAILER]));
    Ok(parts)
}

/// One member: its header block, its bytes verbatim, and the padding up to a whole block.
fn push_member(
    parts: &mut VecDeque<Part>,
    name: &str,
    suffix: &str,
    file: File,
    len: u64,
    mtime: u64,
) {
    parts.push_back(Part::bytes(header(
        &format!("{name}{suffix}"),
        name,
        len,
        mtime,
        0o644,
        TYPE_FILE,
    )));
    parts.push_back(Part::file(file, len));
    let padding = (BLOCK - len % BLOCK) % BLOCK;
    if padding > 0 {
        parts.push_back(Part::bytes(vec![0; padding as usize]));
    }
}

/// One `ustar` header block. A path too long for the 100-byte `name` field is expressed by
/// moving its directory into `prefix`, which readers rejoin with a `/`.
fn header(name: &str, prefix: &str, size: u64, mtime: u64, mode: u32, kind: u8) -> Vec<u8> {
    let mut block = vec![0u8; BLOCK as usize];
    block[..name.len()].copy_from_slice(name.as_bytes());
    write_numeric(&mut block[100..108], u64::from(mode));
    write_numeric(&mut block[108..116], 0);
    write_numeric(&mut block[116..124], 0);
    write_numeric(&mut block[124..136], size);
    write_numeric(&mut block[136..148], mtime);
    block[156] = kind;
    block[257..263].copy_from_slice(b"ustar\0");
    block[263..265].copy_from_slice(b"00");
    block[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    write_checksum(&mut block);
    block
}

/// The checksum counts the header's own field as spaces, and is written back as six octal
/// digits, a NUL and a space (POSIX.1-2001 `ustar` header block).
fn write_checksum(block: &mut [u8]) {
    block[148..156].fill(b' ');
    let sum: u32 = block.iter().map(|&byte| u32::from(byte)).sum();
    write_numeric(&mut block[148..155], u64::from(sum));
    block[155] = b' ';
}

/// A `ustar` numeric field: zero-padded octal in `field.len() - 1` digits, NUL-terminated.
/// `size` outgrows its eleven digits past 8 GiB — minutes of I/Q at any real rate — so a wide
/// field falls back to the base-256 form GNU tar, bsdtar and Python's `tarfile` all read,
/// POSIX having left the overflow itself undefined. Only the 12-byte `size` and `mtime` can
/// reach that branch; the 8-byte fields carry constants that always fit.
fn write_numeric(field: &mut [u8], value: u64) {
    let digits = field.len() - 1;
    let octal = format!("{value:o}");
    if let Some(pad) = digits.checked_sub(octal.len()) {
        field[..pad].fill(b'0');
        field[pad..digits].copy_from_slice(octal.as_bytes());
        field[digits] = 0;
    } else if let Some(start) = field
        .len()
        .checked_sub(size_of::<u64>())
        .filter(|&start| start > 0)
    {
        field.fill(0);
        field[start..].copy_from_slice(&value.to_be_bytes());
        field[0] = 0x80;
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::export::{
        Export, ExportKind,
        tests::{drain, recording},
    };

    /// One parsed member, enough to prove the archive is a tar and not merely tar-shaped.
    #[derive(Debug)]
    struct Member {
        path: String,
        size: u64,
        kind: u8,
        data: Vec<u8>,
    }

    /// A deliberately strict reader: it validates the checksum and the `ustar` magic on every
    /// block, so a header this crate writes wrongly fails here rather than in someone's tar.
    fn parse(bytes: &[u8]) -> Vec<Member> {
        let mut members = Vec::new();
        let mut at = 0;
        loop {
            let block = &bytes[at..at + BLOCK as usize];
            at += BLOCK as usize;
            if block.iter().all(|&byte| byte == 0) {
                let trailer = &bytes[at..];
                assert_eq!(
                    block.len() + trailer.len(),
                    TRAILER,
                    "trailer is two blocks"
                );
                assert!(trailer.iter().all(|&byte| byte == 0), "trailer is zeroed");
                return members;
            }
            assert_eq!(&block[257..263], b"ustar\0", "ustar magic");
            assert_eq!(&block[263..265], b"00", "ustar version");

            let mut zeroed = block.to_vec();
            zeroed[148..156].fill(b' ');
            let expected: u32 = zeroed.iter().map(|&byte| u32::from(byte)).sum();
            assert_eq!(octal(&block[148..155]), u64::from(expected), "checksum");

            let name = text(&block[..100]);
            let prefix = text(&block[345..500]);
            let size = octal(&block[124..136]);
            let data = bytes[at..at + size as usize].to_vec();
            at += (size as usize).next_multiple_of(BLOCK as usize);
            members.push(Member {
                path: if prefix.is_empty() {
                    name
                } else {
                    format!("{prefix}/{name}")
                },
                size,
                kind: block[156],
                data,
            });
        }
    }

    fn text(field: &[u8]) -> String {
        let end = field
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(field.len());
        String::from_utf8(field[..end].to_vec()).expect("utf8 field")
    }

    fn octal(field: &[u8]) -> u64 {
        let text = text(field);
        u64::from_str_radix(text.trim_end_matches(' ').trim(), 8).expect("octal field")
    }

    #[test]
    fn archive_holds_the_pair_under_a_directory_named_for_the_recording() {
        let dir = TempDir::new().expect("tempdir");
        let stem = recording(&dir, "rec_2_19700101T000000Z", 1_000, 48_000.0);
        let mut export = Export::open(&stem, ExportKind::SigmfArchive).expect("open");

        let members = parse(&drain(&mut export));
        assert_eq!(members.len(), 3);
        assert_eq!(members[0].path, "rec_2_19700101T000000Z/");
        assert_eq!(members[0].kind, TYPE_DIR);
        assert_eq!(members[0].size, 0);

        assert_eq!(
            members[1].path,
            "rec_2_19700101T000000Z/rec_2_19700101T000000Z.sigmf-meta"
        );
        assert_eq!(members[1].kind, TYPE_FILE);
        assert_eq!(
            members[1].data,
            std::fs::read(meta_path(&stem)).expect("read meta")
        );

        assert_eq!(
            members[2].path,
            "rec_2_19700101T000000Z/rec_2_19700101T000000Z.sigmf-data"
        );
        assert_eq!(members[2].size, 1_000 * crate::BYTES_PER_SAMPLE);
        assert_eq!(
            members[2].data,
            std::fs::read(data_path(&stem)).expect("read data")
        );
    }

    /// An archive preserves what is on disk, torn tail included — unlike the WAV export,
    /// which cannot represent a partial frame.
    #[test]
    fn a_torn_data_file_is_archived_verbatim() {
        let dir = TempDir::new().expect("tempdir");
        let stem = recording(&dir, "torn", 10, 48_000.0);
        let torn = 10 * crate::BYTES_PER_SAMPLE + 3;
        File::options()
            .write(true)
            .open(data_path(&stem))
            .expect("reopen data")
            .set_len(torn)
            .expect("truncate");

        let mut export = Export::open(&stem, ExportKind::SigmfArchive).expect("open");
        let members = parse(&drain(&mut export));
        assert_eq!(members[2].size, torn);
        assert_eq!(members[2].data.len() as u64, torn);
    }

    /// Padding must land the next header on a block boundary whatever the payload length is.
    #[test]
    fn members_of_any_length_stay_block_aligned() {
        let dir = TempDir::new().expect("tempdir");
        for samples in [0, 1, 63, 64, 65, 512] {
            let stem = recording(&dir, &format!("len{samples}"), samples, 48_000.0);
            let mut export = Export::open(&stem, ExportKind::SigmfArchive).expect("open");
            let bytes = drain(&mut export);
            assert_eq!(bytes.len() % BLOCK as usize, 0, "{samples} samples");
            assert_eq!(parse(&bytes).len(), 3, "{samples} samples");
        }
    }

    /// 8 GiB is where the octal `size` field runs out, and an I/Q recording passes it in
    /// minutes; the base-256 escape is what keeps those archives readable.
    #[test]
    fn sizes_past_the_octal_field_use_the_base_256_form() {
        let mut field = [0u8; 12];
        write_numeric(&mut field, 0o777_7777_7777);
        assert_eq!(
            &field, b"77777777777\0",
            "the largest octal value still fits"
        );

        let huge = 0o777_7777_7777 + 1;
        write_numeric(&mut field, huge);
        assert_eq!(field[0], 0x80, "base-256 flag");
        assert_eq!(
            u64::from_be_bytes(field[4..12].try_into().expect("8 bytes")),
            huge
        );
    }

    #[test]
    fn numeric_fields_are_zero_padded_and_nul_terminated() {
        let mut field = [0u8; 8];
        write_numeric(&mut field, 0o644);
        assert_eq!(&field, b"0000644\0");
    }
}
