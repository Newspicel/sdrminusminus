#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{path::Path, sync::Arc, time::Duration};

use num_complex::Complex;
use sdrmm_channels::{AprsTx, ChannelCtx, ChannelTx, MicE, MicEBit, TxPayload, testgen};
use sdrmm_device::DeviceRegistry;
use sdrmm_device_virtual::VirtualDriver;
use sdrmm_engine::Engine;
use sdrmm_recorder::SigmfWriter;
use sdrmm_wire::{
    AcarsParams, AdsbParams, AisChannel, AisParams, AprsMode, AprsParams, BroadcastSystem,
    ChannelParams, ChannelSettings, CwSkimmerParams, DabParams, DatvParams, DatvStandard,
    DecodedRecord, DecoderEvent, DrmMode, DrmParams, DvFrameKind, DvMode, ErmesParams, FlexParams,
    FreeDvParams, GnssParams, IdentParams, Modulation, MorseParams, NavtexParams, NfmParams,
    NfmToneMode, PocsagBaud, PocsagParams, PskParams, RdsUpdate, RttyParams, SelcallParams,
    SelcallSystem, SubghzEncoding, SubghzParams, VorParams, WfmParams, WsjtParams, WsprParams,
    YsfParams,
};
use tempfile::TempDir;

const DECODE_TIMEOUT: Duration = Duration::from_secs(30);

const NARROW_DEVICE_RATE: f64 = 240_000.0;
const AUDIO_DEVICE_RATE: f64 = 48_000.0;
const ADSB_DEVICE_RATE: f64 = 2_000_000.0;
const GNSS_DEVICE_RATE: f64 = 2_048_000.0;
const CENTER_HZ: f64 = 145_000_000.0;
fn aprs_burst(frame: Vec<u8>) -> Vec<Complex<f32>> {
    let mut tx = AprsTx::new(
        ChannelCtx {
            input_rate: AprsTx::descriptor().input_rate_hz,
        },
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Aprs(AprsParams {
                mode: AprsMode::Afsk1200,
                ..AprsParams::default()
            }),
            audio: Default::default(),
        },
    )
    .unwrap();
    tx.submit(TxPayload::Frame(frame)).unwrap();
    testgen::burst(&mut tx)
}

fn engine_for(dir: &Path) -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(
        10,
        Box::new(VirtualDriver::with_recordings(dir.to_path_buf())),
    );
    Engine::with_registry(registry, Some(dir.to_path_buf()))
}

fn accelerated_engine_for(dir: &Path) -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(
        10,
        Box::new(VirtualDriver::with_accelerated_recordings(
            dir.to_path_buf(),
            20.0,
        )),
    );
    Engine::with_registry(registry, Some(dir.to_path_buf()))
}

fn plant(dir: &Path, stem: &str, mut iq: Vec<Complex<f32>>, rate: f64) -> String {
    let min_len = rate as usize;
    if iq.len() < min_len {
        iq.extend(testgen::silence(min_len - iq.len()));
    }
    let path = dir.join(stem);
    let mut writer = SigmfWriter::create(&path, rate, CENTER_HZ, "decoder fixture").unwrap();
    writer.write_block(&iq).unwrap();
    writer.finalize().unwrap();
    format!("virtual:file:{}", path.display())
}

async fn decode_first(
    engine: &Arc<Engine>,
    device_id: &str,
    settings: ChannelSettings,
    want: impl Fn(&DecoderEvent) -> bool,
) -> DecodedRecord {
    let offset_hz = settings.offset_hz;
    let mut rx = engine.subscribe_decoded();
    let ds = engine.create_device_set(device_id).unwrap();
    let ch = engine.add_channel(ds, 0, settings).unwrap();

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
    let record = found.expect("a matching decode within the timeout");

    assert_eq!(record.device_set, ds, "record names its device set");
    assert_eq!(record.channel, ch, "record names its channel");
    assert_eq!(
        record.freq_hz,
        CENTER_HZ + offset_hz,
        "record carries the absolute frequency the channel was tuned to"
    );
    assert!(
        record.at.ends_with('Z') && record.at.len() == "2026-08-09T12:00:00.000000000Z".len(),
        "unexpected timestamp format: {}",
        record.at
    );
    record
}

#[tokio::test]
async fn pocsag_page_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let offset_hz = 50_000.0;

    let pages = [testgen::pocsag::Page {
        address: 1_234_567,
        function: 3,
        text: "ENGINE E2E".to_owned(),
        numeric: false,
    }];
    let mut iq = testgen::pocsag::transmission(&pages, 1_200, 4_500.0, NARROW_DEVICE_RATE);
    testgen::shift(&mut iq, offset_hz, NARROW_DEVICE_RATE);

    let device = plant(dir.path(), "pocsag", iq, NARROW_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Pocsag(PocsagParams {
                baud: PocsagBaud::Auto,
                ..PocsagParams::default()
            }),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Pocsag(_)),
    )
    .await;

    let DecoderEvent::Pocsag(page) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(page.address, 1_234_567);
    assert_eq!(page.function, 3);
    assert_eq!(page.baud, 1_200);
    assert_eq!(page.text, "ENGINE E2E");
}

