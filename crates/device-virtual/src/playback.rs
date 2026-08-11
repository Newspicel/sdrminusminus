//! SigMF file playback: replays a finalized recording as if it were a live capture, paced to
//! real time. Capabilities pin the recorded center and rate (min == max range, single-entry
//! rate list), so the generic capability UI shows the true tuning and `apply` rejects retunes
//! with the same validation shape as the siggen; the only real knob is the `loop` toggle.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;
use num_complex::Complex;
use sdrmm_device::{DeviceError, RxSink, SdrDevice, Worker, check_stream_settings, single_rx_sink};
use sdrmm_recorder::{SigmfError, SigmfReader};
use sdrmm_wire::{
    Capabilities, DeviceSettings, Duplex, ExtraSetting, ExtraValue, Range, StreamScope,
};

use crate::{BLOCK_SECS, DRIVER_ID, FILE_KEY_PREFIX};

/// The one extra setting: wrap to sample 0 at end of data (true) or park silent (false).
pub const LOOP_SETTING: &str = "loop";

/// Parameters the playback thread reads per block via [`ArcSwap`] (snapshot, no lock).
#[derive(Clone, Copy)]
struct PlaybackParams {
    looping: bool,
}

/// An opened SigMF recording streaming as a device.
pub struct FilePlayback {
    stem: PathBuf,
    sample_rate: f64,
    capabilities: Capabilities,
    settings: DeviceSettings,
    shared: Arc<ArcSwap<PlaybackParams>>,
    worker: Worker,
}

impl FilePlayback {
    pub fn open(stem: &Path) -> Result<Self, DeviceError> {
        let reader = SigmfReader::open(stem).map_err(|err| open_error(stem, err))?;
        let meta = reader.meta();
        // `core:sample_rate` is optional in SigMF, but playback cannot pace without one.
        let Some(sample_rate) = meta.global.sample_rate else {
            return Err(DeviceError::Unsupported(
                "recording has no core:sample_rate".to_string(),
            ));
        };
        // A non-positive or non-finite rate would poison the pacing arithmetic
        // (`Duration::from_secs_f64` panics on non-finite input).
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(DeviceError::Unsupported(format!(
                "recorded sample_rate {sample_rate}"
            )));
        }
        let center_hz = meta
            .captures
            .first()
            .and_then(|capture| capture.frequency)
            .unwrap_or(0.0);
        if !center_hz.is_finite() {
            return Err(DeviceError::Unsupported(format!(
                "recorded center_hz {center_hz}"
            )));
        }
        let capabilities = Capabilities {
            freq_ranges: vec![Range {
                min: center_hz,
                max: center_hz,
                step: None,
            }],
            sample_rates: vec![sample_rate],
            sample_rate_range: None,
            gains: Vec::new(),
            antennas: Vec::new(),
            bandwidths: Vec::new(),
            extra: vec![ExtraSetting::Bool {
                name: LOOP_SETTING.to_string(),
                default: true,
            }],
            duplex: Duplex::RxOnly,
            rx_streams: 1,
            tx_streams: 0,
            per_stream: StreamScope::default(),
        };
        let settings = DeviceSettings {
            center_hz: Some(center_hz),
            sample_rate: Some(sample_rate),
            extra: vec![ExtraValue {
                name: LOOP_SETTING.to_string(),
                value: serde_json::Value::Bool(true),
            }],
            ..DeviceSettings::default()
        };
        Ok(Self {
            stem: stem.to_path_buf(),
            sample_rate,
            capabilities,
            settings,
            shared: Arc::new(ArcSwap::from_pointee(PlaybackParams { looping: true })),
            worker: Worker::new(),
        })
    }

    fn looping(&self) -> bool {
        self.settings
            .extra
            .iter()
            .find(|extra| extra.name == LOOP_SETTING)
            .and_then(|extra| extra.value.as_bool())
            .unwrap_or(true)
    }
}

fn open_error(stem: &Path, err: SigmfError) -> DeviceError {
    match err {
        SigmfError::Io(io) if io.kind() == std::io::ErrorKind::NotFound => {
            DeviceError::NotFound(format!("{DRIVER_ID}:{FILE_KEY_PREFIX}{}", stem.display()))
        }
        SigmfError::UnsupportedDatatype(_) => DeviceError::Unsupported(err.to_string()),
        other => DeviceError::Io(other.to_string()),
    }
}

