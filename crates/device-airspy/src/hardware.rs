use std::{
    sync::{Arc, Mutex, mpsc::RecvTimeoutError},
    time::{Duration, Instant},
};

use sdrmm_device::{DeviceDriver, DeviceError, RxSink, Sample, SampleConverter};
use sdrmm_dsp::fft::FftPair;
use sdrmm_wire::{AgcSetting, DeviceInfo, DeviceSettings, GainKind, GainValue};

use super::*;
use crate::driver::{FREQ_MAX_HZ, FREQ_MIN_HZ, MAX_LNA_GAIN, MAX_MIXER_GAIN, MAX_VGA_GAIN};

const TUNE_HZ: u32 = 100_000_000;
const ORIENTATION_TUNE_HZ: u32 = 88_000_000;
const OFF_CLOCK_HARMONICS_HZ: u32 = 70_123_000;
const SETTLE: Duration = Duration::from_millis(500);
const FFT_LEN: usize = 8192;

fn seconds() -> u64 {
    std::env::var("SDRMM_AIRSPY_TEST_SECONDS")
        .map(|value| value.parse().expect("duration"))
        .unwrap_or(5)
}

fn only_airspy() -> DeviceDescriptor {
    let found = Airspy::list().expect("list USB devices");
    assert_eq!(found.len(), 1, "attach exactly one Airspy: {found:?}");
    found.into_iter().next().expect("one Airspy")
}

fn open_raw() -> Airspy {
    let descriptor = only_airspy();
    match descriptor.serial {
        Some(serial) => Airspy::open_serial(serial),
        None => Airspy::open_at(descriptor.bus, descriptor.address),
    }
    .expect("open Airspy")
}

#[derive(Default)]
struct CodeStats {
    codes: u64,
    stray_high_bits: u64,
    sum: f64,
    sum_sq: f64,
    min: u16,
    max: u16,
}

impl CodeStats {
    fn push(&mut self, bytes: &[u8]) {
        if self.codes == 0 {
            self.min = u16::MAX;
        }
        for pair in bytes.as_chunks::<2>().0 {
            let word = u16::from_le_bytes(*pair);
            if word > 0x0fff {
                self.stray_high_bits += 1;
            }
            let code = word & 0x0fff;
            self.min = self.min.min(code);
            self.max = self.max.max(code);
            self.sum += f64::from(code);
            self.sum_sq += f64::from(code) * f64::from(code);
            self.codes += 1;
        }
    }

    fn mean(&self) -> f64 {
        self.sum / self.codes as f64
    }

    fn rms(&self) -> f64 {
        let mean = self.mean();
        (self.sum_sq / self.codes as f64 - mean * mean).sqrt()
    }
}

struct RawRun {
    bytes_per_second: f64,
    stats: CodeStats,
    dropped: u64,
    missing_bytes: u64,
}

