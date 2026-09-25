use sdrmm_wire::channel::{
    AeroChannel, AisChannel, AprsMode, AtvColor, AtvModulation, AtvStandard, ChannelParams,
    DabMode, DabTransmissionMode, DatvCodeRate, DatvRollOff, DatvStandard, DectBand, DectSides,
    DmrSlots, DrmMode, DvbtBandwidth, DvbtStandard, IlsComponent, IridiumSpan, NfmScramblerMode,
    NfmToneMode, NxdnBandwidth, PocsagBaud, PskBaud, RadioClockStandard, RttyStopBits,
    SelcallSystem, Sideband, SubghzModulation,
};
use zgui::prelude::*;

use super::{
    controls::{
        Ctx, bandwidth, listed, number, presets, segmented, select, service_picker, toggle,
    },
    settings::{NumberLimit, scaled_limit},
    tables::{
        AERO_CHANNELS, AIS_CHANNELS, APRS_MODES, ATV_COLORS, ATV_MODULATIONS, ATV_STANDARDS,
        CTCSS_DEFAULT_HZ, DAB_MODES, DAB_TRANSMISSION_MODES, DATV_CODE_RATES, DATV_ROLL_OFFS,
        DATV_STANDARDS, DCS_DEFAULT_CODE, DECT_BANDS, DECT_SIDES, DEEMPHASIS_US, DMR_SLOTS,
        DRM_MODES, DRM_PLUS_BANDWIDTHS_HZ, DRM30_BANDWIDTHS_HZ, DVBT_BANDWIDTHS, DVBT_STANDARDS,
        ILS_COMPONENTS, INVERSION_DEFAULT_HZ, IRIDIUM_SPANS, NFM_SCRAMBLER_MODES, NFM_TONE_MODES,
        NXDN_WIDTHS, POCSAG_BAUDS, PSK_BAUDS, RADIO_CLOCK_STANDARDS, RTTY_BAUDS, RTTY_SHIFTS_HZ,
        RTTY_STOP_BITS, SELCALL_SYSTEMS, SIDEBANDS, SUBGHZ_MODULATIONS, ctcss_options, dcs_options,
        drm_bandwidth_for, sstv_modes, tone_key,
    },
};
use crate::ui::kit_channel::{NumberSpec, setting_row, text_field};

const INVERT: Option<&str> = Some("Flip the signal's polarity; try it when nothing decodes");
const NARROW: &[f64] = &[12_500.0, 25_000.0];

macro_rules! get {
    ($variant:ident, $p:ident => $e:expr) => {
        |params: &ChannelParams| match params {
            ChannelParams::$variant($p) => Some($e),
            _ => None,
        }
    };
}

macro_rules! set {
    ($variant:ident, $p:ident, $v:ident => $e:expr) => {
        |params: &mut ChannelParams, $v| {
            if let ChannelParams::$variant($p) = params {
                $e;
            }
        }
    };
}

fn rows(rows: Vec<AnyView>) -> AnyView {
    AnyView::new(view! { column(class = "kc-grid") {{rows}} })
}

