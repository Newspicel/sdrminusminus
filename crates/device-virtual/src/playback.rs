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
use sdrmm_device::{
    DeviceError, PlaybackShared, RxSink, SdrDevice, Worker, check_stream_settings, single_rx_sink,
};
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
    playback_speed: f64,
    capabilities: Capabilities,
    settings: DeviceSettings,
    shared: Arc<ArcSwap<PlaybackParams>>,
    transport: Arc<PlaybackShared>,
    worker: Worker,
}

impl FilePlayback {
    pub fn open(stem: &Path) -> Result<Self, DeviceError> {
        Self::open_at_speed(stem, 1.0)
    }

    pub(crate) fn open_at_speed(stem: &Path, playback_speed: f64) -> Result<Self, DeviceError> {
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
            ppm: false,
            duplex: Duplex::RxOnly,
            rx_streams: 1,
            tx_streams: 0,
            per_stream: StreamScope::default(),
            directional: None,
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
            playback_speed,
            capabilities,
            settings,
            shared: Arc::new(ArcSwap::from_pointee(PlaybackParams { looping: true })),
            // From the data file, not the metadata: a crash-truncated pair must report the
            // length that can actually be replayed, or the transport bar promises samples the
            // reader will never reach.
            transport: Arc::new(PlaybackShared::new(reader.total_samples())),
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
        let transport = self.transport.clone();
        let stem = self.stem.clone();
        let sample_rate = self.sample_rate;
        let playback_speed = self.playback_speed;
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
            // [`BLOCK_SECS`] of wall clock, not of recorded time: a block that stayed
            // real-time-sized would make an accelerated stream wake `speed` times as often for
            // the same recording, and per-wake scheduler slack, not the requested speed, would
            // decide how fast the tape ran.
            let n = ((sample_rate * BLOCK_SECS * playback_speed).round() as usize).max(1);
            let mut block = vec![Complex::new(0.0f32, 0.0); n];
            let mut next = Instant::now();
            'stream: while running.load(Ordering::Acquire) {
                let params = *shared.load_full();
                let position_generation = transport.position_generation();
                if let Some(target) = transport.take_seek()
                    && let Err(err) = reader.seek_to(target)
                {
                    sink.fail(DeviceError::Io(err.to_string()));
                    return;
                }
                // A paused transport consumes nothing and emits nothing — the spectrum freezes
                // where it stood, which is the honest picture of a stopped tape. It still has
                // to wake on a resume, a seek or a stop, so the park is one block long.
                if transport.paused() {
                    std::thread::sleep(Duration::from_secs_f64(BLOCK_SECS));
                    next = Instant::now();
                    continue;
                }
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
                transport.set_position(reader.position(), position_generation);
                if filled < block.len() {
                    // End of data with looping off: hold silent (the spectrum freezes —
                    // honest idle), but keep watching so re-enabling `loop`, or scrubbing back
                    // into the recording, resumes it.
                    while running.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_secs_f64(BLOCK_SECS));
                        if shared.load_full().looping || transport.seek_pending() {
                            next = Instant::now();
                            continue 'stream;
                        }
                    }
                    return;
                }

                next += Duration::from_secs_f64(filled as f64 / sample_rate / playback_speed);
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

    fn playback(&self) -> Option<Arc<PlaybackShared>> {
        Some(self.transport.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::mpsc, time::Duration};

    use sdrmm_recorder::{SigmfWriter, data_path, meta_path};
    use sdrmm_wire::{PlaybackAction, PlaybackRequest, StreamSettings};
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

    fn transport(action: PlaybackAction, position_samples: Option<u64>) -> PlaybackRequest {
        PlaybackRequest {
            action,
            position_samples,
        }
    }

    /// Pause has to stop the tape, not mute it: a paused transport that kept reading would run
    /// off the end of the recording while the operator was looking at a frozen spectrum.
    #[test]
    fn pause_stops_consuming_and_play_resumes_where_it_stopped() {
        let dir = TempDir::new().unwrap();
        let recorded = tone(250_000);
        let stem = record(dir.path(), "paused", &recorded);

        let mut dev = FilePlayback::open(&stem).unwrap();
        let control = dev.playback().expect("a recording has a transport");
        assert_eq!(control.status().total_samples, 250_000);

        let rx = start(&mut dev);
        let before = collect(&rx, 1);
        control.control(&transport(PlaybackAction::Pause, None));

        // Whatever block was already in flight may still land; after that, silence.
        while rx.recv_timeout(Duration::from_millis(200)).is_ok() {}
        let at_pause = control.status();
        assert!(at_pause.paused);
        assert!(at_pause.position_samples >= before.len() as u64);
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());

        control.control(&transport(PlaybackAction::Play, None));
        let after = collect(&rx, 1);
        assert!(!control.status().paused);
        assert!(
            control.status().position_samples > at_pause.position_samples,
            "playback resumed"
        );
        assert!(!after.is_empty());
        dev.rx_stop();
    }

    #[test]
    fn stop_pauses_and_rewinds_so_play_starts_over() {
        let dir = TempDir::new().unwrap();
        let recorded = tone(250_000);
        let stem = record(dir.path(), "stopped", &recorded);

        let mut dev = FilePlayback::open(&stem).unwrap();
        let control = dev.playback().unwrap();
        let rx = start(&mut dev);
        collect(&rx, 1);

        control.control(&transport(PlaybackAction::Stop, None));
        while rx.recv_timeout(Duration::from_millis(200)).is_ok() {}
        let stopped = control.status();
        assert!(stopped.paused);
        assert_eq!(stopped.position_samples, 0);

        // Playing again yields the head of the recording, not the middle.
        control.control(&transport(PlaybackAction::Play, None));
        assert_bits_eq(&recorded[..1_000], &collect(&rx, 1_000)[..1_000]);
        dev.rx_stop();
    }

    /// A seek must land on the sample asked for, so the samples that follow are the ones the
    /// operator scrubbed to — an off-by-a-block transport is worse than none.
    #[test]
    fn a_seek_replays_from_exactly_that_sample() {
        let dir = TempDir::new().unwrap();
        let recorded = tone(250_000);
        let stem = record(dir.path(), "sought", &recorded);

        let mut dev = FilePlayback::open(&stem).unwrap();
        let control = dev.playback().unwrap();
        // Paused first, so the seek is not raced by the block the worker is already reading.
        control.control(&transport(PlaybackAction::Pause, None));
        let rx = start(&mut dev);
        while rx.recv_timeout(Duration::from_millis(200)).is_ok() {}

        control.control(&transport(PlaybackAction::Seek, Some(100_000)));
        assert_eq!(control.status().position_samples, 100_000);
        control.control(&transport(PlaybackAction::Play, None));

        assert_bits_eq(&recorded[100_000..101_000], &collect(&rx, 1_000)[..1_000]);
        dev.rx_stop();
    }

    /// Parked at the end with looping off, a scrub backwards has to wake the worker — the park
    /// only watched the `loop` flag, so a seek would have sat there unnoticed.
    #[test]
    fn seeking_back_wakes_a_transport_parked_at_the_end() {
        let dir = TempDir::new().unwrap();
        let recorded = tone(2_000);
        let stem = record(dir.path(), "parked", &recorded);

        let mut dev = FilePlayback::open(&stem).unwrap();
        dev.apply(&loop_setting(false)).unwrap();
        let control = dev.playback().unwrap();
        let rx = start(&mut dev);
        assert_bits_eq(&recorded, &collect(&rx, 2_000));
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());

        control.control(&transport(PlaybackAction::Seek, Some(0)));
        assert_bits_eq(&recorded, &collect(&rx, 2_000));
        dev.rx_stop();
    }

    /// The position is what the progress bar draws; it has to track the samples actually
    /// delivered, and wrap with the recording rather than run past its end.
    #[test]
    fn the_reported_position_follows_playback_and_wraps_with_the_loop() {
        let dir = TempDir::new().unwrap();
        let recorded = tone(50_000);
        let stem = record(dir.path(), "position", &recorded);

        let mut dev = FilePlayback::open(&stem).unwrap();
        let control = dev.playback().unwrap();
        let rx = start(&mut dev);

        collect(&rx, 25_000);
        let status = control.status();
        assert!(status.position_samples > 0);
        assert!(status.position_samples <= 50_000, "never past the end");

        // Two full passes: looping is on by default, so the position wraps instead of running on.
        collect(&rx, 100_000);
        assert!(control.status().position_samples <= 50_000);
        dev.rx_stop();
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
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        dev.rx_stop();

        let rx = start(&mut dev);
        assert_bits_eq(&recorded, &collect(&rx, 10_000));
        dev.rx_stop();
    }

    #[test]
    fn loop_wraps_to_sample_zero_exactly() {
        let dir = TempDir::new().unwrap();
        let recorded = tone(1_000);
        let stem = record(dir.path(), "looped", &recorded);

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

    /// An accelerated playback carries `speed` times as many samples per block as a real-time
    /// one, so both wake the same number of times per second and the samples are unchanged.
    /// A 20× stream sized in recorded time woke 20× as often, which is how a loaded runner
    /// came to replay it at a third of the speed the test had asked for.
    #[test]
    fn accelerated_playback_keeps_the_real_time_wake_up_rate() {
        const SPEED: f64 = 8.0;
        let dir = TempDir::new().unwrap();
        let recorded = tone(300_000);
        let stem = record(dir.path(), "accelerated", &recorded);

        let mut real_time = FilePlayback::open(&stem).unwrap();
        let rx = start(&mut real_time);
        let block = rx.recv_timeout(Duration::from_secs(2)).unwrap().len();
        real_time.rx_stop();
        assert_eq!(block, (250_000.0 * BLOCK_SECS) as usize);

        let mut fast = FilePlayback::open_at_speed(&stem, SPEED).unwrap();
        let rx = start(&mut fast);
        let fast_block = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        fast.rx_stop();

        assert_eq!(fast_block.len(), block * SPEED as usize);
        assert_bits_eq(&recorded[..fast_block.len()], &fast_block);
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
        // The recorded centre is a single point and there is no oscillator to correct, so the
        // dial and the ppm field are both readouts on this device, not controls.
        assert_eq!(caps.freq_ranges[0].min, caps.freq_ranges[0].max);
        assert!(!caps.ppm);
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
