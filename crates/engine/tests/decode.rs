#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{path::Path, sync::Arc, time::Duration};

use num_complex::Complex;
use sdrmm_channels::{AprsTx, ChannelCtx, ChannelTx, MicE, MicEBit, TxPayload, synth};
use sdrmm_device::DeviceRegistry;
use sdrmm_device_recording::RecordingDriver;
use sdrmm_engine::Engine;
use sdrmm_recorder::SigmfWriter;
use sdrmm_wire::{
    AcarsParams, AdsbParams, AisChannel, AisParams, AprsMode, AprsParams, AptParams, AvhrrChannel,
    BroadcastSystem, ChannelParams, ChannelSettings, CwSkimmerParams, DabParams, DatvParams,
    DatvStandard, DecodedRecord, DecoderEvent, DectCapability, DectCipherState, DectParams,
    DmrParams, DrmMode, DrmParams, DvFrameKind, DvMode, ErmesParams, FlexParams, FreeDvParams,
    GnssParams, IdentParams, LrptMode, LrptParams, Modulation, MorseParams, NavtexParams,
    NfmParams, NfmToneMode, PipelineStage, PocsagBaud, PocsagParams, PskBaud, PskParams,
    RadiosondeParams, RdsUpdate, RttyParams, SelcallParams, SelcallSystem, SondeType, SymbolPlane,
    VorParams, WefaxIoc, WefaxLpm, WefaxParams, WfmParams, WsjtParams, WsprParams, YsfParams,
};
use tempfile::TempDir;

mod common;

const DECODE_TIMEOUT: Duration = Duration::from_secs(90);

const NARROW_DEVICE_RATE: f64 = 240_000.0;
const AUDIO_DEVICE_RATE: f64 = 48_000.0;
const ADSB_DEVICE_RATE: f64 = 2_000_000.0;
const GNSS_DEVICE_RATE: f64 = 2_048_000.0;
const DECT_DEVICE_RATE: f64 = 2_304_000.0;
const CENTER_HZ: f64 = 145_000_000.0;
fn aprs_burst(frame: Vec<u8>) -> Vec<Complex<f32>> {
    let mut tx = AprsTx::new(
        ChannelCtx {
            input_rate: AprsTx::descriptor().input_rate_hz,
        },
        ChannelSettings {
            frequency_hz: CENTER_HZ,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Aprs(AprsParams {
                mode: AprsMode::Afsk1200,
                ..AprsParams::default()
            }),
            blanker: Default::default(),
        },
    )
    .unwrap();
    tx.submit(TxPayload::Frame(frame)).unwrap();
    synth::burst(&mut tx)
}

fn engine_for(dir: &Path) -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(10, Box::new(RecordingDriver::new(Some(dir.to_path_buf()))));
    Engine::with_registry(registry, Some(dir.to_path_buf()))
}

fn accelerated_engine_for(dir: &Path) -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(
        10,
        Box::new(RecordingDriver::accelerated(Some(dir.to_path_buf()), 20.0)),
    );
    Engine::with_registry(registry, Some(dir.to_path_buf()))
}

fn plant(dir: &Path, stem: &str, mut iq: Vec<Complex<f32>>, rate: f64) -> String {
    let min_len = rate as usize;
    if iq.len() < min_len {
        iq.extend(synth::silence(min_len - iq.len()));
    }
    let path = dir.join(stem);
    let mut writer = SigmfWriter::create(&path, rate, CENTER_HZ, "decoder fixture").unwrap();
    writer.write_block(&iq).unwrap();
    writer.finalize().unwrap();
    format!("recording:{}", path.file_name().unwrap().display())
}

