use std::{
    f64::consts::TAU,
    sync::{Arc, mpsc},
    time::Duration,
};

use num_complex::Complex;
use sdrmm_device::{
    DeviceDriver, GapScope, LaneEvent, LaneMark, RxSink, SdrDevice, SinkItem, Uncertainty,
};
use sdrmm_dsp::manifold::{Direction, Geometry, LIGHT_SPEED_M_S, Winding, steer};
use sdrmm_wire::{
    Coherence, DcArtifact, DeviceSettings, ExtraValue, GainKind, GainValue, NoiseSource,
    StreamScope,
};

use super::{
    render::{LaneRenderer, LaneSetup},
    *,
};
use crate::VirtualDriver;

type C32 = Complex<f32>;
type C64 = Complex<f64>;

const RATE: f64 = 250_000.0;

fn noise_emitter(azimuth_deg: f64, bandwidth_hz: f64) -> Emitter {
    Emitter {
        azimuth_deg,
        elevation_deg: 0.0,
        offset_hz: 0.0,
        power_dbfs: -10.0,
        waveform: Waveform::Noise { bandwidth_hz },
        paths: Vec::new(),
    }
}

fn tone_emitter(azimuth_deg: f64, offset_hz: f64) -> Emitter {
    Emitter {
        waveform: Waveform::Tone,
        offset_hz,
        power_dbfs: 0.0,
        ..noise_emitter(azimuth_deg, 1.0)
    }
}

fn scene(positions: Vec<[f64; 3]>, emitters: Vec<Emitter>) -> Scene {
    Scene {
        positions,
        emitters,
        echoes: Vec::new(),
        clutter: Clutter::default(),
        thermal_dbfs: -300.0,
        noise_source_dbfs: -20.0,
        noise_bandwidth: 100_000.0,
        seed: 11,
    }
}

fn bench_spec(key: &str, first_element: usize, lanes: Vec<LaneImpairments>) -> BenchDeviceSpec {
    BenchDeviceSpec {
        key: key.to_owned(),
        label: key.to_owned(),
        first_element,
        lanes,
        coherence: Coherence::TimeSync,
        noise_source: NoiseSource::Isolated,
        noise_from: None,
        pilot: None,
        dc_artifact: DcArtifact::Operator,
        retune_keeps_phase: false,
        per_stream: StreamScope {
            tuning: true,
            gain: true,
            antenna: false,
            agc: false,
        },
    }
}

fn setup<'a>(
    position: [f64; 3],
    center_hz: f64,
    impairments: &'a LaneImpairments,
) -> LaneSetup<'a> {
    LaneSetup {
        position,
        center_hz,
        radio_center_hz: center_hz,
        sample_rate: RATE,
        impairments,
        gain_setting_db: 0.0,
        scramble_rad: 0.0,
        pilot: None,
        noise_seed: None,
        isolated: false,
    }
}

fn render(scene: &Scene, setup: &LaneSetup<'_>, n: usize) -> Vec<C32> {
    let mut renderer = LaneRenderer::new(1);
    renderer.configure(scene, setup);
    let mut out = vec![C32::new(0.0, 0.0); n];
    renderer.render(0, 0.0, 1.0 / RATE, false, &mut out);
    out
}

fn power_of(samples: &[C32]) -> f64 {
    samples.iter().map(|s| f64::from(s.norm_sqr())).sum::<f64>() / samples.len().max(1) as f64
}

#[test]
fn an_isolated_noise_source_disconnects_the_antennas() {
    let quiet = LaneImpairments::default();
    let lit = scene(vec![[0.0; 3]], vec![tone_emitter(0.0, 1_000.0)]);
    let noise_power = 10f64.powf(lit.noise_source_dbfs / 10.0);
    for (isolated, expected) in [(true, noise_power), (false, 1.0 + noise_power)] {
        let setup = LaneSetup {
            noise_seed: Some(5),
            isolated,
            ..setup([0.0; 3], 100e6, &quiet)
        };
        let mut renderer = LaneRenderer::new(1);
        renderer.configure(&lit, &setup);
        let mut out = vec![C32::new(0.0, 0.0); 20_000];
        renderer.render(0, 0.0, 1.0 / RATE, true, &mut out);
        let power = power_of(&out);
        assert!(
            (power / expected - 1.0).abs() < 0.2,
            "isolated {isolated}: power {power}, expected {expected}"
        );
        renderer.render(0, 0.0, 1.0 / RATE, false, &mut out);
        assert!((power_of(&out) - 1.0).abs() < 1e-3);
    }
}

#[test]
fn a_wide_noise_source_stays_inside_the_lane_band() {
    let quiet = LaneImpairments::default();
    let mut wide = scene(vec![[0.0; 3]], Vec::new());
    wide.noise_bandwidth = 10.0 * RATE;
    let setup = LaneSetup {
        noise_seed: Some(9),
        ..setup([0.0; 3], 100e6, &quiet)
    };
    let mut renderer = LaneRenderer::new(1);
    renderer.configure(&wide, &setup);
    let len = 1 << 15;
    let mut out = vec![C32::new(0.0, 0.0); len];
    renderer.render(0, 0.0, 1.0 / RATE, true, &mut out);
    rustfft::FftPlanner::new()
        .plan_fft_forward(len)
        .process(&mut out);
    let band = |low: f64, high: f64| {
        let bins: Vec<f64> = out
            .iter()
            .enumerate()
            .filter(|(bin, _)| {
                let f = (*bin as f64 / len as f64 + 0.5).rem_euclid(1.0) - 0.5;
                (low..high).contains(&f.abs())
            })
            .map(|(_, value)| f64::from(value.norm_sqr()))
            .collect();
        bins.iter().sum::<f64>() / bins.len() as f64
    };
    let inside = band(0.0, 0.4);
    let edge = band(0.48, 0.51);
    assert!(
        10.0 * (edge / inside).log10() < -20.0,
        "edge {edge}, inside {inside}"
    );
}

