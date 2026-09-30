use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

use num_complex::Complex;
use rtrb::{Consumer, Producer, RingBuffer};
use sdrmm_channels::array_processor::MAX_LANES;
use sdrmm_device::lock;
use sdrmm_recorder::CollectionWriter;
use sdrmm_wire::ArrayRecordingStatus;

use crate::EngineError;

pub(crate) const RECORD_SECONDS: f64 = 0.5;
pub(crate) const RECORD_NOTES: usize = 256;
const RECORD_MIN: usize = 1 << 16;
const WRITE_CHUNK: usize = 1 << 14;
const IDLE: Duration = Duration::from_millis(5);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum RecordNote {
    Start { at: u64 },
    Gap { at: u64, missing: u64 },
    NoiseOn { at: u64 },
    NoiseOff { at: u64 },
    Retuned { at: u64, centers: [f64; MAX_LANES] },
    Offsets { at: u64, offsets: [i64; MAX_LANES] },
}

impl RecordNote {
    const fn at(&self) -> u64 {
        match *self {
            Self::Start { at }
            | Self::Gap { at, .. }
            | Self::NoiseOn { at }
            | Self::NoiseOff { at }
            | Self::Retuned { at, .. }
            | Self::Offsets { at, .. } => at,
        }
    }
}

pub(crate) struct RecordTap {
    lanes: Vec<Producer<Complex<f32>>>,
    notes: Producer<RecordNote>,
    next: Option<u64>,
    shared: Arc<RecordShared>,
}

impl RecordTap {
    pub(crate) fn push(&mut self, raw: &[&[Complex<f32>]], index: u64, notes: &[RecordNote]) {
        let count = raw.first().map_or(0, |lane| lane.len());
        if count == 0 {
            return;
        }
        match self.next {
            None => self.note(RecordNote::Start { at: index }),
            Some(next) if index > next => self.note(RecordNote::Gap {
                at: next,
                missing: index - next,
            }),
            Some(_) => {}
        }
        for note in notes {
            self.note(*note);
        }
        self.next = Some(index + count as u64);
        let room = self.lanes.iter().map(Producer::slots).min().unwrap_or(0);
        if room < count || raw.len() != self.lanes.len() {
            self.shared
                .dropped
                .fetch_add(count as u64, Ordering::Relaxed);
            self.note(RecordNote::Gap {
                at: index,
                missing: count as u64,
            });
            return;
        }
        for (lane, samples) in self.lanes.iter_mut().zip(raw) {
            let _ = lane.push_entire_slice(samples);
        }
    }

    pub(crate) fn next(&self) -> Option<u64> {
        self.next
    }

    pub(crate) fn note(&mut self, note: RecordNote) {
        if self.notes.push(note).is_err() {
            self.shared.notes_lost.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Default)]
pub(crate) struct RecordShared {
    samples: AtomicU64,
    dropped: AtomicU64,
    notes_lost: AtomicU64,
    error: Mutex<Option<String>>,
}

pub(crate) struct RecordReader {
    lanes: Vec<Consumer<Complex<f32>>>,
    notes: Consumer<RecordNote>,
}

pub(crate) fn record_tap(lanes: usize, sample_rate: f64) -> (Box<RecordTap>, RecordReader) {
    let capacity = ((sample_rate * RECORD_SECONDS) as usize).max(RECORD_MIN);
    let (producers, consumers): (Vec<_>, Vec<_>) = (0..lanes.min(MAX_LANES))
        .map(|_| RingBuffer::new(capacity))
        .unzip();
    let (notes_tx, notes_rx) = RingBuffer::new(RECORD_NOTES);
    (
        Box::new(RecordTap {
            lanes: producers,
            notes: notes_tx,
            next: None,
            shared: Arc::new(RecordShared::default()),
        }),
        RecordReader {
            lanes: consumers,
            notes: notes_rx,
        },
    )
}

pub(crate) struct ArrayRecording {
    stem: String,
    started_at: String,
    shared: Arc<RecordShared>,
    writer: Option<JoinHandle<()>>,
}

impl ArrayRecording {
    pub(crate) fn start(
        name: String,
        stem: String,
        started_at: String,
        writer: CollectionWriter,
        tap: &RecordTap,
        reader: RecordReader,
    ) -> Result<Self, EngineError> {
        let shared = tap.shared.clone();
        let worker_shared = shared.clone();
        let handle = std::thread::Builder::new()
            .name(name)
            .spawn(move || write_loop(writer, reader, &worker_shared))
            .map_err(|error| EngineError::Recording(format!("start array recorder: {error}")))?;
        Ok(Self {
            stem,
            started_at,
            shared,
            writer: Some(handle),
        })
    }

    pub(crate) fn status(&self) -> ArrayRecordingStatus {
        let lost = self.shared.notes_lost.load(Ordering::Relaxed);
        let error = lock(&self.shared.error)
            .clone()
            .or_else(|| (lost > 0).then(|| format!("{lost} recording marks lost")));
        ArrayRecordingStatus {
            stem: self.stem.clone(),
            started_at: self.started_at.clone(),
            samples: self.shared.samples.load(Ordering::Relaxed),
            dropped: self.shared.dropped.load(Ordering::Relaxed),
            error,
        }
    }

