use num_complex::Complex;
use sdrmm_channels::{
    AmTx, AprsTx, ChannelCtx, ChannelTx, SsbTx, TxPayload,
    testgen::{self, dv, weak_signal},
};
use sdrmm_wire::{
    AmParams, ArgumentOption, AtvModulation, AtvParams, AtvStandard, ChannelParams,
    ChannelSettings, DabTransmissionMode, PskBaud, SelcallSystem, Sideband, Squelch, SsbParams,
    SstvMode,
};

pub struct Signal {
    pub id: &'static str,
    pub label: &'static str,
    pub rate_hz: f64,
    pub render: fn() -> Vec<Complex<f32>>,
}

const NARROW: f64 = 240_000.0;
const AUDIO: f64 = 48_000.0;
const WIDE_FM: f64 = 960_000.0;
const WIDEBAND: f64 = 2_000_000.0;
const CENTER_HZ: f64 = 145_000_000.0;
const VOICE_SECONDS: f64 = 3.0;

pub static SIGNALS: &[Signal] = &[
    Signal {
        id: "tone",
        label: "Carrier tone",
        rate_hz: NARROW,
        render: tone,
    },
    Signal {
        id: "two_tone",
        label: "Two tone",
        rate_hz: NARROW,
        render: two_tone,
    },
    Signal {
        id: "noise",
        label: "Noise",
        rate_hz: NARROW,
        render: noise,
    },
    Signal {
        id: "sweep",
        label: "Sweep",
        rate_hz: NARROW,
        render: sweep,
    },
    Signal {
        id: "nfm_voice",
        label: "NFM voice",
        rate_hz: NARROW,
        render: nfm_voice,
    },
    Signal {
        id: "nfm_ctcss",
        label: "NFM voice · CTCSS 88.5",
        rate_hz: NARROW,
        render: nfm_ctcss,
    },
    Signal {
        id: "nfm_dcs",
        label: "NFM voice · DCS 023",
        rate_hz: NARROW,
        render: nfm_dcs,
    },
    Signal {
        id: "am_voice",
        label: "AM voice",
        rate_hz: NARROW,
        render: am_voice,
    },
    Signal {
        id: "ssb_voice",
        label: "SSB voice · USB",
        rate_hz: AUDIO,
        render: ssb_voice,
    },
    Signal {
        id: "wfm_stereo",
        label: "FM broadcast · stereo",
        rate_hz: WIDE_FM,
        render: wfm_stereo,
    },
    Signal {
        id: "wfm_rds",
        label: "FM broadcast · RDS",
        rate_hz: WIDE_FM,
        render: wfm_rds,
    },
    Signal {
        id: "pocsag",
        label: "POCSAG 1200",
        rate_hz: NARROW,
        render: pocsag,
    },
    Signal {
        id: "flex",
        label: "FLEX",
        rate_hz: NARROW,
        render: flex,
    },
    Signal {
        id: "ermes",
        label: "ERMES",
        rate_hz: NARROW,
        render: ermes,
    },
    Signal {
        id: "acars",
        label: "ACARS",
        rate_hz: NARROW,
        render: acars,
    },
    Signal {
        id: "aprs",
        label: "APRS · AFSK1200",
        rate_hz: NARROW,
        render: aprs,
    },
    Signal {
        id: "ais",
        label: "AIS",
        rate_hz: NARROW,
        render: ais,
    },
    Signal {
        id: "adsb",
        label: "ADS-B",
        rate_hz: WIDEBAND,
        render: adsb,
    },
    Signal {
        id: "mode_s",
        label: "Mode S replies",
        rate_hz: WIDEBAND,
        render: mode_s,
    },
    Signal {
        id: "morse",
        label: "Morse",
        rate_hz: AUDIO,
        render: morse,
    },
    Signal {
        id: "rtty",
        label: "RTTY 45.45",
        rate_hz: AUDIO,
        render: rtty,
    },
    Signal {
        id: "navtex",
        label: "NAVTEX",
        rate_hz: AUDIO,
        render: navtex,
    },
    Signal {
        id: "psk31",
        label: "PSK31",
        rate_hz: AUDIO,
        render: psk31,
    },
    Signal {
        id: "selcall",
        label: "Selcall · CCIR",
        rate_hz: AUDIO,
        render: selcall,
    },
    Signal {
        id: "ft8",
        label: "FT8",
        rate_hz: AUDIO,
        render: ft8,
    },
    Signal {
        id: "ft4",
        label: "FT4",
        rate_hz: AUDIO,
        render: ft4,
    },
    Signal {
        id: "wspr",
        label: "WSPR",
        rate_hz: AUDIO,
        render: wspr,
    },
    Signal {
        id: "sstv",
        label: "SSTV · Martin M1",
        rate_hz: AUDIO,
        render: sstv,
    },
    Signal {
        id: "vor",
        label: "VOR",
        rate_hz: AUDIO,
        render: vor,
    },
    Signal {
        id: "dcf77",
        label: "DCF77 time code",
        rate_hz: 2_000.0,
        render: dcf77,
    },
    Signal {
        id: "dmr",
        label: "DMR voice",
        rate_hz: AUDIO,
        render: dmr,
    },
    Signal {
        id: "p25",
        label: "P25 voice",
        rate_hz: AUDIO,
        render: p25,
    },
    Signal {
        id: "nxdn",
        label: "NXDN voice",
        rate_hz: AUDIO,
        render: nxdn,
    },
    Signal {
        id: "dstar",
        label: "D-STAR voice",
        rate_hz: AUDIO,
        render: dstar,
    },
    Signal {
        id: "m17",
        label: "M17 voice",
        rate_hz: AUDIO,
        render: m17,
    },
    Signal {
        id: "ysf",
        label: "YSF voice",
        rate_hz: AUDIO,
        render: ysf,
    },
    Signal {
        id: "subghz",
        label: "Sub-GHz remote · OOK",
        rate_hz: 500_000.0,
        render: subghz,
    },
    Signal {
        id: "dect",
        label: "DECT",
        rate_hz: 2_304_000.0,
        render: dect,
    },
    Signal {
        id: "gnss",
        label: "GPS L1 C/A",
        rate_hz: 2_048_000.0,
        render: gnss,
    },
    Signal {
        id: "dab",
        label: "DAB ensemble",
        rate_hz: 2_048_000.0,
        render: dab,
    },
    Signal {
        id: "dvbs",
        label: "DVB-S",
        rate_hz: WIDEBAND,
        render: dvbs,
    },
    Signal {
        id: "dvbs2",
        label: "DVB-S2",
        rate_hz: WIDEBAND,
        render: dvbs2,
    },
    Signal {
        id: "dvbt",
        label: "DVB-T",
        rate_hz: 64_000_000.0 / 7.0,
        render: dvbt,
    },
    Signal {
        id: "atv",
        label: "ATV colour bars",
        rate_hz: 2_400_000.0,
        render: atv,
    },
];