fn noise_spectrum(ripple: Option<Ripple>, len: usize) -> Vec<f64> {
    let rippled = LaneImpairments {
        ripple,
        ..LaneImpairments::default()
    };
    let mut wide = scene(vec![[0.0; 3]], Vec::new());
    wide.noise_bandwidth = RATE;
    let setup = LaneSetup {
        noise_seed: Some(21),
        ..setup([0.0; 3], 100e6, &rippled)
    };
    let mut renderer = LaneRenderer::new(1);
    renderer.configure(&wide, &setup);
    let mut out = vec![C32::new(0.0, 0.0); len];
    renderer.render(0, 0.0, 1.0 / RATE, true, &mut out);
    rustfft::FftPlanner::new()
        .plan_fft_forward(len)
        .process(&mut out);
    out.iter()
        .map(|value| f64::from(value.norm_sqr()))
        .collect()
}

#[test]
fn a_lane_ripple_shapes_what_enters_the_lane() {
    let ripple = Ripple {
        depth_db: 1.0,
        cycles: 3,
    };
    let len = 1 << 14;
    let flat = noise_spectrum(None, len);
    let shaped = noise_spectrum(Some(ripple), len);
    let bands = 64;
    let width = len / bands;
    for band in 0..bands {
        let bins = band * width..(band + 1) * width;
        let centre = (band as f64 + 0.5) / bands as f64;
        let f = (centre + 0.5).rem_euclid(1.0) - 0.5;
        if f.abs() > 0.4 {
            continue;
        }
        let ratio = shaped[bins.clone()].iter().sum::<f64>() / flat[bins].iter().sum::<f64>();
        let expected = ripple.gain_at(f).powi(2);
        let error_db = 10.0 * (ratio / expected).log10();
        assert!(error_db.abs() < 0.2, "at {f}: {error_db} dB");
    }
    assert!((ripple.gain_at(0.0) - 10f64.powf(0.05)).abs() < 1e-12);
}

fn widen(value: C32) -> C64 {
    C64::new(f64::from(value.re), f64::from(value.im))
}

fn relative_phase(lane: &[C32], reference: &[C32]) -> f64 {
    lane.iter()
        .zip(reference)
        .map(|(a, b)| widen(*a) * widen(*b).conj())
        .sum::<C64>()
        .arg()
}

fn wrapped(radians: f64) -> f64 {
    (radians + std::f64::consts::PI).rem_euclid(TAU) - std::f64::consts::PI
}

fn caf(surveillance: &[C32], reference: &[C32], lag: usize, doppler_hz: f64) -> C64 {
    surveillance
        .iter()
        .enumerate()
        .skip(lag)
        .map(|(n, value)| {
            let turn = C64::from_polar(1.0, -TAU * doppler_hz * n as f64 / RATE);
            widen(*value) * widen(reference[n - lag]).conj() * turn
        })
        .sum()
}

fn peak_doppler(surveillance: &[C32], reference: &[C32], lag: usize, low: f64, high: f64) -> f64 {
    let steps = ((high - low) / 0.25).round() as usize;
    (0..=steps)
        .map(|step| low + step as f64 * 0.25)
        .map(|hz| (hz, caf(surveillance, reference, lag, hz).norm()))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(f64::NAN, |(hz, _)| hz)
}

fn peak_lag(
    surveillance: &[C32],
    reference: &[C32],
    lags: std::ops::Range<usize>,
    doppler_hz: f64,
) -> usize {
    lags.map(|lag| (lag, caf(surveillance, reference, lag, doppler_hz).norm()))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(usize::MAX, |(lag, _)| lag)
}

#[derive(Debug)]
enum Item {
    Samples(u64, Vec<C32>, Option<String>),
    Event(LaneEvent),
}

#[derive(Debug, Default)]
struct Lane {
    samples: Vec<(u64, C32)>,
    events: Vec<LaneEvent>,
    threads: Vec<String>,
}

impl Lane {
    fn values(&self) -> Vec<C32> {
        self.samples.iter().map(|(_, value)| *value).collect()
    }

    fn at(&self, index: u64) -> Option<C32> {
        let first = self.samples.first()?.0;
        let slot = self
            .samples
            .binary_search_by_key(&index, |(at, _)| *at)
            .ok()
            .or_else(|| usize::try_from(index.checked_sub(first)?).ok())?;
        self.samples
            .get(slot)
            .filter(|(at, _)| *at == index)
            .map(|(_, v)| *v)
    }
}

fn driver(scene: Scene, devices: Vec<BenchDeviceSpec>) -> VirtualDriver {
    VirtualDriver::with_world(BenchWorld::new(scene, devices))
}

fn open(driver: &VirtualDriver, key: &str) -> Box<dyn SdrDevice> {
    let info = driver
        .probe()
        .into_iter()
        .find(|info| info.key == key)
        .unwrap();
    let mut device = driver.open(&info).unwrap();
    device
        .apply(&DeviceSettings {
            sample_rate: Some(RATE),
            ..DeviceSettings::default()
        })
        .unwrap();
    device
}