#[tokio::test]
async fn flex_page_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let offset_hz = 35_000.0;
    let page = testgen::flex::Page {
        address: 345_678,
        text: "FLEX ENGINE E2E".to_owned(),
    };
    let mut iq = testgen::flex::transmission(&page, 4, 72, NARROW_DEVICE_RATE);
    testgen::shift(&mut iq, offset_hz, NARROW_DEVICE_RATE);
    let device = plant(dir.path(), "flex", iq, NARROW_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Flex(FlexParams::default()),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Flex(_)),
    )
    .await;
    let DecoderEvent::Flex(message) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(message.address, 345_678);
    assert_eq!(message.text, "FLEX ENGINE E2E");
    assert_eq!((message.cycle, message.frame), (4, 72));
}

#[tokio::test]
async fn ermes_page_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let offset_hz = -30_000.0;
    let page = testgen::ermes::Page {
        local_address: 456_789,
        message_number: 6,
        text: "ERMES ENGINE E2E".to_owned(),
        urgent: true,
        alert: 4,
    };
    let mut iq = testgen::ermes::transmission(&page, NARROW_DEVICE_RATE);
    testgen::shift(&mut iq, offset_hz, NARROW_DEVICE_RATE);
    let device = plant(dir.path(), "ermes", iq, NARROW_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Ermes(ErmesParams::default()),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Ermes(_)),
    )
    .await;
    let DecoderEvent::Ermes(message) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(message.local_address, 456_789);
    assert_eq!(message.text, "ERMES ENGINE E2E");
    assert!(message.urgent);
    assert_eq!(message.alert, 4);
}

#[tokio::test]
async fn aprs_packet_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let offset_hz = -40_000.0;

    let frame = AprsTx::ui_frame(
        "DL1ABC-9",
        "APRS",
        &["WIDE1-1"],
        "!5230.00N/01324.00E>engine e2e",
    );
    let mut iq = testgen::resample(
        &aprs_burst(frame),
        AprsTx::descriptor().input_rate_hz,
        NARROW_DEVICE_RATE,
    );
    testgen::shift(&mut iq, offset_hz, NARROW_DEVICE_RATE);

    let device = plant(dir.path(), "aprs", iq, NARROW_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Aprs(AprsParams {
                mode: AprsMode::Afsk1200,
                ..AprsParams::default()
            }),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Aprs(_)),
    )
    .await;

    let DecoderEvent::Aprs(packet) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(packet.source, "DL1ABC-9");
    assert_eq!(packet.destination, "APRS");
    assert_eq!(packet.path, ["WIDE1-1"]);
    let (lat, lon) = (packet.lat.unwrap(), packet.lon.unwrap());
    assert!((lat - 52.5).abs() < 1e-3, "lat {lat}");
    assert!((lon - 13.4).abs() < 1e-3, "lon {lon}");
}

#[tokio::test]
async fn ais_position_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let offset_hz = 25_000.0;

    let report = testgen::ais::PositionReport {
        mmsi: 211_234_560,
        lat: 53.5413,
        lon: 9.9846,
        sog_kt: 12.3,
        cog_deg: 178.4,
        heading_deg: 179,
        nav_status: 0,
    };
    let mut iq = testgen::ais::burst(&testgen::ais::position_payload(&report), NARROW_DEVICE_RATE);
    testgen::shift(&mut iq, offset_hz, NARROW_DEVICE_RATE);

    let device = plant(dir.path(), "ais", iq, NARROW_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Ais(AisParams {
                ais_channel: AisChannel::B,
            }),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Ais(_)),
    )
    .await;

    let DecoderEvent::Ais(message) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(message.mmsi, 211_234_560);
    assert_eq!(message.msg_type, 1);
    assert_eq!(message.ais_channel, 'B');
    let (lat, lon) = (message.lat.unwrap(), message.lon.unwrap());
    assert!((lat - 53.5413).abs() < 1e-3, "lat {lat}");
    assert!((lon - 9.9846).abs() < 1e-3, "lon {lon}");
    assert!(
        message.nmea.starts_with("!AIVDM"),
        "interop sentence missing: {}",
        message.nmea
    );
}

