#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{path::Path, sync::Arc, time::Duration};

use num_complex::Complex;
use sdrmm_device::DeviceRegistry;
use sdrmm_device_recording::RecordingDriver;
use sdrmm_device_siggen::{
    LEVEL_SETTING, NOISE_OFF_DB, NOISE_SETTING, SIGNAL_SETTING, SIGNALS, SigGenDriver,
};
use sdrmm_engine::Engine;
use sdrmm_recorder::SigmfWriter;
use sdrmm_wire::{
    ChannelParams, ChannelSettings, DecodedRecord, DecoderEvent, DeviceSettings, ExtraValue,
    NfmParams, PocsagParams, Squelch,
};
use tempfile::TempDir;

const CENTER_HZ: f64 = 145_000_000.0;
const DECODE_TIMEOUT: Duration = Duration::from_secs(90);

fn engine_for(dir: &Path) -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(10, Box::new(RecordingDriver::new(Some(dir.to_path_buf()))));
    registry.register(10, Box::new(SigGenDriver::new()));
    Engine::with_registry(registry, Some(dir.to_path_buf()))
}

fn plant(dir: &Path, stem: &str, iq: &[Complex<f32>], rate: f64) -> String {
    let path = dir.join(stem);
    let mut writer = SigmfWriter::create(&path, rate, CENTER_HZ, "source fixture").unwrap();
    writer.write_block(iq).unwrap();
    writer.finalize().unwrap();
    format!("recording:{stem}")
}

fn generator(id: &str, signal: &str) -> (String, DeviceSettings) {
    (
        format!("siggen:{id}"),
        DeviceSettings {
            center_hz: Some(CENTER_HZ),
            extra: vec![
                ExtraValue {
                    name: SIGNAL_SETTING.to_string(),
                    value: serde_json::Value::String(signal.to_string()),
                },
                ExtraValue {
                    name: LEVEL_SETTING.to_string(),
                    value: serde_json::json!(-6.0),
                },
                ExtraValue {
                    name: NOISE_SETTING.to_string(),
                    value: serde_json::json!(NOISE_OFF_DB),
                },
            ],
            ..DeviceSettings::default()
        },
    )
}

async fn decode_first(
    engine: &Arc<Engine>,
    ds: u32,
    settings: ChannelSettings,
    want: impl Fn(&DecoderEvent) -> bool,
) -> DecodedRecord {
    let mut rx = engine.subscribe_decoded();
    engine.add_channel(ds, 0, settings).unwrap();
    let found = tokio::time::timeout(DECODE_TIMEOUT, async {
        loop {
            match rx.recv().await {
                Ok(record) if want(&record.event) => return record,
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("decoded stream closed")
                }
            }
        }
    })
    .await;
    engine.remove_device_set(ds).unwrap();
    found.expect("a matching decode within the timeout")
}

#[tokio::test]
async fn a_recording_node_plays_its_library_stem_into_a_decoder() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let pages = [sdrmm_channels::testgen::pocsag::Page {
        address: 1_234_567,
        function: 3,
        text: "from the library".to_owned(),
        numeric: false,
    }];
    let iq = sdrmm_channels::testgen::pocsag::transmission(&pages, 1_200, 4_500.0, 240_000.0);
    let device = plant(dir.path(), "pocsag-library", &iq, 240_000.0);

    let ds = engine.create_device_set(&device).unwrap();
    let record = decode_first(
        &engine,
        ds,
        ChannelSettings {
            frequency_hz: CENTER_HZ,
            squelch: Squelch::Off,
            params: ChannelParams::Pocsag(PocsagParams::default()),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Pocsag(_)),
    )
    .await;
    let DecoderEvent::Pocsag(page) = record.event else {
        panic!("a POCSAG page");
    };
    assert_eq!(page.address, 1_234_567);
}

#[tokio::test]
async fn a_recording_stem_that_is_not_in_the_library_is_refused() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    for stem in ["gone", "../escape", "sub/dir"] {
        assert!(
            engine
                .create_device_set(&format!("recording:{stem}"))
                .is_err(),
            "{stem} must be refused"
        );
    }
}