fn start(device: &mut dyn SdrDevice) -> Vec<mpsc::Receiver<Item>> {
    let lanes = device.capabilities().rx_streams as usize;
    let mut receivers = Vec::new();
    let sinks = (0..lanes)
        .map(|_| {
            let (tx, rx) = mpsc::channel();
            receivers.push(rx);
            RxSink::with_items(
                move |item| {
                    let item = match item {
                        SinkItem::Samples { samples, index } => Item::Samples(
                            index,
                            samples.to_vec(),
                            std::thread::current().name().map(str::to_owned),
                        ),
                        SinkItem::Event(event) => Item::Event(event),
                    };
                    let _ = tx.send(item);
                },
                |err| panic!("bench lane failed: {err}"),
            )
        })
        .collect();
    device.rx_start(sinks).unwrap();
    receivers
}

fn collect(rx: &mpsc::Receiver<Item>, samples: usize) -> Lane {
    collect_until(rx, |lane| lane.samples.len() >= samples)
}

fn marked(lane: &Lane) -> bool {
    match (lane.events.last(), lane.samples.last()) {
        (Some(LaneEvent::Mark { at, .. }), Some((index, _))) => index >= at,
        _ => false,
    }
}

fn collect_until(rx: &mpsc::Receiver<Item>, done: impl Fn(&Lane) -> bool) -> Lane {
    let mut lane = Lane::default();
    while !done(&lane) {
        match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
            Item::Samples(index, values, thread) => {
                lane.samples.extend(
                    values
                        .into_iter()
                        .enumerate()
                        .map(|(i, value)| (index + i as u64, value)),
                );
                if let Some(thread) = thread.filter(|t| !lane.threads.contains(t)) {
                    lane.threads.push(thread);
                }
            }
            Item::Event(event) => lane.events.push(event),
        }
    }
    lane
}

fn run(device: &mut dyn SdrDevice, samples: usize) -> Vec<Lane> {
    let receivers = start(device);
    let lanes = receivers.iter().map(|rx| collect(rx, samples)).collect();
    device.rx_stop();
    lanes
}

fn assert_close(a: C32, b: C32, what: &str) {
    assert!((a - b).norm() < 1e-4, "{what}: {a} vs {b}");
}

#[test]
fn plane_wave_phase_matches_the_steering_convention() {
    let wavelength_hz = LIGHT_SPEED_M_S;
    let positions = vec![[-0.25, 0.0, 0.0], [0.25, 0.0, 0.0]];
    let quiet = LaneImpairments::default();
    for (azimuth, expected) in [(30.0, TAU / 4.0), (330.0, -TAU / 4.0), (0.0, 0.0)] {
        let scene = scene(positions.clone(), vec![tone_emitter(azimuth, 10_000.0)]);
        let west = render(&scene, &setup(positions[0], wavelength_hz, &quiet), 1_000);
        let east = render(&scene, &setup(positions[1], wavelength_hz, &quiet), 1_000);
        let measured = relative_phase(&east, &west);
        assert!(
            wrapped(measured - expected).abs() < 1e-3,
            "azimuth {azimuth}: east leads west by {measured}, expected {expected}"
        );
    }
}

#[test]
fn bench_element_phases_match_dsp_manifold() {
    let bench = default_scene();
    let geometry = Geometry::uca(0.35, 5, 0.0, Winding::Clockwise).unwrap();
    for (element, position) in geometry.positions().iter().enumerate() {
        let p = bench.positions[element];
        assert!((p[0] - position.x).abs() < 1e-12 && (p[1] - position.y).abs() < 1e-12);
    }
    let center = 433_920_000.0;
    let quiet = LaneImpairments::default();
    for (azimuth, elevation) in [(137.0, 0.0), (211.5, 20.0)] {
        let mut emitter = tone_emitter(azimuth, 5_000.0);
        emitter.elevation_deg = elevation;
        let scene = Scene {
            emitters: vec![emitter],
            ..scene(bench.positions.clone(), Vec::new())
        };
        let mut expected = [C32::new(0.0, 0.0); 5];
        steer(
            geometry.positions(),
            center,
            Direction::new(azimuth, elevation),
            &mut expected,
        );
        let lanes: Vec<Vec<C32>> = (0..5)
            .map(|element| {
                render(
                    &scene,
                    &setup(bench.positions[element], center, &quiet),
                    512,
                )
            })
            .collect();
        for element in 1..5 {
            let measured = relative_phase(&lanes[element], &lanes[0]);
            let manifold = f64::from((expected[element] * expected[0].conj()).arg());
            assert!(
                wrapped(measured - manifold).abs() < 1e-3,
                "element {element} at {azimuth}/{elevation}: {measured} vs {manifold}"
            );
        }
    }
}

#[test]
fn start_offsets_shift_lane_indices() {
    let lanes = vec![
        LaneImpairments::default(),
        LaneImpairments {
            start_offset: 1_234,
            ..LaneImpairments::default()
        },
    ];
    let bench = driver(
        scene(vec![[0.0; 3]; 2], vec![noise_emitter(0.0, 100_000.0)]),
        vec![bench_spec("pair", 0, lanes)],
    );
    let mut device = open(&bench, "pair");
    let lanes = run(device.as_mut(), 20_000);
    for (index, value) in lanes[1].samples.iter().take(15_000) {
        let reference = lanes[0].at(index + 1_234).unwrap();
        assert_close(*value, reference, &format!("lane 1 sample {index}"));
    }
    assert_truth_offset(bench.world(), "pair", 1_234.0);
}

fn assert_truth_offset(world: &BenchWorld, key: &str, expected: f64) {
    let reference = world.lane_truth(key, 0).unwrap();
    let lane = world.lane_truth(key, 1).unwrap();
    let offset = lane.offset_samples(&reference);
    assert!(
        (offset - expected).abs() < 1e-6,
        "truth offset {offset}, expected {expected}"
    );
}