pub fn mode_view(ctx: Ctx, kind: &str) -> AnyView {
    match kind {
        "nfm" => nfm(ctx),
        "selcall" => rows(vec![segmented(
            ctx,
            "Tone plan",
            None,
            SELCALL_SYSTEMS,
            ctx.read(get!(Selcall, p => p.system), SelcallSystem::Ccir1),
            set!(Selcall, p, v => p.system = v),
        )]),
        "am" => rows(vec![bandwidth(
            ctx,
            "Bandwidth",
            &[5_000.0, 8_000.0, 10_000.0],
            ctx.read(get!(Am, p => p.bandwidth_hz), 10_000.0),
            set!(Am, p, v => p.bandwidth_hz = v),
        )]),
        "ssb" => ssb(ctx),
        "wfm" => wfm(ctx),
        "pocsag" => pocsag(ctx),
        "flex" => pager_flex(ctx),
        "ermes" => pager_ermes(ctx),
        "adsb" => rows(vec![toggle(
            ctx,
            "CRC fix",
            Some("Repair single-bit errors the checksum can pin down"),
            ctx.read(get!(Adsb, p => p.crc_fix), true),
            set!(Adsb, p, v => p.crc_fix = v),
        )]),
        "ais" => rows(vec![segmented(
            ctx,
            "Channel",
            None,
            AIS_CHANNELS,
            ctx.read(get!(Ais, p => p.ais_channel), AisChannel::A),
            set!(Ais, p, v => p.ais_channel = v),
        )]),
        "inmarsat_aero" => rows(vec![segmented(
            ctx,
            "Channel",
            Some("P: forward channel to aircraft. R/T: bursts from aircraft. C: voice circuit"),
            AERO_CHANNELS,
            ctx.read(get!(InmarsatAero, p => p.channel), AeroChannel::P),
            set!(InmarsatAero, p, v => p.channel = v),
        )]),
        "iridium" => rows(vec![segmented(
            ctx,
            "Span",
            Some("Wider spans run the radio faster and decode the middle 80%"),
            IRIDIUM_SPANS,
            ctx.read(get!(Iridium, p => p.span), IridiumSpan::Channel),
            set!(Iridium, p, v => p.span = v),
        )]),
        "aprs" => aprs(ctx),
        "rtty" => rtty(ctx),
        "morse" => morse(ctx),
        "cw_skimmer" => cw_skimmer(ctx),
        "ft8" | "ft4" | "wspr" => wsjt(ctx, kind == "wspr"),
        "psk" => psk(ctx),
        "navtex" => rows(vec![toggle(
            ctx,
            "Invert",
            INVERT,
            ctx.read(get!(Navtex, p => p.invert), false),
            set!(Navtex, p, v => p.invert = v),
        )]),
        "radio_clock" => radio_clock(ctx),
        "gnss" => gnss(ctx),
        "vor" => vor(ctx),
        "ils" => ils(ctx),
        "acars" => rows(vec![bandwidth(
            ctx,
            "Bandwidth",
            &[8_000.0, 12_500.0, 25_000.0],
            ctx.read(get!(Acars, p => p.bandwidth_hz), 12_500.0),
            set!(Acars, p, v => p.bandwidth_hz = v),
        )]),
        "subghz" => subghz(ctx),
        "atv" => atv(ctx),
        "sstv" => sstv(ctx),
        "dab" => dab(ctx),
        "datv" => datv(ctx),
        "dvbt" => dvbt(ctx),
        "drm" => drm(ctx),
        "dmr" => rows(vec![
            segmented(
                ctx,
                "Slot",
                None,
                DMR_SLOTS,
                ctx.read(get!(Dmr, p => p.slots), DmrSlots::Both),
                set!(Dmr, p, v => p.slots = v),
            ),
            toggle(
                ctx,
                "Ignore data CRC",
                None,
                ctx.read(get!(Dmr, p => p.ignore_crc), false),
                set!(Dmr, p, v => p.ignore_crc = v),
            ),
        ]),
        "nxdn" => rows(vec![segmented(
            ctx,
            "Width",
            None,
            NXDN_WIDTHS,
            ctx.read(get!(Nxdn, p => p.bandwidth), NxdnBandwidth::Narrow),
            set!(Nxdn, p, v => p.bandwidth = v),
        )]),
        "freedv" => rows(vec![segmented(
            ctx,
            "Sideband",
            None,
            SIDEBANDS,
            ctx.read(get!(Freedv, p => p.sideband), Sideband::Usb),
            set!(Freedv, p, v => p.sideband = v),
        )]),
        "ident" => ident(ctx),
        "dect" => rows(vec![
            segmented(
                ctx,
                "Band",
                None,
                DECT_BANDS,
                ctx.read(get!(Dect, p => p.band), DectBand::Eu),
                set!(Dect, p, v => p.band = v),
            ),
            segmented(
                ctx,
                "Side",
                None,
                DECT_SIDES,
                ctx.read(get!(Dect, p => p.sides), DectSides::Both),
                set!(Dect, p, v => p.sides = v),
            ),
        ]),
        _ => AnyView::new(()),
    }
}

fn nfm(ctx: Ctx) -> AnyView {
    let tone = ctx.read(get!(Nfm, p => p.tone_mode), NfmToneMode::Off);
    let scrambler = ctx.read(get!(Nfm, p => p.scrambler_mode), NfmScramblerMode::Off);
    let ctcss = ctx.read(
        get!(Nfm, p => tone_key(p.ctcss_hz.unwrap_or(CTCSS_DEFAULT_HZ))),
        tone_key(CTCSS_DEFAULT_HZ),
    );
    let dcs = ctx.read(
        get!(Nfm, p => p.dcs_code.unwrap_or(DCS_DEFAULT_CODE)),
        DCS_DEFAULT_CODE,
    );
    let carrier = ctx.read(
        get!(Nfm, p => p.inversion_hz.or(Some(INVERSION_DEFAULT_HZ))),
        None,
    );
    let tone_rows = move || match tone.get() {
        NfmToneMode::Ctcss => Some(select(
            ctx,
            "CTCSS",
            None,
            ctcss_options(),
            ctcss,
            set!(Nfm, p, v => p.ctcss_hz = Some(f64::from(v) / 10.0)),
        )),
        NfmToneMode::Dcs => Some(select(
            ctx,
            "DCS",
            None,
            dcs_options(),
            dcs,
            set!(Nfm, p, v => p.dcs_code = Some(v)),
        )),
        _ => None,
    };
    let carrier_row = move || {
        (scrambler.get() == NfmScramblerMode::Inversion).then(|| {
            number(
                ctx,
                "Carrier",
                None,
                NumberSpec::new("Inversion carrier")
                    .limit(ctx.limit("inversion_hz"))
                    .unit("Hz"),
                carrier,
                set!(Nfm, p, v => p.inversion_hz = v.or(p.inversion_hz)),
            )
        })
    };
    rows(vec![
        bandwidth(
            ctx,
            "Bandwidth",
            NARROW,
            ctx.read(get!(Nfm, p => p.bandwidth_hz), 12_500.0),
            set!(Nfm, p, v => p.bandwidth_hz = v),
        ),
        select(
            ctx,
            "Tone",
            None,
            listed(NFM_TONE_MODES),
            tone,
            set!(Nfm, p, v => {
                p.tone_mode = v;
                p.ctcss_hz.get_or_insert(CTCSS_DEFAULT_HZ);
                p.dcs_code.get_or_insert(DCS_DEFAULT_CODE);
            }),
        ),
        AnyView::new(tone_rows),
        select(
            ctx,
            "Scrambler",
            None,
            listed(NFM_SCRAMBLER_MODES),
            scrambler,
            set!(Nfm, p, v => {
                p.scrambler_mode = v;
                p.inversion_hz.get_or_insert(INVERSION_DEFAULT_HZ);
            }),
        ),
        AnyView::new(carrier_row),
        toggle(
            ctx,
            "Compander",
            Some("Expand audio that was sent with 2:1 compression"),
            ctx.read(get!(Nfm, p => p.compander), false),
            set!(Nfm, p, v => p.compander = v),
        ),
    ])
}

