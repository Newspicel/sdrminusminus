#![allow(clippy::unwrap_used, clippy::expect_used)]

use num_complex::Complex;
use sdrmm_channels::{
    ChannelCtx, ChannelOutputs, Dvbs2Modulation as Modulation, Dvbs2Rate as Rate, testgen,
};
use sdrmm_modem::ber::perf::measure_throughput;
use sdrmm_wire::{
    ChannelParams, ChannelSettings, DatvCodeRate, DatvParams, DatvStandard, DecoderEvent,
};

const BLOCK: usize = 2_048;
const INPUT_RATE_HZ: f64 = 2_000_000.0;
const SECONDS: usize = 1;

struct Row {
    mode: &'static str,
    msamples_per_s: f64,
    realtime_factor: f64,
    frames_ok: u64,
}

fn settings(standard: DatvStandard) -> ChannelSettings {
    ChannelSettings {
        offset_hz: 0.0,
        squelch_db: None,
        squelch_auto_db: None,
        params: ChannelParams::Datv(DatvParams {
            standard,
            symbol_rate: testgen::datv::SYMBOL_RATE,
            code_rate: DatvCodeRate::ThreeQuarters,
            program: None,
            input_stream: None,
        }),
        audio: Default::default(),
    }
}

fn drive(
    rx: &mut dyn sdrmm_channels::ChannelRx,
    iq: &[Complex<f32>],
    outputs: &mut ChannelOutputs,
) -> u64 {
    let mut frames_ok = 0;
    for block in iq.chunks(BLOCK) {
        outputs.reset();
        rx.process(block, outputs);
        for event in &outputs.events {
            if let DecoderEvent::Broadcast(status) = event {
                frames_ok = frames_ok.max(u64::from(status.frames_ok));
            }
        }
    }
    frames_ok
}

fn measure(mode: &'static str, standard: DatvStandard, iq: &[Complex<f32>]) -> Row {
    let ctx = ChannelCtx {
        input_rate: INPUT_RATE_HZ,
    };
    let mut rx = sdrmm_channels::create(ctx, &settings(standard)).expect("a DATV receiver");
    let mut outputs = ChannelOutputs::default();
    let frames_ok = drive(rx.as_mut(), iq, &mut outputs);
    assert!(
        frames_ok > 0,
        "{mode}: the receiver never locked, so this would time the search path"
    );
    let msamples_per_s = measure_throughput(1, iq.len() as u64, || {
        drive(rx.as_mut(), iq, &mut outputs);
    });
    Row {
        mode,
        msamples_per_s,
        realtime_factor: msamples_per_s * 1e6 / INPUT_RATE_HZ,
        frames_ok,
    }
}

#[test]
#[ignore = "throughput survey; run in release: cargo test -p sdrmm-channels --release --test datv_perf -- --ignored --nocapture"]
fn dvb_s2_modes_against_the_realtime_budget() {
    if cfg!(debug_assertions) {
        eprintln!("skipping the survey: throughput is only meaningful in release");
        return;
    }
    let rows = vec![
        measure(
            "dvbs_qpsk_3/4",
            DatvStandard::DvbS,
            &testgen::datv::dvbs(SECONDS),
        ),
        measure(
            "dvbs2_qpsk_3/4_short",
            DatvStandard::DvbS2,
            &testgen::datv::dvbs2_mode(SECONDS, Modulation::Qpsk, Rate::R3_4, true, false),
        ),
        measure(
            "dvbs2_qpsk_3/4_normal",
            DatvStandard::DvbS2,
            &testgen::datv::dvbs2_mode(SECONDS, Modulation::Qpsk, Rate::R3_4, false, true),
        ),
        measure(
            "dvbs2_qpsk_1/4_normal",
            DatvStandard::DvbS2,
            &testgen::datv::dvbs2_mode(SECONDS, Modulation::Qpsk, Rate::R1_4, false, true),
        ),
        measure(
            "dvbs2_8psk_3/4_normal",
            DatvStandard::DvbS2,
            &testgen::datv::dvbs2_mode(SECONDS, Modulation::Psk8, Rate::R3_4, false, true),
        ),
        measure(
            "dvbs2_16apsk_3/4_normal",
            DatvStandard::DvbS2,
            &testgen::datv::dvbs2_mode(SECONDS, Modulation::Apsk16, Rate::R3_4, false, true),
        ),
        measure(
            "dvbs2_32apsk_5/6_normal",
            DatvStandard::DvbS2,
            &testgen::datv::dvbs2_mode(SECONDS, Modulation::Apsk32, Rate::R5_6, false, true),
        ),
    ];
    println!(
        "{:<26} {:>12} {:>11} {:>10}",
        "mode", "Msamples/s", "realtime", "frames"
    );
    for row in &rows {
        println!(
            "{:<26} {:>12.2} {:>10.2}x {:>10}",
            row.mode, row.msamples_per_s, row.realtime_factor, row.frames_ok
        );
    }
}
