//! `sdrmm-recorder` — SigMF v1.2.6 IO (: this crate only reads and writes SigMF
//! pairs; the recording tap lives in the engine, playback in `device-virtual`). One
//! recording is `<stem>.sigmf-meta` + `<stem>.sigmf-data`, mono-channel `cf32_le`.
mod export;

use std::{
    fs::{self, File},
    io::{BufReader, Read, Seek, Write},
    path::{Path, PathBuf},
};

pub use export::{Export, ExportKind};
use num_complex::Complex;
use sdrmm_wire::PositionFix;
use serde::{Deserialize, Serialize};

/// SigMF specification version written into `core:version`.
pub const SIGMF_VERSION: &str = "1.2.6";
pub const DATATYPE_CF32_LE: &str = "cf32_le";
/// Bytes per `cf32_le` sample (two little-endian `f32`).
pub const BYTES_PER_SAMPLE: u64 = 8;

const RECORDER_NAME: &str = "sdr--";
const META_SUFFIX: &str = ".sigmf-meta";
const DATA_SUFFIX: &str = ".sigmf-data";
const TMP_META_SUFFIX: &str = ".sigmf-meta.tmp";

#[derive(Debug, thiserror::Error)]
pub enum SigmfError {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("metadata: {0}")]
    Meta(#[from] serde_json::Error),
    #[error("unsupported datatype `{0}`: only {DATATYPE_CF32_LE} is supported")]
    UnsupportedDatatype(String),
    /// Another recording (in-flight breadcrumb, data file, or finalized pair) already owns
    /// this stem; the caller must pick a different one instead of sharing files.
    #[error("stem `{}` is already claimed by another recording", .0.display())]
    StemTaken(PathBuf),
    /// The recording is fine, but the requested export container cannot express it — see
    /// [`ExportKind`] for what each one carries.
    #[error("cannot export `{}` as {format}: {reason}", .stem.display())]
    Unexportable {
        stem: PathBuf,
        format: &'static str,
        reason: &'static str,
    },
}

/// The `global` object: recording-wide metadata. Field names carry the mandatory `core:`
/// namespace prefix on disk (SigMF v1.2.6 Global Object).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SigmfGlobal {
    #[serde(rename = "core:datatype")]
    pub datatype: String,
    /// Version of the SigMF specification the file conforms to, not of the recording.
    #[serde(rename = "core:version")]
    pub version: String,
    /// Samples per second. Global scope in SigMF, so one rate for the whole file — a rate
    /// change requires a new recording. Optional per SigMF v1.2.6 (only datatype and
    /// version are required): this build always writes it, but foreign metas may omit it,
    /// and consumers that need a rate (playback, indexing) must reject `None` themselves.
    #[serde(
        rename = "core:sample_rate",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub sample_rate: Option<f64>,
    #[serde(
        rename = "core:recorder",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub recorder: Option<String>,
    /// Free-text capture hardware description; this build writes the device label.
    #[serde(rename = "core:hw", default, skip_serializing_if = "Option::is_none")]
    pub hw: Option<String>,
    /// Which receive stream of a multi-stream radio the file captured (sdrmm extension
    /// namespace — SigMF core has no field for it). Absent in foreign files and in
    /// recordings that predate multi-stream devices; those are stream 0, the only stream a
    /// single-stream radio has.
    #[serde(
        rename = "sdrmm:rx_stream",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub rx_stream: Option<u32>,
}

/// One `captures` segment: the sample index where a tuning state begins (SigMF v1.2.6
/// Captures Array).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SigmfCapture {
    #[serde(rename = "core:sample_start")]
    pub sample_start: u64,
    /// Center frequency in Hz.
    #[serde(
        rename = "core:frequency",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub frequency: Option<f64>,
    /// RFC3339 UTC wall-clock start; this build writes it on segment 0 only.
    #[serde(
        rename = "core:datetime",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub datetime: Option<String>,
    #[serde(
        rename = "core:geolocation",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub geolocation: Option<SigmfGeolocation>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SigmfGeolocation {
    #[serde(rename = "type")]
    pub kind: String,
    pub coordinates: Vec<f64>,
}

