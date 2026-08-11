//! I/Q as a WAV file — the form HDSDR, SDR#, SDRuno, Audacity, `inspectrum` and `ffmpeg`
//! open. A `.sigmf-data` is already interleaved little-endian `f32` I/Q, which is exactly the
//! payload of a two-channel 32-bit-float WAV, so the samples are copied byte for byte and
//! only the header is generated.
//!
//! What the container cannot carry, it loses. WAV has no field for a center frequency, so the
//! tuning survives only in the `auxi` chunk, and everything else SigMF holds — capture
//! segments after the first, annotations, the recorder and hardware strings — is dropped.
//! `.sigmf` is the lossless export; this one is for getting a recording into other tools.

use std::{collections::VecDeque, path::Path};

use super::{ExportKind, Part, open_pinned};
use crate::{BYTES_PER_SAMPLE, DATATYPE_CF32_LE, SigmfError, SigmfMeta, data_path, read_meta};

/// `WAVE_FORMAT_IEEE_FLOAT`: the payload is `f32`, not PCM.
const FORMAT_IEEE_FLOAT: u16 = 3;
/// I and Q ride as the two channels of one frame.
const CHANNELS: u16 = 2;
const BITS_PER_SAMPLE: u16 = 32;
/// One frame is one complex sample, so this is [`BYTES_PER_SAMPLE`] seen from the WAV side.
const BLOCK_ALIGN: u16 = CHANNELS * BITS_PER_SAMPLE / 8;

/// A non-PCM `fmt ` chunk carries `cbSize` and must be accompanied by a `fact` chunk
/// (Multimedia Programming Interface and Data Specifications 1.0, "Non-PCM formats"), which
/// is why this is 18 rather than the PCM 16.
const FMT_SIZE: u32 = 18;
const FACT_SIZE: u32 = 4;
/// `auxi` payload: two `SYSTEMTIME`s, nine `u32`s, and a 96-byte next-file name.
const AUXI_SIZE: u32 = 16 + 16 + 9 * 4 + 96;
/// `ds64` payload: the 64-bit RIFF, data and frame counts, plus an empty chunk table.
const DS64_SIZE: u32 = 8 + 8 + 8 + 4;
/// A chunk's four-byte id and its size field.
const CHUNK_HEADER: u64 = 8;
/// `RIFF`/`RF64` id, its size field, and the `WAVE` form type.
const RIFF_HEADER: u64 = 12;

pub(super) fn parts(stem: &Path) -> Result<VecDeque<Part>, SigmfError> {
    let meta = read_meta(stem)?;
    let unexportable = |reason| SigmfError::Unexportable {
        stem: stem.to_path_buf(),
        format: ExportKind::Wav.extension(),
        reason,
    };
    if meta.global.datatype != DATATYPE_CF32_LE {
        return Err(unexportable("only cf32_le samples map onto a float WAV"));
    }
    let Some(rate) = meta.global.sample_rate else {
        return Err(unexportable(
            "the recording has no core:sample_rate to write",
        ));
    };
    if !(rate.is_finite() && rate > 0.0 && rate <= f64::from(u32::MAX)) {
        return Err(unexportable("the sample rate does not fit a WAV header"));
    }
    // WAV rates are integers; SigMF's are not. A fractional rate is rounded, which shifts a
    // reader's idea of the timebase by less than a part per sample-rate and is the only
    // choice the container leaves.
    let sample_rate = rate.round() as u32;

    let (data_file, file_len, _) = open_pinned(&data_path(stem))?;
    // Whole frames only: half an I/Q frame — a crash-torn tail — would desync every reader
    // after it, and WAV cannot describe a partial one. Also keeps the `data` chunk even, so
    // no RIFF pad byte is ever needed.
    let data_len = file_len - file_len % BYTES_PER_SAMPLE;

    let mut parts = VecDeque::new();
    parts.push_back(Part::bytes(header(&meta, sample_rate, data_len)));
    parts.push_back(Part::file(data_file, data_len));
    Ok(parts)
}

/// Bytes before the payload. Fixed per container, which is what lets an export publish its
/// exact length up front.
pub(super) fn header_len(rf64: bool) -> u64 {
    let riff = RIFF_HEADER
        + CHUNK_HEADER
        + u64::from(FMT_SIZE)
        + CHUNK_HEADER
        + u64::from(FACT_SIZE)
        + CHUNK_HEADER
        + u64::from(AUXI_SIZE)
        + CHUNK_HEADER;
    if rf64 {
        riff + CHUNK_HEADER + u64::from(DS64_SIZE)
    } else {
        riff
    }
}