#[tokio::test]
async fn a_mic_e_packet_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let offset_hz = 40_000.0;

    let report = MicE {
        lat: 52.5,
        lon: 13.4,
        speed_kt: 42,
        course_deg: 251,
        symbol: "/j",
        bits: [MicEBit::Standard; 3],
        ..MicE::default()
    };
    let frame = AprsTx::ui_frame(
        "DL1ABC-7",
        &report.destination(),
        &["WIDE2-2"],
        &report.info(),
    );
    let mut iq = testgen::resample(
        &aprs_burst(frame),
        AprsTx::descriptor().input_rate_hz,
        NARROW_DEVICE_RATE,
    );
    testgen::shift(&mut iq, offset_hz, NARROW_DEVICE_RATE);

    let device = plant(dir.path(), "mice", iq, NARROW_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Aprs(AprsParams::default()),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Aprs(p) if p.mic_e_message.is_some()),
    )
    .await;

    let DecoderEvent::Aprs(packet) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(packet.source, "DL1ABC-7");
    assert_eq!(packet.mic_e_message.as_deref(), Some("Off Duty"));
    assert_eq!(packet.speed_kt, Some(42.0));
    assert_eq!(packet.course_deg, Some(251.0));
    assert_eq!(packet.symbol.as_deref(), Some("/j"));
    let (lat, lon) = (packet.lat.unwrap(), packet.lon.unwrap());
    assert!((lat - 52.5).abs() < 1e-3, "lat {lat}");
    assert!((lon - 13.4).abs() < 1e-3, "lon {lon}");
}

#[tokio::test]
async fn a_ctcss_tone_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let offset_hz = -30_000.0;

    let len = (NARROW_DEVICE_RATE * 2.0) as usize;
    let audio = testgen::nfm::mix(
        &testgen::nfm::ctcss_audio(88.5, 0.15, NARROW_DEVICE_RATE, len),
        &testgen::tone_audio(1_000.0, 0.6, NARROW_DEVICE_RATE, len),
    );
    let mut iq = testgen::fm_modulate(&audio, 2_500.0, NARROW_DEVICE_RATE);
    testgen::shift(&mut iq, offset_hz, NARROW_DEVICE_RATE);

    let device = plant(dir.path(), "ctcss", iq, NARROW_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Nfm(NfmParams {
                tone_mode: NfmToneMode::Ctcss,
                ctcss_hz: Some(88.5),
                ..NfmParams::default()
            }),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Tone(t) if t.ctcss_hz.is_some()),
    )
    .await;

    let DecoderEvent::Tone(status) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(status.ctcss_hz, Some(88.5));
    assert_eq!(status.dcs_code, None);
    assert!(status.open, "the tone the channel was set to must open it");
}

#[tokio::test]
async fn selcall_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let offset_hz = 5_000.0;
    let mut iq =
        testgen::selcall::transmission(SelcallSystem::Ccir1, "12234", AUDIO_DEVICE_RATE).unwrap();
    testgen::shift(&mut iq, offset_hz, AUDIO_DEVICE_RATE);
    let device = plant(dir.path(), "selcall_ccir1", iq, AUDIO_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Selcall(SelcallParams {
                system: SelcallSystem::Ccir1,
            }),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Selcall(_)),
    )
    .await;
    let DecoderEvent::Selcall(call) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(call.code, "12234");
    assert_eq!(call.system, SelcallSystem::Ccir1);
}

#[tokio::test]
async fn freedv_recording_survives_the_virtual_device_and_acquires_sync() {
    const FIXTURE: &[u8] = include_bytes!("../../../fixtures/freedv_1600_8k.sigmf-data");
    let iq = FIXTURE
        .as_chunks::<8>()
        .0
        .iter()
        .map(|sample| {
            Complex::new(
                f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]),
                f32::from_le_bytes([sample[4], sample[5], sample[6], sample[7]]),
            )
        })
        .collect();
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let device = plant(dir.path(), "freedv_1600", iq, 8_000.0);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Freedv(FreeDvParams::default()),
            audio: Default::default(),
        },
        |event| {
            matches!(
                event,
                DecoderEvent::Dv(frame)
                    if frame.mode == DvMode::FreeDv && frame.kind == DvFrameKind::Header
            )
        },
    )
    .await;
    let DecoderEvent::Dv(frame) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(frame.opcode.as_deref(), Some("1600"));
}

#[tokio::test]
async fn adsb_squitter_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let offset_hz = 0.0;

    let icao = 0x3C_6444;
    let frames = vec![
        testgen::adsb::squitter(icao, testgen::adsb::me_identification("DLH123")),
        testgen::adsb::squitter(
            icao,
            testgen::adsb::me_airborne_position(38_000, 52.2572, 3.9190, false),
        ),
        testgen::adsb::squitter(
            icao,
            testgen::adsb::me_airborne_position(38_000, 52.2657, 3.9184, true),
        ),
    ];
    let iq = testgen::adsb::transmission(&frames, 500.0, 0.8, ADSB_DEVICE_RATE);

    let device = plant(dir.path(), "adsb", iq, ADSB_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Adsb(AdsbParams::default()),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Adsb(a) if a.lat.is_some()),
    )
    .await;

    let DecoderEvent::Adsb(message) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(message.icao, "3C6444");
    assert_eq!(message.df, 17);
    assert_eq!(message.altitude_ft, Some(38_000));
    let (lat, lon) = (message.lat.unwrap(), message.lon.unwrap());
    assert!((lat - 52.2657).abs() < 0.02, "lat {lat}");
    assert!((lon - 3.9184).abs() < 0.02, "lon {lon}");
}

