#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use common::{assert_tone_dominates, collect_packets, settle_then_collect_second};
use num_complex::Complex;
use sdrmm_channels::testgen;
use sdrmm_device::{
    DeviceDriver, DeviceError, DeviceRegistry, RxSink, SdrDevice, check_stream_settings,
};
use sdrmm_device_recording::RecordingDriver;
use sdrmm_device_virtual::{VirtualDriver, stream_marker_offset_hz};
use sdrmm_engine::{Engine, SpectrumSnapshot};
use sdrmm_wire::{
    Capabilities, ChannelParams, ChannelSettings, DcArtifact, DecoderEvent, DeviceInfo,
    DeviceSettings, Duplex, GainValue, NfmParams, PocsagBaud, PocsagParams, ScanSettings,
    StreamScope, StreamSettings, Tuning,
};
use tokio::sync::broadcast;

const ARRAY: &str = "virtual:array4";
const TRANSCEIVER: &str = "virtual:transceiver";
const RATE: f64 = 2_048_000.0;
const DEFAULT_CENTER_HZ: f64 = 100_000_000.0;

fn engine() -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(10, Box::new(VirtualDriver::new()));
    Engine::with_registry(registry, None)
}

fn nfm(offset_hz: f64, squelch_db: Option<f32>) -> ChannelSettings {
    ChannelSettings {
        frequency_hz: DEFAULT_CENTER_HZ + offset_hz,
        squelch: squelch_db.map_or(sdrmm_wire::Squelch::Off, |level_db| {
            sdrmm_wire::Squelch::Manual { level_db }
        }),
        params: ChannelParams::Nfm(NfmParams::default()),
        audio: Default::default(),
    }
}

fn rms(samples: &[f32]) -> f64 {
    let sum: f64 = samples.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
    (sum / samples.len().max(1) as f64).sqrt()
}

async fn next_snapshot(rx: &mut broadcast::Receiver<SpectrumSnapshot>) -> SpectrumSnapshot {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("spectrum within timeout")
        {
            Ok(snapshot) => return snapshot,
            Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => panic!("spectrum stream closed"),
        }
    }
}

fn peak_offset_hz(snapshot: &SpectrumSnapshot) -> f64 {
    let n = snapshot.db.len();
    let mut peak = 0usize;
    for (i, &db) in snapshot.db.iter().enumerate() {
        if db > snapshot.db[peak] {
            peak = i;
        }
    }
    (peak as f64 - n as f64 / 2.0) / n as f64 * f64::from(snapshot.span_hz)
}

fn band_power(samples: &[Complex<f32>], sample_rate: f64, offset_hz: f64) -> f64 {
    const WINDOW: usize = 512;
    let w = std::f64::consts::TAU * offset_hz / sample_rate;
    let mut total = 0.0f64;
    for window in samples.as_chunks::<WINDOW>().0 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (n, s) in window.iter().enumerate() {
            let (sin, cos) = (w * n as f64).sin_cos();
            re += f64::from(s.re) * cos + f64::from(s.im) * sin;
            im += f64::from(s.im) * cos - f64::from(s.re) * sin;
        }
        total += re.mul_add(re, im * im);
    }
    total
}

