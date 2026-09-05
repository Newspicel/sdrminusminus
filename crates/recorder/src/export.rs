mod sigmf_archive;
mod wav;

use std::{
    collections::VecDeque,
    fs::File,
    io::{self, Cursor, Read},
    path::Path,
    time::UNIX_EPOCH,
};

use crate::SigmfError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind {
    SigmfArchive,
    Wav,
}

impl ExportKind {
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::SigmfArchive => ".sigmf",
            Self::Wav => ".wav",
        }
    }

    #[must_use]
    pub fn content_type(self) -> &'static str {
        match self {
            Self::SigmfArchive => "application/x-tar",
            Self::Wav => "audio/wav",
        }
    }
}

#[derive(Debug)]
pub struct Export {
    parts: VecDeque<Part>,
    byte_len: u64,
    kind: ExportKind,
    file_name: String,
}

impl Export {
    pub fn open(stem: &Path, kind: ExportKind) -> Result<Self, SigmfError> {
        let name = stem
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| SigmfError::Unexportable {
                stem: stem.to_path_buf(),
                format: kind.extension(),
                reason: "the recording has no usable file name",
            })?;
        let parts = match kind {
            ExportKind::SigmfArchive => sigmf_archive::parts(stem, name)?,
            ExportKind::Wav => wav::parts(stem)?,
        };
        Ok(Self {
            byte_len: parts.iter().map(Part::len).sum(),
            parts,
            kind,
            file_name: format!("{name}{}", kind.extension()),
        })
    }

    #[must_use]
    pub fn byte_len(&self) -> u64 {
        self.byte_len
    }

    #[must_use]
    pub fn file_name(&self) -> &str {
        &self.file_name
    }

    #[must_use]
    pub fn content_type(&self) -> &'static str {
        self.kind.content_type()
    }
}

impl Read for Export {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        while let Some(part) = self.parts.front_mut() {
            let read = match part {
                Part::Bytes(cursor) => cursor.read(buf)?,
                Part::File { file, remaining } => read_capped(file, buf, remaining)?,
            };
            if read > 0 {
                return Ok(read);
            }
            self.parts.pop_front();
        }
        Ok(0)
    }
}

#[derive(Debug)]
enum Part {
    Bytes(Cursor<Vec<u8>>),
    File { file: File, remaining: u64 },
}

impl Part {
    fn bytes(bytes: Vec<u8>) -> Self {
        Self::Bytes(Cursor::new(bytes))
    }

    fn file(file: File, len: u64) -> Self {
        Self::File {
            file,
            remaining: len,
        }
    }

    fn len(&self) -> u64 {
        match self {
            Self::Bytes(cursor) => cursor.get_ref().len() as u64,
            Self::File { remaining, .. } => *remaining,
        }
    }
}

fn read_capped(file: &mut File, buf: &mut [u8], remaining: &mut u64) -> io::Result<usize> {
    if *remaining == 0 {
        return Ok(0);
    }
    let cap = buf
        .len()
        .min(usize::try_from(*remaining).unwrap_or(usize::MAX));
    let read = file.read(&mut buf[..cap])?;
    if read == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("recording shrank mid-export: {remaining} bytes short"),
        ));
    }
    *remaining -= read as u64;
    Ok(read)
}