#[tokio::test]
async fn gps_ca_acquisition_survives_virtual_device_playback() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let iq = testgen::gnss::acquisition(7, 1_000.0, 317, 1_000);
    let device = plant(dir.path(), "gps-l1-ca", iq, GNSS_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Gnss(GnssParams {
                prn: 7,
                doppler_hz: 2_000,
                threshold: 2.5,
            }),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Gnss(frame) if frame.prn == 7),
    )
    .await;
    let DecoderEvent::Gnss(frame) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(frame.doppler_hz, 1_000.0);
    assert!((frame.code_phase_chips - 158.34).abs() < 0.6);
    assert!(frame.cn0_db_hz > 40.0);
}

#[tokio::test]
async fn vor_radial_survives_virtual_device_playback() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let iq = testgen::vor::transmission(123.0, 2);
    let device = plant(dir.path(), "vor", iq, testgen::vor::RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Vor(VorParams::default()),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Vor(_)),
    )
    .await;
    let DecoderEvent::Vor(reading) = record.event else {
        unreachable!("filtered above")
    };
    let error = (reading.radial_deg - 123.0 + 180.0).rem_euclid(360.0) - 180.0;
    assert!(error.abs() < 0.5, "radial {}", reading.radial_deg);
    assert!(reading.confidence > 0.8);
}

#[tokio::test]
async fn a_mode_s_identity_reply_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let icao = 0x40_621D;

    let frames = vec![
        testgen::adsb::all_call_reply(icao, 5, 0),
        testgen::adsb::identity_reply(icao, "7421", 0),
        testgen::adsb::altitude_reply(icao, 24_000, 0),
    ];
    let iq = testgen::adsb::transmission(&frames, 500.0, 0.8, ADSB_DEVICE_RATE);

    let device = plant(dir.path(), "modes", iq, ADSB_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Adsb(AdsbParams::default()),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Adsb(a) if a.df == 5),
    )
    .await;

    let DecoderEvent::Adsb(message) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(message.icao, "40621D");
    assert_eq!(message.squawk.as_deref(), Some("7421"));
    assert_eq!(message.type_code, None);
}

#[tokio::test]
async fn rtty_text_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = accelerated_engine_for(dir.path());
    let offset_hz = 5_000.0;
    let params = RttyParams::default();

    let mut iq = testgen::rtty::transmission(
        "CQ CQ DE DL1ABC K\r\n",
        params.baud,
        params.shift_hz,
        params.stop_bits.periods(),
        AUDIO_DEVICE_RATE,
    );
    testgen::shift(&mut iq, offset_hz, AUDIO_DEVICE_RATE);

    let device = plant(dir.path(), "rtty", iq, AUDIO_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Rtty(params),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Rtty(t) if t.text.contains("DL1ABC")),
    )
    .await;

    let DecoderEvent::Rtty(text) = record.event else {
        unreachable!("filtered above")
    };
    assert!(text.text.contains("CQ"), "decoded {:?}", text.text);
}

#[tokio::test]
async fn psk31_and_psk63_text_survive_the_ddc_and_reach_the_decoded_stream() {
    for (stem, params, baud, want) in [
        (
            "psk31",
            ChannelParams::Psk31(PskParams::default()),
            31.25,
            "PSK31 ENGINE",
        ),
        (
            "psk63",
            ChannelParams::Psk63(PskParams::default()),
            62.5,
            "PSK63 ENGINE",
        ),
    ] {
        let dir = TempDir::new().unwrap();
        let engine = accelerated_engine_for(dir.path());
        let offset_hz = 5_000.0;
        let iq = testgen::psk::transmission(&format!("{want}\n"), baud);
        let mut iq = testgen::resample(&iq, 8_000.0, AUDIO_DEVICE_RATE);
        testgen::shift(&mut iq, offset_hz, AUDIO_DEVICE_RATE);
        let device = plant(dir.path(), stem, iq, AUDIO_DEVICE_RATE);
        let record = decode_first(
            &engine,
            &device,
            ChannelSettings {
                offset_hz,
                squelch_db: None,
                squelch_auto_db: None,
                params,
                audio: Default::default(),
            },
            |event| match event {
                DecoderEvent::Psk31(text) | DecoderEvent::Psk63(text) => text.text.contains(want),
                _ => false,
            },
        )
        .await;
        let text = match record.event {
            DecoderEvent::Psk31(text) | DecoderEvent::Psk63(text) => text.text,
            _ => unreachable!("filtered above"),
        };
        assert!(text.contains(want), "decoded {text:?}");
    }
}

