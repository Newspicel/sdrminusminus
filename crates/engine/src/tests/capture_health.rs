use std::sync::atomic::AtomicU64;

use super::*;

mod control;
mod history;
mod monitor;
use control::Control;
use monitor::{AudioMonitor, SpectrumMonitor};

#[test]
fn channel_downconversion_throughput() {
    use sdrmm_test_support::{assert_no_alloc, measure_throughput};

    let input = vec![Complex::new(0.5, -0.5); 2048];
    for (rate, output_rate) in [
        (2_400_000.0, 48_000.0),
        (20_000_000.0, 48_000.0),
        (20_000_000.0, 240_000.0),
    ] {
        let mut ddc = sdrmm_dsp::Ddc::new(rate, output_rate, 100_000.0).expect("rates");
        let mut output = Vec::new();
        for _ in 0..100 {
            ddc.process(&input, &mut output);
        }
        let mut process = || {
            ddc.process(std::hint::black_box(&input), &mut output);
            std::hint::black_box(&output);
        };
        assert_no_alloc("channel downconversion", &mut process);
        let throughput = measure_throughput(2000, input.len() as u64, process);
        eprintln!("DDC {rate:.0} -> {output_rate:.0} S/s: {throughput:.1} MS/s");
        assert!(throughput > 20.0, "DDC: {throughput:.1} MS/s");
    }
}

#[test]
fn shared_usb_conversion_throughput() {
    use sdrmm_device::{LutConverter, SampleConverter};
    use sdrmm_test_support::{assert_no_alloc, measure_throughput};

    static TABLE: [f32; 256] = [0.5; 256];
    let bytes = vec![127; 262_144];
    let mut converter = LutConverter::new(&TABLE, bytes.len() / 2);
    let mut convert = || {
        std::hint::black_box(converter.convert(std::hint::black_box(&bytes)));
    };
    assert_no_alloc("USB conversion", &mut convert);
    let throughput = measure_throughput(100, (bytes.len() / 2) as u64, convert);
    eprintln!("USB conversion: {throughput:.1} MS/s");
    assert!(throughput > 20.0, "USB conversion: {throughput:.1} MS/s");
}

fn configured_radio(driver: &dyn DeviceDriver, info: &DeviceInfo, rate: f64) -> Box<dyn SdrDevice> {
    let mut radio = driver.open(info).expect("open receive-only test radio");
    radio
        .apply(&DeviceSettings {
            center_hz: Some(100_000_000.0),
            sample_rate: Some(rate),
            ..Default::default()
        })
        .expect("configure test radio");
    radio
}

fn measure_transport(driver: &dyn DeviceDriver, info: &DeviceInfo, rate: f64, seconds: u64) {
    let mut radio = configured_radio(driver, info, rate);
    let received = Arc::new(AtomicU64::new(0));
    let missing = Arc::new(AtomicU64::new(0));
    let failures = Arc::new(AtomicUsize::new(0));
    let count = received.clone();
    let gaps = missing.clone();
    let errors = failures.clone();
    let mut next = 0;
    radio
        .rx_start(vec![RxSink::with_fatal_handler(
            move |samples, index| {
                gaps.fetch_add(index.saturating_sub(next), Ordering::Relaxed);
                next = index + samples.len() as u64;
                count.fetch_add(samples.len() as u64, Ordering::Relaxed);
            },
            move |error| {
                eprintln!("transport failure: {error}");
                errors.fetch_add(1, Ordering::Relaxed);
            },
        )])
        .expect("start transport");
    std::thread::sleep(Duration::from_secs(1));
    let baseline = received.load(Ordering::Relaxed);
    let started = Instant::now();
    std::thread::sleep(Duration::from_secs(seconds));
    let samples = received.load(Ordering::Relaxed) - baseline;
    let measured = samples as f64 / started.elapsed().as_secs_f64();
    radio.rx_stop();
    eprintln!(
        "transport {} requested={rate:.0} measured={measured:.0} missing={} failures={}",
        driver.id(),
        missing.load(Ordering::Relaxed),
        failures.load(Ordering::Relaxed)
    );
    assert!(samples > 0, "radio delivered no samples");
    assert_eq!(failures.load(Ordering::Relaxed), 0);
    assert_eq!(missing.load(Ordering::Relaxed), 0, "transport lost samples");
    assert!(
        (measured / rate - 1.0).abs() < 0.01,
        "transport missed the requested sample rate"
    );
}