/// A `.sigmf-meta` document. `annotations` is carried verbatim: this build writes none but
/// must not destroy foreign ones on a parse → serialize trip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SigmfMeta {
    pub global: SigmfGlobal,
    pub captures: Vec<SigmfCapture>,
    #[serde(default)]
    pub annotations: Vec<serde_json::Value>,
}

/// `<stem>.sigmf-meta` — exists only for finalized recordings.
#[must_use]
pub fn meta_path(stem: &Path) -> PathBuf {
    with_suffix(stem, META_SUFFIX)
}

/// `<stem>.sigmf-data`.
#[must_use]
pub fn data_path(stem: &Path) -> PathBuf {
    with_suffix(stem, DATA_SUFFIX)
}

fn tmp_meta_path(stem: &Path) -> PathBuf {
    with_suffix(stem, TMP_META_SUFFIX)
}

/// Appends to the file name instead of `Path::with_extension`, which would truncate a stem
/// containing dots.
fn with_suffix(stem: &Path, suffix: &str) -> PathBuf {
    let mut name = stem.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Parse `<stem>.sigmf-meta`.
pub(crate) fn read_meta(stem: &Path) -> Result<SigmfMeta, SigmfError> {
    Ok(serde_json::from_str(&fs::read_to_string(meta_path(stem))?)?)
}

/// Stems (directory-joined, extension-less) of every finalized recording in `dir`, sorted
/// by name. Keyed on the final `.sigmf-meta`, so in-progress and crashed recordings (which
/// have only the `.tmp` breadcrumb) never appear.
pub fn scan_stems(dir: &Path) -> Result<Vec<PathBuf>, SigmfError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut stems = Vec::new();
    for entry in entries {
        let name = entry?.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some(stem) = name.strip_suffix(META_SUFFIX) {
            stems.push(dir.join(stem));
        }
    }
    stems.sort();
    Ok(stems)
}

/// Streaming SigMF writer. Sample data goes straight to `<stem>.sigmf-data`; metadata lives
/// in memory (plus the `.tmp` breadcrumb) until [`SigmfWriter::finalize`].
#[derive(Debug)]
pub struct SigmfWriter {
    data: File,
    meta: SigmfMeta,
    stem: PathBuf,
    samples: u64,
    scratch: Vec<u8>,
}

