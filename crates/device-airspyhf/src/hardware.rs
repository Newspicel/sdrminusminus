use std::{
    sync::{Arc, Mutex, mpsc::RecvTimeoutError},
    time::{Duration, Instant},
};

use sdrmm_device::{DeviceDriver, RxSink, Sample, SampleConverter};
use sdrmm_dsp::fft::FftPair;
use sdrmm_wire::{AgcSetting, DcArtifact, DeviceSettings, GainKind, GainValue};

use super::*;
use crate::driver::{MAX_ATTENUATION_STEP, MAX_PPM};

const HF_TUNE_HZ: u32 = 7_100_000;
const SETTLE: Duration = Duration::from_millis(500);
const FFT_LEN: usize = 8192;

fn seconds() -> u64 {
    std::env::var("SDRMM_AIRSPYHF_TEST_SECONDS")
        .map(|value| value.parse().expect("duration"))
        .unwrap_or(5)
}

fn only_airspyhf() -> DeviceDescriptor {
    let found = AirspyHf::list().expect("list USB devices");
    assert_eq!(found.len(), 1, "attach exactly one Airspy HF+: {found:?}");
    found.into_iter().next().expect("one Airspy HF+")
}

fn open_raw() -> AirspyHf {
    let descriptor = only_airspyhf();
    match descriptor.serial {
        Some(serial) => AirspyHf::open_serial(serial),
        None => AirspyHf::open_at(descriptor.bus, descriptor.address),
    }
    .expect("open Airspy HF+")
}

fn quiet(radio: &mut AirspyHf) {
    radio.set_agc(false).expect("agc off");
    radio.set_lna(false).expect("preamp off");
    radio.set_attenuation_step(0).expect("no attenuation");
}

fn rate_where(radio: &mut AirspyHf, low_if: bool) -> u32 {
    for rate in radio.sample_rates().to_vec() {
        radio.set_sample_rate_hz(rate).expect("sample rate");
        if radio.is_low_if() == low_if {
            return rate;
        }
    }
    panic!("no rate with low_if={low_if}")
}

#[derive(Default)]
struct WordStats {
    words: u64,
    rails: u64,
    sum: [f64; 2],
    sum_sq: [f64; 2],
}

impl WordStats {
    fn push(&mut self, bytes: &[u8]) {
        for quad in bytes.as_chunks::<4>().0 {
            for (lane, pair) in quad.as_chunks::<2>().0.iter().enumerate() {
                let word = i16::from_le_bytes(*pair);
                if word == i16::MIN || word == i16::MAX {
                    self.rails += 1;
                }
                self.sum[lane] += f64::from(word);
                self.sum_sq[lane] += f64::from(word) * f64::from(word);
            }
            self.words += 1;
        }
    }

    fn mean(&self, lane: usize) -> f64 {
        self.sum[lane] / self.words as f64
    }

    fn rms(&self, lane: usize) -> f64 {
        let mean = self.mean(lane);
        (self.sum_sq[lane] / self.words as f64 - mean * mean).sqrt()
    }
}

struct RawRun {
    bytes_per_second: f64,
    stats: WordStats,
    dropped: u64,
    missing_bytes: u64,
}

fn stream_raw(radio: &mut AirspyHf, duration: Duration) -> RawRun {
    let mut stream = radio.start_rx().expect("start rx");
    let started = Instant::now();
    let mut stats = WordStats::default();
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
        .duration_since(first.expect("the radio sent nothing"))
        .as_secs_f64();
    RawRun {
        bytes_per_second: measured as f64 / elapsed,
        stats,
        dropped: usb.dropped,
        missing_bytes,
    }
}