fn ssb(ctx: Ctx) -> AnyView {
    rows(vec![
        segmented(
            ctx,
            "Sideband",
            None,
            SIDEBANDS,
            ctx.read(get!(Ssb, p => p.sideband), Sideband::Usb),
            set!(Ssb, p, v => p.sideband = v),
        ),
        number(
            ctx,
            "Bandwidth",
            None,
            NumberSpec::new("SSB bandwidth")
                .limit(ctx.limit("bandwidth_hz"))
                .unit("Hz"),
            ctx.read(get!(Ssb, p => Some(p.bandwidth_hz)), Some(2_700.0)),
            set!(Ssb, p, v => if let Some(v) = v { p.bandwidth_hz = v }),
        ),
    ])
}

fn wfm(ctx: Ctx) -> AnyView {
    rows(vec![
        select(
            ctx,
            "De-emphasis",
            None,
            listed(DEEMPHASIS_US),
            ctx.read(get!(Wfm, p => p.deemphasis_us.round() as u32), 50),
            set!(Wfm, p, v => p.deemphasis_us = v as f32),
        ),
        toggle(
            ctx,
            "Stereo",
            Some("Decode the stereo pilot; mono is quieter on weak signals"),
            ctx.read(get!(Wfm, p => p.stereo), true),
            set!(Wfm, p, v => p.stereo = v),
        ),
    ])
}

fn pocsag(ctx: Ctx) -> AnyView {
    rows(vec![
        select(
            ctx,
            "Baud",
            None,
            listed(POCSAG_BAUDS),
            ctx.read(get!(Pocsag, p => p.baud), PocsagBaud::Auto),
            set!(Pocsag, p, v => p.baud = v),
        ),
        bandwidth(
            ctx,
            "Bandwidth",
            NARROW,
            ctx.read(get!(Pocsag, p => p.bandwidth_hz), 12_500.0),
            set!(Pocsag, p, v => p.bandwidth_hz = v),
        ),
        toggle(
            ctx,
            "Invert",
            INVERT,
            ctx.read(get!(Pocsag, p => p.invert), false),
            set!(Pocsag, p, v => p.invert = v),
        ),
    ])
}

fn pager_flex(ctx: Ctx) -> AnyView {
    rows(vec![
        bandwidth(
            ctx,
            "Bandwidth",
            NARROW,
            ctx.read(get!(Flex, p => p.bandwidth_hz), 12_500.0),
            set!(Flex, p, v => p.bandwidth_hz = v),
        ),
        toggle(
            ctx,
            "Invert FLEX",
            INVERT,
            ctx.read(get!(Flex, p => p.invert), false),
            set!(Flex, p, v => p.invert = v),
        ),
    ])
}

fn pager_ermes(ctx: Ctx) -> AnyView {
    rows(vec![
        bandwidth(
            ctx,
            "Bandwidth",
            NARROW,
            ctx.read(get!(Ermes, p => p.bandwidth_hz), 12_500.0),
            set!(Ermes, p, v => p.bandwidth_hz = v),
        ),
        toggle(
            ctx,
            "Invert ERMES",
            INVERT,
            ctx.read(get!(Ermes, p => p.invert), false),
            set!(Ermes, p, v => p.invert = v),
        ),
    ])
}