pub const DEFAULT_SIGNAL: &str = "nfm_ctcss";

#[must_use]
pub fn find(id: &str) -> Option<&'static Signal> {
    SIGNALS.iter().find(|signal| signal.id == id)
}

#[must_use]
pub fn options() -> Vec<ArgumentOption> {
    SIGNALS
        .iter()
        .map(|signal| ArgumentOption {
            value: signal.id.to_owned(),
            label: Some(signal.label.to_owned()),
        })
        .collect()
}

#[must_use]
pub fn rates() -> Vec<f64> {
    let mut rates: Vec<f64> = SIGNALS.iter().map(|signal| signal.rate_hz).collect();
    rates.sort_by(f64::total_cmp);
    rates.dedup();
    rates
}

fn seconds(rate: f64, secs: f64) -> usize {
    (rate * secs) as usize
}

fn tone() -> Vec<Complex<f32>> {
    vec![Complex::new(1.0, 0.0); seconds(NARROW, 1.0)]
}

fn two_tone() -> Vec<Complex<f32>> {
    let len = seconds(NARROW, 1.0);
    let mut low = vec![Complex::new(0.5, 0.0); len];
    let mut high = low.clone();
    testgen::shift(&mut low, -10_000.0, NARROW);
    testgen::shift(&mut high, 10_000.0, NARROW);
    low.iter().zip(high).map(|(a, b)| a + b).collect()
}

fn noise() -> Vec<Complex<f32>> {
    let mut iq = testgen::silence(seconds(NARROW, 1.0));
    testgen::add_noise(&mut iq, 0x5DEE_CE66, 0.5);
    iq
}