fn capture_iq(radio: &mut AirspyHf, samples: usize) -> Vec<Sample> {
    let scale = SampleScale::new(radio.config().filter_gain_db);
    let mut converter = AirspyHfConverter::new(RX_TRANSFER_SIZE / 4, scale);
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

fn variance(iq: &[Sample]) -> f64 {
    let mean = iq.iter().sum::<Sample>() / iq.len() as f32;
    iq.iter().map(|s| f64::from(s.norm_sqr())).sum::<f64>() / iq.len() as f64
        - f64::from(mean.norm_sqr())
}

fn median_bin_power(iq: &[Sample]) -> f64 {
    let mut fft = FftPair::new(FFT_LEN);
    let window: Vec<f32> = (0..FFT_LEN)
        .map(|n| {
            let x = std::f32::consts::TAU * n as f32 / FFT_LEN as f32;
            0.5 - 0.5 * x.cos()
        })
        .collect();
    let mut power = vec![0.0f64; FFT_LEN];
    for frame in iq.as_chunks::<FFT_LEN>().0 {
        let mut buf: Vec<Sample> = frame.iter().zip(&window).map(|(s, w)| s * w).collect();
        fft.forward(&mut buf);
        for (bin, value) in power.iter_mut().zip(&buf) {
            *bin += f64::from(value.norm_sqr());
        }
    }
    power.sort_by(f64::total_cmp);
    power[FFT_LEN / 2] / (iq.len() / FFT_LEN) as f64
}

fn db(ratio: f64) -> f64 {
    10.0 * ratio.log10()
}

#[test]
#[ignore = "requires one idle Airspy HF+"]
fn connected_airspyhf_reports_the_serial_its_descriptor_carries() {
    let descriptor = only_airspyhf();
    let radio = open_raw();
    eprintln!(
        "descriptor={descriptor:?} firmware={:?} serial={:?} rates={:?}",
        radio.version(),
        radio.serial().map(full_serial),
        radio.sample_rates(),
    );
    eprintln!("stored calibration: {} ppm", radio.config().ppm);
    assert!(descriptor.serial.is_some(), "descriptor serial not parsed");
    assert_eq!(radio.serial(), descriptor.serial);
    assert!(!radio.version().is_empty());
    assert!(!radio.sample_rates().is_empty());
    let info = device_info(&descriptor);
    assert_eq!(key_serial(&info.key), descriptor.serial);
}

#[test]
#[ignore = "requires one idle Airspy HF+"]
fn connected_airspyhf_streams_at_every_edge_of_both_windows() {
    let mut radio = open_raw();
    quiet(&mut radio);
    for low_if in [false, true] {
        let rate = rate_where(&mut radio, low_if);
        for (asked, tuned) in [
            (1_000, if low_if { 84_000 } else { 180_000 }),
            (100_000, if low_if { 100_000 } else { 180_000 }),
            (HF_TUNE_HZ, HF_TUNE_HZ),
            (31_000_000, 31_000_000),
            (60_000_000, 60_000_000),
            (260_000_000, 260_000_000),
        ] {
            radio.set_frequency_hz(asked).expect("tune");
            assert_eq!(radio.config().lo_khz * 1000, tuned, "{rate}: {asked} Hz");
            let run = stream_raw(&mut radio, Duration::from_millis(300));
            eprintln!(
                "rate={rate} asked={asked} tuned={tuned} bytes/s={:.0}",
                run.bytes_per_second
            );
        }
    }
    let low_if_rate = rate_where(&mut radio, true);
    radio.set_frequency_hz(100_000).expect("tune");
    assert_eq!(radio.config().lo_khz, 100);
    let zero_if_rate = rate_where(&mut radio, false);
    assert_eq!(
        radio.config().lo_khz,
        180,
        "{low_if_rate} to {zero_if_rate} left the oscillator below the floor"
    );
    stream_raw(&mut radio, Duration::from_millis(300));
}

#[test]
#[ignore = "requires one idle Airspy HF+"]
fn connected_airspyhf_streams_every_rate_without_loss() {
    let mut radio = open_raw();
    quiet(&mut radio);
    radio.set_frequency_hz(HF_TUNE_HZ).expect("tune");
    let duration = Duration::from_secs(seconds());
    for rate in radio.sample_rates().to_vec() {
        radio.set_sample_rate_hz(rate).expect("sample rate");
        let run = stream_raw(&mut radio, duration);
        let expected = f64::from(rate) * 4.0;
        eprintln!(
            "rate={rate} low_if={} filter_gain={} dB bytes/s={:.0} dropped={} missing={} mean=({:.1},{:.1}) rms=({:.1},{:.1}) rails={}",
            radio.is_low_if(),
            radio.config().filter_gain_db,
            run.bytes_per_second,
            run.dropped,
            run.missing_bytes,
            run.stats.mean(0),
            run.stats.mean(1),
            run.stats.rms(0),
            run.stats.rms(1),
            run.stats.rails
        );
        assert_eq!(run.dropped, 0, "{rate}: USB dropped transfers");
        assert_eq!(run.missing_bytes, 0, "{rate}: bytes went missing");
        assert!(
            (run.bytes_per_second / expected - 1.0).abs() < 0.01,
            "{rate}: sample rate mismatch"
        );
        assert_eq!(run.stats.rails, 0, "{rate}: clipped with no antenna");
        for lane in 0..2 {
            assert!(run.stats.rms(lane) > 1.0, "{rate}: lane {lane} is stuck");
        }
        let balance = run.stats.rms(0) / run.stats.rms(1);
        assert!((0.7..1.4).contains(&balance), "{rate}: I and Q differ");
    }
}

#[test]
#[ignore = "requires one idle Airspy HF+"]
fn connected_airspyhf_noise_density_holds_across_rates() {
    let mut radio = open_raw();
    quiet(&mut radio);
    radio.set_frequency_hz(HF_TUNE_HZ).expect("tune");
    let mut densities = Vec::new();
    for rate in radio.sample_rates().to_vec() {
        radio.set_sample_rate_hz(rate).expect("sample rate");
        let density = median_bin_power(&capture_iq(&mut radio, FFT_LEN * 32)) / f64::from(rate);
        eprintln!(
            "rate={rate} filter_gain={} dB density={:.1} dB",
            radio.config().filter_gain_db,
            db(density)
        );
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

#[test]
#[ignore = "requires one idle Airspy HF+"]
fn connected_airspyhf_attenuator_and_preamp_move_the_level() {
    let mut radio = open_raw();
    quiet(&mut radio);
    radio.set_frequency_hz(HF_TUNE_HZ).expect("tune");
    let rate = *radio.sample_rates().first().expect("a rate");
    radio.set_sample_rate_hz(rate).expect("sample rate");
    let level = |radio: &mut AirspyHf, lna: bool, step: u8| {
        radio.set_lna(lna).expect("preamp");
        radio.set_attenuation_step(step).expect("attenuator");
        variance(&capture_iq(radio, FFT_LEN * 32))
    };
    let base = level(&mut radio, false, 0);
    let preamp = level(&mut radio, true, 0);
    let steps: Vec<f64> = (1..=MAX_ATTENUATION_STEP)
        .map(|step| level(&mut radio, false, step))
        .collect();
    eprintln!(
        "base={:.1} dBFS preamp={:+.1} dB attenuation steps={:?}",
        db(base),
        db(preamp / base),
        steps
            .iter()
            .map(|p| format!("{:+.1}", db(p / base)))
            .collect::<Vec<_>>()
    );
    assert!(db(preamp / base) > 3.0, "the preamp did nothing");
    assert!(db(steps[0] / base) < -3.0, "the first step did nothing");
    assert!(steps[1] < steps[0], "the second step did nothing");
    assert!(
        db(steps[steps.len() - 1] / base) < -20.0,
        "full attenuation did not lower the level"
    );
}

#[derive(Default)]
struct Phase {
    samples: u64,
    after_first: u64,
    first: Option<Instant>,
    last: Option<Instant>,
    sum: Sample,
    sum_sq: f64,
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
        self.sum += samples.iter().sum::<Sample>();
        self.sum_sq += samples.iter().map(|s| f64::from(s.norm_sqr())).sum::<f64>();
    }

    fn rate(&self) -> f64 {
        let elapsed = self
            .last
            .zip(self.first)
            .map(|(last, first)| last.duration_since(first).as_secs_f64())
            .expect("samples arrived");
        self.after_first as f64 / elapsed
    }

    fn variance(&self) -> f64 {
        let n = self.samples as f64;
        let mean = self.sum / n as f32;
        self.sum_sq / n - f64::from(mean.norm_sqr())
    }
}

#[derive(Default)]
struct Seen {
    next_index: u64,
    index_gaps: u64,
    non_finite: u64,
    phase: Option<usize>,
    phases: [Phase; 2],
}

type Failure = Arc<Mutex<Option<sdrmm_device::DeviceError>>>;

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

fn open_through_the_driver() -> Box<dyn SdrDevice> {
    let driver = AirspyHfDriver::new();
    let found = driver.probe();
    assert_eq!(found.len(), 1, "attach exactly one Airspy HF+: {found:?}");
    eprintln!("probe: {:?}", found[0]);
    assert!(found[0].serial.is_some(), "keyed by port, not serial");
    driver.open(&found[0]).expect("open through the driver")
}

#[test]
#[ignore = "requires one idle Airspy HF+"]
fn connected_airspyhf_delivers_through_the_device_api() {
    let mut device = open_through_the_driver();
    let rate = *device.capabilities().sample_rates.first().expect("a rate");
    device
        .apply(&DeviceSettings {
            center_hz: Some(f64::from(HF_TUNE_HZ)),
            sample_rate: Some(rate),
            gains: vec![
                GainValue::new(GainKind::Amp, 6.0),
                GainValue::new(GainKind::Attenuator, -12.0),
            ],
            agc: Some(AgcSetting::in_mode(true, caps::AGC_HIGH)),
            ..DeviceSettings::default()
        })
        .expect("apply");
    assert_eq!(device.settings().sample_rate, Some(rate));
    assert_eq!(device.settings().center_hz, Some(f64::from(HF_TUNE_HZ)));
    assert_eq!(device.settings().gain(GainKind::Amp.name()), Some(6.0));
    assert_eq!(
        device.settings().gain(GainKind::Attenuator.name()),
        Some(-12.0)
    );
    assert_eq!(
        device.settings().agc,
        Some(AgcSetting::in_mode(true, caps::AGC_HIGH))
    );

    let (sink, seen, failure) = watching_sink();
    device.rx_start(vec![sink]).expect("rx start");
    std::thread::sleep(Duration::from_secs(seconds()));
    device.rx_stop();

    let seen = sdrmm_device::lock(&seen);
    let failure = sdrmm_device::lock(&failure);
    let measured = seen.phases[0].rate();
    eprintln!(
        "api: rate={rate} measured={measured:.0} samples={} gaps={} non_finite={} failure={failure:?}",
        seen.phases[0].samples, seen.index_gaps, seen.non_finite
    );
    assert!(failure.is_none(), "stream failed: {failure:?}");
    assert_eq!(seen.index_gaps, 0, "samples went missing");
    assert_eq!(seen.non_finite, 0);
    assert!((measured / rate - 1.0).abs() < 0.02, "rate mismatch");
}

#[test]
#[ignore = "requires one idle Airspy HF+"]
fn connected_airspyhf_changes_rate_while_streaming() {
    let mut device = open_through_the_driver();
    let rates = device.capabilities().sample_rates.clone();
    let (wide, narrow) = (rates[0], rates[rates.len() - 1]);
    let quiet = DeviceSettings {
        center_hz: Some(f64::from(HF_TUNE_HZ)),
        sample_rate: Some(wide),
        gains: vec![
            GainValue::new(GainKind::Amp, 0.0),
            GainValue::new(GainKind::Attenuator, 0.0),
        ],
        agc: Some(AgcSetting::off()),
        ..DeviceSettings::default()
    };
    device.apply(&quiet).expect("apply");
    assert!(device.capabilities().dc_artifact.is_managed());

    let (sink, seen, failure) = watching_sink();
    sdrmm_device::lock(&seen).phase = None;
    device.rx_start(vec![sink]).expect("rx start");
    std::thread::sleep(SETTLE);
    sdrmm_device::lock(&seen).phase = Some(0);
    std::thread::sleep(Duration::from_secs(2));
    sdrmm_device::lock(&seen).phase = None;
    device
        .apply(&DeviceSettings {
            sample_rate: Some(narrow),
            ..DeviceSettings::default()
        })
        .expect("new rate while streaming");
    assert_eq!(device.capabilities().dc_artifact, DcArtifact::None);
    std::thread::sleep(SETTLE);
    sdrmm_device::lock(&seen).phase = Some(1);
    std::thread::sleep(Duration::from_secs(2));
    device.rx_stop();

    let seen = sdrmm_device::lock(&seen);
    let failure = sdrmm_device::lock(&failure);
    let before = &seen.phases[0];
    let after = &seen.phases[1];
    let shift = db((after.variance() / narrow) / (before.variance() / wide));
    eprintln!(
        "{wide} Hz measured {:.0} at {:.1} dBFS, {narrow} Hz measured {:.0} at {:.1} dBFS, density moved {shift:+.1} dB, failure={failure:?}",
        before.rate(),
        db(before.variance()),
        after.rate(),
        db(after.variance())
    );
    assert!(failure.is_none(), "stream failed: {failure:?}");
    assert!((before.rate() / wide - 1.0).abs() < 0.03, "wide rate");
    assert!((after.rate() / narrow - 1.0).abs() < 0.03, "narrow rate");
    assert!(shift.abs() < 3.0, "the level jumped with the rate");
}

fn dc_after_settling(stream: &sdrmm_usb_stream::RxStream) -> (f64, f64) {
    let started = Instant::now();
    let mut stats = WordStats::default();
    while started.elapsed() < SETTLE + Duration::from_secs(1) {
        if let Ok(block) = stream.recv_timeout(Duration::from_millis(100))
            && started.elapsed() >= SETTLE
        {
            stats.push(&block);
        }
    }
    (stats.mean(0), stats.mean(1))
}

#[test]
#[ignore = "requires one idle Airspy HF+"]
fn connected_airspyhf_retunes_while_streaming() {
    let mut radio = open_raw();
    quiet(&mut radio);
    let rate = rate_where(&mut radio, false);
    radio.set_frequency_hz(HF_TUNE_HZ).expect("tune");
    let stream = radio.start_rx().expect("start rx");
    let hf = dc_after_settling(&stream);
    radio.set_frequency_hz(145_000_000).expect("retune");
    let live = dc_after_settling(&stream);
    radio.set_mode_off().expect("receiver off");
    drop(stream);
    let stream = radio.start_rx().expect("start rx");
    let fresh = dc_after_settling(&stream);
    radio.set_mode_off().expect("receiver off");
    drop(stream);
    let distance = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).hypot(a.1 - b.1);
    eprintln!("{rate} Hz: dc at hf={hf:?} live={live:?} fresh={fresh:?}");
    assert!(
        distance(live, fresh) * 4.0 < distance(live, hf),
        "a retune while streaming did not reach the radio"
    );
}

const DCF77_HZ: f64 = 77_500.0;
const ZOOM_LEN: usize = 1 << 16;

struct Zoom {
    spectrum: Vec<f64>,
    bin_hz: f64,
    floor: f64,
}

impl Zoom {
    fn capture(radio: &mut AirspyHf) -> Self {
        let iq = capture_iq(radio, ZOOM_LEN * 8);
        let mut fft = FftPair::new(ZOOM_LEN);
        let window: Vec<f32> = (0..ZOOM_LEN)
            .map(|n| 0.5 - 0.5 * (std::f32::consts::TAU * n as f32 / ZOOM_LEN as f32).cos())
            .collect();
        let mut power = vec![0.0f64; ZOOM_LEN];
        for frame in iq.as_chunks::<ZOOM_LEN>().0 {
            let mut buf: Vec<Sample> = frame.iter().zip(&window).map(|(s, w)| s * w).collect();
            fft.forward(&mut buf);
            for (bin, value) in power.iter_mut().zip(&buf) {
                *bin += f64::from(value.norm_sqr());
            }
        }
        let spectrum: Vec<f64> = (0..ZOOM_LEN)
            .map(|bin| power[(bin + ZOOM_LEN / 2) % ZOOM_LEN])
            .collect();
        let mut sorted = spectrum.clone();
        sorted.sort_by(f64::total_cmp);
        Self {
            floor: sorted[ZOOM_LEN / 2],
            spectrum,
            bin_hz: f64::from(radio.config().sample_rate_hz) / ZOOM_LEN as f64,
        }
    }

    fn bin(&self, offset_hz: f64) -> usize {
        ((offset_hz / self.bin_hz).round() as isize + (ZOOM_LEN / 2) as isize) as usize
    }

    fn carrier(&self, offset_hz: f64, span_hz: f64) -> (f64, f64) {
        let centre = self.bin(offset_hz);
        let reach = (span_hz / self.bin_hz) as usize;
        let (bin, power) = (centre - reach..=centre + reach)
            .map(|bin| (bin, self.spectrum[bin]))
            .fold(
                (centre, 0.0),
                |best, next| if next.1 > best.1 { next } else { best },
            );
        let (left, peak, right) = (
            self.spectrum[bin - 1].ln(),
            power.ln(),
            self.spectrum[bin + 1].ln(),
        );
        let nudge = 0.5 * (left - right) / (left - 2.0 * peak + right);
        (
            (bin as f64 + nudge - (ZOOM_LEN / 2) as f64) * self.bin_hz,
            power,
        )
    }
}

#[test]
#[ignore = "requires one Airspy HF+ with an antenna in reach of DCF77"]
fn connected_airspyhf_puts_dcf77_upright_on_77_5_khz() {
    let mut radio = open_raw();
    quiet(&mut radio);
    for low_if in [false, true] {
        let rate = rate_where(&mut radio, low_if);
        for asked in [DCF77_HZ as u32, 100_000, 200_000] {
            radio.set_frequency_hz(asked).expect("tune");
            let centre = radio.config().center_hz();
            let zoom = Zoom::capture(&mut radio);
            let (offset, power) = zoom.carrier(DCF77_HZ - centre, 300.0);
            let error = centre + offset - DCF77_HZ;
            eprintln!(
                "rate={rate} centre={centre} dcf77 at {:.1} Hz ({error:+.1} Hz), {:.1} dB over the floor",
                centre + offset,
                db(power / zoom.floor)
            );
            assert!(
                db(power / zoom.floor) > 12.0,
                "{rate} at {centre}: no DCF77 carrier, or it is mirrored"
            );
            assert!(
                error.abs() < zoom.bin_hz,
                "{rate} at {centre}: off by {error:.1} Hz"
            );
        }
    }
}

#[test]
#[ignore = "requires one Airspy HF+ with an antenna in reach of DCF77"]
fn connected_airspyhf_stages_move_a_received_carrier() {
    let mut radio = open_raw();
    quiet(&mut radio);
    rate_where(&mut radio, true);
    radio.set_frequency_hz(DCF77_HZ as u32).expect("tune");
    let offset = DCF77_HZ - radio.config().center_hz();
    let carrier = |radio: &mut AirspyHf, lna: bool, step: u8| {
        radio.set_lna(lna).expect("preamp");
        radio.set_attenuation_step(step).expect("attenuator");
        Zoom::capture(radio).carrier(offset, 300.0).1
    };
    let base = carrier(&mut radio, false, 0);
    let preamp = db(carrier(&mut radio, true, 0) / base);
    let one = db(carrier(&mut radio, false, 1) / base);
    let two = db(carrier(&mut radio, false, 2) / base);
    eprintln!("preamp {preamp:+.1} dB, one step {one:+.1} dB, two steps {two:+.1} dB");
    assert!((preamp - 6.0).abs() < 3.0, "preamp");
    assert!((one + 6.0).abs() < 3.0, "one attenuator step");
    assert!((two + 12.0).abs() < 3.0, "two attenuator steps");
}

#[test]
#[ignore = "requires one idle Airspy HF+"]
fn connected_airspyhf_blocks_dc_only_where_there_is_a_spike() {
    let mut radio = open_raw();
    quiet(&mut radio);
    radio.set_frequency_hz(HF_TUNE_HZ).expect("tune");
    for rate in radio.sample_rates().to_vec() {
        radio.set_sample_rate_hz(rate).expect("sample rate");
        let hint = caps::dc_artifact(radio.is_low_if());
        let zoom = Zoom::capture(&mut radio);
        let centre = zoom.spectrum[ZOOM_LEN / 2 - 1..=ZOOM_LEN / 2 + 1]
            .iter()
            .copied()
            .fold(0.0, f64::max);
        let beside = zoom.spectrum[ZOOM_LEN / 2 - 40..ZOOM_LEN / 2 - 10]
            .iter()
            .sum::<f64>()
            / 30.0;
        let spike = db(centre / beside);
        eprintln!("rate={rate} {hint:?}: centre {spike:.1} dB over its neighbours");
        if hint.is_managed() {
            assert!(spike > 15.0, "{rate}: blocking a spike that is not there");
        } else {
            assert!(spike < 6.0, "{rate}: a spike is left in");
        }
    }
}

#[test]
#[ignore = "requires one Airspy HF+ with an antenna in reach of DCF77"]
fn connected_airspyhf_ppm_moves_what_it_shows_by_the_correction() {
    let mut radio = open_raw();
    quiet(&mut radio);
    rate_where(&mut radio, false);
    let shown = |radio: &mut AirspyHf, ppm: f64| {
        radio.set_ppm(ppm).expect("ppm");
        radio.set_frequency_hz(450_000).expect("tune");
        let centre = radio.config().center_hz();
        centre + Zoom::capture(radio).carrier(DCF77_HZ - centre, 300.0).0
    };
    let plain = shown(&mut radio, 0.0);
    let corrected = shown(&mut radio, MAX_PPM);
    let expected = f64::from(radio.config().lo_khz) * 1000.0 * MAX_PPM * 1e-6;
    eprintln!(
        "dcf77 shown at {plain:.1} Hz, at {corrected:.1} Hz with {MAX_PPM} ppm, moved {:.1} Hz of {expected:.1} Hz",
        corrected - plain
    );
    assert!(
        (corrected - plain - expected).abs() < 8.0,
        "ppm moved the view wrongly"
    );
}