fn aprs(ctx: Ctx) -> AnyView {
    rows(vec![
        select(
            ctx,
            "Mode",
            None,
            listed(APRS_MODES),
            ctx.read(get!(Aprs, p => p.mode), AprsMode::Afsk1200),
            set!(Aprs, p, v => p.mode = v),
        ),
        bandwidth(
            ctx,
            "Bandwidth",
            NARROW,
            ctx.read(get!(Aprs, p => p.bandwidth_hz), 12_500.0),
            set!(Aprs, p, v => p.bandwidth_hz = v),
        ),
    ])
}

fn rtty(ctx: Ctx) -> AnyView {
    rows(vec![
        presets(
            ctx,
            "Baud",
            NumberSpec::new("RTTY baud").limit(ctx.limit("baud")),
            RTTY_BAUDS,
            ctx.read(get!(Rtty, p => p.baud), 45.45),
            set!(Rtty, p, v => p.baud = v),
        ),
        presets(
            ctx,
            "Shift",
            NumberSpec::new("RTTY shift")
                .limit(ctx.limit("shift_hz"))
                .unit("Hz"),
            RTTY_SHIFTS_HZ,
            ctx.read(get!(Rtty, p => p.shift_hz), 170.0),
            set!(Rtty, p, v => p.shift_hz = v),
        ),
        select(
            ctx,
            "Stop bits",
            None,
            listed(RTTY_STOP_BITS),
            ctx.read(get!(Rtty, p => p.stop_bits), RttyStopBits::OneAndHalf),
            set!(Rtty, p, v => p.stop_bits = v),
        ),
        toggle(
            ctx,
            "Invert",
            INVERT,
            ctx.read(get!(Rtty, p => p.invert), false),
            set!(Rtty, p, v => p.invert = v),
        ),
        toggle(
            ctx,
            "Unshift on space",
            Some("Drop back to letters after a space, as most stations expect"),
            ctx.read(get!(Rtty, p => p.unshift_on_space), true),
            set!(Rtty, p, v => p.unshift_on_space = v),
        ),
    ])
}

fn morse(ctx: Ctx) -> AnyView {
    rows(vec![
        number(
            ctx,
            "Bandwidth",
            None,
            NumberSpec::new("CW filter bandwidth")
                .limit(ctx.limit("bandwidth_hz"))
                .unit("Hz"),
            ctx.read(get!(Morse, p => Some(p.bandwidth_hz)), Some(400.0)),
            set!(Morse, p, v => if let Some(v) = v { p.bandwidth_hz = v }),
        ),
        number(
            ctx,
            "Speed",
            Some("Empty tracks the speed by itself"),
            NumberSpec::new("Morse speed")
                .limit(ctx.limit("wpm"))
                .unit("WPM")
                .optional("auto"),
            ctx.read(get!(Morse, p => p.wpm.map(f64::from)), None),
            set!(Morse, p, v => p.wpm = v.map(|v| v as f32)),
        ),
    ])
}

fn cw_skimmer(ctx: Ctx) -> AnyView {
    rows(vec![
        number(
            ctx,
            "Passband",
            None,
            NumberSpec::new("CW skimmer passband")
                .limit(ctx.limit("bandwidth_hz"))
                .unit("Hz"),
            ctx.read(get!(CwSkimmer, p => Some(p.bandwidth_hz)), Some(24_000.0)),
            set!(CwSkimmer, p, v => if let Some(v) = v { p.bandwidth_hz = v }),
        ),
        number(
            ctx,
            "Acquire",
            Some("Carrier threshold above the noise floor"),
            NumberSpec::new("Carrier threshold")
                .limit(ctx.limit("threshold_db"))
                .unit("dB SNR"),
            ctx.read(
                get!(CwSkimmer, p => Some(f64::from(p.threshold_db))),
                Some(10.0),
            ),
            set!(CwSkimmer, p, v => if let Some(v) = v { p.threshold_db = v as f32 }),
        ),
        number(
            ctx,
            "Signals",
            Some("Most CW signals decoded at once"),
            NumberSpec::new("Maximum signals").limit(ctx.limit("max_signals")),
            ctx.read(
                get!(CwSkimmer, p => Some(f64::from(p.max_signals))),
                Some(32.0),
            ),
            set!(CwSkimmer, p, v => if let Some(v) = v { p.max_signals = v as u16 }),
        ),
        number(
            ctx,
            "Speed",
            Some("Empty tracks each signal's speed"),
            NumberSpec::new("Morse speed")
                .limit(ctx.limit("wpm"))
                .unit("WPM")
                .optional("auto"),
            ctx.read(get!(CwSkimmer, p => p.wpm.map(f64::from)), None),
            set!(CwSkimmer, p, v => p.wpm = v.map(|v| v as f32)),
        ),
    ])
}