fn sweep() -> Vec<Complex<f32>> {
    let len = seconds(NARROW, 1.0);
    let span = NARROW * 0.8;
    let mut phase = 0.0f64;
    (0..len)
        .map(|k| {
            let hz = -span / 2.0 + span * k as f64 / len as f64;
            phase += std::f64::consts::TAU * hz / NARROW;
            Complex::from_polar(1.0, phase as f32)
        })
        .collect()
}

fn voice_audio(rate: f64) -> Vec<f32> {
    testgen::nfm::speech_audio(rate, seconds(rate, VOICE_SECONDS))
}

fn nfm_voice() -> Vec<Complex<f32>> {
    testgen::fm_modulate(&voice_audio(NARROW), 2_500.0, NARROW)
}

fn nfm_ctcss() -> Vec<Complex<f32>> {
    let len = seconds(NARROW, VOICE_SECONDS);
    let audio = testgen::nfm::mix(
        &testgen::nfm::ctcss_audio(88.5, 0.15, NARROW, len),
        &testgen::nfm::speech_audio(NARROW, len),
    );
    testgen::fm_modulate(&audio, 2_500.0, NARROW)
}

fn nfm_dcs() -> Vec<Complex<f32>> {
    let len = seconds(NARROW, VOICE_SECONDS);
    let audio = testgen::nfm::mix(
        &testgen::nfm::dcs_audio(23, 0.15, NARROW, len),
        &testgen::nfm::speech_audio(NARROW, len),
    );
    testgen::fm_modulate(&audio, 2_500.0, NARROW)
}

fn voice_settings(params: ChannelParams) -> ChannelSettings {
    ChannelSettings {
        frequency_hz: CENTER_HZ,
        squelch: Squelch::Off,
        params,
        blanker: Default::default(),
    }
}

fn modulated_voice(
    rate: f64,
    build: fn(ChannelCtx, ChannelSettings) -> Option<Box<dyn ChannelTx>>,
    params: ChannelParams,
) -> Vec<Complex<f32>> {
    let Some(mut tx) = build(ChannelCtx { input_rate: rate }, voice_settings(params)) else {
        return Vec::new();
    };
    if tx
        .submit(TxPayload::Audio(voice_audio(f64::from(
            sdrmm_channels::AUDIO_RATE,
        ))))
        .is_err()
    {
        return Vec::new();
    }
    testgen::burst(tx.as_mut())
}

fn am_voice() -> Vec<Complex<f32>> {
    let rate = AmTx::descriptor().input_rate_hz;
    let iq = modulated_voice(
        rate,
        |ctx, settings| {
            AmTx::new(ctx, settings)
                .ok()
                .map(|tx| Box::new(tx) as Box<dyn ChannelTx>)
        },
        ChannelParams::Am(AmParams::default()),
    );
    testgen::resample(&iq, rate, NARROW)
}

fn ssb_voice() -> Vec<Complex<f32>> {
    let rate = SsbTx::descriptor().input_rate_hz;
    let iq = modulated_voice(
        rate,
        |ctx, settings| {
            SsbTx::new(ctx, settings)
                .ok()
                .map(|tx| Box::new(tx) as Box<dyn ChannelTx>)
        },
        ChannelParams::Ssb(SsbParams {
            sideband: Sideband::Usb,
            ..SsbParams::default()
        }),
    );
    testgen::resample(&iq, rate, AUDIO)
}

fn broadcast_audio(rate: f64) -> (Vec<f32>, Vec<f32>) {
    let len = seconds(rate, 2.0);
    (
        testgen::tone_audio(440.0, 0.5, rate, len),
        testgen::tone_audio(660.0, 0.5, rate, len),
    )
}

fn wfm_stereo() -> Vec<Complex<f32>> {
    let (left, right) = broadcast_audio(WIDE_FM);
    testgen::wfm::transmission(&left, &right, true, WIDE_FM)
}

fn wfm_rds() -> Vec<Complex<f32>> {
    let station = testgen::rds::Station {
        pi: 0xD3C2,
        ps: "SDR-M4  ".to_owned(),
        radiotext: "signal generator".to_owned(),
        pty: 10,
        tp: true,
        ta: false,
        music: true,
        alt_freqs_hz: vec![89_800_000.0, 95_500_000.0],
    };
    testgen::rds::transmission(&station, 6.0, Some(1_000.0), WIDE_FM)
}