#[tokio::test]
async fn a_channel_on_stream_2_hears_stream_2_and_not_stream_0() {
    let engine = engine();
    let ds = engine.create_device_set(ARRAY).unwrap();
    let offset = stream_marker_offset_hz(2);

    let on_stream_2 = engine.add_channel(ds, 2, nfm(offset, None)).unwrap();
    let mut rx = engine.subscribe_audio(ds, on_stream_2).unwrap();
    assert_tone_dominates(&settle_then_collect_second(&mut rx).await);

    let on_stream_0 = engine.add_channel(ds, 0, nfm(offset, Some(-20.0))).unwrap();
    let mut rx = engine.subscribe_audio(ds, on_stream_0).unwrap();
    let level = rms(&settle_then_collect_second(&mut rx).await[0]);
    assert!(
        level < 0.01,
        "stream 0 carries no signal at stream 2's offset, yet audio rms is {level}"
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn per_stream_spectrum_differs_and_a_retune_moves_every_lane() {
    let engine = engine();
    let ds = engine.create_device_set(ARRAY).unwrap();
    let mut rx0 = engine.subscribe_spectrum(ds, 0).unwrap();
    let mut rx3 = engine.subscribe_spectrum(ds, 3).unwrap();

    let peak0 = peak_offset_hz(&next_snapshot(&mut rx0).await);
    let peak3 = peak_offset_hz(&next_snapshot(&mut rx3).await);
    assert!(
        (peak0 - stream_marker_offset_hz(0)).abs() < 5_000.0,
        "stream 0 peaks at {peak0} Hz"
    );
    assert!(
        (peak3 - stream_marker_offset_hz(3)).abs() < 5_000.0,
        "stream 3 peaks at {peak3} Hz"
    );

    engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(101_000_000.0),
                ..DeviceSettings::default()
            },
        )
        .unwrap();
    for (stream, rx) in [(0u32, &mut rx0), (3, &mut rx3)] {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if next_snapshot(rx).await.center_hz == 101_000_000.0 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "stream {stream} never saw the retune"
            );
        }
    }
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn removing_a_channel_on_a_non_zero_stream_frees_it() {
    let engine = engine();
    let ds = engine.create_device_set(ARRAY).unwrap();
    let ch = engine
        .add_channel(ds, 3, nfm(stream_marker_offset_hz(3), None))
        .unwrap();
    let mut rx = engine.subscribe_audio(ds, ch).unwrap();
    collect_packets(&mut rx, 2).await;

    let removal = {
        let engine = engine.clone();
        tokio::task::spawn_blocking(move || engine.remove_channel(ds, ch))
    };
    tokio::time::timeout(Duration::from_secs(5), removal)
        .await
        .expect("remove_channel must not hang on a non-zero stream")
        .expect("join")
        .expect("remove ok");
    assert!(engine.snapshot().device_sets[0].channels.is_empty());

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if matches!(rx.recv().await, Err(broadcast::error::RecvError::Closed)) {
                break;
            }
        }
    })
    .await
    .expect("audio stream must close once the channel is gone");
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn an_out_of_range_stream_is_a_clean_bad_request_naming_the_count() {
    let engine = engine();
    let ds = engine.create_device_set(ARRAY).unwrap();

    // Four antennas and the beam the array sums them into, which is a lane like any other.
    let err = engine.add_channel(ds, 5, nfm(0.0, None)).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(err.to_string().contains("5 rx streams"), "unhelpful: {err}");
    assert!(
        engine.snapshot().device_sets[0].channels.is_empty(),
        "a refused add must not leave a channel behind"
    );

    let err = engine.subscribe_spectrum(ds, 99).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");

    let err = engine.start_recording(ds, 5).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(err.to_string().contains("5 rx streams"), "unhelpful: {err}");

    let siggen = engine.create_device_set("virtual:siggen").unwrap();
    let err = engine.add_channel(siggen, 1, nfm(0.0, None)).unwrap_err();
    assert!(err.to_string().contains("1 rx streams"), "unhelpful: {err}");

    engine.remove_device_set(siggen).unwrap();
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_recording_on_stream_2_captures_stream_2_and_says_so() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut registry = DeviceRegistry::new();
    registry.register(10, Box::new(VirtualDriver::new()));
    registry.register(
        10,
        Box::new(RecordingDriver::new(Some(dir.path().to_path_buf()))),
    );
    let engine = Engine::with_registry(registry, Some(dir.path().to_path_buf()));
    let ds = engine.create_device_set(ARRAY).unwrap();

    engine.start_recording(ds, 2).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = engine.snapshot();
        if let Some(recording) = &snapshot.device_sets[0].recording {
            assert_eq!(recording.stream, 2, "live status must name the stream");
            if recording.samples >= 65_536 {
                break;
            }
        }
        assert!(Instant::now() < deadline, "recording never grew");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let finalized = engine.stop_recording(ds).unwrap();
    assert_eq!(finalized.stream, 2);
    assert_eq!(finalized.error, None);
    engine.remove_device_set(ds).unwrap();

    let mut reader = sdrmm_recorder::SigmfReader::open(&finalized.stem).unwrap();
    assert_eq!(reader.meta().global.rx_stream, Some(2));

    let mut samples = vec![Complex::new(0.0f32, 0.0); 65_536];
    let mut filled = 0;
    loop {
        let n = reader.read_block(&mut samples[filled..]).unwrap();
        if n == 0 || filled + n == samples.len() {
            filled += n;
            break;
        }
        filled += n;
    }
    assert!(filled >= 32_768, "recording too short to judge: {filled}");
    let at_stream_2 = band_power(&samples[..filled], RATE, stream_marker_offset_hz(2));
    let at_stream_0 = band_power(&samples[..filled], RATE, stream_marker_offset_hz(0));
    assert!(
        at_stream_2 > 10.0 * at_stream_0,
        "recorded IQ is not stream 2's: {at_stream_2:.3e} at its marker vs {at_stream_0:.3e} at stream 0's"
    );
}