    pub(crate) fn finish(mut self) -> ArrayRecordingStatus {
        if let Some(writer) = self.writer.take()
            && writer.join().is_err()
        {
            *lock(&self.shared.error) = Some("array recorder panicked".to_owned());
        }
        self.status()
    }
}

impl Drop for ArrayRecording {
    fn drop(&mut self) {
        if let Some(writer) = self.writer.take()
            && writer.join().is_err()
        {
            tracing::error!("array recorder panicked");
        }
    }
}

struct Cursor {
    position: Option<u64>,
    noise_from: Option<u64>,
    chunk: Vec<Vec<Complex<f32>>>,
}

fn write_loop(mut writer: CollectionWriter, mut reader: RecordReader, shared: &RecordShared) {
    let lanes = reader.lanes.len();
    let mut cursor = Cursor {
        position: None,
        noise_from: None,
        chunk: (0..lanes)
            .map(|_| Vec::with_capacity(WRITE_CHUNK))
            .collect(),
    };
    loop {
        match drain(&mut writer, &mut reader, &mut cursor, shared) {
            Ok(true) => {}
            Ok(false) => {
                let done = reader.notes.is_abandoned()
                    && reader.notes.is_empty()
                    && reader.lanes.iter().all(Consumer::is_empty);
                if done {
                    break;
                }
                std::thread::sleep(IDLE);
            }
            Err(error) => {
                *lock(&shared.error) = Some(error);
                return;
            }
        }
    }
    if let Err(error) = writer.finalize() {
        *lock(&shared.error) = Some(error.to_string());
    }
}

fn drain(
    writer: &mut CollectionWriter,
    reader: &mut RecordReader,
    cursor: &mut Cursor,
    shared: &RecordShared,
) -> Result<bool, String> {
    let mut progressed = false;
    while let Ok(note) = reader.notes.peek().copied() {
        let due = match (cursor.position, note) {
            (None, _) | (_, RecordNote::Start { .. }) => true,
            (Some(position), note) => note.at() <= position,
        };
        if !due {
            break;
        }
        let _ = reader.notes.pop();
        apply(writer, cursor, note).map_err(|error| error.to_string())?;
        progressed = true;
    }
    let Some(position) = cursor.position else {
        return Ok(progressed);
    };
    let limit = reader.notes.peek().map_or(usize::MAX, |note| {
        note.at().saturating_sub(position) as usize
    });
    let ready = reader
        .lanes
        .iter()
        .map(Consumer::slots)
        .min()
        .unwrap_or(0)
        .min(limit)
        .min(WRITE_CHUNK);
    if ready == 0 {
        return Ok(progressed);
    }
    for (lane, out) in reader.lanes.iter_mut().zip(&mut cursor.chunk) {
        out.resize(ready, Complex::default());
        let _ = lane.pop_entire_slice(out);
    }
    let views: Vec<&[Complex<f32>]> = cursor.chunk.iter().map(Vec::as_slice).collect();
    writer.write(&views).map_err(|error| error.to_string())?;
    cursor.position = Some(position + ready as u64);
    shared.samples.fetch_add(ready as u64, Ordering::Relaxed);
    Ok(true)
}

fn apply(
    writer: &mut CollectionWriter,
    cursor: &mut Cursor,
    note: RecordNote,
) -> Result<(), sdrmm_recorder::SigmfError> {
    let lanes = writer.lanes();
    match note {
        RecordNote::Start { at } => match cursor.position {
            Some(position) if at > position => {
                writer.gap(at - position)?;
                cursor.position = Some(at);
            }
            Some(_) => {}
            None => cursor.position = Some(at),
        },
        RecordNote::Gap { at, missing } => {
            writer.gap(missing)?;
            cursor.position = Some(at + missing);
        }
        RecordNote::NoiseOn { at } => cursor.noise_from = Some(written_at(writer, cursor, at)),
        RecordNote::NoiseOff { at } => {
            if let Some(from) = cursor.noise_from.take() {
                let until = written_at(writer, cursor, at);
                writer.noise(from, until.saturating_sub(from))?;
            }
        }
        RecordNote::Retuned { centers, .. } => {
            let at = format!("{:.9}", jiff::Timestamp::now());
            writer.retuned(&centers[..lanes.min(MAX_LANES)], &at)?;
        }
        RecordNote::Offsets { offsets, .. } => {
            writer.offsets(&offsets[..lanes.min(MAX_LANES)])?;
        }
    }
    Ok(())
}

fn written_at(writer: &CollectionWriter, cursor: &Cursor, at: u64) -> u64 {
    let behind = cursor
        .position
        .map_or(0, |position| position.saturating_sub(at));
    writer.samples_written().saturating_sub(behind)
}

#[cfg(test)]
mod tests;