#[tokio::test]
async fn ft8_message_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = accelerated_engine_for(dir.path());
    let offset_hz = -8_000.0;
    let iq = testgen::weak_signal::ft8_slot("W1AW", "FN42", 1_500.0);
    let mut iq = testgen::resample(&iq, 12_000.0, AUDIO_DEVICE_RATE);
    testgen::shift(&mut iq, offset_hz, AUDIO_DEVICE_RATE);
    let device = plant(dir.path(), "ft8", iq, AUDIO_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Ft8(WsjtParams::default()),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Ft8(message) if message.text.contains("W1AW")),
    )
    .await;
    let DecoderEvent::Ft8(message) = record.event else {
        unreachable!("filtered above")
    };
    assert!(message.text.contains("CQ W1AW FN42"), "{}", message.text);
    assert!((message.audio_hz - 1_500.0).abs() < 10.0);
}

#[tokio::test]
async fn ft4_message_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = accelerated_engine_for(dir.path());
    let offset_hz = 8_000.0;
    let iq = testgen::weak_signal::ft4_slot("JA1ABC", "PM95", 1_000.0);
    let mut iq = testgen::resample(&iq, 12_000.0, AUDIO_DEVICE_RATE);
    testgen::shift(&mut iq, offset_hz, AUDIO_DEVICE_RATE);
    let device = plant(dir.path(), "ft4", iq, AUDIO_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Ft4(WsjtParams::default()),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Ft4(message) if message.text.contains("JA1ABC")),
    )
    .await;
    let DecoderEvent::Ft4(message) = record.event else {
        unreachable!("filtered above")
    };
    assert!(message.text.contains("CQ JA1ABC PM95"), "{}", message.text);
    assert!((message.audio_hz - 1_000.0).abs() < 20.0);
}

#[tokio::test]
async fn wspr_spot_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = accelerated_engine_for(dir.path());
    let iq = testgen::weak_signal::wspr_slot("K1ABC", "FN42", 37, 1_500.0);
    let iq = testgen::resample(&iq, 12_000.0, AUDIO_DEVICE_RATE);
    let device = plant(dir.path(), "wspr", iq, AUDIO_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Wspr(WsprParams::default()),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Wspr(spot) if spot.callsign == "K1ABC"),
    )
    .await;
    let DecoderEvent::Wspr(spot) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(spot.grid.as_deref(), Some("FN42"));
    assert_eq!(spot.power_dbm, 37);
}

#[tokio::test]
async fn morse_text_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = accelerated_engine_for(dir.path());
    let offset_hz = -5_000.0;

    let mut iq = testgen::morse::transmission("CQ DE DL1ABC K", 20.0, 0.0, AUDIO_DEVICE_RATE);
    testgen::shift(&mut iq, offset_hz, AUDIO_DEVICE_RATE);

    let device = plant(dir.path(), "morse", iq, AUDIO_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Morse(MorseParams::default()),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Morse(m) if m.text.contains("DL1ABC")),
    )
    .await;

    let DecoderEvent::Morse(text) = record.event else {
        unreachable!("filtered above")
    };
    assert!(
        (10.0..40.0).contains(&text.wpm),
        "speed estimate {} wpm is not plausible for 20 wpm sending",
        text.wpm
    );
}

#[tokio::test]
async fn cw_skimmer_spot_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = accelerated_engine_for(dir.path());
    let mut iq =
        testgen::morse::transmission("VVV VVV CQ DE ENGINE K", 20.0, 3_500.0, NARROW_DEVICE_RATE);
    iq.extend(testgen::silence(NARROW_DEVICE_RATE as usize * 4));
    let device = plant(dir.path(), "cw-skimmer", iq, NARROW_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::CwSkimmer(CwSkimmerParams {
                bandwidth_hz: 16_000.0,
                threshold_db: 8.0,
                max_signals: 8,
                wpm: None,
            }),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::CwSkimmer(spot) if spot.text.contains("ENGINE")),
    )
    .await;
    let DecoderEvent::CwSkimmer(spot) = record.event else {
        unreachable!("filtered above")
    };
    assert!(
        (spot.offset_hz - 3_500.0).abs() < 80.0,
        "{}",
        spot.offset_hz
    );
    assert!((15.0..25.0).contains(&spot.wpm), "{}", spot.wpm);
}

#[tokio::test]
async fn navtex_broadcast_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = accelerated_engine_for(dir.path());
    let offset_hz = -3_000.0;

    let mut iq = testgen::navtex::transmission(
        "ZCZC DA07\r\nGALE WARNING\r\nGERMAN BIGHT\r\nNNNN",
        AUDIO_DEVICE_RATE,
    );
    testgen::shift(&mut iq, offset_hz, AUDIO_DEVICE_RATE);

    let device = plant(dir.path(), "navtex", iq, AUDIO_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Navtex(NavtexParams::default()),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Navtex(m) if m.complete),
    )
    .await;

    let DecoderEvent::Navtex(message) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(message.station, Some('D'));
    assert_eq!(
        message.subject_name.as_deref(),
        Some("Navigational warning")
    );
    assert_eq!(message.serial, Some(7));
    assert_eq!(message.text, "GALE WARNING\nGERMAN BIGHT");
}