#[tokio::test]
async fn a_signal_generator_node_opens_once_per_node_and_decodes() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());

    let (first, settings) = generator("signal_gen-a1b2", "pocsag");
    let (second, _) = generator("signal_gen-c3d4", "nfm_ctcss");
    let one = engine
        .adopt_device(&first)
        .expect("a generator names itself");
    assert_eq!(one.id(), first);
    engine.adopt_device(&second).expect("and so does the next");

    let ds = engine.create_device_set(&first).unwrap();
    engine.patch_device(ds, settings.clone()).unwrap();
    let other = engine.create_device_set(&second).unwrap();
    assert_ne!(ds, other, "each node runs its own generator");
    engine.remove_device_set(other).unwrap();

    let record = decode_first(
        &engine,
        ds,
        ChannelSettings {
            frequency_hz: CENTER_HZ,
            squelch: Squelch::Off,
            params: ChannelParams::Pocsag(PocsagParams::default()),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Pocsag(_)),
    )
    .await;
    assert!(matches!(record.event, DecoderEvent::Pocsag(_)));
}

#[tokio::test]
async fn a_generated_signal_reaches_the_decoder_it_is_named_for() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let (device, settings) = generator("signal_gen-voice", "nfm_ctcss");
    engine.adopt_device(&device).expect("named");
    let ds = engine.create_device_set(&device).unwrap();
    engine.patch_device(ds, settings.clone()).unwrap();

    let record = decode_first(
        &engine,
        ds,
        ChannelSettings {
            frequency_hz: CENTER_HZ,
            squelch: Squelch::Off,
            params: ChannelParams::Nfm(NfmParams {
                tone_mode: sdrmm_wire::NfmToneMode::Ctcss,
                ctcss_hz: Some(88.5),
                ..NfmParams::default()
            }),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Tone(tone) if tone.ctcss_hz.is_some()),
    )
    .await;
    let DecoderEvent::Tone(tone) = record.event else {
        unreachable!("filtered above")
    };
    assert!(
        tone.ctcss_hz.is_some_and(|hz| (hz - 88.5).abs() < 2.0),
        "the generated CTCSS tone must be the one the catalog names: {tone:?}"
    );
}

#[tokio::test]
async fn every_signal_the_catalog_offers_can_be_selected_on_a_running_generator() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let (device, _) = generator("signal_gen-sweep", "tone");
    engine.adopt_device(&device).expect("named");
    let ds = engine.create_device_set(&device).unwrap();

    for signal in SIGNALS {
        let (_, settings) = generator("signal_gen-sweep", signal.id);
        engine
            .patch_device(ds, settings)
            .unwrap_or_else(|error| panic!("{} must be selectable: {error}", signal.id));
    }
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_signal_that_needs_another_rate_takes_the_whole_chain_with_it() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let (device, narrow) = generator("signal_gen-rate", "pocsag");
    engine.adopt_device(&device).expect("named");
    let ds = engine.create_device_set(&device).unwrap();
    engine.patch_device(ds, narrow).unwrap();
    engine
        .add_channel(
            ds,
            0,
            ChannelSettings {
                frequency_hz: CENTER_HZ,
                squelch: Squelch::Off,
                params: ChannelParams::Pocsag(PocsagParams::default()),
                audio: Default::default(),
            },
        )
        .unwrap();
    assert_eq!(rate_of(&engine, ds), Some(240_000.0));

    let (_, wide) = generator("signal_gen-rate", "adsb");
    engine.patch_device(ds, wide).unwrap();
    assert_eq!(
        rate_of(&engine, ds),
        Some(2_000_000.0),
        "the generator's own rate must reach the snapshot the DSP is rebuilt from"
    );

    let record = decode_first(
        &engine,
        ds,
        ChannelSettings {
            frequency_hz: CENTER_HZ,
            squelch: Squelch::Off,
            params: ChannelParams::Adsb(sdrmm_wire::AdsbParams::default()),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Adsb(_)),
    )
    .await;
    assert!(
        matches!(record.event, DecoderEvent::Adsb(_)),
        "a decoder added after the rate moved must still decode"
    );
}

fn rate_of(engine: &Engine, ds: u32) -> Option<f64> {
    engine
        .snapshot()
        .device_sets
        .iter()
        .find(|set| set.id == ds)?
        .settings
        .sample_rate
}