fn stream_raw(radio: &mut Airspy, duration: Duration) -> RawRun {
    let mut stream = radio.start_rx().expect("start rx");
    let started = Instant::now();
    let mut stats = CodeStats::default();
    let mut first = None;
    let mut last = started;
    let mut measured = 0usize;
    let mut missing_bytes = 0;
    while started.elapsed() < SETTLE + duration {
        match stream.recv_timeout(Duration::from_millis(100)) {
            Ok(block) => {
                last = Instant::now();
                missing_bytes += block.missing_exact_bytes() + block.missing_estimated_bytes();
                if started.elapsed() < SETTLE {
                    continue;
                }
                if first.is_some() {
                    measured += block.len();
                } else {
                    first = Some(last);
                }
                stats.push(&block);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => panic!("USB ended: {:?}", stream.error()),
        }
    }
    let usb = stream.stats();
    radio.set_mode_off().expect("receiver off");
    stream.stop();
    let elapsed = last
        .duration_since(first.expect("received data"))
        .as_secs_f64();
    RawRun {
        bytes_per_second: measured as f64 / elapsed,
        stats,
        dropped: usb.dropped,
        missing_bytes,
    }
}

fn capture_iq(radio: &mut Airspy, samples: usize) -> Vec<Sample> {
    let mut converter = convert::AirspyConverter::new(RX_TRANSFER_SIZE / 2);
    let mut stream = radio.start_rx().expect("start rx");
    let started = Instant::now();
    let mut out = Vec::with_capacity(samples);
    while out.len() < samples {
        let block = stream
            .recv_timeout(Duration::from_secs(1))
            .expect("a block within a second");
        let iq = converter.convert(&block);
        if started.elapsed() >= SETTLE {
            out.extend_from_slice(iq);
        }
    }
    radio.set_mode_off().expect("receiver off");
    stream.stop();
    out.truncate(samples);
    out
}

fn power_spectrum(iq: &[Sample]) -> Vec<f64> {
    let mut fft = FftPair::new(FFT_LEN);
    let window: Vec<f32> = (0..FFT_LEN)
        .map(|n| {
            let x = std::f32::consts::TAU * n as f32 / FFT_LEN as f32;
            0.5 - 0.5 * x.cos()
        })
        .collect();
    let mut power = vec![0.0f64; FFT_LEN];
    let mut frames = 0;
    for frame in iq.as_chunks::<FFT_LEN>().0 {
        let mut buf: Vec<Sample> = frame.iter().zip(&window).map(|(s, w)| s * w).collect();
        fft.forward(&mut buf);
        for (bin, value) in power.iter_mut().zip(&buf) {
            *bin += f64::from(value.norm_sqr());
        }
        frames += 1;
    }
    let mut shifted = vec![0.0; FFT_LEN];
    for (bin, value) in power.iter().enumerate() {
        shifted[(bin + FFT_LEN / 2) % FFT_LEN] = value / f64::from(frames);
    }
    shifted
}

fn bin_offset_hz(bin: usize, rate: f64) -> f64 {
    (bin as f64 - (FFT_LEN / 2) as f64) * rate / FFT_LEN as f64
}

fn strongest_bin(spectrum: &[f64], rate: f64, within_hz: f64) -> (usize, f64) {
    spectrum
        .iter()
        .enumerate()
        .filter(|(bin, _)| {
            let offset = bin_offset_hz(*bin, rate).abs();
            offset > 20_000.0 && offset < within_hz
        })
        .map(|(bin, power)| (bin, *power))
        .fold(
            (0, 0.0),
            |best, next| if next.1 > best.1 { next } else { best },
        )
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}

#[test]
#[ignore = "requires one idle Airspy R2 or Mini"]
fn connected_airspy_reports_the_serial_its_descriptor_carries() {
    let descriptor = only_airspy();
    let radio = open_raw();
    eprintln!(
        "descriptor={descriptor:?} firmware={:?} serial={:?} rates={:?}",
        radio.version(),
        radio.serial().map(full_serial),
        radio.sample_rates()
    );
    assert!(descriptor.serial.is_some(), "descriptor serial not parsed");
    assert_eq!(radio.serial(), descriptor.serial);
    assert!(radio.version().starts_with("AirSpy"), "{}", radio.version());
    assert!(!radio.sample_rates().is_empty());
}

#[test]
#[ignore = "requires one idle Airspy R2 or Mini"]
fn connected_airspy_streams_every_rate_without_loss() {
    let mut radio = open_raw();
    radio.set_frequency_hz(TUNE_HZ).expect("tune");
    let duration = Duration::from_secs(seconds());
    for rate in radio.sample_rates().to_vec() {
        radio.set_sample_rate_hz(rate).expect("sample rate");
        let run = stream_raw(&mut radio, duration);
        let expected = f64::from(rate) * 4.0;
        eprintln!(
            "rate={rate} bytes/s={:.0} expected={expected:.0} dropped={} missing={} mean={:.1} rms={:.1} min={} max={} stray={}",
            run.bytes_per_second,
            run.dropped,
            run.missing_bytes,
            run.stats.mean(),
            run.stats.rms(),
            run.stats.min,
            run.stats.max,
            run.stats.stray_high_bits
        );
        assert_eq!(run.dropped, 0, "{rate}: USB dropped transfers");
        assert_eq!(run.missing_bytes, 0, "{rate}: bytes went missing");
        assert!(
            (run.bytes_per_second / expected - 1.0).abs() < 0.01,
            "{rate}: sample rate mismatch"
        );
        assert_eq!(
            run.stats.stray_high_bits, 0,
            "{rate}: packed or garbled codes"
        );
        assert!(run.stats.rms() > 1.0, "{rate}: the ADC is stuck");
        assert!(
            (run.stats.mean() - 2048.0).abs() < 200.0,
            "{rate}: off centre"
        );
    }
}

#[test]
#[ignore = "requires one idle Airspy R2 or Mini"]
fn connected_airspy_gain_moves_the_level() {
    let mut radio = open_raw();
    radio.set_frequency_hz(TUNE_HZ).expect("tune");
    let rate = *radio.sample_rates().last().expect("a rate");
    radio.set_sample_rate_hz(rate).expect("sample rate");
    let mut level = |lna: u8, mixer: u8, vga: u8| {
        radio.set_lna_gain(lna).expect("lna");
        radio.set_mixer_gain(mixer).expect("mixer");
        radio.set_vga_gain(vga).expect("vga");
        stream_raw(&mut radio, Duration::from_secs(1)).stats.rms()
    };
    let low = level(0, 0, 0);
    let high = level(MAX_LNA_GAIN, MAX_MIXER_GAIN, MAX_VGA_GAIN);
    eprintln!("rms low={low:.1} high={high:.1}");
    assert!(high > low * 3.0, "gain made no difference");
}

#[test]
#[ignore = "requires one idle Airspy R2 or Mini and a carrier within 3.5 MHz of 88 MHz"]
fn connected_airspy_spectrum_is_upright() {
    let mut radio = open_raw();
    let rate = *radio.sample_rates().first().expect("a rate");
    radio.set_sample_rate_hz(rate).expect("sample rate");
    radio.set_lna_gain(MAX_LNA_GAIN).expect("lna");
    radio.set_mixer_gain(MAX_MIXER_GAIN).expect("mixer");
    radio.set_vga_gain(MAX_VGA_GAIN).expect("vga");
    let rate = f64::from(rate);
    let step_hz = 200_000u32;
    let within = rate * 0.35;

    radio.set_frequency_hz(ORIENTATION_TUNE_HZ).expect("tune");
    let before = power_spectrum(&capture_iq(&mut radio, FFT_LEN * 32));
    let (bin, peak) = strongest_bin(&before, rate, within);
    let carrier_hz = f64::from(ORIENTATION_TUNE_HZ) + bin_offset_hz(bin, rate);
    let floor = median(&before);
    eprintln!(
        "carrier at {carrier_hz:.0} Hz, {:.1} dB above the floor",
        10.0 * (peak / floor).log10()
    );
    assert!(peak > floor * 10.0, "no carrier stands out to follow");

    let retuned = ORIENTATION_TUNE_HZ + step_hz;
    radio.set_frequency_hz(retuned).expect("retune");
    let after = power_spectrum(&capture_iq(&mut radio, FFT_LEN * 32));
    let expected_offset = carrier_hz - f64::from(retuned);
    let mirrored_offset = -expected_offset;
    let bin_hz = rate / FFT_LEN as f64;
    let power_near = |offset: f64| {
        let centre = (offset / bin_hz).round() as isize + (FFT_LEN / 2) as isize;
        (centre - 3..=centre + 3)
            .filter_map(|bin| usize::try_from(bin).ok())
            .filter_map(|bin| after.get(bin).copied())
            .fold(0.0, f64::max)
    };
    let upright = power_near(expected_offset);
    let mirrored = power_near(mirrored_offset);
    eprintln!(
        "after retune: at {expected_offset:.0} Hz {:.1} dB, mirrored {:.1} dB",
        10.0 * (upright / floor).log10(),
        10.0 * (mirrored / floor).log10()
    );
    assert!(upright > mirrored * 4.0, "the spectrum is mirrored");
}

fn db(ratio: f64) -> f64 {
    10.0 * ratio.log10()
}

fn full_gain(radio: &mut Airspy) {
    radio.set_lna_agc(false).expect("lna agc");
    radio.set_mixer_agc(false).expect("mixer agc");
    radio.set_lna_gain(MAX_LNA_GAIN).expect("lna");
    radio.set_mixer_gain(MAX_MIXER_GAIN).expect("mixer");
    radio.set_vga_gain(MAX_VGA_GAIN).expect("vga");
}

fn power_near(spectrum: &[f64], rate: f64, offset_hz: f64) -> f64 {
    let centre = (offset_hz / (rate / FFT_LEN as f64)).round() as isize + (FFT_LEN / 2) as isize;
    (centre - 3..=centre + 3)
        .filter_map(|bin| usize::try_from(bin).ok())
        .filter_map(|bin| spectrum.get(bin).copied())
        .fold(0.0, f64::max)
}

#[derive(Default)]
struct Phase {
    samples: u64,
    after_first: u64,
    first: Option<Instant>,
    last: Option<Instant>,
}

impl Phase {
    fn push(&mut self, samples: &[Sample]) {
        let now = Instant::now();
        if self.first.is_some() {
            self.after_first += samples.len() as u64;
        } else {
            self.first = Some(now);
        }
        self.last = Some(now);
        self.samples += samples.len() as u64;
    }

    fn rate(&self) -> f64 {
        let elapsed = self
            .last
            .zip(self.first)
            .map(|(last, first)| last.duration_since(first).as_secs_f64())
            .expect("samples arrived");
        self.after_first as f64 / elapsed
    }
}

#[derive(Default)]
struct Seen {
    next_index: u64,
    index_gaps: u64,
    non_finite: u64,
    phase: Option<usize>,
    phases: [Phase; 4],
}

type Failure = Arc<Mutex<Option<DeviceError>>>;

fn watching_sink() -> (RxSink, Arc<Mutex<Seen>>, Failure) {
    let seen = Arc::new(Mutex::new(Seen {
        phase: Some(0),
        ..Seen::default()
    }));
    let failure = Arc::new(Mutex::new(None));
    let sink = {
        let seen = seen.clone();
        let failure = failure.clone();
        RxSink::with_fatal_handler(
            move |samples: &[Sample], index: u64| {
                let mut seen = sdrmm_device::lock(&seen);
                if index != seen.next_index {
                    seen.index_gaps += 1;
                }
                seen.next_index = index + samples.len() as u64;
                seen.non_finite += samples
                    .iter()
                    .filter(|s| !s.re.is_finite() || !s.im.is_finite())
                    .count() as u64;
                if let Some(phase) = seen.phase {
                    seen.phases[phase].push(samples);
                }
            },
            move |err| *sdrmm_device::lock(&failure) = Some(err),
        )
    };
    (sink, seen, failure)
}

fn open_through_the_driver() -> (AirspyDriver, DeviceInfo, Box<dyn SdrDevice>) {
    let driver = AirspyDriver::new();
    let found = driver.probe();
    assert_eq!(found.len(), 1, "attach exactly one Airspy: {found:?}");
    let info = found.into_iter().next().expect("one Airspy");
    eprintln!("probe: {info:?}");
    assert!(info.serial.is_some(), "keyed by port, not serial");
    let device = driver.open(&info).expect("open through the driver");
    (driver, info, device)
}

fn stream_phase(device: &mut Box<dyn SdrDevice>, duration: Duration) -> (Phase, Seen) {
    let (sink, seen, failure) = watching_sink();
    device.rx_start(vec![sink]).expect("rx start");
    std::thread::sleep(duration);
    device.rx_stop();
    let failure = sdrmm_device::lock(&failure).take();
    assert!(failure.is_none(), "stream failed: {failure:?}");
    let mut seen = std::mem::take(&mut *sdrmm_device::lock(&seen));
    (std::mem::take(&mut seen.phases[0]), seen)
}

#[test]
#[ignore = "requires one idle Airspy R2 or Mini"]
fn connected_airspy_delivers_every_rate_through_the_device_api() {
    let (_, _, mut device) = open_through_the_driver();
    for rate in device.capabilities().sample_rates.clone() {
        device
            .apply(&DeviceSettings {
                center_hz: Some(f64::from(TUNE_HZ)),
                sample_rate: Some(rate),
                gains: vec![
                    GainValue::new(GainKind::Lna, 5.0),
                    GainValue::new(GainKind::Mixer, 5.0),
                    GainValue::new(GainKind::Vga, 5.0),
                ],
                agc: Some(AgcSetting::off()),
                ..DeviceSettings::default()
            })
            .expect("apply");
        assert_eq!(device.settings().sample_rate, Some(rate));
        assert_eq!(device.settings().center_hz, Some(f64::from(TUNE_HZ)));
        assert_eq!(device.settings().gain(GainKind::Lna.name()), Some(5.0));

        let (phase, seen) = stream_phase(&mut device, Duration::from_secs(seconds()));
        let measured = phase.rate();
        eprintln!(
            "api: rate={rate} measured={measured:.0} samples={} gaps={} non_finite={}",
            phase.samples, seen.index_gaps, seen.non_finite
        );
        assert_eq!(seen.index_gaps, 0, "{rate}: samples went missing");
        assert_eq!(seen.non_finite, 0);
        assert!(
            (measured / rate - 1.0).abs() < 0.02,
            "{rate}: rate mismatch"
        );
    }
}

#[test]
#[ignore = "requires one idle Airspy R2 or Mini"]
fn connected_airspy_changes_rate_while_streaming() {
    let (_, _, mut device) = open_through_the_driver();
    let rates = device.capabilities().sample_rates.clone();
    let (wide, narrow) = (rates[0], rates[rates.len() - 1]);
    device
        .apply(&DeviceSettings {
            center_hz: Some(f64::from(TUNE_HZ)),
            sample_rate: Some(narrow),
            ..DeviceSettings::default()
        })
        .expect("apply");
    let (sink, seen, failure) = watching_sink();
    sdrmm_device::lock(&seen).phase = None;
    device.rx_start(vec![sink]).expect("rx start");
    let plan = [wide, narrow, wide, narrow];
    for (phase, rate) in plan.iter().enumerate() {
        device
            .apply(&DeviceSettings {
                sample_rate: Some(*rate),
                ..DeviceSettings::default()
            })
            .expect("new rate while streaming");
        std::thread::sleep(SETTLE);
        sdrmm_device::lock(&seen).phase = Some(phase);
        std::thread::sleep(Duration::from_secs(2));
        sdrmm_device::lock(&seen).phase = None;
    }
    device.rx_stop();

    let seen = sdrmm_device::lock(&seen);
    let failure = sdrmm_device::lock(&failure);
    let measured: Vec<f64> = seen.phases.iter().map(Phase::rate).collect();
    eprintln!(
        "planned={plan:?} measured={measured:.0?} gaps={} failure={failure:?}",
        seen.index_gaps
    );
    assert!(failure.is_none(), "stream failed: {failure:?}");
    assert_eq!(seen.index_gaps, 0, "samples went missing");
    for (rate, got) in plan.iter().zip(measured) {
        assert!((got / rate - 1.0).abs() < 0.02, "{rate}: measured {got:.0}");
    }
}

#[test]
#[ignore = "requires one idle Airspy R2 or Mini"]
fn connected_airspy_restarts_and_reopens_through_the_device_api() {
    let (driver, info, mut device) = open_through_the_driver();
    let rate = *device.capabilities().sample_rates.first().expect("a rate");
    device
        .apply(&DeviceSettings {
            sample_rate: Some(rate),
            ..DeviceSettings::default()
        })
        .expect("apply");
    for round in 0..3 {
        let (phase, seen) = stream_phase(&mut device, Duration::from_secs(1));
        eprintln!(
            "round {round}: {:.0} gaps={}",
            phase.rate(),
            seen.index_gaps
        );
        assert!((phase.rate() / rate - 1.0).abs() < 0.03, "round {round}");
    }
    drop(device);
    let mut device = driver.open(&info).expect("reopen");
    let (phase, _) = stream_phase(&mut device, Duration::from_secs(1));
    assert!(phase.samples > 0, "a reopened radio sent nothing");
}

#[test]
#[ignore = "requires one idle Airspy R2 or Mini"]
fn connected_airspy_held_elsewhere_reads_as_in_use() {
    let _held = open_raw();
    let driver = AirspyDriver::new();
    let found = driver.probe();
    assert_eq!(found.len(), 1, "a radio in use is still listed");
    let refused = driver.open(&found[0]).err().expect("a second open");
    eprintln!("{refused}");
    assert!(matches!(refused, DeviceError::InUse(_)), "{refused:?}");
}

const PROFILE_SPAN_HZ: f64 = 900_000.0;
const PROFILE_STEP_HZ: f64 = 10_000.0;

fn band_profile(radio: &mut Airspy) -> Vec<f64> {
    let rate = f64::from(radio.config().sample_rate_hz);
    let spectrum = power_spectrum(&capture_iq(radio, FFT_LEN * 32));
    let mut profile = vec![0.0; (2.0 * PROFILE_SPAN_HZ / PROFILE_STEP_HZ) as usize];
    for (bin, power) in spectrum.iter().enumerate() {
        let offset = bin_offset_hz(bin, rate) + PROFILE_SPAN_HZ;
        if (0.0..2.0 * PROFILE_SPAN_HZ).contains(&offset) {
            profile[(offset / PROFILE_STEP_HZ) as usize] += power;
        }
    }
    profile.into_iter().map(db).collect()
}

fn best_lag(reference: &[f64], moved: &[f64]) -> isize {
    let mean = |values: &[f64]| values.iter().sum::<f64>() / values.len() as f64;
    let (a, b) = (mean(reference), mean(moved));
    (-40isize..=40)
        .map(|lag| {
            let score: f64 = (0..reference.len())
                .filter_map(|n| {
                    let m = usize::try_from(n as isize + lag).ok()?;
                    Some((reference[n] - a) * (moved.get(m)? - b))
                })
                .sum();
            (lag, score)
        })
        .fold(
            (0, f64::MIN),
            |best, next| if next.1 > best.1 { next } else { best },
        )
        .0
}

#[test]
#[ignore = "requires one Airspy R2 or Mini with an antenna in reach of stations near 88 MHz"]
fn connected_airspy_keeps_its_tuning_across_a_rate_change() {
    let mut radio = open_raw();
    full_gain(&mut radio);
    let rates = radio.sample_rates().to_vec();
    let (wide, narrow) = (rates[0], rates[rates.len() - 1]);
    radio.set_sample_rate_hz(narrow).expect("rate");
    radio.set_frequency_hz(ORIENTATION_TUNE_HZ).expect("tune");
    let reference = band_profile(&mut radio);
    for rate in [wide, narrow] {
        radio.set_sample_rate_hz(rate).expect("rate");
        let lag = best_lag(&reference, &band_profile(&mut radio));
        eprintln!(
            "{rate}: the band sits {} kHz from where {narrow} put it",
            lag * 10
        );
        assert_eq!(lag, 0, "{rate}: the band moved");
    }
}

fn spectrum_of(
    stream: &sdrmm_usb_stream::RxStream,
    converter: &mut convert::AirspyConverter,
) -> Vec<f64> {
    let started = Instant::now();
    let mut iq = Vec::with_capacity(FFT_LEN * 32);
    while iq.len() < FFT_LEN * 32 {
        let block = stream
            .recv_timeout(Duration::from_secs(1))
            .expect("a block within a second");
        let converted = converter.convert(&block);
        if started.elapsed() >= SETTLE {
            iq.extend_from_slice(converted);
        }
    }
    power_spectrum(&iq)
}

#[test]
#[ignore = "requires one Airspy R2 or Mini with an antenna in reach of a station near 88 MHz"]
fn connected_airspy_retunes_while_streaming() {
    let mut radio = open_raw();
    full_gain(&mut radio);
    let rate = *radio.sample_rates().last().expect("a rate");
    radio.set_sample_rate_hz(rate).expect("rate");
    radio.set_frequency_hz(ORIENTATION_TUNE_HZ).expect("tune");
    let rate = f64::from(rate);
    let mut converter = convert::AirspyConverter::new(RX_TRANSFER_SIZE / 2);
    let mut stream = radio.start_rx().expect("start rx");
    let before = spectrum_of(&stream, &mut converter);
    let (bin, peak) = strongest_bin(&before, rate, 900_000.0);
    assert!(peak > median(&before) * 10.0, "no carrier stands out");
    let old_offset = bin_offset_hz(bin, rate);
    let step_hz = 300_000;
    radio
        .set_frequency_hz(ORIENTATION_TUNE_HZ + step_hz)
        .expect("retune");
    let after = spectrum_of(&stream, &mut converter);
    radio.set_mode_off().expect("receiver off");
    stream.stop();
    let new_offset = old_offset - f64::from(step_hz);
    let moved = power_near(&after, rate, new_offset);
    let stayed = power_near(&after, rate, old_offset);
    eprintln!(
        "carrier at {old_offset:.0} Hz; after the retune {:.1} dB at {new_offset:.0} Hz, {:.1} dB where it was",
        db(moved / median(&after)),
        db(stayed / median(&after))
    );
    assert!(moved > stayed * 10.0, "the retune did not reach the radio");
}

#[test]
#[ignore = "requires one idle Airspy R2 or Mini"]
fn connected_airspy_agc_lifts_a_quiet_front_end() {
    let mut radio = open_raw();
    radio.set_frequency_hz(TUNE_HZ).expect("tune");
    let rate = *radio.sample_rates().last().expect("a rate");
    radio.set_sample_rate_hz(rate).expect("sample rate");
    radio.set_vga_gain(MAX_VGA_GAIN).expect("vga");
    radio.set_lna_gain(0).expect("lna");
    radio.set_mixer_gain(0).expect("mixer");
    let mut level = |lna: bool, mixer: bool| {
        radio.set_lna_agc(lna).expect("lna agc");
        radio.set_mixer_agc(mixer).expect("mixer agc");
        stream_raw(&mut radio, Duration::from_secs(1)).stats.rms()
    };
    let manual = level(false, false);
    let lna = level(true, false);
    let mixer = level(false, true);
    let both = level(true, true);
    let back = level(false, false);
    eprintln!("rms manual={manual:.1} lna={lna:.1} mixer={mixer:.1} both={both:.1} back={back:.1}");
    assert!(lna > manual * 2.0, "the LNA AGC did nothing");
    assert!(mixer > manual * 1.2, "the mixer AGC did nothing");
    assert!(both > lna.max(mixer), "both loops together did no more");
    assert!(
        (back / manual - 1.0).abs() < 0.3,
        "AGC off left the gain where the AGC put it"
    );
}

#[test]
#[ignore = "requires one idle Airspy R2 or Mini"]
fn connected_airspy_streams_at_both_ends_of_its_range() {
    let mut radio = open_raw();
    full_gain(&mut radio);
    for rate in radio.sample_rates().to_vec() {
        radio.set_sample_rate_hz(rate).expect("sample rate");
        for hz in [FREQ_MIN_HZ, TUNE_HZ, FREQ_MAX_HZ] {
            radio.set_frequency_hz(hz).expect("tune");
            let run = stream_raw(&mut radio, Duration::from_millis(500));
            let expected = f64::from(rate) * 4.0;
            eprintln!(
                "rate={rate} hz={hz} bytes/s={:.0} rms={:.1}",
                run.bytes_per_second,
                run.stats.rms()
            );
            assert_eq!(run.dropped, 0, "{rate} at {hz}: USB dropped transfers");
            assert!(
                (run.bytes_per_second / expected - 1.0).abs() < 0.02,
                "{rate} at {hz}: sample rate mismatch"
            );
            assert!(run.stats.rms() > 1.0, "{rate} at {hz}: the ADC is stuck");
        }
    }
}

#[test]
#[ignore = "requires one idle Airspy R2 or Mini"]
fn connected_airspy_has_no_spike_at_the_centre() {
    let mut radio = open_raw();
    full_gain(&mut radio);
    radio
        .set_frequency_hz(OFF_CLOCK_HARMONICS_HZ)
        .expect("tune");
    for rate in radio.sample_rates().to_vec() {
        radio.set_sample_rate_hz(rate).expect("sample rate");
        let spectrum = power_spectrum(&capture_iq(&mut radio, FFT_LEN * 64));
        let centre = spectrum[FFT_LEN / 2 - 1..=FFT_LEN / 2 + 1]
            .iter()
            .copied()
            .fold(0.0, f64::max);
        let beside = (spectrum[FFT_LEN / 2 - 40..FFT_LEN / 2 - 10].iter())
            .chain(&spectrum[FFT_LEN / 2 + 11..FFT_LEN / 2 + 41])
            .sum::<f64>()
            / 60.0;
        let spike = db(centre / beside);
        eprintln!("rate={rate}: centre {spike:.1} dB over its neighbours");
        assert!(spike < 6.0, "{rate}: a spike sits at the centre");
    }
}

#[test]
#[ignore = "requires one idle Airspy R2 or Mini"]
fn connected_airspy_noise_density_holds_across_rates() {
    let mut radio = open_raw();
    radio
        .set_frequency_hz(OFF_CLOCK_HARMONICS_HZ)
        .expect("tune");
    let mut densities = Vec::new();
    for rate in radio.sample_rates().to_vec() {
        radio.set_sample_rate_hz(rate).expect("sample rate");
        let density =
            median(&power_spectrum(&capture_iq(&mut radio, FFT_LEN * 32))) / f64::from(rate);
        eprintln!("rate={rate} density={:.1} dB", db(density));
        densities.push(density);
    }
    let lowest = densities.iter().copied().fold(f64::INFINITY, f64::min);
    let highest = densities.iter().copied().fold(0.0, f64::max);
    assert!(
        db(highest / lowest) < 3.0,
        "the level jumps {:.1} dB between rates",
        db(highest / lowest)
    );
}