#[tokio::test]
async fn acars_block_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let offset_hz = -40_000.0;

    let block = testgen::acars::Block {
        mode: '2',
        registration: ".D-AIBC",
        ack: '\x15',
        label: "H1",
        block_id: '3',
        seq_no: Some("M01A"),
        flight: Some("LH0400"),
        text: "ENGINE E2E",
        more: false,
    };
    let mut iq = testgen::acars::transmission(&block, NARROW_DEVICE_RATE);
    testgen::shift(&mut iq, offset_hz, NARROW_DEVICE_RATE);

    let device = plant(dir.path(), "acars", iq, NARROW_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Acars(AcarsParams::default()),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Acars(_)),
    )
    .await;

    let DecoderEvent::Acars(message) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(message.registration, "D-AIBC");
    assert_eq!(message.label, "H1");
    assert!(message.downlink);
    assert_eq!(message.flight.as_deref(), Some("LH0400"));
    assert_eq!(message.text, "ENGINE E2E");
}

#[tokio::test]
async fn subghz_remote_survives_the_ddc_and_reaches_the_decoded_stream() {
    const SUBGHZ_DEVICE_RATE: f64 = 500_000.0;
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let offset_hz = 100_000.0;

    let remote = testgen::subghz::Pwm {
        bits: (0..24)
            .map(|i| 0x0A_1B_23u32 >> (23 - i) & 1 == 1)
            .collect(),
        short_us: 320,
        long_multiple: 3,
        sync_gap_multiple: 31,
        repeats: 6,
    };
    let mut iq = testgen::subghz::pwm(&remote, SUBGHZ_DEVICE_RATE);
    testgen::shift(&mut iq, offset_hz, SUBGHZ_DEVICE_RATE);

    let device = plant(dir.path(), "subghz", iq, SUBGHZ_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Subghz(SubghzParams::default()),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Subghz(f) if f.bits == 24),
    )
    .await;

    let DecoderEvent::Subghz(frame) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(frame.encoding, SubghzEncoding::Pwm);
    assert_eq!(frame.data, "0A1B23");
    assert_eq!(frame.address, Some(0x0_A1B2));
    assert_eq!(frame.button, Some(3));
    assert!(frame.repeats > 1, "repeats collapsed to {}", frame.repeats);
}

#[tokio::test]
async fn ysf_callsigns_survive_a_recorded_virtual_device() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let call = testgen::dv::ysf::Call::default();
    let iq = testgen::dv::ysf::transmission_with_callsigns(
        &testgen::dv::ysf::Fich::default(),
        &call,
        AUDIO_DEVICE_RATE,
    );
    let device = plant(dir.path(), "ysf-callsigns", iq, AUDIO_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Ysf(YsfParams::default()),
            audio: Default::default(),
        },
        |event| {
            matches!(event, DecoderEvent::Dv(frame) if frame.mode == DvMode::Ysf && frame.source_call.as_deref() == Some("DL1ABC"))
        },
    )
    .await;

    let DecoderEvent::Dv(frame) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(frame.kind, DvFrameKind::Header);
    assert_eq!(frame.destination_call.as_deref(), Some("ALL"));
    assert_eq!(frame.source_call.as_deref(), Some("DL1ABC"));
    assert_eq!(frame.via.as_deref(), Some("DB0XYZ → DB0ABC"));
}

#[tokio::test]
async fn ident_names_an_unknown_transmission_end_to_end() {
    const IDENT_DEVICE_RATE: f64 = 480_000.0;
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let offset_hz = 60_000.0;

    let call = testgen::dv::dmr::Call::default();
    let one = testgen::dv::dmr::transmission(&call, IDENT_DEVICE_RATE);
    let mut iq: Vec<Complex<f32>> = Vec::new();
    for _ in 0..3 {
        iq.extend_from_slice(&one);
    }
    testgen::shift(&mut iq, offset_hz, IDENT_DEVICE_RATE);

    let device = plant(dir.path(), "ident", iq, IDENT_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Ident(IdentParams {
                interval_ms: 500,
                ..IdentParams::default()
            }),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Ident(r) if r.best().is_some_and(|m| m.confirmed)),
    )
    .await;

    let DecoderEvent::Ident(report) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(report.modulation, Modulation::Fsk4);
    let best = report.best().expect("filtered above");
    assert_eq!(best.name, "DMR");
    assert_eq!(best.type_id.as_deref(), Some("dmr"));
    assert!(
        (report.symbol_rate_hz.unwrap_or_default() - 4_800.0).abs() < 250.0,
        "symbol rate {:?}",
        report.symbol_rate_hz
    );
    assert!(
        report.center_offset_hz.abs() < 1_000.0,
        "off tune by {} Hz",
        report.center_offset_hz
    );
}