fn slipped_pair(impairments: LaneImpairments, samples: usize) -> (Vec<Lane>, Arc<BenchWorld>) {
    let bench = driver(
        scene(vec![[0.0; 3]; 2], vec![noise_emitter(0.0, 100_000.0)]),
        vec![bench_spec(
            "pair",
            0,
            vec![LaneImpairments::default(), impairments],
        )],
    );
    let mut device = open(&bench, "pair");
    (run(device.as_mut(), samples), bench.world().clone())
}

#[test]
fn a_silent_slip_is_not_reported() {
    let (lanes, world) = slipped_pair(
        LaneImpairments {
            slips: vec![
                Slip {
                    at: 9_000,
                    samples: -2,
                },
                Slip {
                    at: 5_000,
                    samples: 3,
                },
            ],
            ..LaneImpairments::default()
        },
        14_000,
    );
    assert!(lanes[0].events.is_empty() && lanes[1].events.is_empty());
    for (position, (index, value)) in lanes[1].samples.iter().enumerate().take(13_000) {
        assert_eq!(*index, position as u64, "the slipped lane keeps counting");
        let shift = match index {
            0..5_000 => 0,
            5_000..9_000 => 3,
            _ => 1,
        };
        let reference = lanes[0].at(index + shift).unwrap();
        assert_close(*value, reference, &format!("slipped sample {index}"));
    }
    assert_truth_offset(&world, "pair", 1.0);
}

#[test]
fn a_misestimated_gap_reports_the_wrong_count() {
    let (lanes, world) = slipped_pair(
        LaneImpairments {
            gaps: vec![ReportedGap {
                at: 4_000,
                missing: 700,
                reported: 500,
                error: 50,
            }],
            ..LaneImpairments::default()
        },
        12_000,
    );
    assert_eq!(
        lanes[1].events,
        vec![LaneEvent::Uncertain {
            at: 4_500,
            error: 50,
            scope: GapScope::Lane,
            cause: Uncertainty::EstimatedGap,
        }]
    );
    assert!(
        lanes[1].at(4_200).is_none(),
        "the reported gap holds no samples"
    );
    for (index, value) in lanes[1].samples.iter().take(10_000) {
        let shift = if *index < 4_000 { 0 } else { 200 };
        let reference = lanes[0].at(index + shift).unwrap();
        assert_close(*value, reference, &format!("sample {index} after the gap"));
    }
    assert_truth_offset(&world, "pair", 200.0);
}