struct Hardware {
    driver: Box<dyn DeviceDriver>,
    info: DeviceInfo,
    rate: f64,
}

fn number<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .map(|value| value.parse().ok().expect(name))
        .unwrap_or(default)
}

fn enabled(name: &str) -> bool {
    std::env::var(name).as_deref() == Ok("1")
}

fn channel_settings(index: usize) -> ChannelSettings {
    let params = if enabled("SDRMM_CAPTURE_MIXED") {
        match index % 4 {
            0 => ChannelParams::Nfm(NfmParams::default()),
            1 => ChannelParams::Wfm(sdrmm_wire::WfmParams::default()),
            2 => ChannelParams::Am(sdrmm_wire::AmParams::default()),
            _ => ChannelParams::Ssb(SsbParams::default()),
        }
    } else {
        ChannelParams::Nfm(NfmParams::default())
    };
    ChannelSettings {
        frequency_hz: 100_100_000.0 + index as f64 * 25_000.0,
        squelch: sdrmm_wire::Squelch::Off,
        params,
        audio: Default::default(),
    }
}

fn open_pipeline(engine: &Engine, hardware: &Hardware, channels: usize) -> (u32, Vec<u32>) {
    let ds = engine
        .create_opened_set(
            hardware.info.clone(),
            configured_radio(hardware.driver.as_ref(), &hardware.info, hardware.rate),
            None,
        )
        .expect("start pipeline");
    let ids = (0..channels)
        .map(|index| {
            let settings = channel_settings(index);
            assert!(
                engine.hears(ds, 0, &settings),
                "test channel is outside the capture band"
            );
            engine.add_channel(ds, 0, settings).expect("add channel")
        })
        .collect();
    (ds, ids)
}