fn dab_mode_i_acquisition_frame() -> Vec<Complex<f32>> {
    const USEFUL: usize = 2_048;
    const GUARD: usize = 504;
    let mut state = 0x5a17_91e3u32;
    let useful: Vec<_> = (0..USEFUL)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            Complex::from_polar(
                0.8,
                state as f32 * (std::f32::consts::TAU / u32::MAX as f32),
            )
        })
        .collect();
    let mut iq = vec![Complex::new(0.8, 0.0); 20_000];
    iq.extend(vec![Complex::new(0.0001, 0.0); 2_656]);
    iq.extend_from_slice(&useful[USEFUL - GUARD..]);
    iq.extend_from_slice(&useful);
    iq
}

#[tokio::test]
async fn dab_mode_i_lock_survives_a_recorded_virtual_device_and_ddc() {
    const DEVICE_RATE: f64 = 2_400_000.0;
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let iq = testgen::resample(&dab_mode_i_acquisition_frame(), 2_048_000.0, DEVICE_RATE);
    let device = plant(dir.path(), "dab-mode-i", iq, DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Dab(DabParams::default()),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Broadcast(status) if status.system == BroadcastSystem::Dab && status.locked),
    )
    .await;
    let DecoderEvent::Broadcast(status) = record.event else {
        unreachable!("filtered above")
    };
    assert!(status.snr_db > 10.0, "{status:?}");
}

fn datv_qpsk_fixture() -> Vec<Complex<f32>> {
    let points = [
        Complex::new(0.7, 0.7),
        Complex::new(-0.7, 0.7),
        Complex::new(-0.7, -0.7),
        Complex::new(0.7, -0.7),
    ];
    let mut iq = Vec::with_capacity(2_000_000);
    for i in 0..250_000 {
        iq.extend(std::iter::repeat_n(points[(i * 13 + i / 7) % 4], 8));
    }
    iq
}

#[tokio::test]
async fn datv_qpsk_lock_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let device = plant(dir.path(), "datv-qpsk", datv_qpsk_fixture(), 2_000_000.0);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Datv(DatvParams {
                standard: DatvStandard::DvbS2,
                symbol_rate: 250_000.0,
            }),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Broadcast(status) if status.system == BroadcastSystem::DvbS2 && status.locked),
    )
    .await;
    let DecoderEvent::Broadcast(status) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(status.symbol_rate, Some(250_000.0));
}

fn drm30_mode_b_fixture() -> Vec<Complex<f32>> {
    let mut baseband = Vec::new();
    let mut frame = 0usize;
    while baseband.len() < 48_000 {
        let mut state = 0x6d2b_79f5u32 ^ frame as u32;
        let carriers: Vec<_> = (-96i32..=96)
            .step_by(6)
            .filter(|&bin| bin != 0)
            .map(|bin| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let symbol = match state & 3 {
                    0 => Complex::new(1.0, 1.0),
                    1 => Complex::new(-1.0, 1.0),
                    2 => Complex::new(-1.0, -1.0),
                    _ => Complex::new(1.0, -1.0),
                };
                (bin, symbol)
            })
            .collect();
        let scale = (carriers.len() as f32).sqrt();
        let useful: Vec<_> = (0..1_024)
            .map(|i| {
                carriers
                    .iter()
                    .map(|&(bin, symbol)| {
                        symbol
                            * Complex::from_polar(
                                1.0,
                                std::f32::consts::TAU * bin as f32 * i as f32 / 1_024.0,
                            )
                    })
                    .sum::<Complex<f32>>()
                    * (0.8 / scale)
            })
            .collect();
        baseband.extend_from_slice(&useful[768..]);
        baseband.extend_from_slice(&useful);
        frame += 1;
    }
    baseband.truncate(48_000);
    baseband
        .into_iter()
        .flat_map(|sample| std::iter::repeat_n(sample, 4))
        .collect()
}

#[tokio::test]
async fn drm30_lock_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let device = plant(
        dir.path(),
        "drm30-mode-b",
        drm30_mode_b_fixture(),
        192_000.0,
    );
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Drm(DrmParams {
                mode: DrmMode::Drm30,
                bandwidth_hz: 10_000.0,
            }),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Broadcast(status) if status.system == BroadcastSystem::Drm30 && status.locked),
    )
    .await;
    let DecoderEvent::Broadcast(status) = record.event else {
        unreachable!("filtered above")
    };
    assert!(status.frequency_error_hz.abs() < 30.0, "{status:?}");
}

