#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use num_complex::Complex;
use sdrmm_channels::{ChannelCtx, ChannelOutputs};
use sdrmm_dsp::Ddc;
use sdrmm_wire::{ChannelSettings, DecoderEvent};

const BLOCK: usize = 16_384;
const FILE: &str = "hfdl_48k.cs16";
const RATE: f64 = 48_000.0;
const CENTER_HZ: f64 = 21_931_000.0;
const CHANNEL_HZ: f64 = 21_931_000.0;

fn load() -> Vec<Complex<f32>> {
    let dir = std::env::var_os("XNG_BENCH_DATA")
        .map(PathBuf::from)
        .unwrap();
    std::fs::read(dir.join(FILE))
        .unwrap()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&[a, b, c, d]| {
            Complex::new(
                f32::from(i16::from_le_bytes([a, b])) / 32_768.0,
                f32::from(i16::from_le_bytes([c, d])) / 32_768.0,
            )
        })
        .collect()
}

fn ours(iq: &[Complex<f32>]) -> Vec<DecoderEvent> {
    let settings = ChannelSettings {
        frequency_hz: CHANNEL_HZ,
        ..ChannelSettings::default_for("hfdl").unwrap()
    };
    let rate = sdrmm_channels::input_rate(&settings.params);
    let mut ddc = Ddc::new(RATE, rate, CHANNEL_HZ - CENTER_HZ).unwrap();
    let mut filter = sdrmm_channels::channel_filter(&settings.params).unwrap();
    let mut rx = sdrmm_channels::create(ChannelCtx { input_rate: rate }, &settings).unwrap();
    let (mut narrow, mut filtered) = (Vec::new(), Vec::new());
    let mut outputs = ChannelOutputs::default();
    let mut events = Vec::new();
    for block in iq.chunks(BLOCK) {
        narrow.clear();
        ddc.process(block, &mut narrow);
        filter.process(&narrow, &mut filtered);
        outputs.reset();
        rx.process(&filtered, &mut outputs);
        events.append(&mut outputs.events);
    }
    events
}

fn xng(iq: &[Complex<f32>]) -> usize {
    let mut decoder = xng_mode_hfdl::HfdlChannelDecoder::new(RATE, CHANNEL_HZ - CENTER_HZ).unwrap();
    iq.chunks(BLOCK)
        .map(|block| decoder.process(block).len())
        .sum()
}

#[test]
#[ignore = "needs xng bench fixtures: XNG_BENCH_DATA=dir cargo test -p sdrmm-channels --release --test xng_hfdl -- --ignored --nocapture"]
fn hfdl_beats_xng_off_air() {
    let iq = load();
    let events = ours(&iq);
    let reference = xng(&iq);
    let valid = events
        .iter()
        .filter(|e| matches!(e, DecoderEvent::Hfdl(m) if m.crc_ok))
        .count();
    eprintln!(
        "hfdl_offair: ours {} ({valid} crc ok), xng {reference}",
        events.len()
    );
    assert!(valid == events.len() && valid >= reference && valid >= 36);
}
