use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use num_complex::Complex;
use sdrmm_device::{
    DeviceError, GapScope, LaneMark, RxSink, SdrDevice, Uncertainty, Worker, check_stream_settings,
};
use sdrmm_recorder::{CollectionReader, ReadChunk, SigmfError};
use sdrmm_wire::{
    Capabilities, DeviceSettings, Duplex, NoiseSource, Range, StreamScope, StreamSettings,
};

use crate::{BLOCK_SECS, DRIVER_ID};

const ROOM_POLL: Duration = Duration::from_millis(1);

pub struct CollectionPlayback {
    stem: PathBuf,
    sample_rate: f64,
    centers: Vec<f64>,
    playback_speed: f64,
    capabilities: Capabilities,
    settings: DeviceSettings,
    worker: Worker,
}

impl CollectionPlayback {
    pub fn open(stem: &Path) -> Result<Self, DeviceError> {
        Self::open_at_speed(stem, 1.0)
    }

    pub(crate) fn open_at_speed(stem: &Path, playback_speed: f64) -> Result<Self, DeviceError> {
        let reader = CollectionReader::open(stem).map_err(|err| open_error(stem, err))?;
        let sample_rate = reader.sample_rate();
        let centers = reader.centers_hz().to_vec();
        if let Some(center) = centers.iter().find(|center| !center.is_finite()) {
            return Err(DeviceError::Unsupported(format!(
                "recorded center_hz {center}"
            )));
        }
        let capabilities = capabilities(&reader);
        let settings = DeviceSettings {
            center_hz: centers.first().copied(),
            sample_rate: Some(sample_rate),
            streams: stream_centers(&centers),
            ..DeviceSettings::default()
        };
        Ok(Self {
            stem: stem.to_path_buf(),
            sample_rate,
            centers,
            playback_speed,
            capabilities,
            settings,
            worker: Worker::new(),
        })
    }

    fn check_centers(&self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        if let Some(center) = settings.center_hz
            && self.centers.first() != Some(&center)
        {
            return Err(DeviceError::Unsupported(format!(
                "center_hz {center}: a recording is pinned to its recorded center"
            )));
        }
        for entry in &settings.streams {
            let recorded = self.centers.get(entry.stream as usize);
            if let Some(center) = entry.center_hz
                && recorded != Some(&center)
            {
                return Err(DeviceError::Unsupported(format!(
                    "streams[{}].center_hz {center}: a recorded lane keeps its center",
                    entry.stream
                )));
            }
        }
        Ok(())
    }
}

fn capabilities(reader: &CollectionReader) -> Capabilities {
    let mut freq_ranges: Vec<Range> = Vec::new();
    for center in reader.centers_hz() {
        if !freq_ranges.iter().any(|range| range.min == *center) {
            freq_ranges.push(Range {
                min: *center,
                max: *center,
                step: None,
            });
        }
    }
    Capabilities {
        freq_ranges,
        sample_rates: vec![reader.sample_rate()],
        sample_rate_ranges: Vec::new(),
        gains: Vec::new(),
        antennas: Vec::new(),
        bandwidths: Vec::new(),
        bandwidth_ranges: Vec::new(),
        bandwidth_auto: false,
        bias_tee: false,
        agc: sdrmm_wire::Agc::None,
        extra: Vec::new(),
        ppm: false,
        duplex: Duplex::RxOnly,
        rx_streams: u32::try_from(reader.lanes()).unwrap_or(u32::MAX),
        tx_streams: 0,
        per_stream: StreamScope {
            tuning: true,
            gain: false,
            antenna: false,
            agc: false,
        },
        directional: None,
        dc_artifact: reader.array().dc_artifact,
        hardware_sweep: false,
        coherence: reader.array().tier,
        noise_source: NoiseSource::Replayed,
        retune_keeps_phase: reader.array().retune_keeps_phase,
        rx_stream_choices: Vec::new(),
    }
}

fn stream_centers(centers: &[f64]) -> Vec<StreamSettings> {
    centers
        .iter()
        .enumerate()
        .map(|(stream, center)| StreamSettings {
            stream: u32::try_from(stream).unwrap_or(u32::MAX),
            center_hz: Some(*center),
            ..StreamSettings::default()
        })
        .collect()
}

fn open_error(stem: &Path, err: SigmfError) -> DeviceError {
    match err {
        SigmfError::Io(io) if io.kind() == std::io::ErrorKind::NotFound => {
            DeviceError::NotFound(format!(
                "{DRIVER_ID}:{}",
                stem.file_name().unwrap_or(stem.as_os_str()).display()
            ))
        }
        SigmfError::UnsupportedDatatype(_) => DeviceError::Unsupported(err.to_string()),
        other => DeviceError::Io(other.to_string()),
    }
}

