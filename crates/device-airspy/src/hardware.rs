use std::{
    sync::{Arc, Mutex, mpsc::RecvTimeoutError},
    time::{Duration, Instant},
};

use sdrmm_device::{DeviceDriver, RxSink, Sample, SampleConverter};
use sdrmm_dsp::fft::FftPair;
use sdrmm_wire::{DeviceSettings, GainKind, GainValue};

use super::*;
use crate::driver::{MAX_LNA_GAIN, MAX_MIXER_GAIN, MAX_VGA_GAIN};

const TUNE_HZ: u32 = 100_000_000;
const ORIENTATION_TUNE_HZ: u32 = 88_000_000;
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
                missing_bytes += block.missing_bytes();
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

#[test]
#[ignore = "requires one idle Airspy R2 or Mini"]
fn connected_airspy_delivers_through_the_device_api() {
    let driver = AirspyDriver::new();
    let found = driver.probe();
    assert_eq!(found.len(), 1, "attach exactly one Airspy: {found:?}");
    let info = &found[0];
    eprintln!("probe: {info:?}");
    let mut device = driver.open(info).expect("open through the driver");
    let rate = *device.capabilities().sample_rates.last().expect("a rate");
    device
        .apply(&DeviceSettings {
            center_hz: Some(f64::from(TUNE_HZ)),
            sample_rate: Some(rate),
            gains: vec![
                GainValue::new(GainKind::Lna, 5.0),
                GainValue::new(GainKind::Mixer, 5.0),
                GainValue::new(GainKind::Vga, 5.0),
            ],
            ..DeviceSettings::default()
        })
        .expect("apply");
    assert_eq!(device.settings().sample_rate, Some(rate));
    assert_eq!(device.settings().center_hz, Some(f64::from(TUNE_HZ)));

    #[derive(Default)]
    struct Seen {
        samples: u64,
        next_index: u64,
        index_gaps: u64,
        non_finite: u64,
        first: Option<Instant>,
        last: Option<Instant>,
        after_first: u64,
    }
    let seen = Arc::new(Mutex::new(Seen::default()));
    let failure = Arc::new(Mutex::new(None));
    let sink = {
        let seen = seen.clone();
        let failure = failure.clone();
        RxSink::with_fatal_handler(
            move |samples: &[Sample], index: u64| {
                let mut seen = sdrmm_device::lock(&seen);
                let now = Instant::now();
                if index != seen.next_index {
                    seen.index_gaps += 1;
                }
                seen.next_index = index + samples.len() as u64;
                seen.non_finite += samples
                    .iter()
                    .filter(|s| !s.re.is_finite() || !s.im.is_finite())
                    .count() as u64;
                seen.samples += samples.len() as u64;
                if seen.first.is_some() {
                    seen.after_first += samples.len() as u64;
                } else {
                    seen.first = Some(now);
                }
                seen.last = Some(now);
            },
            move |err| *sdrmm_device::lock(&failure) = Some(err),
        )
    };
    device.rx_start(vec![sink]).expect("rx start");
    std::thread::sleep(Duration::from_secs(seconds()));
    device.rx_stop();

    let seen = sdrmm_device::lock(&seen);
    let failure = sdrmm_device::lock(&failure);
    let elapsed = seen
        .last
        .zip(seen.first)
        .map(|(last, first)| last.duration_since(first).as_secs_f64())
        .expect("samples arrived");
    let measured = seen.after_first as f64 / elapsed;
    eprintln!(
        "api: rate={rate} measured={measured:.0} samples={} gaps={} non_finite={} failure={failure:?}",
        seen.samples, seen.index_gaps, seen.non_finite
    );
    assert!(failure.is_none(), "stream failed: {failure:?}");
    assert_eq!(seen.index_gaps, 0, "samples went missing");
    assert_eq!(seen.non_finite, 0);
    assert!((measured / rate - 1.0).abs() < 0.02, "rate mismatch");
}
