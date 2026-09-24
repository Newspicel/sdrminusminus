#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use num_complex::Complex;
use sdrmm_channels::{ChannelCtx, ChannelOutputs};
use sdrmm_dsp::Ddc;
use sdrmm_wire::{AisChannel, AisParams, ChannelParams, ChannelSettings, DecoderEvent};

const BLOCK: usize = 16_384;

#[derive(Clone, Copy)]
enum Format {
    Cu8,
    Cs16,
}

struct Capture {
    file: &'static str,
    format: Format,
    rate: f64,
    center_hz: f64,
}

fn fixture_dir() -> Option<PathBuf> {
    std::env::var_os("XNG_BENCH_DATA").map(PathBuf::from)
}

fn load(capture: &Capture) -> Option<Vec<Complex<f32>>> {
    let bytes = std::fs::read(fixture_dir()?.join(capture.file)).ok()?;
    Some(match capture.format {
        Format::Cu8 => bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&[i, q]| {
                Complex::new(
                    (f32::from(i) - 127.5) / 127.5,
                    (f32::from(q) - 127.5) / 127.5,
                )
            })
            .collect(),
        Format::Cs16 => bytes
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
    })
}

fn decode(iq: &[Complex<f32>], capture: &Capture, settings: &ChannelSettings) -> Vec<DecoderEvent> {
    let rate = sdrmm_channels::input_rate(&settings.params);
    let mut ddc = Ddc::new(
        capture.rate,
        rate,
        settings.frequency_hz - capture.center_hz,
    )
    .unwrap();
    let mut filter = sdrmm_channels::channel_filter(&settings.params).unwrap();
    let mut rx = sdrmm_channels::create(ChannelCtx { input_rate: rate }, settings).unwrap();
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

fn tuned(type_id: &str, frequency_hz: f64) -> ChannelSettings {
    ChannelSettings {
        frequency_hz,
        ..ChannelSettings::default_for(type_id).unwrap()
    }
}

fn unique_adsb(events: &[DecoderEvent]) -> usize {
    events
        .iter()
        .filter_map(|e| match e {
            DecoderEvent::Adsb(m) => Some(m.raw.as_str()),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}

fn capture(file: &'static str, format: Format, rate: f64, center_hz: f64) -> Capture {
    Capture {
        file,
        format,
        rate,
        center_hz,
    }
}

#[test]
#[ignore = "needs xng bench fixtures: XNG_BENCH_DATA=dir cargo test -p sdrmm-channels --release --test xng_fixtures -- --ignored --nocapture"]
fn adsb_beats_xng_on_modes1() {
    let modes1 = capture("modes1.cu8", Format::Cu8, 2_000_000.0, 1_090_000_000.0);
    let iq = load(&modes1).expect("modes1.cu8");
    let events = decode(&iq, &modes1, &tuned("adsb", 1_090_000_000.0));
    let unique = unique_adsb(&events);
    eprintln!("adsb_modes1: {} frames, {unique} unique", events.len());
    assert!(events.len() > 323 && unique > 161);
}

#[test]
#[ignore = "needs xng bench fixtures"]
fn adsb_stays_silent_on_quiet_air() {
    let quiet = capture(
        "adsb_quiet_24m.cu8",
        Format::Cu8,
        2_400_000.0,
        1_090_000_000.0,
    );
    let iq = load(&quiet).expect("adsb_quiet_24m.cu8");
    let count = decode(&iq, &quiet, &tuned("adsb", 1_090_000_000.0)).len();
    eprintln!("adsb_quiet: {count}");
    assert!(count <= 1);
}

#[test]
#[ignore = "needs xng bench fixtures"]
fn ais_beats_xng_off_air() {
    let ais = capture("ais_96k.cs16", Format::Cs16, 96_000.0, 162_000_000.0);
    let iq = load(&ais).expect("ais_96k.cs16");
    let count: usize = [
        (AisChannel::A, 161_975_000.0),
        (AisChannel::B, 162_025_000.0),
    ]
    .into_iter()
    .map(|(ais_channel, frequency_hz)| {
        let settings = ChannelSettings {
            params: ChannelParams::Ais(AisParams { ais_channel }),
            ..tuned("ais", frequency_hz)
        };
        decode(&iq, &ais, &settings).len()
    })
    .sum();
    eprintln!("ais_offair: {count}");
    assert!(count > 72);
}

#[test]
#[ignore = "needs xng bench fixtures"]
fn navtex_matches_the_uscg_message() {
    let navtex = capture("navtex_62500.cs16", Format::Cs16, 62_500.0, 516_000.0);
    let iq = load(&navtex).expect("navtex_62500.cs16");
    let events = decode(&iq, &navtex, &tuned("navtex", 518_000.0));
    let [DecoderEvent::Navtex(m)] = &events[..] else {
        panic!("one message expected: {events:?}");
    };
    assert!(m.complete && m.text.ends_with("2. CANCEL AT TIME//100400Z SEP 20//"));
}

#[test]
#[ignore = "needs xng bench fixtures"]
fn acars_beats_xng_off_air() {
    let acars = capture("acars_100k.cs16", Format::Cs16, 100_000.0, 131_550_000.0);
    let iq = load(&acars).expect("acars_100k.cs16");
    let count = decode(&iq, &acars, &tuned("acars", 131_550_000.0)).len();
    eprintln!("acars_offair: {count}");
    assert!(count > 15);
}