async fn settle(
    rx: &mut broadcast::Receiver<SpectrumSnapshot>,
    center_hz: f64,
    peak_offset_hz_want: f64,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = next_snapshot(rx).await;
        let peak = peak_offset_hz(&snapshot);
        if snapshot.center_hz == center_hz && (peak - peak_offset_hz_want).abs() < 5_000.0 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "lane never settled at centre {center_hz} Hz / peak {peak_offset_hz_want} Hz: \
             last saw centre {} Hz / peak {peak} Hz",
            snapshot.center_hz
        );
    }
}

#[tokio::test]
async fn a_per_stream_retune_moves_only_that_lanes_centre() {
    const RETUNE_HZ: f64 = 50_000.0;
    let engine = engine();
    let ds = engine.create_device_set(TRANSCEIVER).unwrap();

    let lane1 = DEFAULT_CENTER_HZ - RETUNE_HZ;
    engine
        .patch_device(
            ds,
            DeviceSettings {
                streams: vec![StreamSettings {
                    stream: 1,
                    center_hz: Some(lane1),
                    ..StreamSettings::default()
                }],
                ..DeviceSettings::default()
            },
        )
        .unwrap();

    let mut rx1 = engine.subscribe_spectrum(ds, 1).unwrap();
    settle(&mut rx1, lane1, stream_marker_offset_hz(1) + RETUNE_HZ).await;
    let mut rx0 = engine.subscribe_spectrum(ds, 0).unwrap();
    settle(&mut rx0, DEFAULT_CENTER_HZ, stream_marker_offset_hz(0)).await;

    let radio = DEFAULT_CENTER_HZ + 25_000.0;
    engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(radio),
                ..DeviceSettings::default()
            },
        )
        .unwrap();
    let mut rx0 = engine.subscribe_spectrum(ds, 0).unwrap();
    settle(&mut rx0, radio, stream_marker_offset_hz(0)).await;
    let mut rx1 = engine.subscribe_spectrum(ds, 1).unwrap();
    settle(
        &mut rx1,
        lane1,
        stream_marker_offset_hz(1) + RETUNE_HZ + 25_000.0,
    )
    .await;

    let set = &engine.snapshot().device_sets[0];
    assert_eq!(set.settings.center_hz, Some(radio));
    assert_eq!(
        set.settings.streams,
        vec![StreamSettings {
            stream: 1,
            center_hz: Some(lane1),
            tuning: Some(Tuning::Manual),
            ..StreamSettings::default()
        }],
        "the radio-wide retune must not wipe the per-stream override"
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_bad_streams_entry_is_a_clean_bad_request_naming_the_problem() {
    let engine = engine();
    let entry = |stream: u32, center_hz: Option<f64>| DeviceSettings {
        streams: vec![StreamSettings {
            stream,
            center_hz,
            ..StreamSettings::default()
        }],
        ..DeviceSettings::default()
    };

    let ds = engine.create_device_set(TRANSCEIVER).unwrap();
    let err = engine
        .patch_device(ds, entry(2, Some(101_000_000.0)))
        .unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(
        err.to_string().contains("streams[2]") && err.to_string().contains("2 rx streams"),
        "unhelpful: {err}"
    );

    let err = engine
        .patch_device(ds, entry(1, Some(7_000_000_000.0)))
        .unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(err.to_string().contains("tuning range"), "unhelpful: {err}");
    assert!(
        engine.snapshot().device_sets[0].settings.streams.is_empty(),
        "a refused entry must not reach state"
    );
    engine.remove_device_set(ds).unwrap();

    let ds = engine.create_device_set(ARRAY).unwrap();
    let err = engine
        .patch_device(ds, entry(1, Some(101_000_000.0)))
        .unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(err.to_string().contains("center_hz"), "unhelpful: {err}");
    engine
        .patch_device(
            ds,
            DeviceSettings {
                streams: vec![StreamSettings {
                    stream: 1,
                    gains: vec![GainValue {
                        stage: "GAIN".to_string(),
                        value_db: 12.0,
                    }],
                    ..StreamSettings::default()
                }],
                ..DeviceSettings::default()
            },
        )
        .unwrap();
    let set = &engine.snapshot().device_sets[0];
    assert_eq!(set.settings.streams.len(), 1);
    assert_eq!(set.settings.streams[0].gains[0].value_db, 12.0);
    engine.remove_device_set(ds).unwrap();

    let ds = engine.create_device_set("virtual:halfduplex").unwrap();
    let err = engine.patch_device(ds, entry(0, None)).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(err.to_string().contains("streams[0]"), "unhelpful: {err}");
    assert!(engine.snapshot().device_sets[0].settings.streams.is_empty());
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_scan_moves_its_decoder_and_only_that_lane_follows() {
    let engine = engine();
    let ds = engine.create_device_set(TRANSCEIVER).unwrap();
    let scanned = engine.add_channel(ds, 1, nfm(0.0, None)).unwrap();
    let target = DEFAULT_CENTER_HZ + 3_000_000.0;
    engine
        .start_scan(
            ds,
            ScanSettings {
                frequencies: vec![target],
                ..ScanSettings::for_channel(scanned)
            },
        )
        .unwrap();

    let mut rx1 = engine.subscribe_spectrum(ds, 1).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = next_snapshot(&mut rx1).await;
        let covers = (snapshot.center_hz - target).abs() < f64::from(snapshot.span_hz) / 2.0;
        if covers && snapshot.center_hz != DEFAULT_CENTER_HZ {
            break;
        }
        assert!(Instant::now() < deadline, "lane 2 never followed the scan");
    }
    let scope = engine.snapshot().device_sets[0].capabilities.per_stream;
    let set = &engine.snapshot().device_sets[0];
    assert_eq!(
        set.settings.for_stream(0, &scope).center_hz,
        Some(DEFAULT_CENTER_HZ),
        "a scan on lane 2 moved lane 1"
    );

    let other = DEFAULT_CENTER_HZ + 50_000.0;
    engine
        .patch_device(
            ds,
            DeviceSettings {
                streams: vec![StreamSettings {
                    stream: 0,
                    center_hz: Some(other),
                    ..StreamSettings::default()
                }],
                ..DeviceSettings::default()
            },
        )
        .expect("a scan never owns a dial");

    engine.stop_scan(ds, scanned).unwrap();
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn the_lane_a_scan_is_not_using_keeps_following_its_decoders() {
    let engine = engine();
    let ds = engine.create_device_set(TRANSCEIVER).unwrap();
    let scanned = engine.add_channel(ds, 1, nfm(0.0, None)).unwrap();
    engine
        .start_scan(
            ds,
            ScanSettings {
                frequencies: vec![DEFAULT_CENTER_HZ],
                ..ScanSettings::for_channel(scanned)
            },
        )
        .unwrap();
    engine.add_channel(ds, 0, nfm(1_100_000.0, None)).unwrap();
    let set = &engine.snapshot().device_sets[0];
    let lane1 = set
        .channels
        .iter()
        .find(|c| c.stream == 0)
        .expect("lane 1 decoder");
    assert!(
        !lane1.out_of_band,
        "lane 1 stopped following its decoder during a scan on lane 2"
    );
    engine.stop_scan(ds, scanned).unwrap();
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_recording_on_a_retuned_lane_stamps_that_lanes_centre() {
    const LANE1_HZ: f64 = 99_950_000.0;
    let dir = tempfile::TempDir::new().unwrap();
    let mut registry = DeviceRegistry::new();
    registry.register(10, Box::new(VirtualDriver::new()));
    registry.register(
        10,
        Box::new(RecordingDriver::new(Some(dir.path().to_path_buf()))),
    );
    let engine = Engine::with_registry(registry, Some(dir.path().to_path_buf()));
    let ds = engine.create_device_set(TRANSCEIVER).unwrap();
    engine
        .patch_device(
            ds,
            DeviceSettings {
                streams: vec![StreamSettings {
                    stream: 1,
                    center_hz: Some(LANE1_HZ),
                    ..StreamSettings::default()
                }],
                ..DeviceSettings::default()
            },
        )
        .unwrap();

    engine.start_recording(ds, 1).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !engine.snapshot().device_sets[0]
        .recording
        .as_ref()
        .is_some_and(|recording| recording.samples > 0)
    {
        assert!(Instant::now() < deadline, "recording never grew");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let finalized = engine.stop_recording(ds).unwrap();
    assert_eq!(finalized.error, None);
    engine.remove_device_set(ds).unwrap();

    let reader = sdrmm_recorder::SigmfReader::open(&finalized.stem).unwrap();
    assert_eq!(reader.meta().global.rx_stream, Some(1));
    let captures = &reader.meta().captures;
    assert_eq!(
        captures.len(),
        1,
        "meta and tap disagreeing on the lane's centre would open a second segment"
    );
    assert_eq!(captures[0].frequency, Some(LANE1_HZ));
}

const PAGING_RATE: f64 = 240_000.0;
const PAGING_OFFSET_HZ: f64 = 25_000.0;

struct PagingDriver {
    iq: Arc<Vec<Complex<f32>>>,
}

impl DeviceDriver for PagingDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        vec![DeviceInfo {
            driver: "mock".to_string(),
            key: "paging".to_string(),
            label: "Paging mock".to_string(),
            serial: None,
            profile: None,
        }]
    }

    fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Ok(Box::new(PagingDevice {
            capabilities: Capabilities {
                freq_ranges: Vec::new(),
                sample_rates: Vec::new(),
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
                rx_streams: 2,
                tx_streams: 0,
                per_stream: StreamScope {
                    tuning: true,
                    gain: true,
                    antenna: false,
                },
                directional: None,
                dc_artifact: DcArtifact::Operator,
                hardware_sweep: false,
                coherence: sdrmm_wire::Coherence::None,
                noise_source: false,
            },
            settings: DeviceSettings {
                center_hz: Some(DEFAULT_CENTER_HZ),
                sample_rate: Some(PAGING_RATE),
                ..DeviceSettings::default()
            },
            iq: self.iq.clone(),
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
        }))
    }
}

struct PagingDevice {
    capabilities: Capabilities,
    settings: DeviceSettings,
    iq: Arc<Vec<Complex<f32>>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl SdrDevice for PagingDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        check_stream_settings(settings, &self.capabilities)?;
        self.settings.merge_from(settings);
        Ok(())
    }

    fn rx_start(&mut self, mut sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        if sinks.len() != 2 {
            return Err(DeviceError::Unsupported(format!(
                "this device has 2 rx streams, got {} sinks",
                sinks.len()
            )));
        }
        let iq = self.iq.clone();
        let stop = self.stop.clone();
        self.worker = Some(std::thread::spawn(move || {
            const BLOCK: usize = 4_800;
            let mut pos = 0usize;
            let mut next = Instant::now();
            while !stop.load(Ordering::Acquire) {
                let end = (pos + BLOCK).min(iq.len());
                for sink in &mut sinks {
                    sink.push(&iq[pos..end]);
                }
                let pushed = end - pos;
                pos = if end == iq.len() { 0 } else { end };
                next += Duration::from_secs_f64(pushed as f64 / PAGING_RATE);
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else {
                    next = now;
                }
            }
        }));
        Ok(())
    }

    fn rx_stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[tokio::test]
async fn a_decoded_frame_reports_its_lanes_absolute_frequency() {
    const LANE1_HZ: f64 = 433_000_000.0;
    let pages = [testgen::pocsag::Page {
        address: 1_234_567,
        function: 3,
        text: "LANES".to_owned(),
        numeric: false,
    }];
    let mut iq = testgen::pocsag::transmission(&pages, 1_200, 4_500.0, PAGING_RATE);
    testgen::shift(&mut iq, PAGING_OFFSET_HZ, PAGING_RATE);
    iq.extend(testgen::silence(PAGING_RATE as usize));

    let mut registry = DeviceRegistry::new();
    registry.register(10, Box::new(PagingDriver { iq: Arc::new(iq) }));
    let engine = Engine::with_registry(registry, None);
    let mut rx = engine.subscribe_decoded();
    let ds = engine.create_device_set("mock:paging").unwrap();
    engine
        .patch_device(
            ds,
            DeviceSettings {
                streams: vec![StreamSettings {
                    stream: 1,
                    center_hz: Some(LANE1_HZ),
                    ..StreamSettings::default()
                }],
                ..DeviceSettings::default()
            },
        )
        .unwrap();

    let pocsag = |frequency_hz: f64| ChannelSettings {
        frequency_hz,
        squelch: sdrmm_wire::Squelch::Off,
        params: ChannelParams::Pocsag(PocsagParams {
            baud: PocsagBaud::Auto,
            ..PocsagParams::default()
        }),
        audio: Default::default(),
    };
    let on_lane_0 = engine
        .add_channel(ds, 0, pocsag(DEFAULT_CENTER_HZ + PAGING_OFFSET_HZ))
        .unwrap();
    let on_lane_1 = engine
        .add_channel(ds, 1, pocsag(LANE1_HZ + PAGING_OFFSET_HZ))
        .unwrap();

    let mut freqs: HashMap<u32, f64> = HashMap::new();
    tokio::time::timeout(Duration::from_secs(30), async {
        while freqs.len() < 2 {
            match rx.recv().await {
                Ok(record) if matches!(record.event, DecoderEvent::Pocsag(_)) => {
                    freqs.entry(record.channel).or_insert(record.freq_hz);
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => panic!("decoded stream closed"),
            }
        }
    })
    .await
    .expect("a page decoded on both lanes within the timeout");
    engine.remove_device_set(ds).unwrap();

    assert_eq!(
        freqs[&on_lane_0],
        DEFAULT_CENTER_HZ + PAGING_OFFSET_HZ,
        "lane 0's frames must carry the frequency it was set to"
    );
    assert_eq!(
        freqs[&on_lane_1],
        LANE1_HZ + PAGING_OFFSET_HZ,
        "lane 1's frames must carry the frequency it was set to"
    );
}