/// RIFF sizes are 32-bit, so a plain `.wav` tops out just under 4 GiB — about three and a half
/// minutes at 2.4 MS/s. Past that the file is written as RF64 (EBU Tech 3306), which keeps the
/// real sizes in a `ds64` chunk and leaves the 32-bit ones reading -1.
fn needs_rf64(data_len: u64) -> bool {
    header_len(false) - 8 + data_len > u64::from(u32::MAX)
}

fn header(meta: &SigmfMeta, sample_rate: u32, data_len: u64) -> Vec<u8> {
    let rf64 = needs_rf64(data_len);
    let frames = data_len / BYTES_PER_SAMPLE;
    // The `RIFF` size covers everything after the size field itself.
    let riff_size = header_len(rf64) + data_len - 8;

    let mut out = Vec::with_capacity(header_len(rf64) as usize);
    out.extend_from_slice(if rf64 { b"RF64" } else { b"RIFF" });
    out.extend_from_slice(&clamp32(riff_size).to_le_bytes());
    out.extend_from_slice(b"WAVE");

    if rf64 {
        chunk(&mut out, b"ds64", DS64_SIZE);
        out.extend_from_slice(&riff_size.to_le_bytes());
        out.extend_from_slice(&data_len.to_le_bytes());
        out.extend_from_slice(&frames.to_le_bytes());
        // No chunk-size table: `data` is the only chunk that can outgrow its 32-bit field.
        out.extend_from_slice(&0u32.to_le_bytes());
    }

    chunk(&mut out, b"fmt ", FMT_SIZE);
    out.extend_from_slice(&FORMAT_IEEE_FLOAT.to_le_bytes());
    out.extend_from_slice(&CHANNELS.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(
        &sample_rate
            .saturating_mul(u32::from(BLOCK_ALIGN))
            .to_le_bytes(),
    );
    out.extend_from_slice(&BLOCK_ALIGN.to_le_bytes());
    out.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());

    chunk(&mut out, b"fact", FACT_SIZE);
    out.extend_from_slice(&clamp32(frames).to_le_bytes());

    chunk(&mut out, b"auxi", AUXI_SIZE);
    out.extend_from_slice(&auxi(meta, sample_rate, frames));

    chunk(&mut out, b"data", clamp32(data_len));
    out
}

fn chunk(out: &mut Vec<u8>, id: &[u8; 4], size: u32) {
    out.extend_from_slice(id);
    out.extend_from_slice(&size.to_le_bytes());
}

