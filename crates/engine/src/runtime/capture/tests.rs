use super::*;

#[test]
fn every_rate_keeps_the_same_slack_in_seconds() {
    for rate in [2_048_000.0, 2_400_000.0, 8_000_000.0, 20_000_000.0] {
        let seconds = ring_capacity(rate) as f64 / rate;
        assert!(
            seconds >= RING_SECONDS,
            "{rate} S/s left only {seconds} s of slack"
        );
    }
}

#[test]
fn a_fast_radio_is_capped_and_a_slow_one_floored() {
    assert_eq!(ring_capacity(1_000_000_000.0), RING_MAX);
    assert_eq!(ring_capacity(48_000.0), RING_MIN);
    assert_eq!(ring_capacity(2_400_000.0), 240_000);
}

#[test]
fn the_floor_holds_two_of_the_largest_blocks_a_driver_pushes_at_once() {
    let block = sdrmm_device::capture::CaptureConfig::new("ring", "ring").block_samples;
    assert!(RING_MIN >= 2 * block.max(super::super::DSP_BLOCK));
}

#[test]
fn a_recording_is_held_for_the_dsp_while_a_radio_is_skipped_past() {
    let dir = tempfile::TempDir::new().expect("scratch");
    let stem = dir.path().join("aged");
    let mut writer =
        sdrmm_recorder::SigmfWriter::create(&stem, 48_000.0, 100e6, "age policy").expect("open");
    writer
        .write_block(&[Complex::new(0.0f32, 0.0); 48_000])
        .expect("write");
    writer.finalize().expect("finalize");

    let recordings = sdrmm_device_recording::RecordingDriver::new(Some(dir.path().to_path_buf()));
    let synthetic = sdrmm_device_virtual::VirtualDriver::new();
    let open = |driver: &dyn sdrmm_device::DeviceDriver, key: &str| {
        let info = sdrmm_device::DeviceDriver::probe(driver)
            .into_iter()
            .find(|info| info.key.ends_with(key))
            .expect("probed");
        sdrmm_device::DeviceDriver::open(driver, &info).expect("open")
    };
    assert_eq!(
        max_age_for(open(&recordings, "aged").as_ref()),
        Duration::MAX
    );
    assert_eq!(
        max_age_for(open(&synthetic, "siggen").as_ref()),
        LIVE_MAX_AGE
    );
}

#[test]
fn a_rate_that_is_not_a_number_still_sizes_a_ring() {
    assert_eq!(ring_capacity(f64::NAN), RING_MIN);
    assert_eq!(ring_capacity(-1.0), RING_MIN);
    assert_eq!(ring_capacity(f64::INFINITY), RING_MAX);
}

fn open_virtual(key: &str) -> Box<dyn SdrDevice> {
    let driver = sdrmm_device_virtual::VirtualDriver::new();
    let info = sdrmm_device::DeviceDriver::probe(&driver)
        .into_iter()
        .find(|info| info.key.ends_with(key))
        .expect("probed");
    sdrmm_device::DeviceDriver::open(&driver, &info).expect("open")
}

fn started(key: &str) -> CaptureRuntime {
    let device = open_virtual(key);
    let settings = device.settings().clone();
    CaptureRuntime::start(device, &settings, false, |_| {}).expect("started")
}

fn wait_until(mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(std::time::Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn every_physical_lane_gets_a_tap_and_a_poster() {
    let mut runtime = started("quad");
    let ports = runtime.tap_ports();
    let posters = runtime.mark_posters();
    assert_eq!(ports.len(), 4);
    assert_eq!(posters.len(), 4);
    assert_eq!(ports[2].stream(), 2);
    let rate = runtime
        .device_settings()
        .and_then(|settings| settings.sample_rate)
        .expect("rate");
    let mut feed = ports[2].lease(rate, 1).expect("lease");
    let mut notes = crate::array::align_notes();
    wait_until(|| {
        feed.settle(&mut notes, 0, 0);
        feed.skippable() > 0
    });
    posters[2]
        .post(sdrmm_device::LaneMark::Retuned { in_flight: 7 })
        .expect("posted");
    wait_until(|| {
        feed.settle(&mut notes, 0, 0);
        let skip = feed.skippable();
        feed.skip(skip);
        !crate::array::noted_marks(&notes).is_empty()
    });
    assert_eq!(
        crate::array::noted_marks(&notes),
        [sdrmm_device::LaneMark::Retuned { in_flight: 7 }]
    );
    runtime.stop();
}

#[test]
fn a_virtual_lane_takes_a_free_stream_and_gives_it_back() {
    let mut runtime = started("quad");
    assert!(runtime.add_virtual_lane(1, 100e6, 48_000.0, false).is_err());
    let (mut sink, _commands) = runtime
        .add_virtual_lane(4, 100e6, 48_000.0, false)
        .expect("a free stream");
    assert_eq!(sink.stream(), 4);
    assert!(runtime.add_virtual_lane(4, 100e6, 48_000.0, false).is_err());
    assert_eq!(runtime.virtual_streams(), [4]);
    assert!(runtime.set_virtual_meta(4, 101e6, 48_000.0));
    let mut spectrum = runtime.subscribe(4).expect("a spectrum");
    let tone: Vec<Complex<f32>> = (0..4_800)
        .map(|at| Complex::from_polar(0.5, at as f32 * 0.2))
        .collect();
    wait_until(|| {
        sink.push(&tone);
        spectrum.try_recv().is_ok()
    });
    assert!(
        runtime
            .queue_health(0)
            .iter()
            .any(|queue| queue.stream == 4)
    );
    drop(runtime.remove_virtual_lane(4).expect("removed"));
    assert!(runtime.remove_virtual_lane(4).is_none());
    assert!(!runtime.set_virtual_meta(4, 100e6, 48_000.0));
    runtime.stop();
}

#[test]
fn a_single_lane_radio_gets_one_tap() {
    let runtime = started("siggen");
    assert_eq!(runtime.tap_ports().len(), 1);
    assert!(runtime.virtual_streams().is_empty());
}

#[test]
fn a_skipped_virtual_block_leaves_a_gap_not_shifted_samples() {
    let (mut sink, mut ring) = VirtualLaneSink::detached(5, RING_MIN);
    assert_eq!(sink.stream(), 5);
    sink.push(&[Complex::new(1.0, 0.0); 100]);
    sink.skip(50);
    sink.push(&[Complex::new(2.0, 0.0); 100]);
    assert_eq!(sink.next_index(), 250);
    let mut spans = Vec::new();
    while ring.consume(usize::MAX, |samples, index| {
        spans.push((index, samples.len(), samples[0].re))
    }) > 0
    {}
    assert_eq!(spans, [(0, 100, 1.0), (150, 100, 2.0)]);
}