fn wsjt(ctx: Ctx, wspr: bool) -> AnyView {
    let (low, high) = if wspr {
        (1_400.0, 1_600.0)
    } else {
        (200.0, 3_000.0)
    };
    let read_low = ctx.read(
        |params: &ChannelParams| match params {
            ChannelParams::Ft8(p) | ChannelParams::Ft4(p) => Some(Some(f64::from(p.audio_low_hz))),
            ChannelParams::Wspr(p) => Some(Some(f64::from(p.audio_low_hz))),
            _ => None,
        },
        Some(low),
    );
    let read_high = ctx.read(
        |params: &ChannelParams| match params {
            ChannelParams::Ft8(p) | ChannelParams::Ft4(p) => Some(Some(f64::from(p.audio_high_hz))),
            ChannelParams::Wspr(p) => Some(Some(f64::from(p.audio_high_hz))),
            _ => None,
        },
        Some(high),
    );
    let read_candidates = ctx.read(
        |params: &ChannelParams| match params {
            ChannelParams::Ft8(p) | ChannelParams::Ft4(p) => {
                Some(Some(f64::from(p.max_candidates)))
            }
            ChannelParams::Wspr(p) => Some(Some(f64::from(p.max_candidates))),
            _ => None,
        },
        Some(200.0),
    );
    rows(vec![
        number(
            ctx,
            "Audio from",
            Some("Lowest USB audio frequency searched"),
            NumberSpec::new("Audio from")
                .limit(ctx.limit("audio_low_hz"))
                .unit("Hz"),
            read_low,
            |params: &mut ChannelParams, v: Option<f64>| {
                let Some(v) = v else { return };
                match params {
                    ChannelParams::Ft8(p) | ChannelParams::Ft4(p) => p.audio_low_hz = v as f32,
                    ChannelParams::Wspr(p) => p.audio_low_hz = v as f32,
                    _ => {}
                }
            },
        ),
        number(
            ctx,
            "Audio to",
            Some("Highest USB audio frequency searched"),
            NumberSpec::new("Audio to")
                .limit(ctx.limit("audio_high_hz"))
                .unit("Hz"),
            read_high,
            |params: &mut ChannelParams, v: Option<f64>| {
                let Some(v) = v else { return };
                match params {
                    ChannelParams::Ft8(p) | ChannelParams::Ft4(p) => p.audio_high_hz = v as f32,
                    ChannelParams::Wspr(p) => p.audio_high_hz = v as f32,
                    _ => {}
                }
            },
        ),
        number(
            ctx,
            "Candidates",
            Some("Most synchronized signals tried per decode pass"),
            NumberSpec::new("Candidates").limit(ctx.limit("max_candidates")),
            read_candidates,
            |params: &mut ChannelParams, v: Option<f64>| {
                let Some(v) = v else { return };
                match params {
                    ChannelParams::Ft8(p) | ChannelParams::Ft4(p) => p.max_candidates = v as u16,
                    ChannelParams::Wspr(p) => p.max_candidates = v as u16,
                    _ => {}
                }
            },
        ),
    ])
}

fn psk(ctx: Ctx) -> AnyView {
    rows(vec![
        select(
            ctx,
            "Mode",
            None,
            listed(PSK_BAUDS),
            ctx.read(get!(Psk, p => p.baud), PskBaud::Psk31),
            set!(Psk, p, v => p.baud = v),
        ),
        toggle(
            ctx,
            "Invert",
            INVERT,
            ctx.read(get!(Psk, p => p.invert), false),
            set!(Psk, p, v => p.invert = v),
        ),
    ])
}

fn radio_clock(ctx: Ctx) -> AnyView {
    rows(vec![
        select(
            ctx,
            "Service",
            None,
            listed(RADIO_CLOCK_STANDARDS),
            ctx.read(get!(RadioClock, p => p.standard), RadioClockStandard::Dcf77),
            set!(RadioClock, p, v => p.standard = v),
        ),
        toggle(
            ctx,
            "Invert",
            INVERT,
            ctx.read(get!(RadioClock, p => p.invert), false),
            set!(RadioClock, p, v => p.invert = v),
        ),
    ])
}

fn gnss(ctx: Ctx) -> AnyView {
    rows(vec![
        number(
            ctx,
            "GPS PRN",
            Some("GPS L1 C/A satellite PRN"),
            NumberSpec::new("GPS PRN").limit(ctx.limit("prn")),
            ctx.read(get!(Gnss, p => Some(f64::from(p.prn))), Some(1.0)),
            set!(Gnss, p, v => if let Some(v) = v { p.prn = v as u8 }),
        ),
        number(
            ctx,
            "Doppler",
            Some("Symmetric Doppler search span"),
            NumberSpec::new("Doppler")
                .limit(ctx.limit("doppler_hz"))
                .unit("Hz"),
            ctx.read(
                get!(Gnss, p => Some(f64::from(p.doppler_hz))),
                Some(10_000.0),
            ),
            set!(Gnss, p, v => if let Some(v) = v { p.doppler_hz = v as u32 }),
        ),
        number(
            ctx,
            "Acquire above",
            Some("Correlation peak-to-floor acquisition threshold"),
            NumberSpec::new("Acquire above")
                .limit(ctx.limit("threshold"))
                .unit("× floor"),
            ctx.read(get!(Gnss, p => Some(f64::from(p.threshold))), Some(2.5)),
            set!(Gnss, p, v => if let Some(v) = v { p.threshold = v as f32 }),
        ),
    ])
}