impl SigmfWriter {
    pub fn create(
        stem: &Path,
        sample_rate: f64,
        center_hz: f64,
        hw: &str,
    ) -> Result<Self, SigmfError> {
        let meta = SigmfMeta {
            global: SigmfGlobal {
                datatype: DATATYPE_CF32_LE.to_string(),
                version: SIGMF_VERSION.to_string(),
                sample_rate: Some(sample_rate),
                recorder: Some(RECORDER_NAME.to_string()),
                hw: Some(hw.to_string()),
                rx_stream: None,
            },
            captures: vec![SigmfCapture {
                sample_start: 0,
                frequency: Some(center_hz),
                datetime: Some(jiff::Timestamp::now().to_string()),
                geolocation: None,
            }],
            annotations: Vec::new(),
        };
        let tmp = tmp_meta_path(stem);
        let tmp_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|err| claim_error(stem, err))?;
        if let Err(err) = write_meta_synced(tmp_file, &meta) {
            let _ = fs::remove_file(&tmp);
            return Err(err);
        }
        let data = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(data_path(stem))
        {
            Ok(data) => data,
            Err(err) => {
                let _ = fs::remove_file(&tmp);
                return Err(claim_error(stem, err));
            }
        };
        Ok(Self {
            data,
            meta,
            stem: stem.to_path_buf(),
            samples: 0,
            scratch: Vec::new(),
        })
    }

    pub fn write_block(&mut self, block: &[Complex<f32>]) -> Result<(), SigmfError> {
        self.scratch.clear();
        self.scratch
            .reserve(block.len() * BYTES_PER_SAMPLE as usize);
        for sample in block {
            self.scratch.extend_from_slice(&sample.re.to_le_bytes());
            self.scratch.extend_from_slice(&sample.im.to_le_bytes());
        }
        self.data.write_all(&self.scratch)?;
        self.samples += block.len() as u64;
        Ok(())
    }

    /// Record which receive stream of a multi-stream radio this file captures. Kept off
    /// [`SigmfWriter::create`] so its many single-stream callers stay untouched; the final
    /// meta carries the field, while the `.tmp` breadcrumb (written at create, before the
    /// stream is stamped) does not — a crashed recording is unplayable either way.
    pub fn set_rx_stream(&mut self, stream: u32) {
        self.meta.global.rx_stream = Some(stream);
    }

    /// Record a center retune as a capture segment starting at the next sample. A retune
    /// with no samples since the previous segment supersedes it: capture `sample_start`s
    /// must be unique and ascending (SigMF Captures Array).
    pub fn add_capture(&mut self, frequency_hz: f64) {
        if let Some(last) = self.meta.captures.last_mut()
            && last.sample_start == self.samples
        {
            last.frequency = Some(frequency_hz);
            return;
        }
        self.meta.captures.push(SigmfCapture {
            sample_start: self.samples,
            frequency: Some(frequency_hz),
            datetime: None,
            geolocation: None,
        });
    }

    pub fn set_position(&mut self, fix: Option<&PositionFix>) {
        let frequency = self
            .meta
            .captures
            .last()
            .and_then(|capture| capture.frequency);
        if self
            .meta
            .captures
            .last()
            .is_none_or(|capture| capture.sample_start != self.samples)
        {
            self.meta.captures.push(SigmfCapture {
                sample_start: self.samples,
                frequency,
                datetime: fix.map(|fix| fix.time.clone()),
                geolocation: None,
            });
        }
        if let Some(capture) = self.meta.captures.last_mut() {
            capture.geolocation = fix.map(|fix| {
                let mut coordinates = vec![fix.longitude, fix.latitude];
                if let Some(altitude) = fix.altitude_m {
                    coordinates.push(altitude);
                }
                SigmfGeolocation {
                    kind: "Point".to_owned(),
                    coordinates,
                }
            });
            if let Some(fix) = fix {
                capture.datetime = Some(fix.time.clone());
            }
        }
    }

    #[must_use]
    pub fn samples_written(&self) -> u64 {
        self.samples
    }

    #[must_use]
    pub fn bytes_written(&self) -> u64 {
        self.samples * BYTES_PER_SAMPLE
    }

    #[must_use]
    pub fn meta(&self) -> &SigmfMeta {
        &self.meta
    }

    #[must_use]
    pub fn stem(&self) -> &Path {
        &self.stem
    }

    /// Durably finish: sync the data, rewrite the breadcrumb with the final meta, and rename
    /// it onto `.sigmf-meta`. The rename is atomic (same directory), so a crash mid-finalize
    /// leaves breadcrumb-or-complete — never a listed final meta with torn JSON. Dropping a
    /// writer without calling this leaves the breadcrumb behind, which is exactly what marks
    /// a crashed recording as unplayable.
    pub fn finalize(self) -> Result<SigmfMeta, SigmfError> {
        self.data.sync_all()?;
        let tmp = tmp_meta_path(&self.stem);
        write_meta_synced(File::create(&tmp)?, &self.meta)?;
        fs::rename(&tmp, meta_path(&self.stem))?;
        Ok(self.meta)
    }
}

fn claim_error(stem: &Path, err: std::io::Error) -> SigmfError {
    if err.kind() == std::io::ErrorKind::AlreadyExists {
        SigmfError::StemTaken(stem.to_path_buf())
    } else {
        err.into()
    }
}