fn open_pinned(path: &Path) -> io::Result<(File, u64, u64)> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_secs());
    Ok((file, metadata.len(), mtime))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use num_complex::Complex;
    use tempfile::TempDir;

    use super::*;
    use crate::{SigmfWriter, data_path};

    pub(super) fn recording(dir: &TempDir, name: &str, n: usize, rate: f64) -> std::path::PathBuf {
        let stem = dir.path().join(name);
        let mut writer = SigmfWriter::create(&stem, rate, 100_000_000.0, "Signal Generator")
            .expect("create writer");
        let samples: Vec<_> = (0..n)
            .map(|i| Complex::new(i as f32 * 0.5, i as f32 * -0.25))
            .collect();
        writer.write_block(&samples).expect("write");
        writer.finalize().expect("finalize");
        stem
    }

    pub(super) fn drain(export: &mut Export) -> Vec<u8> {
        let mut out = Vec::new();
        export.read_to_end(&mut out).expect("read export");
        out
    }

    #[test]
    fn byte_len_matches_what_every_container_actually_streams() {
        let dir = TempDir::new().expect("tempdir");
        let stem = recording(&dir, "sized", 300, 48_000.0);

        for kind in [ExportKind::SigmfArchive, ExportKind::Wav] {
            let mut export = Export::open(&stem, kind).expect("open");
            let promised = export.byte_len();
            assert_eq!(
                drain(&mut export).len() as u64,
                promised,
                "{kind:?} streamed a different length than it promised"
            );
        }
    }

    #[test]
    fn names_and_content_types_follow_the_container() {
        let dir = TempDir::new().expect("tempdir");
        let stem = recording(&dir, "rec_1_19700101T000000Z", 4, 48_000.0);

        let archive = Export::open(&stem, ExportKind::SigmfArchive).expect("open");
        assert_eq!(archive.file_name(), "rec_1_19700101T000000Z.sigmf");
        assert_eq!(archive.content_type(), "application/x-tar");

        let wav = Export::open(&stem, ExportKind::Wav).expect("open");
        assert_eq!(wav.file_name(), "rec_1_19700101T000000Z.wav");
        assert_eq!(wav.content_type(), "audio/wav");
    }

    #[test]
    fn a_payload_that_shrinks_mid_stream_fails_instead_of_truncating() {
        let dir = TempDir::new().expect("tempdir");

        for (n, kind) in [ExportKind::SigmfArchive, ExportKind::Wav]
            .into_iter()
            .enumerate()
        {
            let stem = recording(&dir, &format!("vanishing{n}"), 8_192, 48_000.0);
            let mut export = Export::open(&stem, kind).expect("open");
            File::options()
                .write(true)
                .open(data_path(&stem))
                .expect("reopen data")
                .set_len(16)
                .expect("truncate");

            let mut sink = Vec::new();
            let err = export.read_to_end(&mut sink).expect_err("must not succeed");
            assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof, "{kind:?}");
        }
    }

    #[test]
    fn a_missing_pair_fails_at_open() {
        let dir = TempDir::new().expect("tempdir");
        let stem = dir.path().join("never-recorded");
        for kind in [ExportKind::SigmfArchive, ExportKind::Wav] {
            assert!(matches!(
                Export::open(&stem, kind),
                Err(SigmfError::Io(_)) | Err(SigmfError::Meta(_))
            ));
        }
    }

    #[test]
    fn a_torn_trailing_sample_is_dropped_from_the_wav() {
        let dir = TempDir::new().expect("tempdir");
        let stem = recording(&dir, "torn", 10, 48_000.0);
        File::options()
            .write(true)
            .open(data_path(&stem))
            .expect("reopen data")
            .set_len(10 * crate::BYTES_PER_SAMPLE + 3)
            .expect("truncate");

        let wav = Export::open(&stem, ExportKind::Wav).expect("open");
        assert_eq!(
            wav.byte_len(),
            super::wav::header_len(false) + 10 * crate::BYTES_PER_SAMPLE
        );
    }

    #[test]
    fn a_name_too_long_for_a_tar_header_is_refused() {
        let dir = TempDir::new().expect("tempdir");
        let long = "n".repeat(90);
        let stem = recording(&dir, &long, 4, 48_000.0);

        assert!(matches!(
            Export::open(&stem, ExportKind::SigmfArchive),
            Err(SigmfError::Unexportable { .. })
        ));
        assert!(Export::open(&stem, ExportKind::Wav).is_ok());
    }

    #[test]
    fn a_meta_without_a_sample_rate_cannot_become_a_wav() {
        let dir = TempDir::new().expect("tempdir");
        let stem = dir.path().join("rateless");
        std::fs::write(
            crate::meta_path(&stem),
            r#"{"global":{"core:datatype":"cf32_le","core:version":"1.2.6"},"captures":[]}"#,
        )
        .expect("write meta");
        std::fs::File::create(data_path(&stem))
            .expect("create data")
            .write_all(&[0; 16])
            .expect("write data");

        assert!(matches!(
            Export::open(&stem, ExportKind::Wav),
            Err(SigmfError::Unexportable { .. })
        ));
        assert!(Export::open(&stem, ExportKind::SigmfArchive).is_ok());
    }
}