async fn decode_first(
    engine: &Arc<Engine>,
    device_id: &str,
    settings: ChannelSettings,
    want: impl Fn(&DecoderEvent) -> bool,
) -> DecodedRecord {
    let frequency_hz = settings.frequency_hz;
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
    let lost: u64 = engine
        .pipeline_health()
        .iter()
        .filter(|queue| queue.device_set == ds && queue.stage == PipelineStage::Capture)
        .map(|queue| queue.health.dropped)
        .sum();
    engine.remove_device_set(ds).unwrap();
    assert_eq!(lost, 0, "a recording lost capture on its way to the DSP");
    let record = found.expect("a matching decode within the timeout");

    assert_eq!(record.device_set, ds, "record names its device set");
    assert_eq!(record.channel, ch, "record names its channel");
    assert_eq!(
        record.freq_hz, frequency_hz,
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

    let pages = [synth::pocsag::Page {
        address: 1_234_567,
        function: 3,
        text: "ENGINE E2E".to_owned(),
        numeric: false,
    }];
    let mut iq = synth::pocsag::transmission(&pages, 1_200, 4_500.0, NARROW_DEVICE_RATE);
    synth::shift(&mut iq, offset_hz, NARROW_DEVICE_RATE);

    let device = plant(dir.path(), "pocsag", iq, NARROW_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Pocsag(PocsagParams {
                baud: PocsagBaud::Auto,
                ..PocsagParams::default()
            }),
            blanker: Default::default(),
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
    let page = synth::flex::Page {
        address: 345_678,
        text: "FLEX ENGINE E2E".to_owned(),
    };
    let mut iq = synth::flex::transmission(&page, 4, 72, NARROW_DEVICE_RATE);
    synth::shift(&mut iq, offset_hz, NARROW_DEVICE_RATE);
    let device = plant(dir.path(), "flex", iq, NARROW_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Flex(FlexParams::default()),
            blanker: Default::default(),
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
    let page = synth::ermes::Page {
        local_address: 456_789,
        message_number: 6,
        text: "ERMES ENGINE E2E".to_owned(),
        urgent: true,
        alert: 4,
    };
    let mut iq = synth::ermes::transmission(&page, NARROW_DEVICE_RATE);
    synth::shift(&mut iq, offset_hz, NARROW_DEVICE_RATE);
    let device = plant(dir.path(), "ermes", iq, NARROW_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Ermes(ErmesParams::default()),
            blanker: Default::default(),
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
    let mut iq = synth::resample(
        &aprs_burst(frame),
        AprsTx::descriptor().input_rate_hz,
        NARROW_DEVICE_RATE,
    );
    synth::shift(&mut iq, offset_hz, NARROW_DEVICE_RATE);

    let device = plant(dir.path(), "aprs", iq, NARROW_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Aprs(AprsParams {
                mode: AprsMode::Afsk1200,
                ..AprsParams::default()
            }),
            blanker: Default::default(),
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

    let report = synth::ais::PositionReport {
        mmsi: 211_234_560,
        lat: 53.5413,
        lon: 9.9846,
        sog_kt: 12.3,
        cog_deg: 178.4,
        heading_deg: 179,
        nav_status: 0,
    };
    let mut iq = synth::ais::burst(&synth::ais::position_payload(&report), NARROW_DEVICE_RATE);
    synth::shift(&mut iq, offset_hz, NARROW_DEVICE_RATE);

    let device = plant(dir.path(), "ais", iq, NARROW_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Ais(AisParams {
                ais_channel: AisChannel::B,
            }),
            blanker: Default::default(),
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
    let mut iq = synth::resample(
        &aprs_burst(frame),
        AprsTx::descriptor().input_rate_hz,
        NARROW_DEVICE_RATE,
    );
    synth::shift(&mut iq, offset_hz, NARROW_DEVICE_RATE);

    let device = plant(dir.path(), "mice", iq, NARROW_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Aprs(AprsParams::default()),
            blanker: Default::default(),
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
    let audio = synth::nfm::mix(
        &synth::nfm::ctcss_audio(88.5, 0.15, NARROW_DEVICE_RATE, len),
        &synth::tone_audio(1_000.0, 0.6, NARROW_DEVICE_RATE, len),
    );
    let mut iq = synth::fm_modulate(&audio, 2_500.0, NARROW_DEVICE_RATE);
    synth::shift(&mut iq, offset_hz, NARROW_DEVICE_RATE);

    let device = plant(dir.path(), "ctcss", iq, NARROW_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Nfm(NfmParams {
                tone_mode: NfmToneMode::Ctcss,
                ctcss_hz: Some(88.5),
                ..NfmParams::default()
            }),
            blanker: Default::default(),
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
        synth::selcall::transmission(SelcallSystem::Ccir1, "12234", AUDIO_DEVICE_RATE).unwrap();
    synth::shift(&mut iq, offset_hz, AUDIO_DEVICE_RATE);
    let device = plant(dir.path(), "selcall_ccir1", iq, AUDIO_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Selcall(SelcallParams {
                system: SelcallSystem::Ccir1,
            }),
            blanker: Default::default(),
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
            frequency_hz: CENTER_HZ,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Freedv(FreeDvParams::default()),
            blanker: Default::default(),
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
        synth::adsb::squitter(icao, synth::adsb::me_identification("DLH123")),
        synth::adsb::squitter(
            icao,
            synth::adsb::me_airborne_position(38_000, 52.2572, 3.9190, false),
        ),
        synth::adsb::squitter(
            icao,
            synth::adsb::me_airborne_position(38_000, 52.2657, 3.9184, true),
        ),
    ];
    let iq = synth::adsb::transmission(&frames, 500.0, 0.8, ADSB_DEVICE_RATE);

    let device = plant(dir.path(), "adsb", iq, ADSB_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Adsb(AdsbParams::default()),
            blanker: Default::default(),
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
    let iq = synth::gnss::acquisition(7, 1_000.0, 317, 1_000);
    let device = plant(dir.path(), "gps-l1-ca", iq, GNSS_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Gnss(GnssParams {
                prn: 7,
                doppler_hz: 2_000,
                threshold: 2.5,
            }),
            blanker: Default::default(),
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
    let iq = synth::vor::transmission(123.0, 2);
    let device = plant(dir.path(), "vor", iq, synth::vor::RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Vor(VorParams::default()),
            blanker: Default::default(),
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

    const RATE: f64 = 4_800_000.0;

    let frames = vec![
        synth::adsb::all_call_reply(icao, 5, 0),
        synth::adsb::identity_reply(icao, "7421", 0),
        synth::adsb::altitude_reply(icao, 24_000, 0),
    ];
    let iq = synth::adsb::transmission(&frames, 500.0, 0.8, RATE);

    let device = plant(dir.path(), "modes", iq, RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Adsb(AdsbParams::default()),
            blanker: Default::default(),
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

    let mut iq = synth::rtty::transmission(
        "CQ CQ DE DL1ABC K\r\n",
        params.baud,
        params.shift_hz,
        params.stop_bits.periods(),
        AUDIO_DEVICE_RATE,
    );
    synth::shift(&mut iq, offset_hz, AUDIO_DEVICE_RATE);

    let device = plant(dir.path(), "rtty", iq, AUDIO_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Rtty(params),
            blanker: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Rtty(t) if t.text.contains("CQ CQ DE DL1ABC")),
    )
    .await;

    let DecoderEvent::Rtty(text) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(text.text, "CQ CQ DE DL1ABC K\n");
}

#[tokio::test]
async fn psk_text_survives_the_ddc_and_reaches_the_decoded_stream() {
    for (stem, baud, want) in [
        ("psk31", PskBaud::Psk31, "PSK31 ENGINE"),
        ("psk250", PskBaud::Psk250, "PSK250 ENGINE"),
    ] {
        let dir = TempDir::new().unwrap();
        let engine = accelerated_engine_for(dir.path());
        let offset_hz = 5_000.0;
        let iq = synth::psk::transmission(&format!("{want}\n"), baud.rate());
        let mut iq = synth::resample(&iq, 8_000.0, AUDIO_DEVICE_RATE);
        synth::shift(&mut iq, offset_hz, AUDIO_DEVICE_RATE);
        let device = plant(dir.path(), stem, iq, AUDIO_DEVICE_RATE);
        let record = decode_first(
            &engine,
            &device,
            ChannelSettings {
                frequency_hz: CENTER_HZ + offset_hz,
                squelch: sdrmm_wire::Squelch::Off,
                params: ChannelParams::Psk(PskParams {
                    baud,
                    invert: false,
                }),
                blanker: Default::default(),
            },
            |event| match event {
                DecoderEvent::Psk(text) => text.text.contains(want),
                _ => false,
            },
        )
        .await;
        let DecoderEvent::Psk(text) = record.event else {
            unreachable!("filtered above")
        };
        assert_eq!(text.baud, baud);
        assert!(text.text.contains(want), "decoded {:?}", text.text);
    }
}

#[tokio::test]
async fn ft8_message_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = accelerated_engine_for(dir.path());
    let offset_hz = -8_000.0;
    let iq = synth::weak_signal::ft8_slot("W1AW", "FN42", 1_500.0);
    let mut iq = synth::resample(&iq, 12_000.0, AUDIO_DEVICE_RATE);
    synth::shift(&mut iq, offset_hz, AUDIO_DEVICE_RATE);
    let device = plant(dir.path(), "ft8", iq, AUDIO_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Ft8(WsjtParams::default()),
            blanker: Default::default(),
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
    let iq = synth::weak_signal::ft4_slot("JA1ABC", "PM95", 1_000.0);
    let mut iq = synth::resample(&iq, 12_000.0, AUDIO_DEVICE_RATE);
    synth::shift(&mut iq, offset_hz, AUDIO_DEVICE_RATE);
    let device = plant(dir.path(), "ft4", iq, AUDIO_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Ft4(WsjtParams::default()),
            blanker: Default::default(),
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
    let iq = synth::weak_signal::wspr_slot("K1ABC", "FN42", 37, 1_500.0);
    let iq = synth::resample(&iq, 12_000.0, AUDIO_DEVICE_RATE);
    let device = plant(dir.path(), "wspr", iq, AUDIO_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Wspr(WsprParams::default()),
            blanker: Default::default(),
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

    let mut iq = synth::morse::transmission("CQ DE DL1ABC K", 20.0, 0.0, AUDIO_DEVICE_RATE);
    synth::shift(&mut iq, offset_hz, AUDIO_DEVICE_RATE);

    let device = plant(dir.path(), "morse", iq, AUDIO_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Morse(MorseParams::default()),
            blanker: Default::default(),
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
        synth::morse::transmission("VVV VVV CQ DE ENGINE K", 20.0, 3_500.0, NARROW_DEVICE_RATE);
    iq.extend(synth::silence(NARROW_DEVICE_RATE as usize * 4));
    let device = plant(dir.path(), "cw-skimmer", iq, NARROW_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::CwSkimmer(CwSkimmerParams {
                bandwidth_hz: 16_000.0,
                threshold_db: 8.0,
                max_signals: 8,
                wpm: None,
            }),
            blanker: Default::default(),
        },
        |event| {
            matches!(event, DecoderEvent::CwSkimmer(spot)
                if spot.text.contains("ENGINE") && (spot.offset_hz - 3_500.0).abs() < 80.0)
        },
    )
    .await;
    let DecoderEvent::CwSkimmer(spot) = record.event else {
        unreachable!("filtered above")
    };
    assert!((15.0..25.0).contains(&spot.wpm), "{}", spot.wpm);
}

#[tokio::test]
async fn navtex_broadcast_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = accelerated_engine_for(dir.path());
    let offset_hz = -3_000.0;

    let mut iq = synth::navtex::transmission(
        "ZCZC DA07\r\nGALE WARNING\r\nGERMAN BIGHT\r\nNNNN",
        AUDIO_DEVICE_RATE,
    );
    synth::shift(&mut iq, offset_hz, AUDIO_DEVICE_RATE);

    let device = plant(dir.path(), "navtex", iq, AUDIO_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Navtex(NavtexParams::default()),
            blanker: Default::default(),
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

    let block = synth::acars::Block {
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
    let mut iq = synth::acars::transmission(&block, NARROW_DEVICE_RATE);
    synth::shift(&mut iq, offset_hz, NARROW_DEVICE_RATE);

    let device = plant(dir.path(), "acars", iq, NARROW_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Acars(AcarsParams::default()),
            blanker: Default::default(),
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
async fn ysf_callsigns_survive_a_recorded_virtual_device() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let call = synth::dv::ysf::Call::default();
    let iq = synth::dv::ysf::transmission_with_callsigns(
        &synth::dv::ysf::Fich::default(),
        &call,
        AUDIO_DEVICE_RATE,
    );
    let device = plant(dir.path(), "ysf-callsigns", iq, AUDIO_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Ysf(YsfParams::default()),
            blanker: Default::default(),
        },
        |event| {
            matches!(event, DecoderEvent::Dv(frame) if frame.mode == DvMode::Ysf
                && frame.kind == DvFrameKind::Header
                && frame.source_call.as_deref() == Some("DL1ABC"))
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

    let call = synth::dv::dmr::Call::default();
    let one = synth::dv::dmr::transmission(&call, IDENT_DEVICE_RATE);
    let mut iq: Vec<Complex<f32>> = Vec::new();
    for _ in 0..3 {
        iq.extend_from_slice(&one);
    }
    synth::shift(&mut iq, offset_hz, IDENT_DEVICE_RATE);

    let device = plant(dir.path(), "ident", iq, IDENT_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Ident(IdentParams {
                interval_ms: 500,
                ..IdentParams::default()
            }),
            blanker: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Ident(r) if r.best().is_some_and(|m| m.confirmed)),
    )
    .await;

    let DecoderEvent::Ident(report) = record.event else {
        unreachable!("filtered above")
    };
    let signal = report.loudest().expect("filtered above");
    assert_eq!(signal.modulation, Modulation::Fsk4);
    let best = signal.best().expect("filtered above");
    assert_eq!(best.name, "DMR");
    assert_eq!(best.type_id.as_deref(), Some("dmr"));
    assert!(
        (signal.symbol_rate_hz.unwrap_or_default() - 4_800.0).abs() < 250.0,
        "symbol rate {:?}",
        signal.symbol_rate_hz
    );
    assert!(
        signal.center_offset_hz.abs() < 1_000.0,
        "off tune by {} Hz",
        signal.center_offset_hz
    );
    assert!(
        (signal.frequency_hz - (CENTER_HZ + offset_hz)).abs() < 1_000.0,
        "placed at {} Hz",
        signal.frequency_hz
    );
}

#[tokio::test]
async fn a_dab_ensemble_reaches_the_decoded_stream_through_a_virtual_device() {
    for mode in [
        sdrmm_wire::DabTransmissionMode::I,
        sdrmm_wire::DabTransmissionMode::Ii,
        sdrmm_wire::DabTransmissionMode::Iii,
        sdrmm_wire::DabTransmissionMode::Iv,
    ] {
        const DEVICE_RATE: f64 = 2_400_000.0;
        let dir = TempDir::new().unwrap();
        let engine = engine_for(dir.path());
        let iq = synth::resample(
            &synth::dab::ensemble_for_mode(mode, 16),
            2_048_000.0,
            DEVICE_RATE,
        );
        let device = plant(dir.path(), "dab-ensemble", iq, DEVICE_RATE);
        let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Dab(DabParams { transmission_mode: mode, ..DabParams::default() }),
            blanker: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Broadcast(status) if status.locked && !status.services.is_empty()),
    )
    .await;
        let DecoderEvent::Broadcast(status) = record.event else {
            unreachable!("filtered above")
        };
        assert_eq!(
            status.ensemble_label.as_deref(),
            Some(synth::dab::ENSEMBLE_LABEL)
        );
        assert_eq!(status.ensemble_id, Some(u32::from(synth::dab::ENSEMBLE_ID)));
        assert_eq!(status.services.len(), 2, "{status:?}");
        assert_eq!(status.services[0].label, "Rust FM");
        assert!(status.snr_db > 10.0, "{status:?}");
    }
}

#[tokio::test]
async fn a_dvb_s_transport_stream_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let device = plant(dir.path(), "dvb-s", synth::datv::dvbs(4), 2_000_000.0);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Datv(DatvParams {
                standard: DatvStandard::DvbS,
                symbol_rate: synth::datv::SYMBOL_RATE,
                code_rate: synth::datv::CODE_RATE,
                ..DatvParams::default()
            }),
            blanker: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Broadcast(status) if !status.services.is_empty()),
    )
    .await;
    let DecoderEvent::Broadcast(status) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(status.system, BroadcastSystem::DvbS);
    assert_eq!(status.label.as_deref(), Some(synth::datv::PROGRAM_NAME));
    assert_eq!(status.code_rate.as_deref(), Some("3/4"));
    assert!(status.frames_ok > 20, "{status:?}");
}

fn datv_qpsk_fixture() -> Vec<Complex<f32>> {
    let points = [
        Complex::new(0.7, 0.7),
        Complex::new(-0.7, 0.7),
        Complex::new(-0.7, -0.7),
        Complex::new(0.7, -0.7),
    ];
    let mut iq = Vec::with_capacity(2_000_000);
    let mut state = 0x1234_5678u32;
    for _ in 0..250_000 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        iq.extend(std::iter::repeat_n(points[(state >> 24) as usize % 4], 8));
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
            frequency_hz: CENTER_HZ,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Datv(DatvParams {
                standard: DatvStandard::DvbS2,
                symbol_rate: 250_000.0,
                ..DatvParams::default()
            }),
            blanker: Default::default(),
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
            frequency_hz: CENTER_HZ,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Drm(DrmParams {
                mode: DrmMode::Drm30,
                bandwidth_hz: 10_000.0,
                ..DrmParams::default()
            }),
            blanker: Default::default(),
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
        synth::adsb::squitter(icao, synth::adsb::me_identification("DLH123")),
        synth::adsb::squitter(
            icao,
            synth::adsb::me_airborne_position(38_000, 52.2572, 3.9190, false),
        ),
        synth::adsb::squitter(
            icao,
            synth::adsb::me_airborne_position(38_000, 52.2657, 3.9184, true),
        ),
    ];
    let iq = synth::adsb::transmission_at_phase(&frames, 500.0, 0.8, RTL_RATE, 0.37);

    let device = plant(dir.path(), "adsb-rtl", iq, RTL_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Adsb(AdsbParams::default()),
            blanker: Default::default(),
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
async fn adsb_decodes_from_a_wideband_radio_through_the_resampler() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    const RATE: f64 = 10_000_000.0;
    let offset_hz = 1_000_000.0;
    let icao = 0x3C_6444;
    let frames = vec![synth::adsb::squitter(
        icao,
        synth::adsb::me_identification("DLH123"),
    )];
    let mut iq = synth::adsb::transmission(&frames, 500.0, 0.8, RATE);
    synth::shift(&mut iq, offset_hz, RATE);
    let device = plant(dir.path(), "wideband", iq, RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Adsb(AdsbParams::default()),
            blanker: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Adsb(a) if a.callsign.is_some()),
    )
    .await;
    let DecoderEvent::Adsb(message) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(message.icao, "3C6444");
    assert_eq!(message.callsign.as_deref(), Some("DLH123"));
}

#[tokio::test]
async fn rds_station_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    const RATE: f64 = 960_000.0;
    let offset_hz = 200_000.0;

    let station = synth::rds::Station {
        pi: 0xD3C2,
        ps: "SDR-M4  ".to_owned(),
        radiotext: "engine end to end".to_owned(),
        pty: 10,
        tp: true,
        ta: false,
        music: true,
        alt_freqs_hz: vec![89_800_000.0, 95_500_000.0],
    };
    let mut iq = synth::rds::transmission(&station, 6.0, Some(1_000.0), RATE);
    synth::shift(&mut iq, offset_hz, RATE);

    let device = plant(dir.path(), "rds", iq, RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Wfm(WfmParams {
                deemphasis_us: 50.0,
                stereo: false,
            }),
            blanker: Default::default(),
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

    let station = synth::rds::Station {
        pi: 0xD3C2,
        ps: "RETUNE  ".to_owned(),
        radiotext: "retune resets the picture".to_owned(),
        pty: 10,
        tp: true,
        ta: false,
        music: true,
        alt_freqs_hz: Vec::new(),
    };
    let mut iq = synth::rds::transmission(&station, 12.0, Some(1_000.0), RATE);
    synth::shift(&mut iq, offset_hz, RATE);
    let device = plant(dir.path(), "rds_retune", iq, RATE);

    let settings = |offset_hz: f64| ChannelSettings {
        frequency_hz: CENTER_HZ + offset_hz,
        squelch: sdrmm_wire::Squelch::Off,
        params: ChannelParams::Wfm(WfmParams {
            deemphasis_us: 50.0,
            stereo: false,
        }),
        blanker: Default::default(),
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

#[tokio::test]
async fn a_dmr_call_reaches_the_symbol_stream_with_its_measurement() {
    let call = synth::dv::dmr::Call::default();
    let iq = synth::dv::dmr::transmission(&call, AUDIO_DEVICE_RATE);
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let device = plant(dir.path(), "dmr_symbols", iq, AUDIO_DEVICE_RATE);

    let ds = engine.create_device_set(&device).unwrap();
    let ch = engine
        .add_channel(
            ds,
            0,
            ChannelSettings {
                frequency_hz: CENTER_HZ,
                squelch: sdrmm_wire::Squelch::Off,
                params: ChannelParams::Dmr(DmrParams::default()),
                blanker: Default::default(),
            },
        )
        .unwrap();

    let mut rx = engine.subscribe_symbols(ds, ch).unwrap();
    let block = tokio::time::timeout(DECODE_TIMEOUT, async {
        loop {
            match rx.recv().await {
                Ok(block) => return block,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("symbol stream closed")
                }
            }
        }
    })
    .await
    .expect("a symbol block within the timeout");
    engine.remove_device_set(ds).unwrap();

    assert_eq!(block.plane, SymbolPlane::Level);
    assert_eq!(block.symbol_rate, 4_800.0);
    assert_eq!(&*block.reference, &[1.0, 3.0, -1.0, -3.0]);
    assert_eq!(block.symbols.len(), 480);
    assert!(
        block.symbols.iter().all(|s| s.is_finite()),
        "the cloud carried a value nothing can plot"
    );
    assert!(
        block.margin > 1.5,
        "a fixture call left only {} of slicing margin",
        block.margin
    );
    assert!(
        block.mer_db > 8.0,
        "a fixture call measured {} dB MER",
        block.mer_db
    );
}

#[tokio::test]
async fn an_analog_channel_never_pretends_to_have_symbols() {
    let iq = synth::dv::dmr::transmission(&synth::dv::dmr::Call::default(), AUDIO_DEVICE_RATE);
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let device = plant(dir.path(), "nfm_no_symbols", iq, AUDIO_DEVICE_RATE);

    let ds = engine.create_device_set(&device).unwrap();
    let ch = engine
        .add_channel(
            ds,
            0,
            ChannelSettings {
                frequency_hz: CENTER_HZ,
                squelch: sdrmm_wire::Squelch::Off,
                params: ChannelParams::Nfm(NfmParams::default()),
                blanker: Default::default(),
            },
        )
        .unwrap();

    let mut rx = engine.subscribe_symbols(ds, ch).unwrap();
    let got = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await;
    engine.remove_device_set(ds).unwrap();
    assert!(got.is_err(), "an FM channel published a symbol block");
}

#[tokio::test]
async fn a_dect_base_station_survives_the_ddc_and_reports_its_identity_and_security() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());

    let station = synth::dect::Station {
        rfpi: 0x0001_234D_5E6D,
        carrier: 4,
        slot: 2,
        slot_pair: 2,
        capabilities: synth::dect::capability_bits(&[17, 33, 36, 37]),
        ..synth::dect::Station::default()
    };
    let iq = synth::dect::dummy_bearer(&station, 40);

    let device = plant(dir.path(), "dect", iq, DECT_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Dect(DectParams::default()),
            blanker: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Dect(f) if f.identity.is_some()),
    )
    .await;

    let DecoderEvent::Dect(frame) = record.event else {
        unreachable!("filtered above")
    };
    let identity = frame.identity.unwrap();
    assert_eq!(identity.rfpi, "01234D5E6D");
    assert_eq!(identity.emc, Some(0x1234));
    assert_eq!(frame.crc_errors, 0);
}

#[tokio::test]
async fn a_dect_capabilities_broadcast_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());

    let station = synth::dect::Station {
        rfpi: 0x0001_234D_5E6D,
        capabilities: synth::dect::capability_bits(&[17, 33, 36, 37]),
        ..synth::dect::Station::default()
    };
    let iq = synth::dect::dummy_bearer(&station, 40);

    let device = plant(dir.path(), "dect_caps", iq, DECT_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Dect(DectParams::default()),
            blanker: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Dect(f) if !f.capabilities.is_empty()),
    )
    .await;

    let DecoderEvent::Dect(frame) = record.event else {
        unreachable!("filtered above")
    };
    assert!(frame.has(DectCapability::StandardAuthentication));
    assert!(frame.has(DectCapability::StandardCiphering));
    assert_eq!(frame.security.authentication_supported, Some(true));
    assert_eq!(frame.security.ciphering_supported, Some(true));
    assert_eq!(frame.security.cipher_state, DectCipherState::Clear);
}

#[tokio::test]
async fn broadcast_audio_reaches_stereo_opus_through_virtual_devices() {
    for system in [
        BroadcastSystem::DabPlus,
        BroadcastSystem::Dab,
        BroadcastSystem::DvbS,
        BroadcastSystem::DvbS2,
    ] {
        let dir = TempDir::new().unwrap();
        let engine = engine_for(dir.path());
        let (iq, rate, params) = match system {
            BroadcastSystem::DabPlus => (
                synth::dab::ensemble(30),
                2048000.0,
                ChannelParams::Dab(DabParams {
                    mode: sdrmm_wire::DabMode::DabPlus,
                    ..DabParams::default()
                }),
            ),
            BroadcastSystem::Dab => (
                synth::dab::ensemble(30),
                2048000.0,
                ChannelParams::Dab(DabParams {
                    mode: sdrmm_wire::DabMode::Dab,
                    ..DabParams::default()
                }),
            ),
            BroadcastSystem::DvbS => (
                synth::datv::dvbs(4),
                2000000.0,
                ChannelParams::Datv(DatvParams {
                    symbol_rate: synth::datv::SYMBOL_RATE,
                    code_rate: synth::datv::CODE_RATE,
                    ..DatvParams::default()
                }),
            ),
            BroadcastSystem::DvbS2 => (
                synth::datv::dvbs2(4),
                2000000.0,
                ChannelParams::Datv(DatvParams {
                    standard: DatvStandard::DvbS2,
                    symbol_rate: synth::datv::SYMBOL_RATE,
                    ..DatvParams::default()
                }),
            ),
            _ => unreachable!(),
        };
        let mut statuses = engine.subscribe_decoded();
        let device = plant(dir.path(), "broadcast-audio", iq, rate);
        let ds = engine.create_device_set(&device).unwrap();
        let ch = engine
            .add_channel(
                ds,
                0,
                ChannelSettings {
                    frequency_hz: CENTER_HZ,
                    squelch: sdrmm_wire::Squelch::Off,
                    params,
                    blanker: Default::default(),
                },
            )
            .unwrap();
        let mut receiver = engine.subscribe_audio(ds, ch).unwrap();
        let mut packets = Vec::new();
        for _ in 0..12 {
            let next = tokio::time::timeout(Duration::from_secs(12), receiver.recv()).await;
            if let Ok(Ok(packet)) = next {
                packets.push(packet);
            } else {
                let mut last = None;
                while let Ok(record) = statuses.try_recv() {
                    last = Some(record.event);
                }
                panic!("{system:?}: audio stalled, last status {last:?}");
            }
        }

        assert!(
            packets.iter().all(|packet| packet.channels == 2),
            "{system:?}"
        );
        let mut decoder = opus::Decoder::new(48000, opus::Channels::Stereo).unwrap();
        let mut channels = vec![Vec::new(), Vec::new()];
        let mut pcm = vec![0.0; 1920];
        for packet in &packets {
            let frames = decoder.decode_float(&packet.opus, &mut pcm, false).unwrap();
            for (index, sample) in pcm[..frames * 2].iter().enumerate() {
                channels[index % 2].push(*sample);
            }
        }
        if system == BroadcastSystem::DabPlus {
            for (samples, frequency) in channels.iter().zip([700.0, 1300.0]) {
                let power = |frequency: f64| {
                    let sum = samples.iter().enumerate().fold(
                        Complex::new(0.0f64, 0.0),
                        |sum, (i, sample)| {
                            sum + Complex::from_polar(
                                f64::from(*sample),
                                std::f64::consts::TAU * frequency * i as f64 / 48000.0,
                            )
                        },
                    );
                    sum.norm_sqr()
                };
                assert!(power(frequency) > 10.0 * power(2300.0));
            }
        } else {
            common::assert_tone_dominates(&channels);
        }

        engine.remove_device_set(ds).unwrap();
    }
}

#[tokio::test]
async fn dab_pad_slideshow_crosses_the_virtual_receiver_and_decoded_event_stream() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let device = plant(
        dir.path(),
        "dab-slideshow",
        synth::dab::ensemble(30),
        2_048_000.0,
    );
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Dab(DabParams::default()),
            blanker: Default::default(),
        },
        |event| matches!(event, DecoderEvent::BroadcastData(_)),
    )
    .await;
    let DecoderEvent::BroadcastData(data) = record.event else {
        unreachable!()
    };
    assert_eq!(data.name, "slide.png");
    assert_eq!(data.media_type, "image/png");
    assert_eq!(
        data.bytes,
        include_bytes!("../../../fixtures/broadcast_audio/slideshow.png")
    );
}

#[tokio::test]
async fn an_aprs_weather_report_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
    let frame = AprsTx::ui_frame(
        "DL1WX-13",
        "APRS",
        &[],
        "!4903.50N/07201.75W_220/004g005t077r000p000P000h50b09900",
    );
    let iq = synth::resample(
        &aprs_burst(frame),
        AprsTx::descriptor().input_rate_hz,
        NARROW_DEVICE_RATE,
    );
    let device = plant(dir.path(), "aprs_wx", iq, NARROW_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Aprs(AprsParams::default()),
            blanker: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Aprs(p) if p.weather.is_some()),
    )
    .await;
    let DecoderEvent::Aprs(packet) = record.event else {
        unreachable!("filtered above")
    };
    let weather = packet.weather.unwrap();
    assert_eq!(weather.wind_dir_deg, Some(220));
    assert_eq!(weather.humidity_pct, Some(50));
}

