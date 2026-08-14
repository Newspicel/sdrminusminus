//! Engine e2e over the multi-stream virtual radios (, §9, and the per-stream
//! settings ): a channel, a spectrum subscription, and a recording each address one
//! lane, and lane k must carry stream k's signal — the per-stream markers make a wrong lane
//! observable, not just a wrong count. The 2×2 transceiver (per-stream tuning) proves a
//! per-stream retune reaches exactly one lane's DSP meta; the coherent array proves shared
//! tuning is honoured.

// Tests may unwrap/expect (CLAUDE.md); clippy's `allow-unwrap-in-tests` only covers
// `#[cfg(test)]` items, which an integration-test crate's helpers are not.
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
use sdrmm_device_virtual::{VirtualDriver, stream_marker_offset_hz};
use sdrmm_engine::{Engine, SpectrumSnapshot};
use sdrmm_wire::{
    Capabilities, ChannelParams, ChannelSettings, DecoderEvent, DeviceInfo, DeviceSettings, Duplex,
    GainValue, NfmParams, PocsagBaud, PocsagParams, ScanSettings, StreamScope, StreamSettings,
};
use tokio::sync::broadcast;

const ARRAY: &str = "virtual:array4";
const TRANSCEIVER: &str = "virtual:transceiver";
/// The array's default rate; every stream's marker sits under Nyquist here (at low rates
/// the outer markers are muted, not aliased — see `device-virtual`).
const RATE: f64 = 2_048_000.0;
/// What the virtual radios power up tuned to.
const DEFAULT_CENTER_HZ: f64 = 100_000_000.0;

fn engine() -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(10, Box::new(VirtualDriver::new()));
    Engine::with_registry(registry, None)
}

fn nfm(offset_hz: f64, squelch_db: Option<f32>) -> ChannelSettings {
    ChannelSettings {
        offset_hz,
        squelch_db,
        params: ChannelParams::Nfm(NfmParams::default()),
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

/// Offset of the strongest FFT bin from the snapshot's center (the `db` array is
/// DC-centered).
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

/// Power near `offset_hz` in complex baseband: correlation with e^(−j2πft) summed over
/// short windows, so the marker's ±2.5 kHz FM wobble stays inside the window's main lobe.
/// Complex, not the audio helpers' real Goertzel — that cannot tell +150 kHz from −150 kHz.
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

/// The core lane-identity claim: a channel on stream 2 demodulates stream 2's marker, and
/// the same offset on stream 0 — where that frequency holds only noise — never opens the
/// squelch.
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

    // Each lane's spectrum must peak at its own stream's marker — bin resolution is
    // RATE / FFT size = 500 Hz, so a 5 kHz tolerance is generous and unambiguous
    // (markers sit 50 kHz apart).
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

    // One radio, one tuner: a retune must reach every lane, not just stream 0.
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

/// A removal routed to the wrong lane would leave the DSP-side host (and its PCM sender)
/// alive: the encoder join inside `remove_channel` would hang and the audio stream would
/// never close. Both are asserted under a timeout.
#[tokio::test]
async fn removing_a_channel_on_a_non_zero_stream_frees_it() {
    let engine = engine();
    let ds = engine.create_device_set(ARRAY).unwrap();
    let ch = engine
        .add_channel(ds, 3, nfm(stream_marker_offset_hz(3), None))
        .unwrap();
    let mut rx = engine.subscribe_audio(ds, ch).unwrap();
    // The host is demonstrably live on its lane before the removal.
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

    let err = engine.add_channel(ds, 4, nfm(0.0, None)).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(err.to_string().contains("4 rx streams"), "unhelpful: {err}");
    assert!(
        engine.snapshot().device_sets[0].channels.is_empty(),
        "a refused add must not leave a channel behind"
    );

    let err = engine.subscribe_spectrum(ds, 99).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");

    // Checked before the recordings-dir requirement: the stream refusal must name the
    // count even on an engine with recording disabled.
    let err = engine.start_recording(ds, 4).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(err.to_string().contains("4 rx streams"), "unhelpful: {err}");

    // A single-stream radio has exactly stream 0, and its refusal says so.
    let siggen = engine.create_device_set("virtual:siggen").unwrap();
    let err = engine.add_channel(siggen, 1, nfm(0.0, None)).unwrap_err();
    assert!(err.to_string().contains("1 rx streams"), "unhelpful: {err}");

    engine.remove_device_set(siggen).unwrap();
    engine.remove_device_set(ds).unwrap();
}

/// One recording per set, on the named stream (b): the live status, the finalized
/// hand-off, and the SigMF meta all say stream 2 — and the data really is lane 2's, which
/// only the recorded IQ itself can prove.
#[tokio::test]
async fn a_recording_on_stream_2_captures_stream_2_and_says_so() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut registry = DeviceRegistry::new();
    registry.register(
        10,
        Box::new(VirtualDriver::with_recordings(dir.path().to_path_buf())),
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

/// Wait until `rx`'s lane reports `center_hz` with its strongest bin near `peak_offset_hz`
/// (bins are 500 Hz at the default rate; markers sit 50 kHz apart, so ±5 kHz is unambiguous).
/// A lane that never gets there fails at the deadline naming what it last showed.
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

///  + §6.3 on the 2×2 transceiver (per-stream tuning): a per-stream retune must
/// reach exactly its lane's DSP meta — visible as the lane spectrum's `center_hz`, with the
/// lane's marker displaced by the difference — and a later radio-wide retune moves only the
/// lanes without an override, never wiping the override that exists.
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

    // Lane 1 sits on its own centre with its marker displaced by the difference; lane 0
    // (a fresh subscription, so post-retune frames only) stays on the radio-wide dial.
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
    // Lane 1 keeps its centre; its marker (radiating on the radio dial) drifts 25 kHz further.
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
            ..StreamSettings::default()
        }],
        "the radio-wide retune must not wipe the per-stream override"
    );
    engine.remove_device_set(ds).unwrap();
}

