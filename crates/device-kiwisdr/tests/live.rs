#![allow(clippy::expect_used)]
use std::{
    f64::consts::TAU,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use num_complex::Complex;
use sdrmm_device::{DeviceDriver, DeviceError, RxSink, Sample, SdrDevice, lock};
use sdrmm_device_kiwisdr::KiwiSdrDriver;
use sdrmm_wire::DeviceSettings;

const ENV: &str = "KIWISDR_LIVE";
const SWEEP_ENV: &str = "KIWISDR_SWEEP";
const CARRIER_HZ: f64 = 77_500.0;
const OFFSET_HZ: f64 = 1_000.0;
const LISTEN: Duration = Duration::from_secs(4);

fn power(samples: &[Sample], rate: f64, hz: f64) -> f64 {
    let sum: Complex<f64> = samples
        .iter()
        .enumerate()
        .map(|(n, s)| {
            let phase = -TAU * hz * n as f64 / rate;
            Complex::new(f64::from(s.re), f64::from(s.im)) * Complex::from_polar(1.0, phase)
        })
        .sum();
    sum.norm_sqr() / samples.len() as f64
}

fn listen(device: &mut Box<dyn SdrDevice>) -> Result<(Vec<Sample>, f64), DeviceError> {
    let heard = Arc::new(Mutex::new(Vec::new()));
    let into = heard.clone();
    device.rx_start(vec![RxSink::new(move |samples, _| {
        lock(&into).extend_from_slice(samples)
    })])?;
    let started = Instant::now();
    std::thread::sleep(LISTEN);
    device.rx_stop();
    let samples = lock(&heard).clone();
    let per_second = samples.len() as f64 / started.elapsed().as_secs_f64();
    Ok((samples, per_second))
}

fn visit(host: &str) -> Result<String, String> {
    let driver = KiwiSdrDriver::new();
    let info = driver.resolve(host).ok_or("not addressable")?;
    let mut device = match driver.open(&info) {
        Ok(device) => device,
        Err(DeviceError::InUse(why) | DeviceError::PermissionDenied(why)) => {
            return Ok(format!("refused: {why}"));
        }
        Err(e) => return Err(format!("open: {e}")),
    };
    let rate = device.settings().sample_rate.ok_or("no rate")?;
    let band = device.capabilities().freq_ranges[0];
    let (samples, per_second) = listen(&mut device).map_err(|e| format!("stream: {e}"))?;
    let rms = (samples.iter().map(|s| f64::from(s.norm_sqr())).sum::<f64>()
        / samples.len().max(1) as f64)
        .sqrt();
    let summary = format!(
        "rate {rate:.3} got {per_second:.0}/s band {}..{} MHz rms {rms:.4}",
        band.min / 1e6,
        band.max / 1e6
    );
    if per_second < rate * 0.6 || rms == 0.0 {
        return Err(summary);
    }
    Ok(summary)
}

#[test]
#[ignore = "needs public KiwiSDRs; set KIWISDR_SWEEP=host:port,host:port"]
fn every_listed_kiwi_opens_and_streams() {
    let hosts = std::env::var(SWEEP_ENV).expect("KIWISDR_SWEEP=host:port,...");
    let visits: Vec<_> = hosts
        .split(',')
        .map(|host| {
            let host = host.trim().to_string();
            std::thread::spawn(move || {
                let outcome = visit(&host);
                (host, outcome)
            })
        })
        .collect();
    let outcomes: Vec<_> = visits
        .into_iter()
        .map(|visit| visit.join().expect("visited"))
        .collect();
    for (host, outcome) in &outcomes {
        eprintln!("{host}: {outcome:?}");
    }
    let streamed = outcomes
        .iter()
        .filter(|(_, o)| o.as_ref().is_ok_and(|s| s.starts_with("rate")))
        .count();
    let failed: Vec<_> = outcomes.iter().filter(|(_, o)| o.is_err()).collect();
    assert!(streamed > 0, "no Kiwi streamed");
    assert!(failed.is_empty(), "{failed:?}");
}

#[test]
#[ignore = "needs a public KiwiSDR in Europe; set KIWISDR_LIVE=host:port"]
fn a_public_kiwi_hears_dcf77_above_the_tuned_center() {
    let host = std::env::var(ENV).expect("KIWISDR_LIVE=host:port");
    let driver = KiwiSdrDriver::new();
    let info = driver.resolve(&host).expect("addressable");
    let mut device = driver.open(&info).expect("opens");
    let rate = device.settings().sample_rate.expect("a rate");
    device
        .apply(&DeviceSettings {
            center_hz: Some(CARRIER_HZ - OFFSET_HZ),
            ..DeviceSettings::default()
        })
        .expect("tunes");

    let (samples, got_rate) = listen(&mut device).expect("streams");
    let settled = &samples[samples.len() / 4..];
    let above = power(settled, rate, OFFSET_HZ);
    let mirror = power(settled, rate, -OFFSET_HZ);
    let noise = power(settled, rate, 2_345.0);
    eprintln!(
        "rate {rate} got {got_rate:.0}/s above {above:.3e} mirror {mirror:.3e} noise {noise:.3e}"
    );
    assert!(got_rate > rate * 0.6, "only {got_rate:.0} samples/s");
    assert!(above > 100.0 * mirror, "carrier on the wrong side");
    assert!(above > 100.0 * noise, "no carrier");
}