impl SdrDevice for CollectionPlayback {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn in_flight_samples(&self) -> u64 {
        0
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        check_stream_settings(settings, &self.capabilities)?;
        if let Some(rate) = settings.sample_rate
            && rate != self.sample_rate
        {
            return Err(DeviceError::Unsupported(format!(
                "sample_rate {rate}: a recording plays at its recorded rate"
            )));
        }
        self.check_centers(settings)?;
        if settings.agc.is_some() {
            return Err(DeviceError::Unsupported(
                "agc: a recording has no AGC".to_string(),
            ));
        }
        if let Some(extra) = settings.extra.first() {
            return Err(DeviceError::Unsupported(format!("extra `{}`", extra.name)));
        }
        self.settings.merge_from(settings);
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        if sinks.len() != self.centers.len() {
            return Err(DeviceError::Unsupported(format!(
                "this recording has {} lanes, got {} sinks",
                self.centers.len(),
                sinks.len()
            )));
        }
        let stem = self.stem.clone();
        let pace = Pace {
            sample_rate: self.sample_rate,
            playback_speed: self.playback_speed,
        };
        self.worker.start("sdrmm-collection-rx", move |running| {
            play(&stem, sinks, pace, running);
        })
    }

    fn rx_stop(&mut self) {
        self.worker.stop();
    }
}

#[derive(Clone, Copy)]
struct Pace {
    sample_rate: f64,
    playback_speed: f64,
}

impl Pace {
    fn piece(self) -> usize {
        ((self.sample_rate * BLOCK_SECS * self.playback_speed).round() as usize).max(1)
    }

    fn duration(self, samples: usize) -> Duration {
        Duration::from_secs_f64(samples as f64 / self.sample_rate / self.playback_speed)
    }
}

fn play(stem: &Path, mut sinks: Vec<RxSink>, pace: Pace, running: &AtomicBool) {
    let mut reader = match CollectionReader::open(stem) {
        Ok(reader) => reader,
        Err(err) => return fail_all(&mut sinks, &err),
    };
    let piece = pace.piece();
    let mut lanes: Vec<Vec<Complex<f32>>> = vec![Vec::with_capacity(piece); sinks.len()];
    let mut offsets: Option<Vec<i64>> = None;
    let mut next = Instant::now();
    while running.load(Ordering::Acquire) {
        match reader.read(&mut lanes, piece) {
            Ok(ReadChunk::Samples(n)) => {
                if !hand_over(&mut sinks, &lanes, n, running) {
                    return;
                }
                next = sleep_until(next + pace.duration(n));
            }
            Ok(ReadChunk::Gap(missing)) => sinks.iter_mut().for_each(|sink| sink.dropped(missing)),
            Ok(ReadChunk::Noise { on }) => {
                mark_all(&mut sinks, LaneMark::NoiseSource { on, in_flight: 0 })
            }
            Ok(ReadChunk::Retuned(_)) => mark_all(&mut sinks, LaneMark::Retuned { in_flight: 0 }),
            Ok(ReadChunk::Offsets(now)) => realign_moved(&mut sinks, &mut offsets, now),
            Ok(ReadChunk::End) => {
                mark_all(&mut sinks, LaneMark::Ended);
                return idle(running);
            }
            Err(err) => return fail_all(&mut sinks, &err),
        }
    }
}

fn hand_over(
    sinks: &mut [RxSink],
    lanes: &[Vec<Complex<f32>>],
    n: usize,
    running: &AtomicBool,
) -> bool {
    while sinks
        .iter()
        .any(|sink| sink.room().is_some_and(|room| room.free() < n))
    {
        if !running.load(Ordering::Acquire) {
            return false;
        }
        std::thread::sleep(ROOM_POLL);
    }
    for (sink, lane) in sinks.iter_mut().zip(lanes) {
        sink.push(&lane[..n]);
    }
    true
}

fn realign_moved(sinks: &mut [RxSink], held: &mut Option<Vec<i64>>, now: Vec<i64>) {
    if let Some(before) = held.as_ref() {
        for ((sink, was), offset) in sinks.iter_mut().zip(before).zip(&now) {
            if was != offset {
                sink.realigned(
                    Uncertainty::Unaligned,
                    offset.abs_diff(*was),
                    GapScope::Lane,
                );
            }
        }
    }
    *held = Some(now);
}

fn mark_all(sinks: &mut [RxSink], mark: LaneMark) {
    for sink in sinks {
        sink.mark(mark);
    }
}

fn fail_all(sinks: &mut [RxSink], err: &SigmfError) {
    for sink in sinks {
        sink.fail(DeviceError::Io(err.to_string()));
    }
}

fn idle(running: &AtomicBool) {
    while running.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_secs_f64(BLOCK_SECS));
    }
}

fn sleep_until(deadline: Instant) -> Instant {
    let now = Instant::now();
    if deadline > now {
        std::thread::sleep(deadline - now);
        deadline
    } else {
        now
    }
}

#[cfg(test)]
mod tests;