#[tokio::test]
async fn an_rs41_radiosonde_survives_the_ddc_and_reports_its_position() {
    let dir = TempDir::new().unwrap();
    let engine = accelerated_engine_for(dir.path());
    let offset_hz = 25_000.0;
    let native = synth::radiosonde::transmission(SondeType::Rs41, 6, AUDIO_DEVICE_RATE);
    let mut iq = synth::resample(&native, AUDIO_DEVICE_RATE, NARROW_DEVICE_RATE);
    synth::shift(&mut iq, offset_hz, NARROW_DEVICE_RATE);
    let device = plant(dir.path(), "rs41", iq, NARROW_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Radiosonde(RadiosondeParams::default()),
            blanker: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Radiosonde(f) if f.lat.is_some()),
    )
    .await;
    let DecoderEvent::Radiosonde(frame) = record.event.clone() else {
        unreachable!("filtered above")
    };
    let truth = synth::radiosonde::flight(SondeType::Rs41);
    assert_eq!(frame.sonde, SondeType::Rs41);
    assert_eq!(frame.serial, truth.serial);
    assert!(DecoderEvent::Radiosonde(frame).position().is_some());
}

#[tokio::test]
async fn a_noaa_apt_pass_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = accelerated_engine_for(dir.path());
    const APT_RATE: f64 = 60_000.0;
    let offset_hz = 50_000.0;
    let native = synth::apt::transmission(160, AvhrrChannel::Ch2, AvhrrChannel::Ch4, APT_RATE);
    let mut iq = synth::resample(&native, APT_RATE, NARROW_DEVICE_RATE);
    synth::shift(&mut iq, offset_hz, NARROW_DEVICE_RATE);
    iq.extend(synth::silence((NARROW_DEVICE_RATE * 12.0) as usize));
    let device = plant(dir.path(), "apt", iq, NARROW_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Apt(AptParams::default()),
            blanker: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Apt(_)),
    )
    .await;
    let DecoderEvent::Apt(image) = record.event else {
        unreachable!("filtered above")
    };
    assert!(image.lines >= 150, "{} lines", image.lines);
    assert_eq!(image.channel_a, Some(AvhrrChannel::Ch2));
    assert_eq!(image.channel_b, Some(AvhrrChannel::Ch4));
}

