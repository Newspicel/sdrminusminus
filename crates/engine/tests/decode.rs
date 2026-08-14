#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{path::Path, sync::Arc, time::Duration};

use num_complex::Complex;
use sdrmm_channels::{AprsTx, ChannelCtx, ChannelTx, MicE, MicEBit, TxPayload, testgen};
use sdrmm_device::DeviceRegistry;
use sdrmm_device_virtual::VirtualDriver;
use sdrmm_engine::Engine;
use sdrmm_recorder::SigmfWriter;
use sdrmm_wire::{
    AcarsParams, AdsbParams, AisChannel, AisParams, AprsMode, AprsParams, ChannelParams,
    ChannelSettings, DecodedRecord, DecoderEvent, MorseParams, NavtexParams, NfmParams,
    NfmToneMode, PocsagBaud, PocsagParams, RdsUpdate, RttyParams, SubghzEncoding, SubghzParams,
    WfmParams,
};
use tempfile::TempDir;

/// Playback is real-time paced, so a case's transmission length is also its wall-clock cost;
/// the timeout has to cover a full loop pass plus scheduler slack on a loaded CI box.
const DECODE_TIMEOUT: Duration = Duration::from_secs(30);

/// Device rate for the narrowband decoders: 5× their 48 kHz channel rate, so the DDC really
/// mixes and decimates instead of passing through.
const NARROW_DEVICE_RATE: f64 = 240_000.0;
/// Device rate for the audio-rate decoders (RTTY, Morse run at 8 kHz).
const AUDIO_DEVICE_RATE: f64 = 48_000.0;
/// ADS-B fills its whole 2 Msps channel, so it is the one mode that cannot be resampled into
/// place: the device has to run at exactly the channel rate (see the wideband check in
/// `validate_channel`). Everything else here deliberately runs at a different device rate.
const ADSB_DEVICE_RATE: f64 = 2_000_000.0;
const CENTER_HZ: f64 = 145_000_000.0;
/// The AX.25 burst a station would key for `frame`, straight out of the modulator that pairs
/// with the decoder under test. A modulator produces its own channel rate and nothing else, so
/// the caller resamples it to whatever the device replays at.
fn aprs_burst(frame: Vec<u8>) -> Vec<Complex<f32>> {
    let mut tx = AprsTx::new(
        ChannelCtx {
            input_rate: AprsTx::descriptor().input_rate_hz,
        },
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            params: ChannelParams::Aprs(AprsParams {
                mode: AprsMode::Afsk1200,
                ..AprsParams::default()
            }),
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

/// Write `iq` as a finalized SigMF pair and return the `virtual:file:` device id that replays
/// it. Padding to at least `rate` samples keeps a short burst from looping so tightly that the
/// decoder never sees a clean lead-in.
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

/// Open `device_id`, add one channel, and wait for the first decoded record that `want`
/// accepts. Tears the set down before returning so a failing assertion cannot leave a
/// real-time playback thread running.
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
                // Drop-oldest is the contract for this stream; a starved runner may lag.
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
    // The pump stamps wall-clock time off the DSP thread; the log's index is a text
    // comparison over exactly this string, so the format is part of the contract.
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
            params: ChannelParams::Pocsag(PocsagParams {
                baud: PocsagBaud::Auto,
                ..PocsagParams::default()
            }),
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
            params: ChannelParams::Aprs(AprsParams {
                mode: AprsMode::Afsk1200,
                ..AprsParams::default()
            }),
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
            params: ChannelParams::Ais(AisParams {
                ais_channel: AisChannel::B,
            }),
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

/// Mic-E's position is split between the destination callsign and a binary information field,
/// so the whole pipeline has to carry both halves for either to mean anything.
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
            params: ChannelParams::Aprs(AprsParams::default()),
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

/// Subaudible signalling is the one decoder whose events describe a channel rather than a
/// message, and the one that shares a channel with audio — so the plumbing worth proving is
/// that it reaches the decoded stream at all while the NFM audio path goes on working.
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
            params: ChannelParams::Nfm(NfmParams {
                tone_mode: NfmToneMode::Ctcss,
                ctcss_hz: Some(88.5),
                ..NfmParams::default()
            }),
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

/// ADS-B end to end at 2 Msps, the lowest rate that carries it — one sample per half-chip.
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
            params: ChannelParams::Adsb(AdsbParams::default()),
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

/// A roll-call reply carries its address only on the parity, so it is decodable only in the
/// company of the frames that proved that address. The whole transmission has to reach the
/// decoder in order for the last frame in it to mean anything.
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
    // A generous gap: a short frame is only scanned once a long frame's worth of samples sits
    // behind it, so the transmission must not end on one.
    let iq = testgen::adsb::transmission(&frames, 500.0, 0.8, ADSB_DEVICE_RATE);

    let device = plant(dir.path(), "modes", iq, ADSB_DEVICE_RATE);
    let record = decode_first(
        &engine,
        &device,
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            params: ChannelParams::Adsb(AdsbParams::default()),
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
    let engine = engine_for(dir.path());
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
            params: ChannelParams::Rtty(params),
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
async fn morse_text_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
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
            params: ChannelParams::Morse(MorseParams::default()),
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
async fn navtex_broadcast_survives_the_ddc_and_reaches_the_decoded_stream() {
    let dir = TempDir::new().unwrap();
    let engine = engine_for(dir.path());
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
            params: ChannelParams::Navtex(NavtexParams::default()),
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
            params: ChannelParams::Acars(AcarsParams::default()),
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

/// The widest channel in the registry: 250 kHz out of a 500 kHz device, so the DDC decimates
/// hard while the decoder is still timing 320 µs edges off the result.
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
            params: ChannelParams::Subghz(SubghzParams::default()),
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
            params: ChannelParams::Adsb(AdsbParams::default()),
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

/// Above its range ADS-B is refused with an actionable message rather than run: the scan costs
/// a magnitude per sample, and a 20 Msps receiver would spend the DSP thread on samples no
/// slicer can use. A refusal that names the range is the difference between a setting to change
/// and a receiver that looks broken.
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
                params: ChannelParams::Adsb(AdsbParams::default()),
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

/// RDS rides on the WFM channel rather than a channel type of its own, so this case also
/// proves the two halves coexist: the station's identity arrives on the decoded stream while
/// the audio keeps demodulating.
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
            params: ChannelParams::Wfm(WfmParams {
                deemphasis_us: 50.0,
                stereo: false,
            }),
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

/// A retune is a different station. The engine sends no settings command for an offset-only
/// patch, so `DspCommand::Retune` is the only path a decoder learns it moved — this drives
/// exactly that path and asserts the accreted RDS picture did not follow the channel.
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
        params: ChannelParams::Wfm(WfmParams {
            deemphasis_us: 50.0,
            stereo: false,
        }),
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