/// /§6: a `streams` entry the capability cannot honour is a clean bad request from
/// the engine, refused before the device sees any of the delta — and the refusal is per
/// capability, not blanket: what one radio refuses another accepts.
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

    // A stream the radio lacks is refused naming the count.
    let ds = engine.create_device_set(TRANSCEIVER).unwrap();
    let err = engine
        .patch_device(ds, entry(2, Some(101_000_000.0)))
        .unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(
        err.to_string().contains("streams[2]") && err.to_string().contains("2 rx streams"),
        "unhelpful: {err}"
    );

    // A per-stream centre is range-checked like the radio-wide dial: same tuner.
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

    // Shared tuning (the coherent array): a per-stream centre would desynchronise the array
    // and is refused naming the setting, while per-stream gain is exactly what it scopes.
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

    // A radio that declares nothing per stream refuses any entry at all.
    let ds = engine.create_device_set("virtual:halfduplex").unwrap();
    let err = engine.patch_device(ds, entry(0, None)).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(err.to_string().contains("streams[0]"), "unhelpful: {err}");
    assert!(engine.snapshot().device_sets[0].settings.streams.is_empty());
    engine.remove_device_set(ds).unwrap();
}

/// : a scan owns the radio-wide dial, and a per-stream-tuning radio has none — a
/// sweep would silently drag every unpinned lane. Shared-tuning radios stay sweepable.
#[tokio::test]
async fn a_scan_is_refused_where_tuning_is_per_stream() {
    let engine = engine();
    let scan = ScanSettings {
        frequencies: vec![100_000_000.0],
        ..ScanSettings::default()
    };

    let ds = engine.create_device_set(TRANSCEIVER).unwrap();
    let err = engine.start_scan(ds, scan.clone()).unwrap_err();
    assert!(err.is_bad_request(), "expected bad request, got {err}");
    assert!(err.to_string().contains("stream"), "unhelpful: {err}");
    assert!(engine.snapshot().device_sets[0].scanner.is_none());
    engine.remove_device_set(ds).unwrap();

    let ds = engine.create_device_set(ARRAY).unwrap();
    engine.start_scan(ds, scan).unwrap();
    engine.stop_scan(ds).unwrap();
    engine.remove_device_set(ds).unwrap();
}

/// A recording on a per-stream-retuned lane must file under that lane's centre ():
/// the meta's opening capture and the tap's block stamps agree, so the pair holds exactly one
/// segment at the lane's frequency — a radio-wide value in either place would split or
/// mislabel it.
#[tokio::test]
async fn a_recording_on_a_retuned_lane_stamps_that_lanes_centre() {
    const LANE1_HZ: f64 = 99_950_000.0;
    let dir = tempfile::TempDir::new().unwrap();
    let mut registry = DeviceRegistry::new();
    registry.register(
        10,
        Box::new(VirtualDriver::with_recordings(dir.path().to_path_buf())),
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

/// Fixed rate of [`PagingDriver`]'s lanes: 5× POCSAG's 48 kHz channel rate, so the DDC
/// really mixes and decimates (the decode e2e convention).
const PAGING_RATE: f64 = 240_000.0;
/// Where the POCSAG burst sits in each lane's baseband — non-zero so the decoded stamp is
/// centre *plus offset*, not just the centre echoed back.
const PAGING_OFFSET_HZ: f64 = 25_000.0;

/// Two-lane radio with per-stream tuning whose every lane replays the same POCSAG burst, so
/// a decoder on either lane decodes the same page. The *stamp* is then the only difference
/// between the lanes' records — exactly the field a shared-centre bug corrupts silently.
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
                sample_rate_range: None,
                gains: Vec::new(),
                antennas: Vec::new(),
                bandwidths: Vec::new(),
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
        // The backend contract: a delta the capability cannot honour is refused, never
        // silently dropped — the engine refuses first, but the double must stay honest.
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
            // ~20 ms blocks, paced to real time, both lanes fed from one clock.
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

/// : a decoded frame's `freq_hz` is *its lane's* centre plus the channel offset.
/// After a per-stream retune, lane 1's records must carry lane 1's absolute frequency while
/// lane 0's still carry the radio-wide one — a frame filed under the wrong frequency is
/// silent and wrong, the failure this asserts against end to end.
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
    // A silence tail so the loop never re-enters mid-frame without a clean lead-in.
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

    let pocsag = ChannelSettings {
        offset_hz: PAGING_OFFSET_HZ,
        squelch_db: None,
        params: ChannelParams::Pocsag(PocsagParams {
            baud: PocsagBaud::Auto,
            ..PocsagParams::default()
        }),
    };
    let on_lane_0 = engine.add_channel(ds, 0, pocsag.clone()).unwrap();
    let on_lane_1 = engine.add_channel(ds, 1, pocsag).unwrap();

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
        "lane 0 rides the radio-wide dial"
    );
    assert_eq!(
        freqs[&on_lane_1],
        LANE1_HZ + PAGING_OFFSET_HZ,
        "lane 1's frames must carry lane 1's own centre"
    );
}