impl SdrDevice for FilePlayback {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        check_stream_settings(settings, &self.capabilities)?;
        if let Some(rate) = settings.sample_rate
            && !self.capabilities.sample_rates.contains(&rate)
        {
            return Err(DeviceError::Unsupported(format!(
                "sample_rate {rate}: a recording plays at its recorded rate"
            )));
        }
        if let Some(f) = settings.center_hz
            && !self
                .capabilities
                .freq_ranges
                .iter()
                .any(|r| r.min <= f && f <= r.max)
        {
            return Err(DeviceError::Unsupported(format!(
                "center_hz {f}: a recording is pinned to its recorded center"
            )));
        }
        for extra in &settings.extra {
            if extra.name != LOOP_SETTING {
                return Err(DeviceError::Unsupported(format!("extra `{}`", extra.name)));
            }
            if !extra.value.is_boolean() {
                return Err(DeviceError::Unsupported(format!(
                    "`{LOOP_SETTING}` must be a boolean, got {}",
                    extra.value
                )));
            }
        }
        self.settings.merge_from(settings);
        self.shared.store(Arc::new(PlaybackParams {
            looping: self.looping(),
        }));
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let mut sink = single_rx_sink(sinks)?;
        let shared = self.shared.clone();
        let stem = self.stem.clone();
        let sample_rate = self.sample_rate;
        self.worker.start("sdrmm-playback-rx", move |running| {
            // Opened on the worker so a file that vanished since `open` surfaces through the
            // fault channel like any other dead capture, not as an rx_start error.
            let mut reader = match SigmfReader::open(&stem) {
                Ok(reader) => reader,
                Err(err) => {
                    sink.fail(DeviceError::Io(err.to_string()));
                    return;
                }
            };
            let n = ((sample_rate * BLOCK_SECS).round() as usize).max(1);
            let mut block = vec![Complex::new(0.0f32, 0.0); n];
            let mut next = Instant::now();
            'stream: while running.load(Ordering::Acquire) {
                let params = *shared.load_full();
                let mut filled = 0;
                while filled < block.len() {
                    match reader.read_block(&mut block[filled..]) {
                        // An empty recording must park below, not spin on rewind.
                        Ok(0) if params.looping && reader.total_samples() > 0 => {
                            if let Err(err) = reader.rewind() {
                                sink.fail(DeviceError::Io(err.to_string()));
                                return;
                            }
                        }
                        Ok(0) => break,
                        Ok(read) => filled += read,
                        Err(err) => {
                            sink.fail(DeviceError::Io(err.to_string()));
                            return;
                        }
                    }
                }
                if filled > 0 {
                    sink.push(&block[..filled]);
                }
                if filled < block.len() {
                    // End of data with looping off: hold silent (the spectrum freezes —
                    // honest idle), but keep watching so re-enabling `loop` resumes at 0.
                    while running.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_secs_f64(BLOCK_SECS));
                        if shared.load_full().looping {
                            next = Instant::now();
                            continue 'stream;
                        }
                    }
                    return;
                }