#[test]
fn lanes_on_separate_threads_drift_by_their_ppm() {
    let ppm = 500.0;
    let bench = driver(
        scene(vec![[0.0; 3]; 2], vec![noise_emitter(0.0, 100_000.0)]),
        vec![bench_spec(
            "drift",
            0,
            vec![
                LaneImpairments::default(),
                LaneImpairments {
                    ppm,
                    ..LaneImpairments::default()
                },
            ],
        )],
    );
    let mut device = open(&bench, "drift");
    let lanes = run(device.as_mut(), 100_000);
    assert_eq!(lanes[0].threads, vec!["sdrmm-bench-drift-0"]);
    assert_eq!(lanes[1].threads, vec!["sdrmm-bench-drift-1"]);
    let envelope = |lane: &Lane| -> Vec<f64> {
        let power: Vec<f64> = lane
            .values()
            .iter()
            .map(|v| f64::from(v.norm_sqr()))
            .collect();
        let mean = power.iter().sum::<f64>() / power.len() as f64;
        power.iter().map(|p| p - mean).collect()
    };
    let (reference, drifting) = (envelope(&lanes[0]), envelope(&lanes[1]));
    for at in [10_000usize, 90_000] {
        let window = &drifting[at..at + 4_096];
        let expected = (at as f64 * ppm * 1e-6).round() as usize;
        let lag = (0..80)
            .map(|lag| {
                let base = &reference[at - lag..at - lag + 4_096];
                let power: f64 = window.iter().zip(base).map(|(a, b)| a * b).sum();
                (lag, power)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map_or(usize::MAX, |(lag, _)| lag);
        assert!(
            lag.abs_diff(expected) <= 1,
            "at {at}: lag {lag}, expected {expected}"
        );
    }
}

fn power_db(values: &[(u64, C32)]) -> f64 {
    let power = values
        .iter()
        .map(|(_, v)| f64::from(v.norm_sqr()))
        .sum::<f64>()
        / values.len() as f64;
    10.0 * power.log10()
}

#[test]
fn a_radio_closed_mid_burst_comes_back_with_its_noise_source_off() {
    let mut quiet = scene(vec![[0.0; 3]; 2], Vec::new());
    quiet.thermal_dbfs = -80.0;
    let bench = driver(
        quiet,
        vec![bench_spec("bank", 0, vec![LaneImpairments::default(); 2])],
    );
    let mut device = open(&bench, "bank");
    let receivers = start(device.as_mut());
    device.set_noise_source(true).unwrap();
    let lit = collect_until(&receivers[0], |lane| {
        lane.samples
            .iter()
            .any(|(_, value)| f64::from(value.norm_sqr()) > 1e-6)
    });
    assert!(!lit.events.is_empty(), "the burst was marked");
    device.rx_stop();
    drop(device);
    let mut device = open(&bench, "bank");
    let lanes = run(device.as_mut(), 4 * BLOCK_LEN);
    for lane in &lanes {
        assert!(lane.events.is_empty(), "{:?}", lane.events);
        assert!(power_of(&lane.values()) < 1e-6);
    }
}

#[test]
fn the_noise_source_reaches_every_lane_through_its_response() {
    let gains = [0.0, -3.0, 2.0];
    let phases = [0.0, 40.0, -70.0];
    let offsets = [0i64, 300, -500];
    let bank = (0..3)
        .map(|lane| LaneImpairments {
            gain_db: gains[lane],
            phase_deg: phases[lane],
            start_offset: offsets[lane],
            ..LaneImpairments::default()
        })
        .collect();
    let fed = BenchDeviceSpec {
        noise_source: NoiseSource::None,
        noise_from: Some("bank".to_owned()),
        ..bench_spec(
            "fed",
            3,
            vec![LaneImpairments {
                gain_db: 1.0,
                ..LaneImpairments::default()
            }],
        )
    };
    let dark = BenchDeviceSpec {
        noise_source: NoiseSource::None,
        ..bench_spec("dark", 4, vec![LaneImpairments::default()])
    };
    let mut quiet = scene(vec![[0.0; 3]; 5], Vec::new());
    quiet.thermal_dbfs = -80.0;
    let bench = driver(quiet, vec![bench_spec("bank", 0, bank), fed, dark]);
    let mut devices: Vec<Box<dyn SdrDevice>> = ["bank", "fed", "dark"]
        .iter()
        .map(|key| open(&bench, key))
        .collect();
    let receivers: Vec<Vec<mpsc::Receiver<Item>>> = devices
        .iter_mut()
        .map(|device| start(device.as_mut()))
        .collect();
    std::thread::sleep(Duration::from_millis(40));
    devices[0].set_noise_source(true).unwrap();
    let needed = ((bench.world().true_time_s() + 0.01) * RATE) as usize + 40_000;
    let lanes: Vec<Vec<Lane>> = receivers
        .iter()
        .map(|lanes| lanes.iter().map(|rx| collect(rx, needed)).collect())
        .collect();
    devices.iter_mut().for_each(|device| device.rx_stop());

    let mut switched = Vec::new();
    for (lane, seen) in lanes[0].iter().enumerate() {
        let [LaneEvent::Mark { at, mark }] = seen.events.as_slice() else {
            panic!("bank lane {lane} needs one mark, got {:?}", seen.events);
        };
        assert_eq!(
            *mark,
            LaneMark::NoiseSource {
                on: true,
                in_flight: BLOCK_LEN as u64
            }
        );
        let onset = seen
            .samples
            .iter()
            .find(|(_, v)| f64::from(v.norm_sqr()) > 1e-6)
            .map_or(u64::MAX, |(i, _)| *i);
        assert!(
            (*at..at + BLOCK_LEN as u64).contains(&onset),
            "lane {lane} lights at {onset}, marked at {at}"
        );
        switched.push(onset as i64 + offsets[lane]);
        let before: Vec<_> = seen
            .samples
            .iter()
            .filter(|(i, _)| *i < onset)
            .copied()
            .collect();
        let after: Vec<_> = seen
            .samples
            .iter()
            .filter(|(i, _)| *i >= onset)
            .copied()
            .collect();
        assert!(
            power_db(&before) < -75.0,
            "lane {lane} is noisy before the switch"
        );
        let lit = power_db(&after);
        assert!(
            (lit - (-20.0 + gains[lane])).abs() < 0.3,
            "lane {lane} at {lit} dB"
        );
    }
    assert!(
        switched.iter().all(|t| t.abs_diff(switched[0]) <= 1),
        "every lane switches at the same true time: {switched:?}"
    );
    let reference = &lanes[0][0];
    let settled = switched[0] + BLOCK_LEN as i64;
    for lane in 1..3 {
        let turn: C64 = lanes[0][lane]
            .samples
            .iter()
            .filter_map(|(i, v)| {
                let shared = *i as i64 + offsets[lane];
                let paired = reference.at(u64::try_from(shared).ok()?)?;
                (shared >= settled).then(|| widen(*v) * widen(paired).conj())
            })
            .sum();
        let measured = turn.arg().to_degrees();
        assert!(
            wrapped((measured - phases[lane]).to_radians()).abs() < 1f64.to_radians(),
            "lane {lane} turns {measured} deg"
        );
    }
    assert!(lanes[1][0].events.is_empty() && lanes[2][0].events.is_empty());
    let tail = |lane: &Lane| lane.samples[lane.samples.len() - 10_000..].to_vec();
    assert!((power_db(&tail(&lanes[1][0])) - (-19.0)).abs() < 0.3);
    assert!(power_db(&tail(&lanes[2][0])) < -75.0);
}

#[test]
fn a_moving_echo_has_the_set_doppler() {
    let center = 10_000_000.0;
    let delay_samples = 30.0;
    let doppler = 125.0;
    let direct = scene(vec![[0.0; 3]], vec![noise_emitter(0.0, 100_000.0)]);
    let echoing = Scene {
        echoes: vec![Echo {
            emitter: 0,
            azimuth_deg: 0.0,
            delay_s: delay_samples / RATE,
            doppler_hz: doppler,
            gain_db: 0.0,
        }],
        ..direct.clone()
    };
    let quiet = LaneImpairments::default();
    let lane = setup([0.0; 3], center, &quiet);
    let n = 500_000;
    let reference = render(&direct, &lane, n);
    let echo: Vec<C32> = render(&echoing, &lane, n)
        .iter()
        .zip(&reference)
        .map(|(both, direct)| both - direct)
        .collect();
    let rate = doppler / center;
    for start_s in [0.1, 1.8] {
        let from = (start_s * RATE) as usize;
        let window = 50_000;
        let expected_lag = delay_samples - rate * (start_s + 0.1) * RATE;
        let lag = peak_lag(
            &echo[from..from + window],
            &reference[from..from + window],
            15..40,
            doppler,
        );
        assert!(
            (lag as f64 - expected_lag).abs() <= 1.0,
            "at {start_s} s the echo sits at {lag}, expected {expected_lag:.2}"
        );
        let measured = peak_doppler(
            &echo[from..from + window],
            &reference[from..from + window],
            lag,
            100.0,
            150.0,
        );
        assert!((measured - doppler).abs() <= 1.0, "Doppler {measured} Hz");
    }
}

#[test]
fn clutter_is_zero_doppler() {
    let clean = scene(vec![[0.0; 3]], vec![noise_emitter(0.0, 100_000.0)]);
    let cluttered = Scene {
        clutter: Clutter {
            echoes: 3,
            max_delay_samples: 40,
            gain_db: -6.0,
        },
        ..clean.clone()
    };
    let quiet = LaneImpairments::default();
    let lane = setup([0.0; 3], 100e6, &quiet);
    let n = 60_000;
    let reference = render(&clean, &lane, n);
    let mut renderer = LaneRenderer::new(1);
    renderer.configure(&cluttered, &lane);
    let taps = renderer.clutter_taps();
    assert_eq!(taps.len(), 3);
    let mut both = vec![C32::new(0.0, 0.0); n];
    for (index, chunk) in both.chunks_mut(5_000).enumerate() {
        let at = index * 5_000;
        renderer.render(at as i64, at as f64 / RATE, 1.0 / RATE, false, chunk);
    }
    let clutter: Vec<C32> = both.iter().zip(&reference).map(|(b, r)| b - r).collect();
    let level = 10f64.powf(-10.0 / 20.0);
    for n in 40..n {
        let expected: C64 = taps
            .iter()
            .map(|(delay, coefficient)| coefficient / level * widen(reference[n - delay]))
            .sum();
        assert!((widen(clutter[n]) - expected).norm() < 1e-4, "sample {n}");
    }
    let (delay, _) = taps
        .iter()
        .max_by(|a, b| a.1.norm().total_cmp(&b.1.norm()))
        .copied()
        .unwrap();
    assert!((1..=40).contains(&delay));
    let measured = peak_doppler(&clutter, &reference, delay, -20.0, 20.0);
    assert!(measured.abs() <= 0.5, "clutter at {measured} Hz");
}

fn measured_hz(values: &[C32]) -> f64 {
    let turn: C64 = values
        .windows(2)
        .map(|pair| widen(pair[1]) * widen(pair[0]).conj())
        .sum();
    turn.arg() * RATE / TAU
}

#[test]
fn a_ppm_lane_shifts_its_carrier() {
    let center = 433_920_000.0;
    let offset = 10_000.0;
    let ppm = 2.0;
    let bench = driver(
        scene(vec![[0.0; 3]; 2], vec![tone_emitter(0.0, offset)]),
        vec![bench_spec(
            "clock",
            0,
            vec![
                LaneImpairments::default(),
                LaneImpairments {
                    ppm,
                    ..LaneImpairments::default()
                },
            ],
        )],
    );
    let mut device = open(&bench, "clock");
    device
        .apply(&DeviceSettings {
            center_hz: Some(center),
            ..DeviceSettings::default()
        })
        .unwrap();
    let lanes = run(device.as_mut(), 50_000);
    let carrier = ppm * 1e-6 * center;
    assert!((carrier - 867.84).abs() < 1e-6);
    let expected = (offset + carrier) / (1.0 + ppm * 1e-6);
    let steady = measured_hz(&lanes[0].values());
    let shifted = measured_hz(&lanes[1].values());
    assert!((steady - offset).abs() < 0.05, "steady lane at {steady} Hz");
    assert!(
        (shifted - expected).abs() < 0.05,
        "ppm lane at {shifted} Hz, expected {expected}"
    );
}

#[test]
fn a_lane_delays_its_signal_by_its_fraction() {
    let offset = 40_000.0;
    let scene = scene(vec![[0.0; 3]], vec![tone_emitter(0.0, offset)]);
    let quiet = LaneImpairments::default();
    let impaired = LaneImpairments {
        frac_delay: 0.37,
        gain_db: -2.0,
        phase_deg: 25.0,
        ..LaneImpairments::default()
    };
    let reference = render(&scene, &setup([0.0; 3], 100e6, &quiet), 4_000);
    let delayed = render(&scene, &setup([0.0; 3], 100e6, &impaired), 4_000);
    let expected = 25f64.to_radians() - TAU * offset * 0.37 / RATE;
    let measured = relative_phase(&delayed, &reference);
    assert!(
        wrapped(measured - expected).abs() < 1e-3,
        "the delayed lane turns {measured} rad, expected {expected}"
    );
    let ratio = power_db(&indexed(&delayed)) - power_db(&indexed(&reference));
    assert!((ratio + 2.0).abs() < 1e-3, "gain {ratio} dB");
}

fn indexed(values: &[C32]) -> Vec<(u64, C32)> {
    values
        .iter()
        .enumerate()
        .map(|(n, value)| (n as u64, *value))
        .collect()
}

#[test]
fn dc_sits_after_the_lane_response() {
    let dark = scene(vec![[0.0; 3]], Vec::new());
    let leaky = LaneImpairments {
        gain_db: 6.0,
        phase_deg: 80.0,
        dc_dbfs: Some(-30.0),
        dc_phase_deg: 60.0,
        ..LaneImpairments::default()
    };
    let expected = C64::from_polar(10f64.powf(-30.0 / 20.0), 60f64.to_radians());
    for value in render(&dark, &setup([0.0; 3], 100e6, &leaky), 2_000) {
        assert!((widen(value) - expected).norm() < 1e-6, "{value}");
    }
}

#[test]
fn the_pilot_is_conducted_to_every_lane_alike() {
    let center = 433_920_000.0;
    let positions = [[0.3, 0.0, 0.0], [-0.3, 0.1, 0.0]];
    let dark = scene(positions.to_vec(), Vec::new());
    let pilot = Some(Pilot {
        offset_hz: 100_000.0,
        power_dbfs: -25.0,
    });
    let quiet = LaneImpairments::default();
    let turned = LaneImpairments {
        gain_db: -3.0,
        phase_deg: 40.0,
        ..LaneImpairments::default()
    };
    let lane = |position: [f64; 3], impairments: &LaneImpairments| {
        let setup = LaneSetup {
            pilot,
            ..setup(position, center, impairments)
        };
        render(&dark, &setup, 4_000)
    };
    let first = lane(positions[0], &quiet);
    let second = lane(positions[1], &turned);
    assert!((measured_hz(&first) - 100_000.0).abs() < 0.01);
    assert!((power_db(&indexed(&first)) + 25.0).abs() < 1e-3);
    assert!((power_db(&indexed(&second)) + 28.0).abs() < 1e-3);
    let turn = relative_phase(&second, &first);
    assert!(
        wrapped(turn - 40f64.to_radians()).abs() < 1e-3,
        "the pilot turns {turn} rad between lanes"
    );
}

#[test]
fn the_lane_truth_places_radios_started_apart() {
    let tone_hz = 20_000.0;
    let late = LaneImpairments {
        start_offset: 5_000,
        frac_delay: 0.4,
        ..LaneImpairments::default()
    };
    let bench = driver(
        scene(vec![[0.0; 3]; 2], vec![tone_emitter(0.0, tone_hz)]),
        vec![
            bench_spec("first", 0, vec![LaneImpairments::default()]),
            bench_spec("second", 1, vec![late]),
        ],
    );
    let mut first = open(&bench, "first");
    let mut second = open(&bench, "second");
    let early = start(first.as_mut());
    std::thread::sleep(Duration::from_millis(25));
    let later = start(second.as_mut());
    let a = collect(&early[0], 20_000);
    let b = collect(&later[0], 20_000);
    first.rx_stop();
    second.rx_stop();

    let world = bench.world();
    let offset = world
        .lane_truth("second", 0)
        .unwrap()
        .offset_samples(&world.lane_truth("first", 0).unwrap());
    assert!(
        offset > 5_000.0 + 0.02 * RATE,
        "the later start must show in the truth: {offset}"
    );
    let span = 20_000;
    let measured = relative_phase(&b.values()[..span], &a.values()[..span]);
    let expected = TAU * (tone_hz * offset / RATE).rem_euclid(1.0);
    assert!(
        wrapped(measured - expected).abs() < 2e-3,
        "second leads first by {measured} rad, truth says {expected}"
    );
}

#[test]
fn the_default_shapes_carry_their_tiers_and_noise_sources() {
    let driver = VirtualDriver::new();
    let profile = |key: &str| {
        driver
            .probe()
            .into_iter()
            .find(|info| info.key == key)
            .and_then(|info| info.profile)
            .unwrap()
    };
    for (key, lanes, coherence, noise) in [
        ("kraken5", 5, Coherence::TimeSync, NoiseSource::Isolated),
        ("array4", 4, Coherence::PhaseCoherent, NoiseSource::None),
        ("dongle1", 1, Coherence::TimeSync, NoiseSource::Isolated),
        ("dongle2", 1, Coherence::TimeSync, NoiseSource::None),
    ] {
        let profile = profile(key);
        assert_eq!(profile.rx_streams, lanes, "{key}");
        assert_eq!(profile.coherence, coherence, "{key}");
        assert_eq!(profile.noise_source, noise, "{key}");
    }
    let kraken = open(&driver, "kraken5");
    assert_eq!(kraken.in_flight_samples(), BLOCK_LEN as u64);
    assert!(!kraken.capabilities().retune_keeps_phase);
    assert!(kraken.capabilities().per_stream.tuning);
    let array = open(&driver, "array4");
    assert!(array.capabilities().retune_keeps_phase);
    assert!(!array.capabilities().per_stream.tuning);
    let names: Vec<&str> = kraken
        .capabilities()
        .extra
        .iter()
        .map(|e| e.name())
        .collect();
    assert_eq!(names, [BEARING_SETTING, RADIUS_SETTING]);
    let dongle = open(&driver, "dongle1");
    let names: Vec<&str> = dongle
        .capabilities()
        .extra
        .iter()
        .map(|e| e.name())
        .collect();
    assert_eq!(names, [BEARING_SETTING]);

    let world = driver.world();
    let scene = world.scene();
    assert_eq!(scene.emitters[0].azimuth_deg, 137.0);
    assert!(matches!(scene.emitters[0].waveform, Waveform::Fm { .. }));
    assert_eq!(scene.echoes.len(), 1);
    for (element, x) in [(9, -0.25), (10, 0.25)] {
        let p = scene.positions[element];
        assert!((p[0] - x).abs() < 1e-12 && p[1].abs() < 1e-12, "{p:?}");
    }
    let devices = default_devices();
    assert_eq!(devices[3].lanes[0].start_offset, 480_000);
    assert_eq!(devices[3].noise_from.as_deref(), Some("dongle1"));
    assert!(
        devices[0]
            .lanes
            .iter()
            .all(|lane| lane.start_offset.abs() <= 20_000
                && lane.scramble_on_retune
                && (lane.phase_per_db - 0.3).abs() < 1e-12)
    );
    assert!(
        devices[1]
            .pilot
            .is_some_and(|pilot| pilot.offset_hz == 100_000.0)
    );
}

#[test]
fn the_bearing_and_radius_settings_move_the_scene() {
    let driver = VirtualDriver::new();
    let mut kraken = open(&driver, "kraken5");
    let setting = |name: &str, value: f64| DeviceSettings {
        extra: vec![ExtraValue {
            name: name.to_owned(),
            value: value.into(),
        }],
        ..DeviceSettings::default()
    };
    kraken.apply(&setting(BEARING_SETTING, 148.3)).unwrap();
    assert_eq!(driver.world().scene().emitters[0].azimuth_deg, 148.3);
    kraken.apply(&setting(RADIUS_SETTING, 0.5)).unwrap();
    let first = driver.world().scene().positions[0];
    assert!((first[1] - 0.5).abs() < 1e-12 && first[0].abs() < 1e-12);
    assert!(kraken.apply(&setting(BEARING_SETTING, 400.0)).is_err());
    assert!(kraken.apply(&setting("lane_slip_samples", 1.0)).is_err());
    let mut dongle = open(&driver, "dongle1");
    assert!(dongle.apply(&setting(RADIUS_SETTING, 0.5)).is_err());
    let mut array = open(&driver, "array4");
    assert!(matches!(
        array.set_noise_source(true),
        Err(sdrmm_device::DeviceError::Unsupported(_))
    ));
    assert!(
        kraken
            .apply(&DeviceSettings {
                gains: vec![GainValue::new(GainKind::Lna, 10.0)],
                ..DeviceSettings::default()
            })
            .is_err()
    );
}

#[test]
fn a_retune_marks_every_lane_and_redraws_the_scramble() {
    let lanes = vec![
        LaneImpairments {
            scramble_on_retune: true,
            phase_per_db: 0.3,
            ..LaneImpairments::default()
        };
        2
    ];
    let bench = driver(
        scene(vec![[0.0; 3]; 2], vec![noise_emitter(0.0, 100_000.0)]),
        vec![bench_spec("tuned", 0, lanes)],
    );
    let world = bench.world().clone();
    let mut device = open(&bench, "tuned");
    let receivers = start(device.as_mut());
    let first: Vec<Lane> = receivers.iter().map(|rx| collect(rx, 10_000)).collect();
    let before = world.lane_truth("tuned", 0).unwrap();
    device
        .apply(&DeviceSettings {
            center_hz: Some(101_000_000.0),
            ..DeviceSettings::default()
        })
        .unwrap();
    let retuned: Vec<Lane> = receivers
        .iter()
        .map(|rx| collect_until(rx, marked))
        .collect();
    let middle = world.lane_truth("tuned", 0).unwrap();
    device
        .apply(&DeviceSettings {
            gains: vec![GainValue::new(GainKind::Tuner, 10.0)],
            ..DeviceSettings::default()
        })
        .unwrap();
    let regained: Vec<Lane> = receivers
        .iter()
        .map(|rx| collect_until(rx, marked))
        .collect();
    device.rx_stop();
    let after = world.lane_truth("tuned", 0).unwrap();
    for lane in 0..2 {
        assert!(first[lane].events.is_empty());
        assert!(matches!(
            retuned[lane].events.as_slice(),
            [LaneEvent::Mark {
                mark: LaneMark::Retuned { in_flight },
                ..
            }] if *in_flight == BLOCK_LEN as u64
        ));
        assert!(matches!(
            regained[lane].events.as_slice(),
            [LaneEvent::Mark {
                mark: LaneMark::GainChanged { .. },
                ..
            }]
        ));
    }
    assert!(wrapped((middle.phase_deg - before.phase_deg).to_radians()).abs() > 1e-6);
    assert!(wrapped((after.phase_deg - middle.phase_deg - 3.0).to_radians()).abs() < 1e-9);
}

#[test]
fn broken_worlds_and_impairments_are_refused() {
    let world = BenchWorld::new(
        scene(vec![[0.0; 3]], vec![noise_emitter(0.0, 1e5)]),
        vec![
            bench_spec("wide", 0, vec![LaneImpairments::default(); 2]),
            BenchDeviceSpec {
                noise_from: Some("nobody".to_owned()),
                ..bench_spec("orphan", 0, vec![LaneImpairments::default()])
            },
        ],
    );
    let driver = VirtualDriver::with_world(world.clone());
    for key in ["wide", "orphan"] {
        let info = driver
            .probe()
            .into_iter()
            .find(|info| info.key == key)
            .unwrap();
        assert!(
            matches!(
                driver.open(&info),
                Err(sdrmm_device::DeviceError::Unsupported(_))
            ),
            "{key}"
        );
    }
    assert!(
        world
            .set_impairments("missing", 0, LaneImpairments::default())
            .is_err()
    );
    assert!(
        world
            .set_impairments("orphan", 3, LaneImpairments::default())
            .is_err()
    );
    let nan = LaneImpairments {
        gain_db: f64::NAN,
        ..LaneImpairments::default()
    };
    assert!(world.set_impairments("orphan", 0, nan).is_err());
    let mut broken = world.scene().as_ref().clone();
    broken.emitters[0].waveform = Waveform::Noise { bandwidth_hz: 0.0 };
    assert!(world.set_scene(broken).is_err());
    assert!(world.set_scene(scene(Vec::new(), Vec::new())).is_err());
    let mut grown = world.scene().as_ref().clone();
    grown.positions.push([1.0, 0.0, 0.0]);
    world.set_scene(grown).unwrap();
    let info = driver
        .probe()
        .into_iter()
        .find(|info| info.key == "wide")
        .unwrap();
    assert!(driver.open(&info).is_ok());
    let earlier = world.true_time_s();
    std::thread::sleep(Duration::from_millis(2));
    assert!(world.true_time_s() >= earlier + 0.002);
}