fn vor(ctx: Ctx) -> AnyView {
    let station = ctx.read(
        get!(Vor, p => p.station.clone().unwrap_or_default()),
        String::new(),
    );
    rows(vec![
        setting_row(
            "Station",
            None,
            text_field(
                station,
                "VOR station identifier",
                "Optional identifier",
                "kc-num kc-num--wide",
                move |text| {
                    ctx.write(move |params| {
                        if let ChannelParams::Vor(p) = params {
                            p.station = (!text.is_empty()).then_some(text);
                        }
                    });
                    true
                },
            ),
        ),
        number(
            ctx,
            "Station latitude",
            None,
            NumberSpec::new("VOR station latitude")
                .limit(ctx.limit("station_lat"))
                .unit("°")
                .optional("Unknown"),
            ctx.read(get!(Vor, p => p.station_lat), None),
            set!(Vor, p, v => p.station_lat = v),
        ),
        number(
            ctx,
            "Station longitude",
            None,
            NumberSpec::new("VOR station longitude")
                .limit(ctx.limit("station_lon"))
                .unit("°")
                .optional("Unknown"),
            ctx.read(get!(Vor, p => p.station_lon), None),
            set!(Vor, p, v => p.station_lon = v),
        ),
        number(
            ctx,
            "Declination",
            Some("East-positive magnetic declination at the VOR"),
            NumberSpec::new("Declination")
                .limit(ctx.limit("magnetic_declination_deg"))
                .unit("°"),
            ctx.read(get!(Vor, p => Some(p.magnetic_declination_deg)), Some(0.0)),
            set!(Vor, p, v => if let Some(v) = v { p.magnetic_declination_deg = v }),
        ),
        number(
            ctx,
            "Report every",
            None,
            NumberSpec::new("VOR report interval")
                .limit(ctx.limit("report_ms"))
                .unit("ms"),
            ctx.read(get!(Vor, p => Some(f64::from(p.report_ms))), Some(500.0)),
            set!(Vor, p, v => if let Some(v) = v { p.report_ms = v as u32 }),
        ),
    ])
}

fn ils(ctx: Ctx) -> AnyView {
    rows(vec![
        segmented(
            ctx,
            "Component",
            None,
            ILS_COMPONENTS,
            ctx.read(get!(Ils, p => p.component), IlsComponent::Localizer),
            set!(Ils, p, v => p.component = v),
        ),
        number(
            ctx,
            "Report every",
            None,
            NumberSpec::new("ILS report interval")
                .limit(ctx.limit("report_ms"))
                .unit("ms"),
            ctx.read(get!(Ils, p => Some(f64::from(p.report_ms))), Some(500.0)),
            set!(Ils, p, v => if let Some(v) = v { p.report_ms = v as u32 }),
        ),
    ])
}

fn subghz(ctx: Ctx) -> AnyView {
    rows(vec![
        segmented(
            ctx,
            "Modulation",
            None,
            SUBGHZ_MODULATIONS,
            ctx.read(get!(Subghz, p => p.modulation), SubghzModulation::Ook),
            set!(Subghz, p, v => p.modulation = v),
        ),
        bandwidth(
            ctx,
            "Bandwidth",
            &[50_000.0, 100_000.0, 150_000.0],
            ctx.read(get!(Subghz, p => p.bandwidth_hz), 150_000.0),
            set!(Subghz, p, v => p.bandwidth_hz = v),
        ),
        number(
            ctx,
            "Min pulse",
            Some("Shortest keying edge accepted"),
            NumberSpec::new("Min pulse")
                .limit(ctx.limit("min_pulse_us"))
                .unit("µs"),
            ctx.read(
                get!(Subghz, p => Some(f64::from(p.min_pulse_us))),
                Some(80.0),
            ),
            set!(Subghz, p, v => if let Some(v) = v { p.min_pulse_us = v as u32 }),
        ),
        number(
            ctx,
            "Frame gap",
            Some("Silence that ends a frame"),
            NumberSpec::new("Frame gap")
                .limit(ctx.limit("frame_gap_us"))
                .unit("µs"),
            ctx.read(
                get!(Subghz, p => Some(f64::from(p.frame_gap_us))),
                Some(5_000.0),
            ),
            set!(Subghz, p, v => if let Some(v) = v { p.frame_gap_us = v as u32 }),
        ),
    ])
}

