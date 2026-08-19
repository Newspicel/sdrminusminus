#![allow(clippy::unwrap_used, clippy::expect_used)]

use num_complex::Complex;
use sdrmm_channels::{ChannelCtx, ChannelOutputs, ChannelRx};
use sdrmm_modem::ber::perf::measure_throughput;
use sdrmm_wire::ChannelSettings;

const BLOCK: usize = 2_048;
const SURVEY_SECONDS: f64 = 0.1;
const BATCH_ITERS: u64 = 2;
const NOISE_AMPLITUDE: f32 = 0.1;

struct Row {
    type_id: String,
    input_rate_hz: f64,
    msamples_per_s: f64,
    realtime_factor: f64,
}

fn searching_signal(rate: f64) -> Vec<Complex<f32>> {
    let len = (rate * SURVEY_SECONDS) as usize;
    let mut iq = vec![Complex::new(0.0f32, 0.0); len.max(BLOCK)];
    sdrmm_channels::testgen::add_noise(&mut iq, 0x5EED, NOISE_AMPLITUDE);
    iq
}

fn drive(rx: &mut dyn ChannelRx, iq: &[Complex<f32>], outputs: &mut ChannelOutputs) {
    for block in iq.chunks(BLOCK) {
        outputs.reset();
        rx.process(block, outputs);
    }
}

fn survey() -> Vec<Row> {
    let mut rows = Vec::new();
    for descriptor in sdrmm_channels::descriptors() {
        let Some(settings) = ChannelSettings::default_for(&descriptor.type_id) else {
            continue;
        };
        let rate = descriptor.input_rate_hz;
        let ctx = ChannelCtx { input_rate: rate };
        let Ok(mut rx) = sdrmm_channels::create(ctx, &settings) else {
            continue;
        };
        let iq = searching_signal(rate);
        let mut outputs = ChannelOutputs::default();
        drive(rx.as_mut(), &iq, &mut outputs);
        let msamples_per_s = measure_throughput(BATCH_ITERS, iq.len() as u64, || {
            drive(rx.as_mut(), &iq, &mut outputs);
        });
        rows.push(Row {
            type_id: descriptor.type_id.clone(),
            input_rate_hz: rate,
            msamples_per_s,
            realtime_factor: msamples_per_s * 1e6 / rate,
        });
    }
    rows
}

#[test]
#[ignore = "throughput survey; run in release: cargo test -p sdrmm-channels --release --test channel_perf -- --ignored --nocapture"]
fn survey_every_channel_search_path() {
    if cfg!(debug_assertions) {
        eprintln!("skipping the survey: throughput is only meaningful in release");
        return;
    }
    let mut rows = survey();
    rows.sort_by(|a, b| a.realtime_factor.total_cmp(&b.realtime_factor));
    println!(
        "{:<18} {:>12} {:>14} {:>12}",
        "channel", "rate", "Msamples/s", "realtime"
    );
    for row in &rows {
        println!(
            "{:<18} {:>10.0} k {:>14.2} {:>11.1}x",
            row.type_id,
            row.input_rate_hz / 1e3,
            row.msamples_per_s,
            row.realtime_factor
        );
    }
    assert!(
        !rows.is_empty(),
        "the registry produced no measurable channel"
    );
}
