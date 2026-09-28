use std::{
    collections::VecDeque,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

use num_complex::Complex;
use sdrmm_wire::{
    ArrayGeometry, Coherence, DcArtifact, LaneKey, NoiseSource, recording_stem_valid,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};

use crate::{
    SIGMF_VERSION, SigmfCapture, SigmfError, SigmfMeta, SigmfReader, SigmfWriter, claim_error,
    data_path, holds_recording, meta_path, tmp_meta_path, with_suffix,
};

pub const COLLECTION_SUFFIX: &str = ".sigmf-collection";
const TMP_COLLECTION_SUFFIX: &str = ".sigmf-collection.tmp";
const EXTENSION_NAME: &str = "sdrmm";
const EXTENSION_VERSION: &str = "1.0.0";
const NOISE_LABEL: &str = "noise";

#[derive(Clone, Debug, PartialEq)]
pub struct LaneMeta {
    pub lane: LaneKey,
    pub center_hz: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CollectionArray {
    pub node: String,
    pub tier: Coherence,
    pub geometry: ArrayGeometry,
    pub noise_source: NoiseSource,
    #[serde(default)]
    pub retune_keeps_phase: bool,
    #[serde(default)]
    pub dc_artifact: DcArtifact,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ReadChunk {
    Samples(usize),
    Gap(u64),
    Noise { on: bool },
    Retuned(Vec<f64>),
    Offsets(Vec<i64>),
    End,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CollectionFile {
    collection: CollectionBody,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CollectionBody {
    #[serde(rename = "core:version")]
    version: String,
    #[serde(rename = "core:extensions", default)]
    extensions: Vec<Extension>,
    #[serde(rename = "core:streams")]
    streams: Vec<StreamTuple>,
    #[serde(rename = "sdrmm:array")]
    array: ArrayRecord,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Extension {
    name: String,
    version: String,
    optional: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StreamTuple {
    name: String,
    hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ArrayRecord {
    #[serde(flatten)]
    array: CollectionArray,
    lanes: Vec<LaneKey>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct NoiseAnnotation {
    #[serde(rename = "core:sample_start")]
    sample_start: u64,
    #[serde(rename = "core:sample_count")]
    sample_count: u64,
    #[serde(rename = "core:label")]
    label: String,
}

#[must_use]
pub fn collection_path(stem: &Path) -> PathBuf {
    with_suffix(stem, COLLECTION_SUFFIX)
}

fn tmp_collection_path(stem: &Path) -> PathBuf {
    with_suffix(stem, TMP_COLLECTION_SUFFIX)
}

pub(crate) fn holds(stem: &Path) -> bool {
    collection_path(stem).exists() || tmp_collection_path(stem).exists()
}

const LANE_INFIX: &str = "-lane";

#[must_use]
pub fn lane_stem(stem: &Path, lane: usize) -> PathBuf {
    with_suffix(stem, &format!("{LANE_INFIX}{lane}"))
}

#[must_use]
pub fn lane_of(collection: &Path, stem: &Path) -> Option<usize> {
    if stem.parent() != collection.parent() {
        return None;
    }
    let prefix = format!("{}{LANE_INFIX}", collection.file_name()?.to_str()?);
    let lane = stem.file_name()?.to_str()?.strip_prefix(&prefix)?;
    if lane.is_empty() || !lane.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    lane.parse().ok()
}

pub fn scan_collections(dir: &Path) -> Result<Vec<PathBuf>, SigmfError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut stems = Vec::new();
    for entry in entries {
        let name = entry?.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some(stem) = name.strip_suffix(COLLECTION_SUFFIX) {
            stems.push(dir.join(stem));
        }
    }
    stems.sort();
    Ok(stems)
}

fn meta_hash(stem: &Path) -> Result<String, SigmfError> {
    let digest = Sha512::digest(fs::read(meta_path(stem))?);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn stream_name(stem: &Path) -> Result<String, SigmfError> {
    stem.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| SigmfError::Malformed(format!("stem `{}` has no name", stem.display())))
}

#[derive(Debug)]
pub struct CollectionWriter {
    stem: PathBuf,
    lanes: Vec<SigmfWriter>,
    record: ArrayRecord,
    samples: u64,
    missing: u64,
}

impl CollectionWriter {
    pub fn create(
        stem: &Path,
        lanes: &[LaneMeta],
        sample_rate: f64,
        array: CollectionArray,
    ) -> Result<Self, SigmfError> {
        if lanes.is_empty() {
            return Err(SigmfError::LaneCount {
                expected: 1,
                got: 0,
            });
        }
        let record = ArrayRecord {
            array,
            lanes: lanes.iter().map(|meta| meta.lane.clone()).collect(),
        };
        claim(stem, &record)?;
        let mut writers = Vec::with_capacity(lanes.len());
        for (index, meta) in lanes.iter().enumerate() {
            match open_lane(stem, index, meta, sample_rate) {
                Ok(writer) => writers.push(writer),
                Err(err) => {
                    abandon(stem, writers.len());
                    return Err(err);
                }
            }
        }
        Ok(Self {
            stem: stem.to_path_buf(),
            lanes: writers,
            record,
            samples: 0,
            missing: 0,
        })
    }

    #[must_use]
    pub fn lanes(&self) -> usize {
        self.lanes.len()
    }

    #[must_use]
    pub fn samples_written(&self) -> u64 {
        self.samples
    }

    pub fn write(&mut self, lanes: &[&[Complex<f32>]]) -> Result<(), SigmfError> {
        self.check_lanes(lanes.len())?;
        let len = lanes.first().map_or(0, |lane| lane.len());
        if lanes.iter().any(|lane| lane.len() != len) {
            return Err(SigmfError::LaneLength);
        }
        for (writer, block) in self.lanes.iter_mut().zip(lanes) {
            writer.write_block(block)?;
        }
        self.samples += len as u64;
        Ok(())
    }

    pub fn gap(&mut self, missing: u64) -> Result<(), SigmfError> {
        if missing == 0 {
            return Ok(());
        }
        self.missing += missing;
        let global = self.samples + self.missing;
        for writer in &mut self.lanes {
            writer.capture_here().global_index = Some(global);
        }
        Ok(())
    }

    pub fn retuned(&mut self, centers_hz: &[f64], at: &str) -> Result<(), SigmfError> {
        self.check_lanes(centers_hz.len())?;
        let global = self.global_index();
        for (writer, center) in self.lanes.iter_mut().zip(centers_hz) {
            let capture = writer.capture_here();
            capture.frequency = Some(*center);
            capture.datetime = Some(at.to_owned());
            capture.global_index = global;
        }
        Ok(())
    }

    pub fn offsets(&mut self, offsets: &[i64]) -> Result<(), SigmfError> {
        self.check_lanes(offsets.len())?;
        let global = self.global_index();
        for writer in &mut self.lanes {
            let capture = writer.capture_here();
            capture.offsets = Some(offsets.to_vec());
            capture.global_index = capture.global_index.or(global);
        }
        Ok(())
    }

    pub fn noise(&mut self, start: u64, count: u64) -> Result<(), SigmfError> {
        let annotation = serde_json::to_value(NoiseAnnotation {
            sample_start: start,
            sample_count: count,
            label: NOISE_LABEL.to_owned(),
        })?;
        for writer in &mut self.lanes {
            writer.push_annotation(annotation.clone());
        }
        Ok(())
    }

    pub fn finalize(self) -> Result<(), SigmfError> {
        let mut streams = Vec::with_capacity(self.lanes.len());
        for writer in self.lanes {
            let stem = writer.stem().to_path_buf();
            writer.finalize()?;
            streams.push(StreamTuple {
                name: stream_name(&stem)?,
                hash: meta_hash(&stem)?,
            });
        }
        let tmp = tmp_collection_path(&self.stem);
        write_collection(File::create(&tmp)?, &body(streams, self.record))?;
        fs::rename(&tmp, collection_path(&self.stem))?;
        Ok(())
    }

    fn global_index(&self) -> Option<u64> {
        (self.missing > 0).then_some(self.samples + self.missing)
    }

    fn check_lanes(&self, got: usize) -> Result<(), SigmfError> {
        if got == self.lanes.len() {
            Ok(())
        } else {
            Err(SigmfError::LaneCount {
                expected: self.lanes.len(),
                got,
            })
        }
    }
}

fn body(streams: Vec<StreamTuple>, array: ArrayRecord) -> CollectionFile {
    CollectionFile {
        collection: CollectionBody {
            version: SIGMF_VERSION.to_owned(),
            extensions: vec![Extension {
                name: EXTENSION_NAME.to_owned(),
                version: EXTENSION_VERSION.to_owned(),
                optional: true,
            }],
            streams,
            array,
        },
    }
}

fn write_collection(mut file: File, collection: &CollectionFile) -> Result<(), SigmfError> {
    file.write_all(serde_json::to_string_pretty(collection)?.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

fn claim(stem: &Path, record: &ArrayRecord) -> Result<(), SigmfError> {
    if collection_path(stem).exists() || holds_recording(stem) {
        return Err(SigmfError::StemTaken(stem.to_path_buf()));
    }
    let tmp = tmp_collection_path(stem);
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|err| claim_error(stem, err))?;
    if let Err(err) = write_collection(file, &body(Vec::new(), record.clone())) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    Ok(())
}

fn open_lane(
    stem: &Path,
    index: usize,
    meta: &LaneMeta,
    sample_rate: f64,
) -> Result<SigmfWriter, SigmfError> {
    let lane = u32::try_from(index)
        .map_err(|_| SigmfError::Malformed(format!("lane {index} is out of range")))?;
    let mut writer = SigmfWriter::create(
        &lane_stem(stem, index),
        sample_rate,
        meta.center_hz,
        &meta.lane.device,
    )?;
    writer.set_lane(lane);
    writer.set_rx_stream(meta.lane.stream);
    Ok(writer)
}

fn abandon(stem: &Path, lanes: usize) {
    for index in 0..lanes {
        let lane = lane_stem(stem, index);
        let _ = fs::remove_file(data_path(&lane));
        let _ = fs::remove_file(tmp_meta_path(&lane));
    }
    let _ = fs::remove_file(tmp_collection_path(stem));
}

#[derive(Clone, Debug, PartialEq)]
enum Event {
    Gap(u64),
    Retuned(Vec<f64>),
    Offsets(Vec<i64>),
    Noise(bool),
}

impl Event {
    const fn rank(&self) -> u8 {
        match self {
            Self::Gap(_) => 0,
            Self::Retuned(_) => 1,
            Self::Offsets(_) => 2,
            Self::Noise(false) => 3,
            Self::Noise(true) => 4,
        }
    }

    fn into_chunk(self) -> ReadChunk {
        match self {
            Self::Gap(missing) => ReadChunk::Gap(missing),
            Self::Retuned(centers) => ReadChunk::Retuned(centers),
            Self::Offsets(offsets) => ReadChunk::Offsets(offsets),
            Self::Noise(on) => ReadChunk::Noise { on },
        }
    }
}

#[derive(Debug)]
pub struct CollectionReader {
    lanes: Vec<SigmfReader>,
    array: CollectionArray,
    keys: Vec<LaneKey>,
    sample_rate: f64,
    centers: Vec<f64>,
    events: VecDeque<(u64, Event)>,
    position: u64,
    total: u64,
}

impl CollectionReader {
    pub fn open(stem: &Path) -> Result<Self, SigmfError> {
        let file: CollectionFile =
            serde_json::from_str(&fs::read_to_string(collection_path(stem))?)?;
        let body = file.collection;
        if body.streams.is_empty() {
            return Err(SigmfError::Malformed(
                "a collection needs at least one stream".to_owned(),
            ));
        }
        if body.array.lanes.len() != body.streams.len() {
            return Err(SigmfError::LaneCount {
                expected: body.streams.len(),
                got: body.array.lanes.len(),
            });
        }
        let dir = stem.parent().unwrap_or_else(|| Path::new(""));
        let mut lanes = Vec::with_capacity(body.streams.len());
        let mut metas = Vec::with_capacity(body.streams.len());
        for stream in &body.streams {
            let lane = open_stream(dir, stream)?;
            metas.push(lane.meta().clone());
            lanes.push(lane);
        }
        let layout = Layout::of(&lanes, &metas)?;
        Ok(Self {
            lanes,
            array: body.array.array,
            keys: body.array.lanes,
            sample_rate: layout.sample_rate,
            centers: layout.centers,
            events: events_of(&metas)?.into(),
            position: 0,
            total: layout.total,
        })
    }

    #[must_use]
    pub fn lanes(&self) -> usize {
        self.lanes.len()
    }

    #[must_use]
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    #[must_use]
    pub fn centers_hz(&self) -> &[f64] {
        &self.centers
    }

    #[must_use]
    pub fn array(&self) -> &CollectionArray {
        &self.array
    }

    #[must_use]
    pub fn lane_keys(&self) -> &[LaneKey] {
        &self.keys
    }

    #[must_use]
    pub fn total_samples(&self) -> u64 {
        self.total
    }

    #[must_use]
    pub fn position(&self) -> u64 {
        self.position
    }

    pub fn read(
        &mut self,
        out: &mut [Vec<Complex<f32>>],
        max: usize,
    ) -> Result<ReadChunk, SigmfError> {
        if out.len() != self.lanes.len() {
            return Err(SigmfError::LaneCount {
                expected: self.lanes.len(),
                got: out.len(),
            });
        }
        let due = self
            .events
            .front()
            .is_some_and(|(at, _)| *at <= self.position || self.position >= self.total);
        if due && let Some((_, event)) = self.events.pop_front() {
            return Ok(event.into_chunk());
        }
        let until = self
            .events
            .front()
            .map_or(self.total, |(at, _)| (*at).min(self.total));
        let left = until.saturating_sub(self.position);
        if left == 0 {
            return Ok(ReadChunk::End);
        }
        let n = left.min(max as u64) as usize;
        for (index, (reader, lane)) in self.lanes.iter_mut().zip(out.iter_mut()).enumerate() {
            lane.resize(n, Complex::new(0.0, 0.0));
            let read = reader.read_block(lane)?;
            if read != n {
                return Err(SigmfError::Malformed(format!(
                    "lane {index} ended {} samples early",
                    n - read
                )));
            }
        }
        self.position += n as u64;
        Ok(ReadChunk::Samples(n))
    }
}

fn open_stream(dir: &Path, stream: &StreamTuple) -> Result<SigmfReader, SigmfError> {
    if !recording_stem_valid(&stream.name) {
        return Err(SigmfError::Malformed(format!(
            "stream `{}` is not a recording in the collection's directory",
            stream.name
        )));
    }
    let stem = dir.join(&stream.name);
    if meta_hash(&stem)? != stream.hash {
        return Err(SigmfError::Malformed(format!(
            "stream `{}` does not match the collection's hash",
            stream.name
        )));
    }
    SigmfReader::open(&stem)
}

struct Layout {
    sample_rate: f64,
    centers: Vec<f64>,
    total: u64,
}

impl Layout {
    fn of(lanes: &[SigmfReader], metas: &[SigmfMeta]) -> Result<Self, SigmfError> {
        let first = &metas[0];
        let sample_rate = first
            .global
            .sample_rate
            .filter(|rate| rate.is_finite() && *rate > 0.0)
            .ok_or_else(|| SigmfError::Malformed("lane 0 has no sample rate".to_owned()))?;
        let total = lanes[0].total_samples();
        for (index, (lane, meta)) in lanes.iter().zip(metas).enumerate().skip(1) {
            let starts = |meta: &SigmfMeta| -> Vec<u64> {
                meta.captures.iter().map(|c| c.sample_start).collect()
            };
            let differs = meta.global.sample_rate != Some(sample_rate)
                || lane.total_samples() != total
                || starts(meta) != starts(first);
            if differs {
                return Err(SigmfError::Malformed(format!(
                    "lane {index} does not line up with lane 0"
                )));
            }
        }
        let centers = metas
            .iter()
            .map(|meta| {
                meta.captures
                    .first()
                    .and_then(|capture| capture.frequency)
                    .unwrap_or(0.0)
            })
            .collect();
        Ok(Self {
            sample_rate,
            centers,
            total,
        })
    }
}

fn events_of(metas: &[SigmfMeta]) -> Result<Vec<(u64, Event)>, SigmfError> {
    let mut events = capture_events(metas)?;
    for annotation in &metas[0].annotations {
        let Ok(noise) = serde_json::from_value::<NoiseAnnotation>(annotation.clone()) else {
            continue;
        };
        if noise.label == NOISE_LABEL {
            events.push((noise.sample_start, Event::Noise(true)));
            events.push((
                noise.sample_start.saturating_add(noise.sample_count),
                Event::Noise(false),
            ));
        }
    }
    events.sort_by_key(|(at, event)| (*at, event.rank()));
    Ok(events)
}

fn capture_events(metas: &[SigmfMeta]) -> Result<Vec<(u64, Event)>, SigmfError> {
    let captures = &metas[0].captures;
    let mut events = Vec::new();
    let Some(first) = captures.first() else {
        return Ok(events);
    };
    if let Some(offsets) = &first.offsets {
        events.push((first.sample_start, Event::Offsets(offsets.clone())));
    }
    let mut global = first.global_index.unwrap_or(first.sample_start);
    for (index, pair) in captures.windows(2).enumerate() {
        let (previous, capture) = (&pair[0], &pair[1]);
        let expected = global + (capture.sample_start - previous.sample_start);
        let missing = gap_before(capture, expected)?;
        global = expected + missing;
        if missing > 0 {
            events.push((capture.sample_start, Event::Gap(missing)));
        }
        if capture.datetime.is_some() || capture.frequency != previous.frequency {
            events.push((
                capture.sample_start,
                Event::Retuned(centers_at(metas, index + 1)),
            ));
        }
        if let Some(offsets) = &capture.offsets {
            events.push((capture.sample_start, Event::Offsets(offsets.clone())));
        }
    }
    Ok(events)
}

fn gap_before(capture: &SigmfCapture, expected: u64) -> Result<u64, SigmfError> {
    match capture.global_index {
        Some(global) => global.checked_sub(expected).ok_or_else(|| {
            SigmfError::Malformed(format!(
                "capture at {} steps the global index backwards",
                capture.sample_start
            ))
        }),
        None => Ok(0),
    }
}

fn centers_at(metas: &[SigmfMeta], capture: usize) -> Vec<f64> {
    metas
        .iter()
        .map(|meta| {
            meta.captures
                .get(capture)
                .and_then(|capture| capture.frequency)
                .unwrap_or(0.0)
        })
        .collect()
}

#[cfg(test)]
mod tests;