fn atv(ctx: Ctx) -> AnyView {
    rows(vec![
        segmented(
            ctx,
            "Modulation",
            None,
            ATV_MODULATIONS,
            ctx.read(get!(Atv, p => p.modulation), AtvModulation::Am),
            set!(Atv, p, v => p.modulation = v),
        ),
        select(
            ctx,
            "Lines",
            Some("Scanning standard"),
            listed(ATV_STANDARDS),
            ctx.read(get!(Atv, p => p.standard), AtvStandard::Ccir625),
            set!(Atv, p, v => p.standard = v),
        ),
        bandwidth(
            ctx,
            "Bandwidth",
            &[500_000.0, 1_000_000.0, 1_500_000.0, 1_600_000.0],
            ctx.read(get!(Atv, p => p.bandwidth_hz), 1_500_000.0),
            set!(Atv, p, v => p.bandwidth_hz = v),
        ),
        select(
            ctx,
            "Colour",
            None,
            listed(ATV_COLORS),
            ctx.read(get!(Atv, p => p.color), AtvColor::Monochrome),
            set!(Atv, p, v => p.color = v),
        ),
        number(
            ctx,
            "Sound",
            Some("FM sound subcarrier, empty for none"),
            NumberSpec::new("Sound subcarrier")
                .limit(scaled_limit(ctx.limit("sound_subcarrier_hz"), 1e-6))
                .unit("MHz")
                .optional("off"),
            ctx.read(
                get!(Atv, p => p.sound_subcarrier_hz.map(|hz| hz / 1e6)),
                None,
            ),
            set!(Atv, p, v => p.sound_subcarrier_hz = v.map(|mhz| (mhz * 1e6).round())),
        ),
        toggle(
            ctx,
            "Interlace",
            Some("Weave both fields into one frame"),
            ctx.read(get!(Atv, p => p.interlace), true),
            set!(Atv, p, v => p.interlace = v),
        ),
        toggle(
            ctx,
            "Invert",
            INVERT,
            ctx.read(get!(Atv, p => p.invert), false),
            set!(Atv, p, v => p.invert = v),
        ),
    ])
}

fn sstv(ctx: Ctx) -> AnyView {
    rows(vec![
        select(
            ctx,
            "Mode",
            Some("Scanning mode"),
            sstv_modes(),
            ctx.read(get!(Sstv, p => p.mode), None),
            set!(Sstv, p, v => p.mode = v),
        ),
        toggle(
            ctx,
            "Slant correction",
            Some("Straighten pictures from a sender whose clock runs off"),
            ctx.read(get!(Sstv, p => p.slant_correction), true),
            set!(Sstv, p, v => p.slant_correction = v),
        ),
        toggle(
            ctx,
            "Keep unfinished pictures",
            None,
            ctx.read(get!(Sstv, p => p.keep_partial), true),
            set!(Sstv, p, v => p.keep_partial = v),
        ),
    ])
}

fn dab(ctx: Ctx) -> AnyView {
    rows(vec![
        segmented(
            ctx,
            "Generation",
            None,
            DAB_MODES,
            ctx.read(get!(Dab, p => p.mode), DabMode::Auto),
            set!(Dab, p, v => p.mode = v),
        ),
        segmented(
            ctx,
            "Transmission",
            None,
            DAB_TRANSMISSION_MODES,
            ctx.read(get!(Dab, p => p.transmission_mode), DabTransmissionMode::I),
            set!(Dab, p, v => p.transmission_mode = v),
        ),
        service_picker(
            ctx,
            f64::from(u32::MAX),
            ctx.read(get!(Dab, p => p.service_id), None),
            set!(Dab, p, v => p.service_id = v),
        ),
    ])
}

fn datv(ctx: Ctx) -> AnyView {
    let standard = ctx.read(get!(Datv, p => p.standard), DatvStandard::DvbS);
    let by_standard = move || {
        if standard.get() == DatvStandard::DvbS2 {
            return AnyView::new(view! {
                column(class = "kc-grid") {
                    {select(
                        ctx,
                        "Roll-off",
                        Some("Match the transmitter's filter shape; DVB-S2 signals it in the base band header"),
                        listed(DATV_ROLL_OFFS),
                        ctx.read(get!(Datv, p => p.roll_off), DatvRollOff::Pct35),
                        set!(Datv, p, v => p.roll_off = v),
                    )}
                    {toggle(
                        ctx,
                        "Superframes",
                        Some("Receive Annex E format 0 or 1 with the default scrambling codes"),
                        ctx.read(get!(Datv, p => p.superframes), false),
                        set!(Datv, p, v => p.superframes = v),
                    )}
                    {number(
                        ctx,
                        "Input stream",
                        Some("Choose an input stream on a multistream carrier"),
                        NumberSpec::new("DVB-S2 input stream")
                            .limit(NumberLimit::new(0.0, 255.0, 1.0))
                            .optional("Auto"),
                        ctx.read(get!(Datv, p => p.input_stream.map(f64::from)), None),
                        set!(Datv, p, v => p.input_stream = v.map(|v| v as u8)),
                    )}
                }
            });
        }
        select(
            ctx,
            "Code rate",
            None,
            listed(DATV_CODE_RATES),
            ctx.read(get!(Datv, p => p.code_rate), DatvCodeRate::Auto),
            set!(Datv, p, v => p.code_rate = v),
        )
    };
    rows(vec![
        segmented(
            ctx,
            "Standard",
            None,
            DATV_STANDARDS,
            standard,
            set!(Datv, p, v => p.standard = v),
        ),
        number(
            ctx,
            "Symbol rate",
            None,
            NumberSpec::new("DATV symbol rate")
                .limit(ctx.limit("symbol_rate"))
                .unit("Bd"),
            ctx.read(get!(Datv, p => Some(p.symbol_rate)), Some(333_000.0)),
            set!(Datv, p, v => if let Some(v) = v { p.symbol_rate = v }),
        ),
        service_picker(
            ctx,
            65_535.0,
            ctx.read(get!(Datv, p => p.program.map(u32::from)), None),
            set!(Datv, p, v => p.program = v.and_then(|v| u16::try_from(v).ok())),
        ),
        AnyView::new(by_standard),
    ])
}

