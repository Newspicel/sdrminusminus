#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use num_complex::Complex;
use sdrmm_channels::{ChannelCtx, ChannelOutputs};
use sdrmm_dsp::Ddc;
use sdrmm_wire::{ChannelSettings, DecoderEvent};
use xng_mode_vdl2::Vdl2ChannelDecoder;

const BLOCK: usize = 16_384;
const CAPTURE_RATE: f64 = 105_000.0;
const CENTER_HZ: f64 = 136_975_000.0;

fn load(file: &str) -> Option<Vec<Complex<f32>>> {
    let dir = PathBuf::from(std::env::var_os("XNG_BENCH_DATA")?);
    let bytes = std::fs::read(dir.join(file)).ok()?;
    Some(
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&[a, b, c, d]| {
                Complex::new(
                    f32::from(i16::from_le_bytes([a, b])) / 32_768.0,
                    f32::from(i16::from_le_bytes([c, d])) / 32_768.0,
                )
            })
            .collect(),
    )
}

fn decode_ours(iq: &[Complex<f32>]) -> Vec<DecoderEvent> {
    let settings = ChannelSettings {
        frequency_hz: CENTER_HZ,
        ..ChannelSettings::default_for("vdl2").unwrap()
    };
    let rate = sdrmm_channels::input_rate(&settings.params);
    let mut ddc = Ddc::new(CAPTURE_RATE, rate, settings.frequency_hz - CENTER_HZ).unwrap();
    let mut filter = sdrmm_channels::channel_filter(&settings.params).unwrap();
    let mut rx = sdrmm_channels::create(ChannelCtx { input_rate: rate }, &settings).unwrap();
    let mut narrow = Vec::new();
    let mut filtered = Vec::new();
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

fn decode_xng(iq: &[Complex<f32>]) -> Vec<String> {
    let mut decoder = Vdl2ChannelDecoder::new(CAPTURE_RATE, 0.0).unwrap();
    iq.chunks(BLOCK)
        .flat_map(|block| decoder.process(block))
        .map(|frame| hex(&frame.avlc.raw))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn compare(file: &str) -> (usize, usize) {
    let iq = load(file).expect(file);
    let ours: Vec<_> = decode_ours(&iq)
        .into_iter()
        .map(|event| match event {
            DecoderEvent::Vdl2(message) => message,
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    let xng = decode_xng(&iq);
    for raw in &xng {
        assert!(ours.iter().any(|m| m.raw.as_ref() == Some(raw)), "lost {raw}");
    }
    for extra in ours.iter().filter(|m| m.raw.as_ref().is_none_or(|r| !xng.contains(r))) {
        eprintln!("extra {} {:?} {:?}", extra.message_type, extra.station, extra.raw);
    }
    eprintln!("{file}: ours {} xng {}", ours.len(), xng.len());
    (ours.len(), xng.len())
}

#[test]
#[ignore = "needs xng bench fixtures: XNG_BENCH_DATA=dir cargo test -p sdrmm-channels --release --test xng_vdl2 -- --ignored --nocapture"]
fn vdl2_matches_xng_off_air() {
    let (ours, xng) = compare("vdl2_105k_conj.s16");
    assert!(ours >= xng && ours >= 47);
}

#[test]
#[ignore = "needs xng bench fixtures"]
fn vdl2_matches_xng_on_opflasher() {
    let (ours, xng) = compare("vdl2_opflasher_105k.cs16");
    assert!(ours >= xng && ours >= 17);
}