#[tokio::test]
async fn adsb_decodes_at_an_rtl_sdr_rate_the_ddc_could_not_have_resampled() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    const RTL_RATE: f64 = 2_048_000.0;

    let icao = 0x3C_6444;
    let frames = vec![
        testgen::adsb::squitter(icao, testgen::adsb::me_identification("DLH123")),
        testgen::adsb::squitter(
            icao,
            testgen::adsb::me_airborne_position(38_000, 52.2572, 3.9190, false),
        ),
        testgen::adsb::squitter(
            icao,
            testgen::adsb::me_airborne_position(38_000, 52.2657, 3.9184, true),
        ),
    ];
    let iq = testgen::adsb::transmission_at_phase(&frames, 500.0, 0.8, RTL_RATE, 0.37);

    let device = plant(dir.path(), "adsb-rtl", iq, RTL_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Adsb(AdsbParams::default()),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Adsb(a) if a.lat.is_some()),
    )
    .await;

    let DecoderEvent::Adsb(message) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(message.icao, "3C6444");
    assert_eq!(message.altitude_ft, Some(38_000));
}

#[tokio::test]
async fn adsb_is_rejected_above_the_rate_its_slicer_can_use() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let device = plant(
        dir.path(),
        "wideband",
        testgen::silence(4_800),
        10_000_000.0,
    );
    let ds = engine.create_device_set(&device).unwrap();

    let err = engine
        .add_channel(
            ds,
            0,
            ChannelSettings {
                offset_hz: 0.0,
                squelch_db: None,
                squelch_auto_db: None,
                params: ChannelParams::Adsb(AdsbParams::default()),
                audio: Default::default(),
            },
        )
        .expect_err("a rate past the slicer's range must be refused, not silently expensive");
    let message = err.to_string();
    assert!(
        message.contains("2.000") && message.contains("4.000"),
        "the rejection must name the range that works: {message}"
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn rds_station_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    const RATE: f64 = 960_000.0;
    let offset_hz = 200_000.0;

    let station = testgen::rds::Station {
        pi: 0xD3C2,
        ps: "SDR-M4  ".to_owned(),
        radiotext: "engine end to end".to_owned(),
        pty: 10,
        tp: true,
        ta: false,
        music: true,
        alt_freqs_hz: vec![89_800_000.0, 95_500_000.0],
    };
    let mut iq = testgen::rds::transmission(&station, 6.0, Some(1_000.0), RATE);
    testgen::shift(&mut iq, offset_hz, RATE);

    let device = plant(dir.path(), "rds", iq, RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Wfm(WfmParams {
                deemphasis_us: 50.0,
                stereo: false,
            }),
            audio: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Rds(u) if u.ps.is_some()),
    )
    .await;

    let DecoderEvent::Rds(update) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(update.pi.as_deref(), Some("D3C2"));
    assert_eq!(update.ps.as_deref(), Some("SDR-M4"));
    assert_eq!(update.pty, Some(10));
    assert_eq!(update.tp, Some(true));
    assert!(update.groups > 0, "groups counted");
}

async fn next_rds(rx: &mut tokio::sync::broadcast::Receiver<DecodedRecord>) -> RdsUpdate {
    tokio::time::timeout(DECODE_TIMEOUT, async {
        loop {
            if let Ok(record) = rx.recv().await
                && let DecoderEvent::Rds(update) = record.event
            {
                return update;
            }
        }
    })
    .await
    .expect("an RDS update within the timeout")
}

#[tokio::test]
async fn retuning_resets_the_decoder_through_the_engine_path() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    const RATE: f64 = 960_000.0;
    let offset_hz = 200_000.0;

    let station = testgen::rds::Station {
        pi: 0xD3C2,
        ps: "RETUNE  ".to_owned(),
        radiotext: "retune resets the picture".to_owned(),
        pty: 10,
        tp: true,
        ta: false,
        music: true,
        alt_freqs_hz: Vec::new(),
    };
    let mut iq = testgen::rds::transmission(&station, 12.0, Some(1_000.0), RATE);
    testgen::shift(&mut iq, offset_hz, RATE);
    let device = plant(dir.path(), "rds_retune", iq, RATE);

    let settings = |offset_hz: f64| ChannelSettings {
        offset_hz,
        squelch_db: None,
        squelch_auto_db: None,
        params: ChannelParams::Wfm(WfmParams {
            deemphasis_us: 50.0,
            stereo: false,
        }),
        audio: Default::default(),
    };

    let mut rx = engine.subscribe_decoded();
    let ds = engine.create_device_set(&device).unwrap();
    let ch = engine.add_channel(ds, 0, settings(offset_hz)).unwrap();

    let mut before = next_rds(&mut rx).await;
    while before.groups < 10 {
        before = next_rds(&mut rx).await;
    }
    engine
        .patch_channel(ds, ch, settings(offset_hz + 5_000.0))
        .unwrap();

    let after = next_rds(&mut rx).await;
    engine.remove_device_set(ds).unwrap();
    assert!(
        after.groups < before.groups,
        "the retune did not reset the decoder: {} groups before, {} after",
        before.groups,
        after.groups
    );
}