fn dvbt(ctx: Ctx) -> AnyView {
    let standard = ctx.read(get!(Dvbt, p => p.standard), DvbtStandard::DvbT);
    let by_standard = move || {
        if standard.get() == DvbtStandard::DvbT2 {
            return number(
                ctx,
                "PLP",
                Some("PLP ID, empty for automatic selection"),
                NumberSpec::new("PLP")
                    .limit(NumberLimit::new(0.0, 255.0, 1.0))
                    .optional("auto"),
                ctx.read(get!(Dvbt, p => p.plp.map(f64::from)), None),
                set!(Dvbt, p, v => p.plp = v.map(|v| v as u8)),
            );
        }
        toggle(
            ctx,
            "Low priority stream",
            Some("Decode the low priority stream of a hierarchical multiplex"),
            ctx.read(get!(Dvbt, p => p.low_priority), false),
            set!(Dvbt, p, v => p.low_priority = v),
        )
    };
    rows(vec![
        segmented(
            ctx,
            "Standard",
            None,
            DVBT_STANDARDS,
            standard,
            set!(Dvbt, p, v => p.standard = v),
        ),
        segmented(
            ctx,
            "Bandwidth",
            None,
            DVBT_BANDWIDTHS,
            ctx.read(get!(Dvbt, p => p.bandwidth), DvbtBandwidth::Mhz8),
            set!(Dvbt, p, v => p.bandwidth = v),
        ),
        AnyView::new(by_standard),
        service_picker(
            ctx,
            65_535.0,
            ctx.read(get!(Dvbt, p => p.program.map(u32::from)), None),
            set!(Dvbt, p, v => p.program = v.and_then(|v| u16::try_from(v).ok())),
        ),
    ])
}

fn drm(ctx: Ctx) -> AnyView {
    let mode = ctx.read(get!(Drm, p => p.mode), DrmMode::Auto);
    let width = move || {
        let options = if mode.get() == DrmMode::Drm30 {
            DRM30_BANDWIDTHS_HZ
        } else {
            DRM_PLUS_BANDWIDTHS_HZ
        };
        bandwidth(
            ctx,
            "Bandwidth",
            options,
            ctx.read(get!(Drm, p => p.bandwidth_hz), 100_000.0),
            set!(Drm, p, v => p.bandwidth_hz = v),
        )
    };
    rows(vec![
        segmented(
            ctx,
            "Mode",
            None,
            DRM_MODES,
            mode,
            set!(Drm, p, v => {
                p.bandwidth_hz = drm_bandwidth_for(v, p.mode, p.bandwidth_hz);
                p.mode = v;
            }),
        ),
        AnyView::new(width),
    ])
}

fn ident(ctx: Ctx) -> AnyView {
    rows(vec![
        bandwidth(
            ctx,
            "Search width",
            &[12_500.0, 50_000.0, 100_000.0, 192_000.0],
            ctx.read(get!(Ident, p => p.bandwidth_hz), 192_000.0),
            set!(Ident, p, v => p.bandwidth_hz = v),
        ),
        number(
            ctx,
            "Report every",
            Some("Milliseconds of signal each report is measured from"),
            NumberSpec::new("Report every")
                .limit(ctx.limit("interval_ms"))
                .unit("ms"),
            ctx.read(
                get!(Ident, p => Some(f64::from(p.interval_ms))),
                Some(1_000.0),
            ),
            set!(Ident, p, v => if let Some(v) = v { p.interval_ms = v as u32 }),
        ),
        number(
            ctx,
            "Detect above",
            Some("Decibels above the noise floor a signal must reach"),
            NumberSpec::new("Detect above")
                .limit(ctx.limit("threshold_db"))
                .unit("dB"),
            ctx.read(get!(Ident, p => Some(f64::from(p.threshold_db))), Some(8.0)),
            set!(Ident, p, v => if let Some(v) = v { p.threshold_db = v as f32 }),
        ),
    ])
}