                // Pace to ~real time, resyncing without debt when behind (as the siggen does).
                next += Duration::from_secs_f64(filled as f64 / sample_rate);
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else {
                    next = now;
                }
            }
        })
    }

    fn rx_stop(&mut self) {
        self.worker.stop();
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::mpsc, time::Duration};

    use sdrmm_recorder::{SigmfWriter, data_path, meta_path};
    use sdrmm_wire::StreamSettings;
    use tempfile::TempDir;

    use super::*;

    fn tone(n: usize) -> Vec<Complex<f32>> {
        (0..n)
            .map(|i| {
                let phase = i as f32 * 0.01;
                Complex::new(phase.cos(), phase.sin())
            })
            .collect()
    }

    fn record(dir: &Path, name: &str, samples: &[Complex<f32>]) -> PathBuf {
        let stem = dir.join(name);
        let mut writer = SigmfWriter::create(&stem, 250_000.0, 100_000_000.0, "test").unwrap();
        writer.write_block(samples).unwrap();
        writer.finalize().unwrap();
        stem
    }

    fn loop_setting(on: bool) -> DeviceSettings {
        DeviceSettings {
            extra: vec![ExtraValue {
                name: LOOP_SETTING.to_string(),
                value: serde_json::Value::Bool(on),
            }],
            ..DeviceSettings::default()
        }
    }

    fn start(dev: &mut FilePlayback) -> mpsc::Receiver<Vec<Complex<f32>>> {
        let (tx, rx) = mpsc::channel();
        dev.rx_start(vec![RxSink::new(move |s| {
            let _ = tx.send(s.to_vec());
        })])
        .unwrap();
        rx
    }

    fn collect(rx: &mpsc::Receiver<Vec<Complex<f32>>>, at_least: usize) -> Vec<Complex<f32>> {
        let mut out = Vec::new();
        while out.len() < at_least {
            out.extend(rx.recv_timeout(Duration::from_secs(2)).unwrap());
        }
        out
    }

    fn assert_bits_eq(a: &[Complex<f32>], b: &[Complex<f32>]) {
        assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(b).enumerate() {
            assert_eq!(x.re.to_bits(), y.re.to_bits(), "re mismatch at {i}");
            assert_eq!(x.im.to_bits(), y.im.to_bits(), "im mismatch at {i}");
        }
    }

    #[test]
    fn plays_exact_samples_once_then_parks() {
        let dir = TempDir::new().unwrap();
        let recorded = tone(10_000);
        let stem = record(dir.path(), "once", &recorded);

        let mut dev = FilePlayback::open(&stem).unwrap();
        dev.apply(&loop_setting(false)).unwrap();
        let rx = start(&mut dev);
        assert_bits_eq(&recorded, &collect(&rx, 10_000));
        // Parked at end of data: nothing further may arrive.
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        dev.rx_stop();

        // A restart replays from sample 0.
        let rx = start(&mut dev);
        assert_bits_eq(&recorded, &collect(&rx, 10_000));
        dev.rx_stop();
    }

    #[test]
    fn loop_wraps_to_sample_zero_exactly() {
        let dir = TempDir::new().unwrap();
        let recorded = tone(1_000);
        let stem = record(dir.path(), "looped", &recorded);

        // `loop` defaults on; 12_000 samples span the file 12 times.
        let mut dev = FilePlayback::open(&stem).unwrap();
        let rx = start(&mut dev);
        let streamed = collect(&rx, 12_000);
        dev.rx_stop();
        for (i, sample) in streamed.iter().enumerate() {
            let expected = recorded[i % recorded.len()];
            assert_eq!(
                sample.re.to_bits(),
                expected.re.to_bits(),
                "re mismatch at {i}"
            );
            assert_eq!(
                sample.im.to_bits(),
                expected.im.to_bits(),
                "im mismatch at {i}"
            );
        }
    }

    #[test]
    fn pins_capabilities_to_the_recording() {
        let dir = TempDir::new().unwrap();
        let stem = record(dir.path(), "pinned", &tone(10));

        let dev = FilePlayback::open(&stem).unwrap();
        let caps = dev.capabilities();
        assert_eq!(caps.sample_rates, vec![250_000.0]);
        assert_eq!(caps.freq_ranges.len(), 1);
        assert_eq!(caps.freq_ranges[0].min, 100_000_000.0);
        assert_eq!(caps.freq_ranges[0].max, 100_000_000.0);
        assert!(matches!(
            &caps.extra[..],
            [ExtraSetting::Bool { name, default: true }] if name == LOOP_SETTING
        ));
        assert_eq!(dev.settings().center_hz, Some(100_000_000.0));
        assert_eq!(dev.settings().sample_rate, Some(250_000.0));
    }

    #[test]
    fn rejects_retune_rate_change_and_bad_extras() {
        let dir = TempDir::new().unwrap();
        let stem = record(dir.path(), "fixed", &tone(10));
        let mut dev = FilePlayback::open(&stem).unwrap();

        for bad in [
            DeviceSettings {
                sample_rate: Some(2_048_000.0),
                ..DeviceSettings::default()
            },
            DeviceSettings {
                center_hz: Some(101_000_000.0),
                ..DeviceSettings::default()
            },
            DeviceSettings {
                extra: vec![ExtraValue {
                    name: "agc".to_string(),
                    value: serde_json::Value::Bool(true),
                }],
                ..DeviceSettings::default()
            },
            DeviceSettings {
                extra: vec![ExtraValue {
                    name: LOOP_SETTING.to_string(),
                    value: serde_json::json!("yes"),
                }],
                ..DeviceSettings::default()
            },
            // A recording has one stream and declares nothing per-stream.
            DeviceSettings {
                streams: vec![StreamSettings {
                    stream: 0,
                    center_hz: Some(100_000_000.0),
                    ..StreamSettings::default()
                }],
                ..DeviceSettings::default()
            },
        ] {
            assert!(
                matches!(dev.apply(&bad), Err(DeviceError::Unsupported(_))),
                "must reject {bad:?}"
            );
        }
        // Rejected deltas must not leak into settings.
        assert!(dev.looping());

        // Re-applying the recorded values and toggling `loop` round-trip.
        dev.apply(&DeviceSettings {
            center_hz: Some(100_000_000.0),
            sample_rate: Some(250_000.0),
            ..DeviceSettings::default()
        })
        .unwrap();
        dev.apply(&loop_setting(false)).unwrap();
        assert!(!dev.looping());
        assert_eq!(dev.settings().extra.len(), 1);
    }

    #[test]
    fn open_missing_recording_is_not_found() {
        let dir = TempDir::new().unwrap();
        let stem = dir.path().join("nope");
        match FilePlayback::open(&stem) {
            Err(DeviceError::NotFound(id)) => {
                assert_eq!(id, format!("virtual:file:{}", stem.display()));
            }
            Err(other) => panic!("expected NotFound, got {other:?}"),
            Ok(_) => panic!("expected NotFound, got a device"),
        }
    }

    #[test]
    fn open_corrupt_meta_is_io() {
        let dir = TempDir::new().unwrap();
        let stem = dir.path().join("corrupt");
        fs::write(meta_path(&stem), "not json").unwrap();
        fs::write(data_path(&stem), []).unwrap();
        assert!(matches!(FilePlayback::open(&stem), Err(DeviceError::Io(_))));
    }

    #[test]
    fn open_rejects_foreign_datatype_and_bad_rate() {
        let dir = TempDir::new().unwrap();
        for (name, meta) in [
            (
                "foreign",
                r#"{"global":{"core:datatype":"ci16_le","core:version":"1.2.6","core:sample_rate":48000.0},"captures":[]}"#,
            ),
            (
                "zero_rate",
                r#"{"global":{"core:datatype":"cf32_le","core:version":"1.2.6","core:sample_rate":0.0},"captures":[]}"#,
            ),
            // Spec-legal minimal meta: parses, but playback needs a rate to pace.
            (
                "no_rate",
                r#"{"global":{"core:datatype":"cf32_le","core:version":"1.2.6"},"captures":[]}"#,
            ),
        ] {
            let stem = dir.path().join(name);
            fs::write(meta_path(&stem), meta).unwrap();
            fs::write(data_path(&stem), []).unwrap();
            assert!(
                matches!(FilePlayback::open(&stem), Err(DeviceError::Unsupported(_))),
                "{name} must be rejected"
            );
        }
    }

    #[test]
    fn empty_recording_streams_nothing() {
        let dir = TempDir::new().unwrap();
        let stem = record(dir.path(), "empty", &[]);
        let mut dev = FilePlayback::open(&stem).unwrap();
        let rx = start(&mut dev);
        assert!(rx.recv_timeout(Duration::from_millis(150)).is_err());
        dev.rx_stop();
    }

    #[test]
    fn vanished_data_surfaces_through_the_fatal_handler() {
        let dir = TempDir::new().unwrap();
        let stem = record(dir.path(), "vanishing", &tone(10));
        let mut dev = FilePlayback::open(&stem).unwrap();
        fs::remove_file(data_path(&stem)).unwrap();

        let (tx, rx) = mpsc::channel();
        dev.rx_start(vec![RxSink::with_fatal_handler(
            |_| {},
            move |err| {
                let _ = tx.send(err);
            },
        )])
        .unwrap();
        let err = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(err, DeviceError::Io(_)));
        dev.rx_stop();
    }
}
