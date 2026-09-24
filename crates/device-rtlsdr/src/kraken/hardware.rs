use std::time::{Duration, Instant};

use super::*;

const FREQUENCIES_HZ: [u32; 9] = [
    30_000_000,
    100_000_000,
    433_920_000,
    700_000_000,
    868_000_000,
    1_000_000_000,
    1_090_000_000,
    1_300_000_000,
    1_700_000_000,
];

fn clipped_fraction(sdr: &mut RtlSdr) -> f64 {
    let stream = sdr.start_streaming().expect("stream");
    let started = Instant::now();
    let (mut clipped, mut total) = (0u64, 0u64);
    while started.elapsed() < Duration::from_millis(300) {
        if let Ok(block) = stream.recv_timeout(Duration::from_millis(100)) {
            if started.elapsed() < Duration::from_millis(100) {
                continue;
            }
            clipped += block.iter().filter(|byte| matches!(byte, 0 | 255)).count() as u64;
            total += block.len() as u64;
        }
    }
    clipped as f64 / total.max(1) as f64
}

#[test]
#[ignore = "requires an idle KrakenSDR; prints how hard its noise source drives each gain"]
fn connected_kraken_noise_source_levels() {
    let descriptors = DeviceDescriptors::new().expect("enumerate");
    let listed: Vec<_> = descriptors.iter().cloned().collect();
    let unit = unit::units(&listed)
        .into_iter()
        .next()
        .expect("a KrakenSDR");
    let mut control = descriptors.open(unit.members[0]).expect("open lane 0");
    control.set_sample_rate(2_400_000).expect("rate");
    control
        .set_gpio(apply::NOISE_SOURCE_PIN, true)
        .expect("noise on");
    for freq in FREQUENCIES_HZ {
        control.set_center_freq(freq).expect("tune");
        let mut row = Vec::new();
        for &tenths in crate::driver::GAIN_VALUES.iter().take(19) {
            control.set_gain_manual(tenths).expect("gain");
            row.push(format!(
                "{:.1}dB:{:.4}",
                f64::from(tenths) / 10.0,
                clipped_fraction(&mut control)
            ));
        }
        println!("{} MHz {}", freq / 1_000_000, row.join(" "));
    }
    control
        .set_gpio(apply::NOISE_SOURCE_PIN, false)
        .expect("noise off");
}

fn lane_samples(sdr: &mut RtlSdr, len: usize) -> sdrmm_usb_stream::RxStream {
    let _ = len;
    sdr.start_streaming().expect("stream")
}

fn collect(stream: &sdrmm_usb_stream::RxStream, len: usize) -> Vec<num_complex::Complex<f32>> {
    use sdrmm_device::SampleConverter;
    let mut converter = crate::convert::converter();
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        if let Ok(block) = stream.recv_timeout(Duration::from_millis(200)) {
            out.extend_from_slice(converter.convert(&block));
        }
    }
    out.truncate(len);
    out
}

#[test]
#[ignore = "requires an idle KrakenSDR; prints how well two lanes agree on the noise source per gain"]
fn connected_kraken_noise_source_coherence() {
    const FRAME: usize = 32_768;
    let descriptors = DeviceDescriptors::new().expect("enumerate");
    let listed: Vec<_> = descriptors.iter().cloned().collect();
    let unit = unit::units(&listed)
        .into_iter()
        .next()
        .expect("a KrakenSDR");
    let mut lanes: Vec<RtlSdr> = unit.members[..2]
        .iter()
        .map(|member| descriptors.open(*member).expect("open lane"))
        .collect();
    for lane in &mut lanes {
        lane.set_sample_rate(2_400_000).expect("rate");
        lane.set_dither(false).expect("dither");
    }
    lanes[0]
        .set_gpio(apply::NOISE_SOURCE_PIN, true)
        .expect("noise on");
    let mut xcorr = sdrmm_dsp::xcorr::XCorr::new(FRAME);
    for freq in [
        100_000_000u32,
        433_920_000,
        868_000_000,
        1_090_000_000,
        1_600_000_000,
    ] {
        let mut row = Vec::new();
        for tenths in [0, 9, 14, 27, 37, 77, 125, 166, 207, 297] {
            for lane in &mut lanes {
                lane.set_center_freq(freq).expect("tune");
                lane.set_gain_manual(tenths).expect("gain");
            }
            let streams: Vec<_> = lanes
                .iter_mut()
                .map(|lane| lane_samples(lane, FRAME))
                .collect();
            std::thread::sleep(Duration::from_millis(100));
            let a = collect(&streams[0], 2 * FRAME);
            let b = collect(&streams[1], 2 * FRAME);
            let estimate = xcorr.estimate(&a[FRAME..], &b[FRAME..]);
            row.push(format!(
                "{:.1}dB:c{:.2}/p{:.0}",
                f64::from(tenths) / 10.0,
                estimate.coherence,
                estimate.peak_to_floor_db
            ));
        }
        println!("{} MHz {}", freq / 1_000_000, row.join(" "));
    }
    lanes[0]
        .set_gpio(apply::NOISE_SOURCE_PIN, false)
        .expect("noise off");
}