fn pocsag() -> Vec<Complex<f32>> {
    let pages = [testgen::pocsag::Page {
        address: 1_234_567,
        function: 3,
        text: "SDR-- signal generator".to_owned(),
        numeric: false,
    }];
    testgen::pocsag::transmission(&pages, 1_200, 4_500.0, NARROW)
}

fn flex() -> Vec<Complex<f32>> {
    let page = testgen::flex::Page {
        address: 1_234_567,
        text: "SDR-- signal generator".to_owned(),
    };
    testgen::flex::transmission(&page, 4, 72, NARROW)
}

fn ermes() -> Vec<Complex<f32>> {
    let page = testgen::ermes::Page {
        local_address: 4_242,
        message_number: 1,
        text: "SDR-- signal generator".to_owned(),
        urgent: false,
        alert: 1,
    };
    testgen::ermes::transmission(&page, NARROW)
}

fn acars() -> Vec<Complex<f32>> {
    let block = testgen::acars::Block {
        mode: '2',
        registration: ".D-AIBL",
        ack: '\u{15}',
        label: "H1",
        block_id: '4',
        seq_no: Some("M01A"),
        flight: Some("LH0400"),
        text: "SDR-- signal generator",
        more: false,
    };
    testgen::acars::transmission(&block, NARROW)
}

fn aprs() -> Vec<Complex<f32>> {
    let rate = AprsTx::descriptor().input_rate_hz;
    let Ok(mut tx) = AprsTx::new(
        ChannelCtx { input_rate: rate },
        voice_settings(ChannelParams::Aprs(sdrmm_wire::AprsParams {
            mode: sdrmm_wire::AprsMode::Afsk1200,
            ..sdrmm_wire::AprsParams::default()
        })),
    ) else {
        return Vec::new();
    };
    if tx
        .submit(TxPayload::Frame(aprs_frame("SDR-- signal generator")))
        .is_err()
    {
        return Vec::new();
    }
    testgen::resample(&testgen::burst(&mut tx), rate, NARROW)
}

fn aprs_frame(text: &str) -> Vec<u8> {
    let mut frame = Vec::new();
    for call in ["APRS  ", "DL0SDR"] {
        frame.extend(call.bytes().map(|byte| byte << 1));
        frame.push(0x60);
    }
    let last = frame.len() - 1;
    frame[last] |= 1;
    frame.extend([0x03, 0xF0]);
    frame.extend(text.bytes());
    frame
}

fn ais() -> Vec<Complex<f32>> {
    let report = testgen::ais::PositionReport {
        mmsi: 244_010_000,
        lat: 52.3702,
        lon: 4.8952,
        sog_kt: 12.4,
        cog_deg: 87.5,
        heading_deg: 88,
        nav_status: 0,
    };
    testgen::ais::burst(&testgen::ais::position_payload(&report), NARROW)
}

fn adsb_frames() -> Vec<Vec<u8>> {
    let icao = 0x3C_65_AC;
    vec![
        testgen::adsb::squitter(icao, testgen::adsb::me_identification("DLH123")),
        testgen::adsb::squitter(
            icao,
            testgen::adsb::me_airborne_position(38_000, 52.2572, 3.9190, false),
        ),
        testgen::adsb::squitter(
            icao,
            testgen::adsb::me_airborne_position(38_000, 52.2657, 3.9184, true),
        ),
        testgen::adsb::squitter(icao, testgen::adsb::me_velocity(451.0, 87.0, -1_216)),
    ]
}

fn adsb() -> Vec<Complex<f32>> {
    testgen::adsb::transmission(&adsb_frames(), 500.0, 0.8, WIDEBAND)
}

fn mode_s() -> Vec<Complex<f32>> {
    let icao = 0x3C_65_AC;
    let frames = vec![
        testgen::adsb::all_call_reply(icao, 5, 0),
        testgen::adsb::identity_reply(icao, "7421", 0),
        testgen::adsb::altitude_reply(icao, 24_000, 0),
    ];
    testgen::adsb::transmission(&frames, 500.0, 0.8, WIDEBAND)
}

fn morse() -> Vec<Complex<f32>> {
    testgen::morse::transmission("CQ CQ DE SDR-- K", 20.0, 800.0, AUDIO)
}

fn rtty() -> Vec<Complex<f32>> {
    testgen::rtty::transmission("RYRYRY DE SDR-- ", 45.45, 170.0, 1.5, AUDIO)
}