/// A 32-bit size field saturates to -1 in RF64, where the real value lives in `ds64`.
fn clamp32(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// The `auxi` chunk: an RFSpace/SpectraVue extension that HDSDR, SDR# and SDRangel all agree
/// on, and the only place a WAV can keep the center frequency SigMF carries. Field order
/// follows SDRangel's `WavFileRecord` (GPL-3.0 — the layout is copied, not the code).
fn auxi(meta: &SigmfMeta, sample_rate: u32, frames: u64) -> Vec<u8> {
    let capture = meta.captures.first();
    let start = capture
        .and_then(|capture| capture.datetime.as_deref())
        .and_then(|at| at.parse::<jiff::Timestamp>().ok());
    let stop = start.and_then(|start| {
        let seconds = frames as f64 / f64::from(sample_rate);
        jiff::SignedDuration::try_from_secs_f64(seconds)
            .ok()
            .and_then(|elapsed| start.checked_add(elapsed).ok())
    });

    let mut out = Vec::with_capacity(AUXI_SIZE as usize);
    out.extend_from_slice(&system_time(start));
    out.extend_from_slice(&system_time(stop));
    // The field is a `u32`, so it cannot hold a center above ~4.295 GHz. Zero is the
    // convention's "unknown", which beats a wrapped frequency that would point a reader at
    // the wrong band entirely; `.sigmf` keeps the true value either way.
    let center = capture
        .and_then(|capture| capture.frequency)
        .filter(|hz| hz.is_finite() && *hz >= 0.0 && *hz <= f64::from(u32::MAX))
        .map_or(0, |hz| hz.round() as u32);
    // centerFreq, adFrequency, then ifFrequency, bandwidth, iqOffset and four unused words.
    for field in [center, sample_rate, 0, 0, 0, 0, 0, 0, 0] {
        out.extend_from_slice(&field.to_le_bytes());
    }
    // `nextFilename` stays zeroed: this is not a split recording.
    out.resize(AUXI_SIZE as usize, 0);
    out
}

/// A Windows `SYSTEMTIME`: eight little-endian `u16`s. An absent timestamp is written as
/// zeros, which is what the chunk's readers take as "not set".
fn system_time(at: Option<jiff::Timestamp>) -> [u8; 16] {
    let mut out = [0u8; 16];
    let Some(at) = at else {
        return out;
    };
    let civil = at.to_zoned(jiff::tz::TimeZone::UTC).datetime();
    let fields = [
        u16::try_from(civil.year()).unwrap_or(0),
        u16::try_from(civil.month()).unwrap_or(0),
        u16::try_from(civil.weekday().to_sunday_zero_offset()).unwrap_or(0),
        u16::try_from(civil.day()).unwrap_or(0),
        u16::try_from(civil.hour()).unwrap_or(0),
        u16::try_from(civil.minute()).unwrap_or(0),
        u16::try_from(civil.second()).unwrap_or(0),
        u16::try_from(civil.millisecond()).unwrap_or(0),
    ];
    for (slot, value) in out.as_chunks_mut::<2>().0.iter_mut().zip(fields) {
        *slot = value.to_le_bytes();
    }
    out
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::export::{
        Export, ExportKind,
        tests::{drain, recording},
    };

    fn u16_at(bytes: &[u8], at: usize) -> u16 {
        u16::from_le_bytes(bytes[at..at + 2].try_into().expect("2 bytes"))
    }

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"))
    }

    fn u64_at(bytes: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(bytes[at..at + 8].try_into().expect("8 bytes"))
    }

    /// Walk the chunk list the way a reader does, so a mis-sized chunk desynchronises here.
    fn chunks(bytes: &[u8]) -> Vec<(String, usize, u32)> {
        let mut found = Vec::new();
        let mut at = RIFF_HEADER as usize;
        while at + CHUNK_HEADER as usize <= bytes.len() {
            let id = String::from_utf8(bytes[at..at + 4].to_vec()).expect("chunk id");
            let size = u32_at(bytes, at + 4);
            let body = at + CHUNK_HEADER as usize;
            found.push((id.clone(), body, size));
            if id == "data" {
                break;
            }
            at = body + (size as usize).next_multiple_of(2);
        }
        found
    }

    #[test]
    fn header_describes_two_channel_float_iq_at_the_recorded_rate() {
        let dir = TempDir::new().expect("tempdir");
        let stem = recording(&dir, "iq", 1_000, 2_400_000.0);
        let mut export = Export::open(&stem, ExportKind::Wav).expect("open");
        let bytes = drain(&mut export);

        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(u32_at(&bytes, 4) as u64, bytes.len() as u64 - 8);
        assert_eq!(&bytes[8..12], b"WAVE");

        let found = chunks(&bytes);
        let ids: Vec<_> = found.iter().map(|(id, ..)| id.as_str()).collect();
        assert_eq!(ids, ["fmt ", "fact", "auxi", "data"]);

        let (_, fmt, fmt_size) = found[0];
        assert_eq!(fmt_size, FMT_SIZE);
        assert_eq!(u16_at(&bytes, fmt), FORMAT_IEEE_FLOAT);
        assert_eq!(u16_at(&bytes, fmt + 2), 2);
        assert_eq!(u32_at(&bytes, fmt + 4), 2_400_000);
        assert_eq!(u32_at(&bytes, fmt + 8), 2_400_000 * 8, "byte rate");
        assert_eq!(u16_at(&bytes, fmt + 12), 8, "block align");
        assert_eq!(u16_at(&bytes, fmt + 14), 32, "bits per sample");
        assert_eq!(u16_at(&bytes, fmt + 16), 0, "cbSize");

        let (_, fact, _) = found[1];
        assert_eq!(u32_at(&bytes, fact), 1_000, "frame count");

        let (_, data, data_size) = found[3];
        assert_eq!(u64::from(data_size), 1_000 * BYTES_PER_SAMPLE);
        assert_eq!(
            &bytes[data..],
            &std::fs::read(data_path(&stem)).expect("read data")[..],
            "payload is the .sigmf-data verbatim"
        );
    }

    #[test]
    fn auxi_carries_the_center_frequency_and_the_capture_start() {
        let dir = TempDir::new().expect("tempdir");
        let stem = recording(&dir, "tuned", 48_000, 48_000.0);
        let mut export = Export::open(&stem, ExportKind::Wav).expect("open");
        let bytes = drain(&mut export);

        let (_, auxi, size) = chunks(&bytes)[2];
        assert_eq!(size, AUXI_SIZE);
        assert_eq!(u32_at(&bytes, auxi + 32), 100_000_000, "center frequency");
        assert_eq!(u32_at(&bytes, auxi + 36), 48_000, "A/D frequency");

        // `recording` stamps a real `core:datetime`, so the start must be a plausible date and
        // the stop exactly one second later — 48 000 frames at 48 kS/s.
        let year = u16_at(&bytes, auxi);
        assert!((2000..2200).contains(&year), "start year {year}");
        assert_eq!(u16_at(&bytes, auxi + 16), year, "stop year");
        let start_second = u16_at(&bytes, auxi + 12);
        let stop_second = u16_at(&bytes, auxi + 28);
        assert_eq!(
            (stop_second + 60 - start_second) % 60,
            1,
            "one second of I/Q"
        );
    }

    /// A center above what the `auxi` field can hold must read as "unknown" rather than as a
    /// wrapped frequency pointing at some other band.
    #[test]
    fn a_center_beyond_the_auxi_field_is_written_as_unknown() {
        let meta = SigmfMeta {
            global: crate::SigmfGlobal {
                datatype: DATATYPE_CF32_LE.to_string(),
                version: crate::SIGMF_VERSION.to_string(),
                sample_rate: Some(20_000_000.0),
                recorder: None,
                hw: None,
                rx_stream: None,
            },
            captures: vec![crate::SigmfCapture {
                sample_start: 0,
                frequency: Some(5_800_000_000.0),
                datetime: None,
            }],
            annotations: Vec::new(),
        };
        let chunk = auxi(&meta, 20_000_000, 0);
        assert_eq!(
            u32_at(&chunk, 32),
            0,
            "center is not wrapped into the field"
        );
        assert_eq!(u32_at(&chunk, 36), 20_000_000);
    }

    /// Past 4 GiB the header must switch to RF64, since the 32-bit sizes can no longer
    /// describe the file. Exercised on the header alone — the payload is the point of the
    /// limit and writing one would make this a five-gigabyte test.
    #[test]
    fn oversized_recordings_are_written_as_rf64() {
        let meta = SigmfMeta {
            global: crate::SigmfGlobal {
                datatype: DATATYPE_CF32_LE.to_string(),
                version: crate::SIGMF_VERSION.to_string(),
                sample_rate: Some(20_000_000.0),
                recorder: None,
                hw: None,
                rx_stream: None,
            },
            captures: Vec::new(),
            annotations: Vec::new(),
        };
        let data_len = 5 * 1024 * 1024 * 1024;
        assert!(needs_rf64(data_len));

        let bytes = header(&meta, 20_000_000, data_len);
        assert_eq!(bytes.len() as u64, header_len(true));
        assert_eq!(&bytes[..4], b"RF64");
        assert_eq!(u32_at(&bytes, 4), u32::MAX, "32-bit RIFF size reads -1");

        let found = chunks(&bytes);
        let ids: Vec<_> = found.iter().map(|(id, ..)| id.as_str()).collect();
        assert_eq!(ids, ["ds64", "fmt ", "fact", "auxi", "data"]);

        let (_, ds64, ds64_size) = found[0];
        assert_eq!(ds64_size, DS64_SIZE);
        assert_eq!(u64_at(&bytes, ds64), header_len(true) + data_len - 8);
        assert_eq!(u64_at(&bytes, ds64 + 8), data_len);
        assert_eq!(u64_at(&bytes, ds64 + 16), data_len / BYTES_PER_SAMPLE);
        assert_eq!(u32_at(&bytes, ds64 + 24), 0, "no chunk-size table");

        let (_, _, data_size) = found[4];
        assert_eq!(data_size, u32::MAX, "32-bit data size reads -1");
    }

    /// The boundary itself: the largest file that still fits a plain RIFF must not be
    /// promoted, and one byte more must be.
    #[test]
    fn rf64_starts_exactly_where_riff_runs_out() {
        let largest = u64::from(u32::MAX) - (header_len(false) - 8);
        assert!(!needs_rf64(largest));
        assert!(needs_rf64(largest + 1));
    }
}