fn measure_pipeline(hardware: &[Hardware], seconds: u64) {
    let directory = tempfile::tempdir().expect("recording directory");
    let engine = Engine::with_registry(DeviceRegistry::new(), Some(directory.path().into()));
    let channels = number("SDRMM_CAPTURE_CHANNELS", 4);
    let allow_drops = enabled("SDRMM_CAPTURE_ALLOW_DROPS");
    let recording = enabled("SDRMM_CAPTURE_RECORD");
    let sets: Vec<_> = hardware
        .iter()
        .map(|radio| open_pipeline(&engine, radio, channels))
        .collect();
    let history = enabled("SDRMM_CAPTURE_HISTORY");
    if history {
        history::start(&engine, &sets);
    }
    let mut audio: Vec<_> = sets
        .iter()
        .flat_map(|(ds, ids)| {
            ids.iter()
                .map(|channel| AudioMonitor::new(&engine, *ds, *channel))
        })
        .collect();
    let mut spectra: Vec<_> = sets
        .iter()
        .map(|(ds, _)| SpectrumMonitor::new(&engine, *ds))
        .collect();
    if recording {
        for (ds, _) in &sets {
            engine.start_recording(*ds, 0).expect("start IQ recording");
        }
    }
    let mut peak_age = 0.0f64;
    let mut peak_queued = 0;
    let before = engine.pipeline_health();
    let mut control = Control::start(engine.clone(), sets.clone());
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(seconds) {
        for monitor in &mut audio {
            monitor.poll();
        }
        for monitor in &mut spectra {
            monitor.poll();
        }
        let health_started = Instant::now();
        for queue in engine.pipeline_health() {
            if queue.stage == sdrmm_wire::PipelineStage::Capture {
                peak_age = peak_age.max(queue.health.oldest_ms);
                peak_queued = peak_queued.max(queue.health.queued);
            }
        }
        if health_started.elapsed() > Duration::from_millis(100) {
            eprintln!("slow health elapsed={:?}", health_started.elapsed());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let retunes = control.finish();
    eprintln!(
        "pipeline radios={} channels_per_radio={channels} seconds={seconds} mixed={} recording={recording} retunes={retunes} cpu_threads={} peak_age_ms={peak_age:.2} peak_queued={peak_queued}",
        hardware.len(),
        enabled("SDRMM_CAPTURE_MIXED"),
        number("SDRMM_CAPTURE_CPU_THREADS", 0)
    );
    let mut losses = 0;
    for queue in engine.pipeline_health() {
        let initial = before
            .iter()
            .find(|initial| {
                initial.stage == queue.stage
                    && initial.device_set == queue.device_set
                    && initial.channel == queue.channel
                    && initial.stream == queue.stream
            })
            .expect("initial queue");
        eprintln!(
            "  ds={} {:?}: {:?} setup_dropped={} measured_dropped={}",
            queue.device_set,
            queue.stage,
            queue.health,
            initial.health.dropped,
            queue.health.dropped - initial.health.dropped
        );
        losses += queue.health.dropped;
    }
    if recording {
        for (ds, _) in &sets {
            let result = engine.stop_recording(*ds).expect("finish IQ recording");
            eprintln!(
                "recording ds={ds} samples={} bytes={} overruns={} error={:?}",
                result.samples, result.bytes, result.overruns, result.error
            );
            assert!(result.error.is_none(), "recording failed");
            let mut reader =
                sdrmm_recorder::SigmfReader::open(&result.stem).expect("read recording");
            assert_eq!(reader.total_samples(), result.samples);
            assert!(result.samples > 0);
            let mut samples = [Complex::new(0.0, 0.0); 2048];
            for offset in [0, result.samples / 2, result.samples.saturating_sub(2048)] {
                reader.seek_to(offset).expect("seek recording");
                let read = reader.read_block(&mut samples).expect("read IQ");
                assert!(read > 0);
                assert!(
                    samples[..read]
                        .iter()
                        .all(|sample| sample.re.is_finite() && sample.im.is_finite())
                );
            }
        }
    }
    if history {
        history::finish(&engine, &sets, directory.path(), seconds, allow_drops);
    }
    let snapshot = engine.snapshot();
    engine.shutdown();
    for set in &snapshot.device_sets {
        eprintln!("device={} overruns={}", set.device.id(), set.overruns);
        assert_eq!(set.status, DeviceSetStatus::Running);
    }
    for monitor in &audio {
        monitor.verify(allow_drops, seconds);
    }
    for monitor in &spectra {
        monitor.verify(allow_drops);
    }
    if !allow_drops {
        assert_eq!(losses, 0, "pipeline dropped samples or frames");
    }
}

#[test]
#[ignore = "requires an idle connected radio; measures receive-only transport and DSP"]
fn connected_radio_capture_health() {
    let drivers: Vec<Box<dyn DeviceDriver>> = match std::env::var("SDRMM_CAPTURE_DRIVER").as_deref()
    {
        Ok("rtlsdr") => vec![Box::new(sdrmm_device_rtlsdr::RtlSdrDriver::new())],
        Ok("hackrf") | Err(_) => vec![Box::new(sdrmm_device_hackrf::HackRfDriver::new())],
        Ok("both") => vec![
            Box::new(sdrmm_device_rtlsdr::RtlSdrDriver::new()),
            Box::new(sdrmm_device_hackrf::HackRfDriver::new()),
        ],
        Ok(other) => panic!("unsupported test driver: {other}"),
    };
    let seconds = number("SDRMM_CAPTURE_SECONDS", 10);
    assert!(seconds > 0);
    let hardware: Vec<_> = drivers
        .into_iter()
        .map(|driver| {
            let rate = number(
                "SDRMM_CAPTURE_RATE",
                if driver.id() == "hackrf" { 20e6 } else { 2.4e6 },
            );
            let info = driver.probe().into_iter().next().expect("connected radio");
            Hardware { driver, info, rate }
        })
        .collect();
    let _awake = sdrmm_device::schedule::stay_awake("capture health test");
    let transport_seconds = number("SDRMM_CAPTURE_TRANSPORT_SECONDS", seconds.min(30));
    if transport_seconds > 0 {
        for radio in &hardware {
            measure_transport(
                radio.driver.as_ref(),
                &radio.info,
                radio.rate,
                transport_seconds,
            );
        }
    }
    measure_pipeline(&hardware, seconds);
}
