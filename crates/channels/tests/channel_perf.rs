#![allow(clippy::unwrap_used, clippy::expect_used)]

use num_complex::Complex;
use sdrmm_channels::{ChannelCtx, ChannelOutputs, ChannelRx};
use sdrmm_modem_test_support::ber::perf::{
    CALIBRATION_BENCH, PerfBaseline, compare_perf, host_id, load_baselines, save_baselines,
};
use sdrmm_test_support::{CountingAlloc, assert_no_alloc, measure_throughput};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();
use sdrmm_wire::ChannelSettings;

const BLOCK: usize = 2_048;
const SURVEY_SECONDS: f64 = 0.1;
const BATCH_ITERS: u64 = 20;
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
        let settings = ChannelSettings::default_for(&descriptor.type_id)
            .expect("every registered channel has defaults");
        let rate = descriptor.input_rate_hz;
        let ctx = ChannelCtx { input_rate: rate };
        let mut rx = sdrmm_channels::create(ctx, &settings)
            .unwrap_or_else(|error| panic!("{} cannot be measured: {error}", descriptor.type_id));
        let seconds = match descriptor.type_id.as_str() {
            "ft8" => 15.0,
            "ft4" => 7.5,
            "wspr" => 120.0,
            _ => SURVEY_SECONDS,
        };
        let mut iq = vec![Complex::new(0.0, 0.0); ((rate * seconds) as usize).max(BLOCK)];
        sdrmm_channels::testgen::add_noise(&mut iq, 0x5EED, NOISE_AMPLITUDE);
        let mut outputs = ChannelOutputs::default();
        drive(rx.as_mut(), &iq, &mut outputs);
        let msamples_per_s = measure_throughput(
            if seconds > 1.0 { 1 } else { BATCH_ITERS },
            iq.len() as u64,
            || {
                drive(rx.as_mut(), &iq, &mut outputs);
            },
        );
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
    assert_eq!(rows.len(), sdrmm_channels::descriptors().len());
    let committed =
        load_baselines(&baseline_path()).expect("committed channel throughput baselines");
    let measured = baselines(&rows);
    assert_eq!(
        committed
            .iter()
            .filter(|row| row.bench != CALIBRATION_BENCH)
            .count(),
        measured.len(),
        "registry changed: measure and review its performance budgets"
    );
    let changes = compare_perf(&measured, &committed, 0.35).unwrap_or_else(|regressions| {
        panic!(
            "channel throughput regressed more than 35% after host calibration: {regressions:#?}"
        )
    });
    for change in changes {
        println!(
            "{}: {:+.1}% calibrated",
            change.bench,
            change.change_fraction * 100.0
        );
    }
}

fn baseline_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("baselines/channel_perf.json")
}

fn baselines(rows: &[Row]) -> Vec<PerfBaseline> {
    rows.iter()
        .map(|row| PerfBaseline {
            bench: row.type_id.clone(),
            msamples_per_s: row.msamples_per_s,
            realtime_factor: row.realtime_factor,
            config: format!(
                "noise including deferred decode slots, {:.0} Hz, {BLOCK} sample blocks",
                row.input_rate_hz
            ),
            host: host_id(),
        })
        .collect()
}

#[test]
#[ignore = "rewrites the committed baseline; run explicitly in release"]
fn write_perf_baseline() {
    if cfg!(debug_assertions) {
        panic!("measure baselines in release");
    }
    let path = baseline_path();
    std::fs::create_dir_all(path.parent().expect("baseline directory")).expect("directory");
    save_baselines(&path, &baselines(&survey())).expect("write baseline");
}

#[test]
fn analog_channels_allocate_nothing_after_warmup_and_exceed_realtime() {
    for type_id in ["am", "nfm", "wfm", "ssb"] {
        let settings = ChannelSettings::default_for(type_id).expect("settings");
        let descriptor = sdrmm_channels::descriptors()
            .into_iter()
            .find(|descriptor| descriptor.type_id == type_id)
            .expect("descriptor");
        let rate = descriptor.input_rate_hz;
        let mut rx =
            sdrmm_channels::create(ChannelCtx { input_rate: rate }, &settings).expect("receiver");
        let iq = searching_signal(rate);
        let mut outputs = ChannelOutputs::default();
        for _ in 0..4 {
            drive(rx.as_mut(), &iq, &mut outputs);
        }
        assert_no_alloc(type_id, || drive(rx.as_mut(), &iq, &mut outputs));
        let msps = measure_throughput(2, iq.len() as u64, || drive(rx.as_mut(), &iq, &mut outputs));
        assert!(
            msps * 1e6 / rate >= 2.0,
            "{type_id} fell below twice realtime: {msps} Msamples/s"
        );
    }
}