#[tokio::test]
async fn a_wefax_chart_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = accelerated_engine_for(dir.path());
    const WEFAX_RATE: f64 = 12_000.0;
    let offset_hz = 5_000.0;
    let chart = synth::wefax::bars(WefaxIoc::Ioc576, 60);
    let native = synth::wefax::transmission(WefaxIoc::Ioc576, WefaxLpm::Lpm240, &chart, WEFAX_RATE);
    let mut iq = synth::resample(&native, WEFAX_RATE, AUDIO_DEVICE_RATE);
    synth::shift(&mut iq, offset_hz, AUDIO_DEVICE_RATE);
    let device = plant(dir.path(), "wefax", iq, AUDIO_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Wefax(WefaxParams {
                lpm: WefaxLpm::Lpm240,
                ..WefaxParams::default()
            }),
            blanker: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Wefax(_)),
    )
    .await;
    let DecoderEvent::Wefax(picture) = record.event else {
        unreachable!("filtered above")
    };
    assert!(picture.complete);
    assert_eq!(picture.lines, 60);
    assert_eq!(picture.width, 1_810);
}

#[tokio::test]
async fn a_meteor_lrpt_pass_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = accelerated_engine_for(dir.path());
    const LRPT_RATE: f64 = 288_000.0;
    const LRPT_DEVICE_RATE: f64 = 576_000.0;
    let offset_hz = 100_000.0;
    let native = synth::lrpt::transmission(LrptMode::Oqpsk72, 4, LRPT_RATE);
    let mut iq = synth::resample(&native, LRPT_RATE, LRPT_DEVICE_RATE);
    synth::shift(&mut iq, offset_hz, LRPT_DEVICE_RATE);
    iq.extend(synth::silence((LRPT_DEVICE_RATE * 4.0) as usize));
    let device = plant(dir.path(), "lrpt", iq, LRPT_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            frequency_hz: CENTER_HZ + offset_hz,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Lrpt(LrptParams::default()),
            blanker: Default::default(),
        },
        |event| matches!(event, DecoderEvent::Lrpt(_)),
    )
    .await;
    let DecoderEvent::Lrpt(image) = record.event else {
        unreachable!("filtered above")
    };
    assert_eq!(image.width, 1_568);
    assert_eq!(image.lines, 32);
    assert_eq!(image.frames_failed, 0);
    assert_eq!(image.packets_lost, 0);
}