fn navtex() -> Vec<Complex<f32>> {
    testgen::navtex::transmission("ZCZC FA01 SDR-- SIGNAL GENERATOR NNNN", AUDIO)
}

fn psk31() -> Vec<Complex<f32>> {
    let iq = testgen::psk::transmission("CQ CQ DE SDR-- \n", PskBaud::Psk31.rate());
    testgen::resample(&iq, 8_000.0, AUDIO)
}

fn selcall() -> Vec<Complex<f32>> {
    testgen::selcall::transmission(SelcallSystem::Ccir1, "12234", AUDIO).unwrap_or_default()
}

fn weak(slot: Vec<Complex<f32>>) -> Vec<Complex<f32>> {
    testgen::resample(&slot, 12_000.0, AUDIO)
}

fn ft8() -> Vec<Complex<f32>> {
    weak(weak_signal::ft8_slot("W1AW", "FN42", 1_500.0))
}

fn ft4() -> Vec<Complex<f32>> {
    weak(weak_signal::ft4_slot("W1AW", "FN42", 1_500.0))
}

fn wspr() -> Vec<Complex<f32>> {
    weak(weak_signal::wspr_slot("W1AW", "FN42", 33, 1_500.0))
}

fn sstv() -> Vec<Complex<f32>> {
    let mode = SstvMode::MartinM1;
    let frame = testgen::sstv::bars(mode);
    let native = testgen::sstv::transmission(mode, &frame, 16_000.0);
    testgen::resample(&native, 16_000.0, AUDIO)
}

fn vor() -> Vec<Complex<f32>> {
    testgen::vor::transmission(123.0, 2)
}

fn dcf77() -> Vec<Complex<f32>> {
    testgen::radio_clock::dcf77_example()
}

fn dmr() -> Vec<Complex<f32>> {
    dv::dmr::transmission(&dv::dmr::Call::default(), AUDIO)
}

fn p25() -> Vec<Complex<f32>> {
    dv::p25::transmission(0x293, AUDIO)
}

fn nxdn() -> Vec<Complex<f32>> {
    dv::nxdn::transmission(&dv::nxdn::Shape::default(), 1, true, AUDIO)
}

fn dstar() -> Vec<Complex<f32>> {
    dv::dstar::transmission(&dv::dstar::Call::default(), AUDIO)
}

fn m17() -> Vec<Complex<f32>> {
    dv::m17::transmission("ALL", "DL1ABC", AUDIO)
}

fn ysf() -> Vec<Complex<f32>> {
    dv::ysf::transmission_with_callsigns(
        &dv::ysf::Fich::default(),
        &dv::ysf::Call::default(),
        AUDIO,
    )
}

fn subghz() -> Vec<Complex<f32>> {
    let remote = testgen::subghz::Pwm {
        bits: vec![true, false, true, true, false, false, true, false],
        short_us: 350,
        long_multiple: 2,
        sync_gap_multiple: 30,
        repeats: 4,
    };
    testgen::subghz::pwm(&remote, 500_000.0)
}

fn dect() -> Vec<Complex<f32>> {
    let station = testgen::dect::Station {
        capabilities: testgen::dect::capability_bits(&[17, 33, 36, 37]),
        ..testgen::dect::Station::default()
    };
    testgen::dect::dummy_bearer(&station, 40)
}

fn gnss() -> Vec<Complex<f32>> {
    testgen::gnss::acquisition(7, 1_000.0, 317, 1_000)
}

fn dab() -> Vec<Complex<f32>> {
    testgen::dab::ensemble_for_mode(DabTransmissionMode::I, 16)
}

fn dvbs() -> Vec<Complex<f32>> {
    testgen::datv::dvbs(4)
}

fn dvbs2() -> Vec<Complex<f32>> {
    testgen::datv::dvbs2(4)
}

fn dvbt() -> Vec<Complex<f32>> {
    testgen::dvbt::waveform(testgen::dvbt::defaults(), 544)
}

fn atv() -> Vec<Complex<f32>> {
    let params = AtvParams {
        modulation: AtvModulation::Am,
        standard: AtvStandard::Ccir625,
        ..AtvParams::default()
    };
    let source = testgen::atv::AtvSource::new(&params, 2_400_000.0);
    testgen::atv::bars(&source, 8)
}