/// `sync_all` because `fs::write` alone is not power-loss durable — the finalize rename
/// must never promote a meta whose bytes could still vanish.
fn write_meta_synced(mut file: File, meta: &SigmfMeta) -> Result<(), SigmfError> {
    file.write_all(serde_json::to_string_pretty(meta)?.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

/// Reader for a finalized recording pair.
#[derive(Debug)]
pub struct SigmfReader {
    meta: SigmfMeta,
    data: BufReader<File>,
    total_samples: u64,
    pos: u64,
    scratch: Vec<u8>,
}

impl SigmfReader {
    /// Open and validate `<stem>.sigmf-meta` + `<stem>.sigmf-data`.
    pub fn open(stem: &Path) -> Result<Self, SigmfError> {
        let meta = read_meta(stem)?;
        if meta.global.datatype != DATATYPE_CF32_LE {
            return Err(SigmfError::UnsupportedDatatype(meta.global.datatype));
        }
        let data = File::open(data_path(stem))?;
        // Whole samples only: a torn trailing write (crash mid-sample) is excluded so reads
        // always yield the intact prefix.
        let total_samples = data.metadata()?.len() / BYTES_PER_SAMPLE;
        Ok(Self {
            meta,
            data: BufReader::new(data),
            total_samples,
            pos: 0,
            scratch: Vec::new(),
        })
    }

    #[must_use]
    pub fn meta(&self) -> &SigmfMeta {
        &self.meta
    }

    #[must_use]
    pub fn total_samples(&self) -> u64 {
        self.total_samples
    }

    /// Fill `buf` from the current position; returns samples read, 0 at end of data.
    pub fn read_block(&mut self, buf: &mut [Complex<f32>]) -> Result<usize, SigmfError> {
        let remaining = self.total_samples - self.pos;
        let n = remaining.min(buf.len() as u64) as usize;
        if n == 0 {
            return Ok(0);
        }
        self.scratch.resize(n * BYTES_PER_SAMPLE as usize, 0);
        self.data.read_exact(&mut self.scratch)?;
        let (pairs, _) = self.scratch.as_chunks::<{ BYTES_PER_SAMPLE as usize }>();
        for (sample, bytes) in buf[..n].iter_mut().zip(pairs) {
            *sample = Complex::new(
                f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
                f32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            );
        }
        self.pos += n as u64;
        Ok(n)
    }

    /// Seek back to sample 0 (looped playback).
    pub fn rewind(&mut self) -> Result<(), SigmfError> {
        self.data.rewind()?;
        self.pos = 0;
        Ok(())
    }

    /// Seek to a sample index, clamped to the end of the data. Clamped rather than refused: a
    /// scrub to the far end of a recording that a torn tail made shorter than its metadata
    /// claims should land at the end, not fail the transport.
    pub fn seek_to(&mut self, sample: u64) -> Result<(), SigmfError> {
        let target = sample.min(self.total_samples);
        self.data
            .seek(std::io::SeekFrom::Start(target * BYTES_PER_SAMPLE))?;
        self.pos = target;
        Ok(())
    }

    /// Samples read so far — the playback position.
    #[must_use]
    pub fn position(&self) -> u64 {
        self.pos
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn samples(n: usize) -> Vec<Complex<f32>> {
        (0..n)
            .map(|i| {
                let phase = i as f32 * 0.01;
                Complex::new(phase.cos(), phase.sin())
            })
            .collect()
    }

    fn assert_bits_eq(a: &[Complex<f32>], b: &[Complex<f32>]) {
        assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(b).enumerate() {
            assert_eq!(x.re.to_bits(), y.re.to_bits(), "re mismatch at {i}");
            assert_eq!(x.im.to_bits(), y.im.to_bits(), "im mismatch at {i}");
        }
    }

    fn read_all(reader: &mut SigmfReader) -> Vec<Complex<f32>> {
        let mut out = Vec::new();
        let mut buf = vec![Complex::new(0.0f32, 0.0); 1_000];
        loop {
            let n = reader.read_block(&mut buf).unwrap();
            if n == 0 {
                return out;
            }
            out.extend_from_slice(&buf[..n]);
        }
    }

    #[test]
    fn writer_reader_roundtrip_bit_exact() {
        let dir = TempDir::new().unwrap();
        let stem = dir.path().join("rec");
        let tone = samples(10_000);

        let mut writer =
            SigmfWriter::create(&stem, 2_400_000.0, 100_000_000.0, "Signal Generator").unwrap();
        writer.write_block(&tone[..4_000]).unwrap();
        writer.write_block(&tone[4_000..]).unwrap();
        assert_eq!(writer.samples_written(), 10_000);
        assert_eq!(writer.bytes_written(), 80_000);
        let meta = writer.finalize().unwrap();

        assert_eq!(meta.global.datatype, DATATYPE_CF32_LE);
        assert_eq!(meta.global.version, SIGMF_VERSION);
        assert_eq!(meta.global.sample_rate, Some(2_400_000.0));
        assert_eq!(meta.global.recorder.as_deref(), Some("sdr--"));
        assert_eq!(meta.global.hw.as_deref(), Some("Signal Generator"));
        assert_eq!(meta.captures.len(), 1);
        assert_eq!(meta.captures[0].sample_start, 0);
        assert_eq!(meta.captures[0].frequency, Some(100_000_000.0));
        meta.captures[0]
            .datetime
            .as_deref()
            .unwrap()
            .parse::<jiff::Timestamp>()
            .unwrap();

        let mut reader = SigmfReader::open(&stem).unwrap();
        assert_eq!(reader.meta(), &meta);
        assert_eq!(reader.total_samples(), 10_000);
        assert_bits_eq(&tone, &read_all(&mut reader));
    }

    #[test]
    fn rewind_restarts_at_sample_zero() {
        let dir = TempDir::new().unwrap();
        let stem = dir.path().join("looped");
        let tone = samples(2_500);

        let mut writer = SigmfWriter::create(&stem, 48_000.0, 7_100_000.0, "hw").unwrap();
        writer.write_block(&tone).unwrap();
        writer.finalize().unwrap();

        let mut reader = SigmfReader::open(&stem).unwrap();
        assert_bits_eq(&tone, &read_all(&mut reader));
        assert_eq!(reader.read_block(&mut [Complex::new(0.0, 0.0)]).unwrap(), 0);
        reader.rewind().unwrap();
        assert_bits_eq(&tone, &read_all(&mut reader));
    }

    #[test]
    fn retune_appends_capture_segment() {
        let dir = TempDir::new().unwrap();
        let stem = dir.path().join("retuned");
        let tone = samples(1_500);

        let mut writer = SigmfWriter::create(&stem, 2_400_000.0, 100_000_000.0, "hw").unwrap();
        writer.write_block(&tone[..1_000]).unwrap();
        writer.add_capture(146_000_000.0);
        // A second retune before any new samples supersedes rather than stacks.
        writer.add_capture(145_500_000.0);
        writer.write_block(&tone[1_000..]).unwrap();
        let meta = writer.finalize().unwrap();

        assert_eq!(meta.captures.len(), 2);
        assert_eq!(meta.captures[0].sample_start, 0);
        assert_eq!(meta.captures[0].frequency, Some(100_000_000.0));
        assert_eq!(meta.captures[1].sample_start, 1_000);
        assert_eq!(meta.captures[1].frequency, Some(145_500_000.0));
        assert_eq!(meta.captures[1].datetime, None);
    }

    #[test]
    fn clearing_position_at_same_sample_clears_coordinates_and_preserves_timestamp() {
        let dir = TempDir::new().unwrap();
        let stem = dir.path().join("geotagged");
        let mut writer = SigmfWriter::create(&stem, 48_000.0, 1_000_000.0, "hw").unwrap();
        let fix = PositionFix {
            latitude: 52.52,
            longitude: 13.405,
            altitude_m: Some(40.0),
            accuracy_m: None,
            speed_mps: None,
            track_deg: None,
            time: "2026-08-14T12:00:00Z".to_owned(),
        };

        writer.set_position(Some(&fix));
        writer.set_position(None);

        let capture = writer.meta().captures.last().unwrap();
        assert_eq!(capture.geolocation, None);
        assert_eq!(capture.datetime.as_deref(), Some(fix.time.as_str()));
    }

    #[test]
    fn position_serializes_as_two_or_three_dimensional_geojson() {
        let dir = TempDir::new().unwrap();
        let stem = dir.path().join("geojson");
        let mut writer = SigmfWriter::create(&stem, 48_000.0, 1_000_000.0, "hw").unwrap();
        let mut fix = PositionFix {
            latitude: 52.52,
            longitude: 13.405,
            altitude_m: None,
            accuracy_m: None,
            speed_mps: None,
            track_deg: None,
            time: "2026-08-14T12:00:00Z".to_owned(),
        };

        writer.write_block(&samples(12)).unwrap();
        writer.set_position(Some(&fix));
        assert_eq!(writer.meta().captures.len(), 2);
        let capture = &writer.meta().captures[1];
        assert_eq!(capture.sample_start, 12);
        assert_eq!(capture.frequency, Some(1_000_000.0));
        assert_eq!(capture.datetime.as_deref(), Some(fix.time.as_str()));
        let json = serde_json::to_value(writer.meta()).unwrap();
        assert_eq!(
            json["captures"][1]["core:geolocation"],
            serde_json::json!({"type": "Point", "coordinates": [13.405, 52.52]})
        );

        fix.altitude_m = Some(40.0);
        writer.set_position(Some(&fix));
        let json = serde_json::to_value(writer.meta()).unwrap();
        assert_eq!(
            json["captures"][1]["core:geolocation"],
            serde_json::json!({"type": "Point", "coordinates": [13.405, 52.52, 40.0]})
        );
    }

    #[test]
    fn finalize_swaps_tmp_breadcrumb_for_real_meta() {
        let dir = TempDir::new().unwrap();
        let stem = dir.path().join("inflight");

        let mut writer = SigmfWriter::create(&stem, 48_000.0, 1_000_000.0, "hw").unwrap();
        writer.write_block(&samples(10)).unwrap();
        assert!(tmp_meta_path(&stem).exists());
        assert!(data_path(&stem).exists());
        assert!(!meta_path(&stem).exists());
        assert!(scan_stems(dir.path()).unwrap().is_empty());

        writer.finalize().unwrap();
        assert!(!tmp_meta_path(&stem).exists());
        assert!(meta_path(&stem).exists());
        assert_eq!(scan_stems(dir.path()).unwrap(), vec![stem]);
    }

    #[test]
    fn scan_skips_crashed_recordings_and_foreign_files() {
        let dir = TempDir::new().unwrap();

        let done = dir.path().join("b_done");
        let mut writer = SigmfWriter::create(&done, 48_000.0, 1_000_000.0, "hw").unwrap();
        writer.write_block(&samples(10)).unwrap();
        writer.finalize().unwrap();

        let done_first = dir.path().join("a_done");
        SigmfWriter::create(&done_first, 48_000.0, 1_000_000.0, "hw")
            .unwrap()
            .finalize()
            .unwrap();

        // Crash: writer dropped without finalize leaves only the breadcrumb + data.
        let crashed = dir.path().join("crashed");
        drop(SigmfWriter::create(&crashed, 48_000.0, 1_000_000.0, "hw").unwrap());

        fs::write(dir.path().join("notes.txt"), "not sigmf").unwrap();

        assert_eq!(scan_stems(dir.path()).unwrap(), vec![done_first, done]);
        assert!(scan_stems(&dir.path().join("missing")).unwrap().is_empty());
    }

    #[test]
    fn truncated_data_yields_complete_prefix() {
        let dir = TempDir::new().unwrap();
        let stem = dir.path().join("torn");
        let tone = samples(100);

        let mut writer = SigmfWriter::create(&stem, 48_000.0, 1_000_000.0, "hw").unwrap();
        writer.write_block(&tone).unwrap();
        writer.finalize().unwrap();

        let data = fs::OpenOptions::new()
            .write(true)
            .open(data_path(&stem))
            .unwrap();
        data.set_len(50 * BYTES_PER_SAMPLE + 3).unwrap();

        let mut reader = SigmfReader::open(&stem).unwrap();
        assert_eq!(reader.total_samples(), 50);
        assert_bits_eq(&tone[..50], &read_all(&mut reader));
    }

    #[test]
    fn create_refuses_a_claimed_stem() {
        let dir = TempDir::new().unwrap();
        let stem = dir.path().join("claimed");
        let mut first = SigmfWriter::create(&stem, 48_000.0, 1_000_000.0, "hw").unwrap();
        first.write_block(&samples(8)).unwrap();

        // A racing second create must fail instead of truncating the live data file.
        match SigmfWriter::create(&stem, 48_000.0, 1_000_000.0, "hw") {
            Err(SigmfError::StemTaken(taken)) => assert_eq!(taken, stem),
            other => panic!("expected StemTaken, got {other:?}"),
        }
        assert!(tmp_meta_path(&stem).exists());
        assert_eq!(
            fs::metadata(data_path(&stem)).unwrap().len(),
            8 * BYTES_PER_SAMPLE
        );

        first.finalize().unwrap();
        let mut reader = SigmfReader::open(&stem).unwrap();
        assert_eq!(reader.total_samples(), 8);
        assert_bits_eq(&samples(8), &read_all(&mut reader));
    }

    #[test]
    fn create_over_a_stray_data_file_fails_and_removes_only_its_own_breadcrumb() {
        let dir = TempDir::new().unwrap();
        let stem = dir.path().join("stray");
        fs::write(data_path(&stem), [1, 2, 3]).unwrap();

        assert!(matches!(
            SigmfWriter::create(&stem, 48_000.0, 1_000_000.0, "hw"),
            Err(SigmfError::StemTaken(_))
        ));
        assert!(!tmp_meta_path(&stem).exists());
        assert_eq!(fs::read(data_path(&stem)).unwrap(), [1, 2, 3]);
    }

    /// SigMF v1.2.6 requires only `core:datatype` and `core:version` in Global; a foreign
    /// meta without a rate must parse (consumers reject `None` where a rate is needed).
    #[test]
    fn meta_without_sample_rate_parses_as_none() {
        let meta: SigmfMeta = serde_json::from_str(
            r#"{"global":{"core:datatype":"cf32_le","core:version":"1.2.6"},"captures":[]}"#,
        )
        .unwrap();
        assert_eq!(meta.global.sample_rate, None);
        let json = serde_json::to_value(&meta).unwrap();
        assert!(json["global"].get("core:sample_rate").is_none());
    }

    #[test]
    fn rejects_non_cf32_datatype() {
        let dir = TempDir::new().unwrap();
        let stem = dir.path().join("foreign");
        // Minimal foreign meta: optional core fields and `annotations` absent must parse.
        fs::write(
            meta_path(&stem),
            r#"{"global":{"core:datatype":"ci16_le","core:version":"1.2.6","core:sample_rate":48000.0},"captures":[]}"#,
        )
        .unwrap();
        fs::write(data_path(&stem), []).unwrap();

        match SigmfReader::open(&stem) {
            Err(SigmfError::UnsupportedDatatype(datatype)) => assert_eq!(datatype, "ci16_le"),
            other => panic!("expected UnsupportedDatatype, got {other:?}"),
        }
    }

    /// The on-disk keys are the interop contract with every other SigMF tool; lock them.
    #[test]
    fn meta_serializes_core_prefixed_keys() {
        let meta = SigmfMeta {
            global: SigmfGlobal {
                datatype: DATATYPE_CF32_LE.to_string(),
                version: SIGMF_VERSION.to_string(),
                sample_rate: Some(2_400_000.0),
                recorder: Some("sdr--".to_string()),
                hw: None,
                rx_stream: None,
            },
            captures: vec![SigmfCapture {
                sample_start: 7,
                frequency: Some(100_000_000.0),
                datetime: None,
                geolocation: None,
            }],
            annotations: Vec::new(),
        };
        let json = serde_json::to_value(&meta).unwrap();
        assert_eq!(json["global"]["core:datatype"], "cf32_le");
        assert_eq!(json["global"]["core:version"], "1.2.6");
        assert_eq!(json["global"]["core:sample_rate"], 2_400_000.0);
        assert_eq!(json["global"]["core:recorder"], "sdr--");
        assert!(json["global"].get("core:hw").is_none());
        // The extension field must stay off the wire until a stream is stamped, so foreign
        // and pre-multi-stream metas round-trip byte-identical.
        assert!(json["global"].get("sdrmm:rx_stream").is_none());
        assert_eq!(json["captures"][0]["core:sample_start"], 7);
        assert_eq!(json["captures"][0]["core:frequency"], 100_000_000.0);
        assert_eq!(json["annotations"], serde_json::json!([]));

        let back: SigmfMeta = serde_json::from_value(json).unwrap();
        assert_eq!(back, meta);
    }

    /// A multi-stream radio's recording must say which stream it captured — the file is
    /// otherwise indistinguishable from any other mono `cf32` pair (b).
    #[test]
    fn rx_stream_is_stamped_into_the_final_meta_and_read_back() {
        let dir = TempDir::new().unwrap();
        let stem = dir.path().join("lane2");
        let mut writer = SigmfWriter::create(&stem, 48_000.0, 1_000_000.0, "hw").unwrap();
        writer.set_rx_stream(2);
        writer.write_block(&samples(4)).unwrap();
        let meta = writer.finalize().unwrap();
        assert_eq!(meta.global.rx_stream, Some(2));

        let json = serde_json::to_value(&meta).unwrap();
        assert_eq!(json["global"]["sdrmm:rx_stream"], 2);
        let reader = SigmfReader::open(&stem).unwrap();
        assert_eq!(reader.meta().global.rx_stream, Some(2));
    }
}
