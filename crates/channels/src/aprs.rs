use std::{f64::consts::TAU, sync::LazyLock};

use num_complex::Complex;
use sdrmm_dsp::{
    BitSync, DcBlocker, Decimator, Descrambler, FmDemod, HdlcDeframer, NrziDecoder, RealDecimator,
    Scrambler, ToneCorrelator, crc16_x25, design_lowpass, hdlc_fcs_ok,
};
use sdrmm_wire::{
    AprsMode, AprsPacket, AprsParams, ChannelDescriptor, ChannelParams, ChannelSettings,
    DecoderEvent,
};

use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, ChannelTx, TxPayload,
    check_input_rate,
    tx::{Burst, TxQueue},
};

const CHANNEL_TAPS: usize = 129;

pub(crate) const AFSK_MARK_HZ: f64 = 1_200.0;
pub(crate) const AFSK_SPACE_HZ: f64 = 2_200.0;
pub(crate) const AFSK_BAUD: f64 = 1_200.0;
pub(crate) const G3RUH_BAUD: f64 = 9_600.0;

pub(crate) const DEVIATION_HZ: f64 = 3_000.0;

const G3RUH_CUTOFF_HZ: f64 = 7_200.0;
const G3RUH_TAPS: usize = 15;

pub(crate) const ADDRESS_LEN: usize = 7;
const MAX_ADDRESSES: usize = 10;
pub(crate) const CONTROL_UI: u8 = 0x03;
const CONTROL_PF: u8 = 0x10;
pub(crate) const PID_NO_LAYER3: u8 = 0xF0;

const MIN_FRAME_BYTES: usize = 2 * ADDRESS_LEN + 1 + 2;
const MAX_FRAME_BYTES: usize = MAX_ADDRESSES * ADDRESS_LEN + 2 + 256 + 2;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "aprs".to_owned(),
    name: "APRS / AX.25".to_owned(),
    bandwidth_hz: 12_500.0,
    input_rate_hz: 48_000.0,
    has_audio: false,
    decoder_kind: Some("aprs".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct AprsChannel {
    mode: AprsMode,
    demod: FmDemod,
    slicer: Slicer,
    nrzi: NrziDecoder,
    deframer: HdlcDeframer,
    discriminated: Vec<f32>,
    levels: Vec<bool>,
}

enum Slicer {
    Afsk {
        mark: ToneCorrelator,
        space: ToneCorrelator,
        sync: BitSync,
    },
    G3ruh {
        lowpass: RealDecimator,
        dc: DcBlocker,
        filtered: Vec<f32>,
        sync: BitSync,
        descrambler: Descrambler,
    },
}

impl Slicer {
    fn new(mode: AprsMode, rate: f64) -> Self {
        match mode {
            AprsMode::Afsk1200 => {
                let window = (rate / (AFSK_SPACE_HZ - AFSK_MARK_HZ)).round() as usize;
                Self::Afsk {
                    mark: ToneCorrelator::new(rate, AFSK_MARK_HZ, window),
                    space: ToneCorrelator::new(rate, AFSK_SPACE_HZ, window),
                    sync: BitSync::new(rate, AFSK_BAUD),
                }
            }
            AprsMode::G3ruh9600 => Self::G3ruh {
                lowpass: RealDecimator::new(&design_lowpass(G3RUH_TAPS, G3RUH_CUTOFF_HZ / rate), 1),
                dc: DcBlocker::new(),
                filtered: Vec::new(),
                sync: BitSync::new(rate, G3RUH_BAUD),
                descrambler: Descrambler::g3ruh(),
            },
        }
    }

    fn levels(&mut self, discriminated: &[f32], out: &mut Vec<bool>) {
        match self {
            Self::Afsk { mark, space, sync } => {
                for &s in discriminated {
                    let baseband = mark.push(s) - space.push(s);
                    if let Some(level) = sync.push(baseband) {
                        out.push(level);
                    }
                }
            }
            Self::G3ruh {
                lowpass,
                dc,
                filtered,
                sync,
                descrambler,
            } => {
                lowpass.process(discriminated, filtered);
                dc.process(filtered);
                for &s in filtered.iter() {
                    if let Some(level) = sync.push(s) {
                        out.push(descrambler.push(level));
                    }
                }
            }
        }
    }
}

fn params(settings: &ChannelSettings) -> Result<&AprsParams, ChannelError> {
    match &settings.params {
        ChannelParams::Aprs(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "aprs channel got {} params",
            other.type_id()
        ))),
    }
}

fn check_params(p: &AprsParams) -> Result<(), ChannelError> {
    let rate = DESCRIPTOR.input_rate_hz;
    if p.bandwidth_hz.is_finite() && p.bandwidth_hz > 0.0 && p.bandwidth_hz < rate {
        Ok(())
    } else {
        Err(ChannelError::InvalidSettings(format!(
            "aprs bandwidth must be in (0, {rate}) Hz, got {}",
            p.bandwidth_hz
        )))
    }
}

pub(crate) fn occupied_band(p: &AprsParams) -> (f64, f64) {
    let half = p.bandwidth_hz / 2.0;
    (-half, half)
}

pub(crate) fn channel_filter(p: &AprsParams) -> Result<ChannelFilter, ChannelError> {
    check_params(p)?;
    let (_, half) = occupied_band(p);
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(CHANNEL_TAPS, half / DESCRIPTOR.input_rate_hz),
        1,
    )))
}

impl ChannelRx for AprsChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        check_params(p)?;
        Ok(Self {
            mode: p.mode,
            demod: FmDemod::new(ctx.input_rate, DEVIATION_HZ),
            slicer: Slicer::new(p.mode, ctx.input_rate),
            nrzi: NrziDecoder::new(),
            deframer: HdlcDeframer::new(MIN_FRAME_BYTES, MAX_FRAME_BYTES),
            discriminated: Vec::new(),
            levels: Vec::new(),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        check_params(p)?;
        if p.mode != self.mode {
            self.mode = p.mode;
            self.slicer = Slicer::new(p.mode, DESCRIPTOR.input_rate_hz);
            self.nrzi.reset();
            self.deframer.reset();
        }
        Ok(())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.demod.process(iq, &mut self.discriminated);
        self.levels.clear();
        self.slicer.levels(&self.discriminated, &mut self.levels);
        for &level in &self.levels {
            let bit = self.nrzi.decode(level);
            if let Some(frame) = self.deframer.push(bit)
                && hdlc_fcs_ok(&frame)
                && let Some(packet) = parse_frame(&frame)
            {
                out.events.push(DecoderEvent::Aprs(packet));
            }
        }
    }
}

struct Address {
    call: String,
    last: bool,
    repeated: bool,
}

fn decode_address(field: &[u8]) -> Option<Address> {
    let (&ssid_octet, chars) = field.split_last()?;
    let mut call = String::with_capacity(9);
    let mut padded = false;
    for &octet in chars {
        if octet & 1 != 0 {
            return None;
        }
        let c = char::from(octet >> 1);
        if c == ' ' {
            padded = true;
            continue;
        }
        if padded || !c.is_ascii_alphanumeric() {
            return None;
        }
        call.push(c);
    }
    if call.is_empty() {
        return None;
    }
    let ssid = ssid_octet >> 1 & 0x0F;
    if ssid != 0 {
        call.push('-');
        call.push_str(&ssid.to_string());
    }
    Some(Address {
        call,
        last: ssid_octet & 1 != 0,
        repeated: ssid_octet & 0x80 != 0,
    })
}

fn carries_pid(control: u8) -> bool {
    control & 1 == 0 || control & !CONTROL_PF == CONTROL_UI
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

fn parse_frame(frame: &[u8]) -> Option<AprsPacket> {
    let body = frame.get(..frame.len().checked_sub(2)?)?;
    let mut addresses = Vec::with_capacity(4);
    let mut at = 0;
    loop {
        let address = decode_address(body.get(at..at + ADDRESS_LEN)?)?;
        at += ADDRESS_LEN;
        let last = address.last;
        addresses.push(address);
        if last {
            break;
        }
        if addresses.len() >= MAX_ADDRESSES {
            return None;
        }
    }

    let mut fields = addresses.into_iter();
    let destination = fields.next()?;
    let source = fields.next()?;
    let path = fields
        .map(|hop| {
            if hop.repeated {
                format!("{}*", hop.call)
            } else {
                hop.call
            }
        })
        .collect();

    let control = *body.get(at)?;
    at += 1;
    let pid = carries_pid(control)
        .then(|| body.get(at).copied())
        .flatten();
    at += usize::from(pid.is_some());
    let info = body.get(at..).unwrap_or_default();

    let mut packet = AprsPacket {
        source: source.call,
        destination: destination.call,
        path,
        info: latin1(info),
        ..AprsPacket::default()
    };
    packet.tnc2 = tnc2_line(&packet);
    if control & !CONTROL_PF == CONTROL_UI && pid == Some(PID_NO_LAYER3) {
        apply_aprs(info, &mut packet);
    }
    Some(packet)
}

fn address_chars(call: &str) -> &str {
    call.split('-').next().unwrap_or(call)
}

fn tnc2_line(packet: &AprsPacket) -> String {
    let mut line = String::with_capacity(packet.info.len() + 32);
    line.push_str(&packet.source);
    line.push('>');
    line.push_str(&packet.destination);
    for hop in &packet.path {
        line.push(',');
        line.push_str(hop);
    }
    line.push(':');
    line.push_str(&packet.info);
    line
}

struct Report {
    lat: f64,
    lon: f64,
    symbol: String,
    course_deg: Option<f64>,
    speed_kt: Option<f64>,
    comment: Option<String>,
    altitude_ft: Option<i32>,
}

const TIMESTAMP_LEN: usize = 7;

fn apply_aprs(info: &[u8], packet: &mut AprsPacket) {
    let Some((kind, rest)) = info.split_first() else {
        return;
    };
    let report = match *kind {
        MIC_E_CURRENT | MIC_E_OLD | MIC_E_CURRENT_BETA | MIC_E_OLD_BETA => mic_e(rest, packet),
        b'!' | b'=' => parse_position(rest),
        b'/' | b'@' => rest.get(TIMESTAMP_LEN..).and_then(parse_position),
        _ => None,
    };
    let Some(report) = report else {
        return;
    };
    packet.lat = Some(report.lat);
    packet.lon = Some(report.lon);
    packet.symbol = Some(report.symbol);
    packet.course_deg = report.course_deg;
    packet.speed_kt = report.speed_kt;
    packet.comment = report.comment;
    packet.altitude_ft = report.altitude_ft;
}

fn parse_position(body: &[u8]) -> Option<Report> {
    if body.first()?.is_ascii_digit() {
        uncompressed_position(body)
    } else {
        compressed_position(body)
    }
}

const UNCOMPRESSED_LEN: usize = 19;

fn uncompressed_position(body: &[u8]) -> Option<Report> {
    let field = body.get(..UNCOMPRESSED_LEN)?;
    let lat = parse_latitude(field.get(..8)?)?;
    let lon = parse_longitude(field.get(9..18)?)?;
    let symbol = symbol(*field.get(8)?, *field.get(18)?)?;
    let rest = body.get(UNCOMPRESSED_LEN..).unwrap_or_default();
    let (course_deg, speed_kt, rest) = course_speed(rest);
    let (comment, altitude_ft) = describe(rest);
    Some(Report {
        lat,
        lon,
        symbol,
        course_deg,
        speed_kt,
        comment,
        altitude_ft,
    })
}

const COMPRESSED_LEN: usize = 13;
const BASE91_MIN: u8 = b'!';
const BASE91_MAX: u8 = b'{';
const COMPRESSED_LAT_SCALE: f64 = 380_926.0;
const COMPRESSED_LON_SCALE: f64 = 190_463.0;

fn compressed_position(body: &[u8]) -> Option<Report> {
    let field = body.get(..COMPRESSED_LEN)?;
    let table = *field.first()?;
    if !(table == b'/'
        || table == b'\\'
        || table.is_ascii_uppercase()
        || (b'a'..=b'j').contains(&table))
    {
        return None;
    }
    let y = f64::from(base91(field.get(1..5)?)?);
    let x = f64::from(base91(field.get(5..9)?)?);
    let symbol = symbol(table, *field.get(9)?)?;
    let lat = 90.0 - y / COMPRESSED_LAT_SCALE;
    let lon = -180.0 + x / COMPRESSED_LON_SCALE;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    let (course_deg, speed_kt, compressed_alt_ft) =
        compressed_cs(*field.get(10)?, *field.get(11)?, *field.get(12)?);
    let (comment, comment_alt_ft) = describe(body.get(COMPRESSED_LEN..).unwrap_or_default());
    Some(Report {
        lat,
        lon,
        symbol,
        course_deg,
        speed_kt,
        comment,
        altitude_ft: comment_alt_ft.or(compressed_alt_ft),
    })
}

const COMPRESSED_TYPE_NMEA_MASK: u8 = 0b0001_1000;
const COMPRESSED_TYPE_NMEA_GGA: u8 = 0b0001_0000;

fn compressed_cs(c: u8, s: u8, t: u8) -> (Option<f64>, Option<f64>, Option<i32>) {
    if !(BASE91_MIN..=BASE91_MAX).contains(&t) {
        return (None, None, None);
    }
    if (t - BASE91_MIN) & COMPRESSED_TYPE_NMEA_MASK == COMPRESSED_TYPE_NMEA_GGA {
        return (None, None, compressed_altitude(c, s));
    }
    let (course, speed) = compressed_course_speed(c, s);
    (course, speed, None)
}

fn compressed_altitude(c: u8, s: u8) -> Option<i32> {
    if !(BASE91_MIN..=BASE91_MAX).contains(&c) || !(BASE91_MIN..=BASE91_MAX).contains(&s) {
        return None;
    }
    let cs = f64::from(u32::from(c - BASE91_MIN) * 91 + u32::from(s - BASE91_MIN));
    let feet = 1.002_f64.powf(cs);
    feet.is_finite().then(|| feet.round() as i32)
}

fn base91(field: &[u8]) -> Option<u32> {
    let mut value = 0u32;
    for &b in field {
        if !(BASE91_MIN..=BASE91_MAX).contains(&b) {
            return None;
        }
        value = value * 91 + u32::from(b - BASE91_MIN);
    }
    Some(value)
}

fn compressed_course_speed(c: u8, s: u8) -> (Option<f64>, Option<f64>) {
    if !(BASE91_MIN..BASE91_MAX).contains(&c) {
        return (None, None);
    }
    if !(BASE91_MIN..=BASE91_MAX).contains(&s) {
        return (None, None);
    }
    let course = f64::from(c - BASE91_MIN) * 4.0;
    let speed = 1.08_f64.powi(i32::from(s - BASE91_MIN)) - 1.0;
    (Some(course), Some(speed))
}

const MIC_E_CURRENT: u8 = b'`';
const MIC_E_OLD: u8 = b'\'';
const MIC_E_CURRENT_BETA: u8 = 0x1C;
const MIC_E_OLD_BETA: u8 = 0x1D;

const MIC_E_FIELD_LEN: usize = 8;

const MIC_E_OFFSET: u8 = 28;

const MIC_E_ALTITUDE_DATUM_M: i32 = 10_000;
const MIC_E_ALTITUDE_LEN: usize = 4;
const METRES_PER_FOOT: f64 = 0.304_8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum MessageBit {
    Zero,
    Standard,
    Custom,
}

struct MicEDestination {
    lat: f64,
    north: bool,
    lon_offset: bool,
    west: bool,
    ambiguity: usize,
    message: Option<&'static str>,
}

fn destination_char(c: u8) -> Option<(Option<u8>, MessageBit, bool)> {
    match c {
        b'0'..=b'9' => Some((Some(c - b'0'), MessageBit::Zero, false)),
        b'A'..=b'J' => Some((Some(c - b'A'), MessageBit::Custom, false)),
        b'K' => Some((None, MessageBit::Custom, false)),
        b'L' => Some((None, MessageBit::Zero, false)),
        b'P'..=b'Y' => Some((Some(c - b'P'), MessageBit::Standard, true)),
        b'Z' => Some((None, MessageBit::Standard, true)),
        _ => None,
    }
}

const STANDARD_MESSAGES: [&str; 7] = [
    "Priority",
    "Special",
    "Committed",
    "Returning",
    "In Service",
    "En Route",
    "Off Duty",
];
const CUSTOM_MESSAGES: [&str; 7] = [
    "Custom-6", "Custom-5", "Custom-4", "Custom-3", "Custom-2", "Custom-1", "Custom-0",
];

fn message_type(bits: [MessageBit; 3]) -> Option<&'static str> {
    let code = bits.iter().fold(0usize, |acc, &b| {
        acc << 1 | usize::from(b != MessageBit::Zero)
    });
    if code == 0 {
        return Some("Emergency");
    }
    let standard = bits.contains(&MessageBit::Standard);
    let custom = bits.contains(&MessageBit::Custom);
    match (standard, custom) {
        (true, false) => STANDARD_MESSAGES.get(code - 1).copied(),
        (false, true) => CUSTOM_MESSAGES.get(code - 1).copied(),
        _ => None,
    }
}

fn mic_e_destination(call: &str) -> Option<MicEDestination> {
    let chars = address_chars(call).as_bytes();
    if chars.len() != 6 {
        return None;
    }
    let mut digits = [0u8; 6];
    let mut bits = [MessageBit::Zero; 3];
    let mut indicators = [false; 3];
    let mut ambiguity = 0usize;
    for (i, &c) in chars.iter().enumerate() {
        let (digit, bit, indicator) = destination_char(c)?;
        match digit {
            Some(d) if ambiguity == 0 => digits[i] = d,
            Some(_) => return None,
            None => ambiguity += 1,
        }
        match i {
            0..=2 => bits[i] = bit,
            _ if bit == MessageBit::Custom => return None,
            _ => indicators[i - 3] = indicator,
        }
    }
    let degrees = f64::from(digits[0]) * 10.0 + f64::from(digits[1]);
    let minutes = f64::from(digits[2]) * 10.0 + f64::from(digits[3]);
    let hundredths = f64::from(digits[4]) * 10.0 + f64::from(digits[5]);
    if degrees > 90.0 || minutes >= 60.0 {
        return None;
    }
    Some(MicEDestination {
        lat: degrees + (minutes + hundredths / 100.0) / 60.0,
        north: indicators[0],
        lon_offset: indicators[1],
        west: indicators[2],
        ambiguity,
        message: message_type(bits),
    })
}

fn mic_e(body: &[u8], packet: &mut AprsPacket) -> Option<Report> {
    let destination = mic_e_destination(&packet.destination)?;
    let report = mic_e_report(&destination, body)?;
    packet.mic_e_message = destination.message.map(str::to_owned);
    Some(report)
}

fn mic_e_report(destination: &MicEDestination, body: &[u8]) -> Option<Report> {
    let field = body.get(..MIC_E_FIELD_LEN)?;
    let value = |at: usize| field.get(at).and_then(|b| b.checked_sub(MIC_E_OFFSET));

    let mut minutes = u32::from(value(1).filter(|&m| (10..=69).contains(&m))?);
    if minutes >= 60 {
        minutes -= 60;
    }
    let mut hundredths = u32::from(value(2).filter(|&h| h <= 99)?);
    let degrees = longitude_degrees(value(0)?, destination.lon_offset)?;
    if destination.ambiguity >= 1 {
        hundredths -= hundredths % 10;
    }
    if destination.ambiguity >= 2 {
        hundredths = 0;
    }
    if destination.ambiguity >= 3 {
        minutes -= minutes % 10;
    }
    if destination.ambiguity >= 4 {
        minutes = 0;
    }
    let lon = f64::from(degrees) + (f64::from(minutes) + f64::from(hundredths) / 100.0) / 60.0;
    let (speed_kt, course_deg) = speed_course(value(3)?, value(4)?, value(5)?)?;
    let symbol = symbol(*field.get(7)?, *field.get(6)?)?;
    let status = body.get(MIC_E_FIELD_LEN..).unwrap_or_default();
    let (comment, comment_alt_ft) = describe(mic_e_status(status));
    Some(Report {
        lat: if destination.north {
            destination.lat
        } else {
            -destination.lat
        },
        lon: if destination.west { -lon } else { lon },
        symbol,
        course_deg,
        speed_kt,
        comment,
        altitude_ft: comment_alt_ft.or_else(|| mic_e_altitude_ft(status)),
    })
}

fn longitude_degrees(raw: u8, offset: bool) -> Option<u32> {
    if !(10..=99).contains(&raw) {
        return None;
    }
    let mut degrees = u32::from(raw) + if offset { 100 } else { 0 };
    if (180..=189).contains(&degrees) {
        degrees -= 80;
    } else if (190..=199).contains(&degrees) {
        degrees -= 190;
    }
    Some(degrees)
}

fn speed_course(sp: u8, dc: u8, se: u8) -> Option<(Option<f64>, Option<f64>)> {
    if sp > 99 || dc > 99 || se > 99 {
        return None;
    }
    let mut speed = u32::from(sp) * 10 + u32::from(dc) / 10;
    let mut course = u32::from(dc) % 10 * 100 + u32::from(se);
    if speed >= 800 {
        speed -= 800;
    }
    if course >= 400 {
        course -= 400;
    }
    if course > 360 {
        return None;
    }
    let course_deg = (course != 0).then(|| f64::from(course % 360));
    Some((Some(f64::from(speed)), course_deg))
}

fn mic_e_status(status: &[u8]) -> &[u8] {
    match status.first().copied() {
        Some(MIC_E_CURRENT | MIC_E_OLD | MIC_E_OLD_BETA) => &[],
        Some(b'>' | b']') => status.get(1..).unwrap_or_default(),
        _ => status,
    }
}

fn mic_e_altitude_ft(status: &[u8]) -> Option<i32> {
    let field = mic_e_status(status).get(..MIC_E_ALTITUDE_LEN)?;
    if *field.get(3)? != b'}' {
        return None;
    }
    let mut metres = 0i32;
    for &b in field.get(..3)? {
        if !(BASE91_MIN..=BASE91_MAX).contains(&b) {
            return None;
        }
        metres = metres * 91 + i32::from(b - BASE91_MIN);
    }
    Some(((f64::from(metres - MIC_E_ALTITUDE_DATUM_M) / METRES_PER_FOOT).round()) as i32)
}

fn symbol(table: u8, code: u8) -> Option<String> {
    (table.is_ascii_graphic() && code.is_ascii_graphic())
        .then(|| [char::from(table), char::from(code)].into_iter().collect())
}

fn parse_latitude(field: &[u8]) -> Option<f64> {
    let degrees = f64::from(ascii_u32(field.get(..2)?)?);
    let minutes = parse_minutes(field.get(2..7)?)?;
    if degrees > 90.0 {
        return None;
    }
    let value = degrees + minutes / 60.0;
    match *field.get(7)? {
        b'N' => Some(value),
        b'S' => Some(-value),
        _ => None,
    }
}

fn parse_longitude(field: &[u8]) -> Option<f64> {
    let degrees = f64::from(ascii_u32(field.get(..3)?)?);
    let minutes = parse_minutes(field.get(3..8)?)?;
    if degrees > 180.0 {
        return None;
    }
    let value = degrees + minutes / 60.0;
    match *field.get(8)? {
        b'E' => Some(value),
        b'W' => Some(-value),
        _ => None,
    }
}

fn parse_minutes(field: &[u8]) -> Option<f64> {
    if *field.get(2)? != b'.' {
        return None;
    }
    let mut whole = 0u32;
    let mut hundredths = 0u32;
    for (n, &at) in [0usize, 1, 3, 4].iter().enumerate() {
        let digit = match *field.get(at)? {
            b' ' => 0,
            b if b.is_ascii_digit() => u32::from(b - b'0'),
            _ => return None,
        };
        if n < 2 {
            whole = whole * 10 + digit;
        } else {
            hundredths = hundredths * 10 + digit;
        }
    }
    if whole >= 60 {
        return None;
    }
    Some(f64::from(whole) + f64::from(hundredths) / 100.0)
}

fn ascii_u32(field: &[u8]) -> Option<u32> {
    let mut value = 0u32;
    for &b in field {
        if !b.is_ascii_digit() {
            return None;
        }
        value = value * 10 + u32::from(b - b'0');
    }
    Some(value)
}

fn course_speed(rest: &[u8]) -> (Option<f64>, Option<f64>, &[u8]) {
    const LEN: usize = 7;
    let Some(field) = rest.get(..LEN) else {
        return (None, None, rest);
    };
    let (Some(course), Some(b'/'), Some(speed)) = (
        field.get(..3).and_then(ascii_u32),
        field.get(3).copied(),
        field.get(4..7).and_then(ascii_u32),
    ) else {
        return (None, None, rest);
    };
    (
        Some(f64::from(course)),
        Some(f64::from(speed)),
        rest.get(LEN..).unwrap_or_default(),
    )
}

const ALTITUDE_TAG: &[u8] = b"/A=";
const ALTITUDE_DIGITS: usize = 6;

fn describe(rest: &[u8]) -> (Option<String>, Option<i32>) {
    let end = rest
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(0, |i| i + 1);
    let trimmed = rest.get(..end).unwrap_or_default();
    let text = latin1(trimmed);
    ((!text.is_empty()).then_some(text), altitude_ft(trimmed))
}

fn altitude_ft(comment: &[u8]) -> Option<i32> {
    let at = comment
        .windows(ALTITUDE_TAG.len())
        .position(|w| w == ALTITUDE_TAG)?;
    let field = comment
        .get(at + ALTITUDE_TAG.len()..)?
        .get(..ALTITUDE_DIGITS)?;
    match field.split_first()? {
        (b'-', digits) => ascii_u32(digits)
            .and_then(|v| i32::try_from(v).ok())
            .map(|v| -v),
        _ => ascii_u32(field).and_then(|v| i32::try_from(v).ok()),
    }
}

const FLAG: u8 = 0x7E;
const PREAMBLE_FLAGS: usize = 24;
const TRAILING_FLAGS: usize = 2;
const SSID_RESERVED: u8 = 0x60;

pub struct AprsTx {
    rate: f64,
    mode: AprsMode,
    pending: TxQueue<bool>,
    staging: Vec<bool>,
    line: Line,
    keyer: Keyer,
    remaining: usize,
    samples_per_symbol: usize,
    level: bool,
    carrier_phase: f64,
    burst: Burst,
}

struct Line {
    level: bool,
    scrambler: Option<Scrambler>,
}

impl Line {
    fn new(mode: AprsMode) -> Self {
        Self {
            level: false,
            scrambler: match mode {
                AprsMode::Afsk1200 => None,
                AprsMode::G3ruh9600 => Some(Scrambler::g3ruh()),
            },
        }
    }

    fn push(&mut self, bit: bool) -> bool {
        if !bit {
            self.level = !self.level;
        }
        match self.scrambler.as_mut() {
            Some(scrambler) => scrambler.push(self.level),
            None => self.level,
        }
    }
}

enum Keyer {
    Afsk { tone_phase: f64 },
    G3ruh,
}

impl Keyer {
    fn new(mode: AprsMode) -> Self {
        match mode {
            AprsMode::Afsk1200 => Self::Afsk { tone_phase: 0.0 },
            AprsMode::G3ruh9600 => Self::G3ruh,
        }
    }
}

fn baud(mode: AprsMode) -> f64 {
    match mode {
        AprsMode::Afsk1200 => AFSK_BAUD,
        AprsMode::G3ruh9600 => G3RUH_BAUD,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MicEBit {
    #[default]
    Zero,
    Standard,
    Custom,
}

#[derive(Clone, Copy, Debug)]
pub struct MicE<'a> {
    pub lat: f64,
    pub lon: f64,
    pub speed_kt: u32,
    pub course_deg: u32,
    pub symbol: &'a str,
    pub bits: [MicEBit; 3],
    pub ambiguity: usize,
    pub status: &'a str,
}

impl Default for MicE<'_> {
    fn default() -> Self {
        Self {
            lat: 0.0,
            lon: 0.0,
            speed_kt: 0,
            course_deg: 0,
            symbol: "/>",
            bits: [MicEBit::Zero; 3],
            ambiguity: 0,
            status: "",
        }
    }
}

impl MicE<'_> {
    #[must_use]
    pub fn destination(&self) -> String {
        let digits = hundredths_of_a_minute(self.lat);
        let carried = [
            self.bits[0],
            self.bits[1],
            self.bits[2],
            indicator(self.lat >= 0.0),
            indicator(!(10.0..100.0).contains(&self.lon.abs().trunc())),
            indicator(self.lon < 0.0),
        ];
        (0..6)
            .map(|i| {
                let ambiguous = i + self.ambiguity >= 6;
                encode_destination_char(digits[i], carried[i], ambiguous)
            })
            .collect()
    }

    #[must_use]
    pub fn info(&self) -> String {
        let digits = hundredths_of_a_minute(self.lon);
        let minutes = u32::from(digits[2]) * 10 + u32::from(digits[3]);
        let hundredths = u32::from(digits[4]) * 10 + u32::from(digits[5]);
        let symbol = self.symbol.as_bytes();
        let longitude = [
            longitude_degrees_byte(self.lon.abs().trunc() as u32),
            longitude_minutes_byte(minutes),
            (hundredths + 28) as u8,
        ];

        let mut out = String::with_capacity(9 + self.status.len());
        out.push('`');
        for byte in longitude
            .into_iter()
            .chain(speed_course_bytes(self.speed_kt, self.course_deg))
        {
            out.push(char::from(byte));
        }
        out.push(char::from(symbol.get(1).copied().unwrap_or(b'>')));
        out.push(char::from(symbol.first().copied().unwrap_or(b'/')));
        out.push_str(self.status);
        out
    }
}

impl MicE<'_> {
    #[must_use]
    pub fn altitude_field(alt_ft: i32) -> String {
        let metres = (f64::from(alt_ft) * 0.304_8).round() as i32 + 10_000;
        let clamped = metres.clamp(0, 91 * 91 * 91 - 1);
        let digits = [clamped / (91 * 91), clamped / 91 % 91, clamped % 91];
        digits
            .iter()
            .map(|&d| char::from(d as u8 + 33))
            .chain(std::iter::once('}'))
            .collect()
    }
}

fn hundredths_of_a_minute(degrees: f64) -> [u8; 6] {
    let total = (degrees.abs() * 6_000.0).round() as u32;
    let (deg, minutes, hundredths) = (total / 6_000, total / 100 % 60, total % 100);
    [
        (deg / 10 % 10) as u8,
        (deg % 10) as u8,
        (minutes / 10) as u8,
        (minutes % 10) as u8,
        (hundredths / 10) as u8,
        (hundredths % 10) as u8,
    ]
}

fn indicator(set: bool) -> MicEBit {
    if set {
        MicEBit::Standard
    } else {
        MicEBit::Zero
    }
}

fn encode_destination_char(digit: u8, bit: MicEBit, ambiguous: bool) -> char {
    let base = match bit {
        MicEBit::Zero => b'0',
        MicEBit::Standard => b'P',
        MicEBit::Custom => b'A',
    };
    if ambiguous {
        return char::from(match bit {
            MicEBit::Zero => b'L',
            MicEBit::Standard => b'Z',
            MicEBit::Custom => b'K',
        });
    }
    char::from(base + digit)
}

fn longitude_degrees_byte(degrees: u32) -> u8 {
    let raw = match degrees {
        0..=9 => degrees + 90,
        10..=99 => degrees,
        100..=109 => degrees - 20,
        _ => degrees.saturating_sub(100),
    };
    (raw + 28).min(127) as u8
}

fn longitude_minutes_byte(minutes: u32) -> u8 {
    let raw = if minutes < 10 { minutes + 60 } else { minutes };
    (raw + 28) as u8
}

fn speed_course_bytes(speed_kt: u32, course_deg: u32) -> [u8; 3] {
    let (tens, units) = (speed_kt.min(799) / 10, speed_kt.min(799) % 10);
    let course = course_deg.min(360);
    let sp = if tens < 20 { tens + 80 } else { tens };
    let dc = units * 10 + course / 100 + 4;
    [(sp + 28) as u8, (dc + 28) as u8, (course % 100 + 28) as u8]
}

impl AprsTx {
    #[must_use]
    pub fn ui_frame(source: &str, destination: &str, path: &[&str], info: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(ADDRESS_LEN * (2 + path.len()) + 2 + info.len());
        push_address(&mut out, destination, true, false);
        push_address(&mut out, source, false, path.is_empty());
        for (i, hop) in path.iter().enumerate() {
            let repeated = hop.ends_with('*');
            push_address(
                &mut out,
                hop.trim_end_matches('*'),
                repeated,
                i + 1 == path.len(),
            );
        }
        out.push(CONTROL_UI);
        out.push(PID_NO_LAYER3);
        out.extend_from_slice(info.as_bytes());
        out
    }

    fn next_level(&mut self) -> Option<bool> {
        let bit = self.pending.pop()?;
        Some(self.line.push(bit))
    }

    fn modulating(&mut self) -> f32 {
        let (level, rate) = (self.level, self.rate);
        match &mut self.keyer {
            Keyer::Afsk { tone_phase } => {
                let freq = if level { AFSK_MARK_HZ } else { AFSK_SPACE_HZ };
                *tone_phase += TAU * freq / rate;
                if *tone_phase > TAU {
                    *tone_phase -= TAU;
                }
                tone_phase.sin() as f32
            }
            Keyer::G3ruh => {
                if level {
                    1.0
                } else {
                    -1.0
                }
            }
        }
    }
}

impl ChannelTx for AprsTx {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        check_params(p)?;
        Ok(Self {
            rate: ctx.input_rate,
            mode: p.mode,
            pending: TxQueue::new(DESCRIPTOR.type_id.as_str(), baud(p.mode)),
            staging: Vec::new(),
            line: Line::new(p.mode),
            keyer: Keyer::new(p.mode),
            remaining: 0,
            samples_per_symbol: (ctx.input_rate / baud(p.mode)) as usize,
            level: false,
            carrier_phase: 0.0,
            burst: Burst::new(ctx.input_rate),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        check_params(p)?;
        if p.mode != self.mode {
            self.mode = p.mode;
            self.pending.clear();
            self.line = Line::new(p.mode);
            self.keyer = Keyer::new(p.mode);
            self.samples_per_symbol = (self.rate / baud(p.mode)) as usize;
            self.remaining = 0;
        }
        Ok(())
    }

    fn submit(&mut self, payload: TxPayload) -> Result<(), ChannelError> {
        let TxPayload::Frame(frame) = payload else {
            return Err(ChannelError::InvalidPayload(
                "aprs carries frames, not audio".to_owned(),
            ));
        };
        let (min, max) = (MIN_FRAME_BYTES - 2, MAX_FRAME_BYTES - 2);
        if !(min..=max).contains(&frame.len()) {
            return Err(ChannelError::InvalidPayload(format!(
                "an ax.25 frame is {min}..={max} octets before the fcs, got {}",
                frame.len()
            )));
        }
        self.staging.clear();
        if self.pending.is_empty() {
            push_flags(&mut self.staging, PREAMBLE_FLAGS);
        }
        push_frame(&mut self.staging, &frame, 0);
        push_flags(&mut self.staging, TRAILING_FLAGS);
        self.pending.accept(self.staging.len())?;
        self.pending.extend(self.staging.iter().copied());
        Ok(())
    }

    fn generate(&mut self, out: &mut [Complex<f32>]) -> usize {
        let mut written = 0;
        for slot in out {
            if self.remaining == 0
                && let Some(level) = self.next_level()
            {
                self.level = level;
                self.remaining = self.samples_per_symbol;
            }
            let Some(envelope) = self.burst.next(self.remaining > 0) else {
                break;
            };
            let audio = self.modulating();
            self.carrier_phase += TAU * DEVIATION_HZ * f64::from(audio) / self.rate;
            if self.carrier_phase > TAU {
                self.carrier_phase -= TAU;
            } else if self.carrier_phase < -TAU {
                self.carrier_phase += TAU;
            }
            *slot = Complex::from_polar(envelope, self.carrier_phase as f32);
            self.remaining = self.remaining.saturating_sub(1);
            written += 1;
        }
        written
    }
}

fn push_address(out: &mut Vec<u8>, call: &str, command: bool, last: bool) {
    let (base, ssid) = match call.split_once('-') {
        Some((base, ssid)) => (base, ssid.parse::<u8>().unwrap_or(0)),
        None => (call, 0),
    };
    let mut chars = [b' '; ADDRESS_LEN - 1];
    for (slot, byte) in chars.iter_mut().zip(base.bytes()) {
        *slot = byte.to_ascii_uppercase();
    }
    for c in chars {
        out.push((c & 0x7F) << 1);
    }
    out.push(u8::from(command) << 7 | SSID_RESERVED | (ssid & 0x0F) << 1 | u8::from(last));
}

fn push_flags(bits: &mut Vec<bool>, count: usize) {
    for _ in 0..count {
        for i in 0..8 {
            bits.push(FLAG >> i & 1 == 1);
        }
    }
}

fn push_frame(bits: &mut Vec<bool>, frame: &[u8], fcs_error: u16) {
    let fcs = (crc16_x25(frame) ^ fcs_error).to_le_bytes();
    let mut ones = 0u8;
    for &byte in frame.iter().chain(fcs.iter()) {
        for i in 0..8 {
            push_stuffed(bits, byte >> i & 1 == 1, &mut ones);
        }
    }
    push_flags(bits, 1);
}

fn push_stuffed(bits: &mut Vec<bool>, bit: bool, ones: &mut u8) {
    bits.push(bit);
    *ones = if bit { *ones + 1 } else { 0 };
    if *ones == 5 {
        bits.push(false);
        *ones = 0;
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::NfmParams;

    use super::*;
    use crate::{testgen::burst, testutil::settings};

    const RATE: f64 = 48_000.0;

    fn mode_settings(mode: AprsMode) -> ChannelSettings {
        settings(ChannelParams::Aprs(AprsParams {
            mode,
            ..AprsParams::default()
        }))
    }

    fn channel(mode: AprsMode) -> AprsChannel {
        AprsChannel::new(ChannelCtx { input_rate: RATE }, mode_settings(mode)).unwrap()
    }

    fn transmitter(mode: AprsMode) -> AprsTx {
        AprsTx::new(ChannelCtx { input_rate: RATE }, mode_settings(mode)).unwrap()
    }

    fn keyed(mode: AprsMode, frame: &[u8]) -> Vec<Complex<f32>> {
        let mut tx = transmitter(mode);
        tx.submit(TxPayload::Frame(frame.to_vec())).unwrap();
        burst(&mut tx)
    }

    fn keyed_frames(mode: AprsMode, frames: &[&[u8]], fcs_error: u16) -> Vec<Complex<f32>> {
        let mut tx = transmitter(mode);
        let mut bits = Vec::new();
        push_flags(&mut bits, PREAMBLE_FLAGS);
        for frame in frames {
            push_frame(&mut bits, frame, fcs_error);
        }
        push_flags(&mut bits, TRAILING_FLAGS);
        tx.pending.accept(bits.len()).unwrap();
        tx.pending.extend(bits);
        burst(&mut tx)
    }

    fn decode(mode: AprsMode, iq: &[Complex<f32>]) -> Vec<AprsPacket> {
        let mut chan = channel(mode);
        let mut out = ChannelOutputs::default();
        chan.process(iq, &mut out);
        packets(&out)
    }

    fn decode_ragged(mode: AprsMode, iq: &[Complex<f32>]) -> Vec<AprsPacket> {
        let mut chan = channel(mode);
        let mut out = ChannelOutputs::default();
        let mut all = Vec::new();
        let mut pos = 0;
        for len in [997usize, 1, 4_096, 65, 2_048, 7, 1_024].iter().cycle() {
            if pos >= iq.len() {
                break;
            }
            let end = (pos + len).min(iq.len());
            out.reset();
            chan.process(&iq[pos..end], &mut out);
            assert!(out.audio_pcm.is_empty(), "aprs must not produce audio");
            all.extend(packets(&out));
            pos = end;
        }
        all
    }

    fn packets(out: &ChannelOutputs) -> Vec<AprsPacket> {
        out.events
            .iter()
            .map(|e| match e {
                DecoderEvent::Aprs(p) => p.clone(),
                other => panic!("unexpected event {other:?}"),
            })
            .collect()
    }

    fn only(packets: Vec<AprsPacket>) -> AprsPacket {
        assert_eq!(packets.len(), 1, "{packets:#?}");
        packets.into_iter().next().unwrap()
    }

    const POSITION_INFO: &str = "!4807.38N/01131.00E>088/036/A=000432 Testing";

    fn position_frame() -> Vec<u8> {
        AprsTx::ui_frame("DL1ABC-9", "APRS", &["WIDE1-1*", "WIDE2-1"], POSITION_INFO)
    }

    fn assert_position_packet(packet: &AprsPacket) {
        assert_eq!(packet.source, "DL1ABC-9");
        assert_eq!(packet.destination, "APRS");
        assert_eq!(packet.path, ["WIDE1-1*", "WIDE2-1"]);
        assert_eq!(packet.info, POSITION_INFO);
        assert_eq!(
            packet.tnc2,
            "DL1ABC-9>APRS,WIDE1-1*,WIDE2-1:!4807.38N/01131.00E>088/036/A=000432 Testing"
        );
        let (lat, lon) = (packet.lat.unwrap(), packet.lon.unwrap());
        assert!((lat - 48.123).abs() < 1e-4, "lat {lat}");
        assert!((lon - 11.516_666_7).abs() < 1e-4, "lon {lon}");
        assert_eq!(packet.symbol.as_deref(), Some("/>"));
        assert_eq!(packet.course_deg, Some(88.0));
        assert_eq!(packet.speed_kt, Some(36.0));
        assert_eq!(packet.altitude_ft, Some(432));
        assert_eq!(packet.comment.as_deref(), Some("/A=000432 Testing"));
    }

    #[test]
    fn afsk1200_round_trips_a_position_report() {
        let iq = keyed(AprsMode::Afsk1200, &position_frame());
        assert_position_packet(&only(decode(AprsMode::Afsk1200, &iq)));
    }

    #[test]
    fn g3ruh9600_round_trips_the_same_frame() {
        let iq = keyed(AprsMode::G3ruh9600, &position_frame());
        assert_position_packet(&only(decode(AprsMode::G3ruh9600, &iq)));
    }

    #[test]
    fn ragged_blocks_match_one_shot_exactly() {
        for (mode, iq) in [
            (
                AprsMode::Afsk1200,
                keyed(AprsMode::Afsk1200, &position_frame()),
            ),
            (
                AprsMode::G3ruh9600,
                keyed(AprsMode::G3ruh9600, &position_frame()),
            ),
        ] {
            assert_eq!(
                decode(mode, &iq),
                decode_ragged(mode, &iq),
                "{mode:?} split-dependent"
            );
        }
    }

    #[test]
    fn compressed_position_decodes_to_the_same_place_as_the_spec_example() {
        let frame = AprsTx::ui_frame("DL1ABC", "APRS", &[], "!/5L!!<*e7>7P[Compressed");
        let packet = only(decode(
            AprsMode::Afsk1200,
            &keyed(AprsMode::Afsk1200, &frame),
        ));
        let (lat, lon) = (packet.lat.unwrap(), packet.lon.unwrap());
        assert!((lat - 49.5).abs() < 1e-4, "lat {lat}");
        assert!((lon + 72.75).abs() < 1e-4, "lon {lon}");
        assert_eq!(packet.symbol.as_deref(), Some("/>"));
        assert_eq!(packet.course_deg, Some(88.0));
        let speed = packet.speed_kt.unwrap();
        assert!((speed - 36.2).abs() < 0.1, "speed {speed}");
        assert_eq!(packet.comment.as_deref(), Some("Compressed"));
    }

    #[test]
    fn compressed_gga_report_carries_altitude_instead_of_course_and_speed() {
        let frame = AprsTx::ui_frame("DL1ABC", "APRS", &[], "!/5L!!<*e7>S]T");
        let packet = only(decode(
            AprsMode::Afsk1200,
            &keyed(AprsMode::Afsk1200, &frame),
        ));
        assert_eq!(packet.course_deg, None, "course must not be invented");
        assert_eq!(packet.speed_kt, None, "speed must not be invented");
        let alt = packet.altitude_ft.expect("altitude from the cs pair");
        assert!((alt - 10_004).abs() <= 1, "altitude {alt} ft");
    }

    #[test]
    fn explicit_altitude_overrides_the_compressed_one() {
        let frame = AprsTx::ui_frame("DL1ABC", "APRS", &[], "!/5L!!<*e7>S]T/A=001234");
        let packet = only(decode(
            AprsMode::Afsk1200,
            &keyed(AprsMode::Afsk1200, &frame),
        ));
        assert_eq!(packet.altitude_ft, Some(1_234));
    }

    #[test]
    fn timestamped_report_skips_the_timestamp() {
        let frame = AprsTx::ui_frame("DL1ABC", "APRS", &[], "@092345z4807.38N/01131.00E>Zulu");
        let packet = only(decode(
            AprsMode::Afsk1200,
            &keyed(AprsMode::Afsk1200, &frame),
        ));
        assert!((packet.lat.unwrap() - 48.123).abs() < 1e-4);
        assert_eq!(packet.comment.as_deref(), Some("Zulu"));
    }

    #[test]
    fn a_corrupt_fcs_emits_nothing() {
        let iq = keyed_frames(AprsMode::Afsk1200, &[&position_frame()], 1);
        assert!(decode(AprsMode::Afsk1200, &iq).is_empty());
    }

    #[test]
    fn long_runs_of_ones_in_the_info_field_survive_stuffing() {
        let info = ">stuffing \u{7f}\u{7f}~~ test";
        let frame = AprsTx::ui_frame("DL1ABC-1", "APRS", &[], info);
        let packet = only(decode(
            AprsMode::Afsk1200,
            &keyed(AprsMode::Afsk1200, &frame),
        ));
        assert_eq!(packet.info, info);
        assert_eq!(packet.lat, None);
    }

    #[test]
    fn two_frames_sharing_a_flag_both_decode() {
        let first = AprsTx::ui_frame("DL1ABC-1", "APRS", &[], ">first");
        let second = AprsTx::ui_frame("DL1ABC-2", "APRS", &["WIDE1-1"], ">second");
        let iq = keyed_frames(AprsMode::Afsk1200, &[&first, &second], 0);
        let decoded = decode(AprsMode::Afsk1200, &iq);
        assert_eq!(decoded.len(), 2, "{decoded:#?}");
        assert_eq!(decoded[0].tnc2, "DL1ABC-1>APRS:>first");
        assert_eq!(decoded[1].tnc2, "DL1ABC-2>APRS,WIDE1-1:>second");
    }

    #[test]
    fn the_spec_worked_example_decodes_to_its_published_values() {
        let frame = AprsTx::ui_frame("DL1ABC-7", "S32U6T", &["WIDE2-2"], "`(_fn\"Oj/");
        let packet = only(decode(
            AprsMode::Afsk1200,
            &keyed(AprsMode::Afsk1200, &frame),
        ));
        assert_eq!(packet.source, "DL1ABC-7");
        assert_eq!(packet.destination, "S32U6T");
        assert_eq!(packet.path, ["WIDE2-2"]);
        assert_eq!(packet.info, "`(_fn\"Oj/");
        assert_eq!(packet.tnc2, "DL1ABC-7>S32U6T,WIDE2-2:`(_fn\"Oj/");

        let lat = packet.lat.unwrap();
        assert!((lat - (33.0 + 25.64 / 60.0)).abs() < 1e-9, "lat {lat}");
        let lon = packet.lon.unwrap();
        assert!((lon + (12.0 + 7.74 / 60.0)).abs() < 1e-9, "lon {lon}");
        assert_eq!(packet.speed_kt, Some(20.0));
        assert_eq!(packet.course_deg, Some(251.0));
        assert_eq!(packet.symbol.as_deref(), Some("/j"));
        assert_eq!(packet.mic_e_message.as_deref(), Some("Returning"));
        assert_eq!(packet.comment, None);
        assert_eq!(packet.altitude_ft, None);
    }

    #[test]
    fn the_spec_longitude_example_needs_the_hundred_degree_offset() {
        let frame = AprsTx::ui_frame("DL1ABC-7", "S32UVT", &[], "`(_fn\"Oj/");
        let packet = only(decode(
            AprsMode::Afsk1200,
            &keyed(AprsMode::Afsk1200, &frame),
        ));
        let lon = packet.lon.unwrap();
        assert!((lon + (112.0 + 7.74 / 60.0)).abs() < 1e-9, "lon {lon}");
        assert!((packet.lat.unwrap() - (33.0 + 25.64 / 60.0)).abs() < 1e-9);
    }

    fn mic_e_frame(report: &MicE) -> Vec<u8> {
        AprsTx::ui_frame("DL1ABC-9", &report.destination(), &[], &report.info())
    }

    fn mic_e_packet(report: &MicE) -> AprsPacket {
        only(decode(
            AprsMode::Afsk1200,
            &keyed(AprsMode::Afsk1200, &mic_e_frame(report)),
        ))
    }

    #[test]
    fn mic_e_round_trips_positions_in_every_hemisphere_and_longitude_range() {
        for (lat, lon) in [
            (48.123_0, 11.516_67),
            (-33.875_0, -151.205_0),
            (37.775_0, -122.418_3),
            (60.170_0, 5.183_3),
            (-1.283_3, 36.816_7),
            (0.0, 0.0),
        ] {
            let packet = mic_e_packet(&MicE {
                lat,
                lon,
                ..MicE::default()
            });
            let (got_lat, got_lon) = (packet.lat.unwrap(), packet.lon.unwrap());
            assert!((got_lat - lat).abs() < 1.0 / 6_000.0, "{lat} -> {got_lat}");
            assert!((got_lon - lon).abs() < 1.0 / 6_000.0, "{lon} -> {got_lon}");
        }
    }

    #[test]
    fn mic_e_round_trips_speed_and_course() {
        for (speed_kt, course_deg, expected_course) in [
            (0, 0, None),
            (20, 251, Some(251.0)),
            (1, 1, Some(1.0)),
            (199, 199, Some(199.0)),
            (200, 200, Some(200.0)),
            (799, 359, Some(359.0)),
            (55, 360, Some(0.0)),
        ] {
            let packet = mic_e_packet(&MicE {
                lat: 48.0,
                lon: 11.0,
                speed_kt,
                course_deg,
                ..MicE::default()
            });
            assert_eq!(
                packet.speed_kt,
                Some(f64::from(speed_kt)),
                "{speed_kt} kt / {course_deg}°"
            );
            assert_eq!(
                packet.course_deg, expected_course,
                "{speed_kt} kt / {course_deg}°"
            );
        }
    }

    #[test]
    fn mic_e_names_every_message_code() {
        use MicEBit::{Custom, Standard, Zero};
        for (bits, name) in [
            ([Standard, Standard, Standard], Some("Off Duty")),
            ([Standard, Standard, Zero], Some("En Route")),
            ([Standard, Zero, Standard], Some("In Service")),
            ([Standard, Zero, Zero], Some("Returning")),
            ([Zero, Standard, Standard], Some("Committed")),
            ([Zero, Standard, Zero], Some("Special")),
            ([Zero, Zero, Standard], Some("Priority")),
            ([Zero, Zero, Zero], Some("Emergency")),
            ([Custom, Custom, Custom], Some("Custom-0")),
            ([Custom, Custom, Zero], Some("Custom-1")),
            ([Custom, Zero, Custom], Some("Custom-2")),
            ([Custom, Zero, Zero], Some("Custom-3")),
            ([Zero, Custom, Custom], Some("Custom-4")),
            ([Zero, Custom, Zero], Some("Custom-5")),
            ([Zero, Zero, Custom], Some("Custom-6")),
            ([Standard, Custom, Zero], None),
            ([Custom, Zero, Standard], None),
        ] {
            let packet = mic_e_packet(&MicE {
                lat: 48.0,
                lon: 11.0,
                bits,
                ..MicE::default()
            });
            assert_eq!(packet.mic_e_message.as_deref(), name, "{bits:?}");
            assert!(packet.lat.is_some(), "{bits:?}");
        }
    }

    #[test]
    fn mic_e_position_ambiguity_blanks_both_coordinates() {
        let (lat, lon) = (33.427_33, -112.129_0);
        for (ambiguity, lat_hundredths, lon_hundredths) in [
            (0usize, 25.64, 7.74),
            (1, 25.60, 7.70),
            (2, 25.00, 7.00),
            (3, 20.00, 0.00),
            (4, 0.00, 0.00),
        ] {
            let packet = mic_e_packet(&MicE {
                lat,
                lon,
                ambiguity,
                ..MicE::default()
            });
            let want_lat = 33.0 + lat_hundredths / 60.0;
            let want_lon = -(112.0 + lon_hundredths / 60.0);
            assert!(
                (packet.lat.unwrap() - want_lat).abs() < 1e-9,
                "ambiguity {ambiguity}: lat {:?}",
                packet.lat
            );
            assert!(
                (packet.lon.unwrap() - want_lon).abs() < 1e-9,
                "ambiguity {ambiguity}: lon {:?}",
                packet.lon
            );
        }
    }

    #[test]
    fn mic_e_altitude_decodes_from_the_status_text() {
        assert_eq!(MicE::altitude_field(200), "\"4T}");
        for alt_ft in [-30, 0, 200, 1_234, 38_000] {
            let status = MicE::altitude_field(alt_ft);
            let packet = mic_e_packet(&MicE {
                lat: 48.0,
                lon: 11.0,
                status: &status,
                ..MicE::default()
            });
            let got = packet.altitude_ft.unwrap();
            assert!((got - alt_ft).abs() <= 2, "{alt_ft} ft -> {got} ft");
            assert_eq!(packet.comment.as_deref(), Some(status.as_str()));
        }
    }

    #[test]
    fn mic_e_altitude_survives_a_kenwood_type_code() {
        for prefix in [">", "]"] {
            let status = format!("{prefix}{} Hello", MicE::altitude_field(1_000));
            let packet = mic_e_packet(&MicE {
                lat: 48.0,
                lon: 11.0,
                status: &status,
                ..MicE::default()
            });
            assert!(
                (packet.altitude_ft.unwrap() - 1_000).abs() <= 2,
                "{prefix}: {:?}",
                packet.altitude_ft
            );
            assert_eq!(
                packet.comment.as_deref(),
                Some(format!("{} Hello", MicE::altitude_field(1_000)).as_str())
            );
        }
    }

    #[test]
    fn mic_e_reads_neither_an_altitude_nor_a_comment_out_of_what_is_not_one() {
        let packet = mic_e_packet(&MicE {
            lat: 48.0,
            lon: 11.0,
            status: "not an altitude}",
            ..MicE::default()
        });
        assert_eq!(packet.altitude_ft, None);
        assert_eq!(packet.comment.as_deref(), Some("not an altitude}"));

        let packet = mic_e_packet(&MicE {
            lat: 48.0,
            lon: 11.0,
            status: "'7200007100",
            ..MicE::default()
        });
        assert_eq!(packet.comment, None);
        assert_eq!(packet.altitude_ft, None);
        assert!(packet.lat.is_some(), "telemetry must not lose the position");
    }

    #[test]
    fn a_truncated_mic_e_field_yields_no_position() {
        let frame = AprsTx::ui_frame("DL1ABC-9", "S32U6T", &[], "`(_fn\"O");
        let packet = only(decode(
            AprsMode::Afsk1200,
            &keyed(AprsMode::Afsk1200, &frame),
        ));
        assert_eq!(packet.lat, None);
        assert_eq!(packet.mic_e_message, None);
        assert_eq!(packet.info, "`(_fn\"O");
    }

    #[test]
    fn a_mic_e_information_field_under_an_ordinary_destination_decodes_nothing() {
        for destination in ["APRS", "N0CALL", "S32U6"] {
            let frame = AprsTx::ui_frame("DL1ABC-9", destination, &[], "`(_fn\"Oj/");
            let packet = only(decode(
                AprsMode::Afsk1200,
                &keyed(AprsMode::Afsk1200, &frame),
            ));
            assert_eq!(packet.lat, None, "{destination}");
            assert_eq!(packet.mic_e_message, None, "{destination}");
        }
    }

    #[test]
    fn a_destination_ssid_does_not_disturb_the_six_data_characters() {
        let frame = AprsTx::ui_frame("DL1ABC-9", "S32U6T-3", &[], "`(_fn\"Oj/");
        let packet = only(decode(
            AprsMode::Afsk1200,
            &keyed(AprsMode::Afsk1200, &frame),
        ));
        assert_eq!(packet.destination, "S32U6T-3");
        assert!((packet.lat.unwrap() - (33.0 + 25.64 / 60.0)).abs() < 1e-9);
        assert_eq!(packet.mic_e_message.as_deref(), Some("Returning"));
    }

    #[test]
    fn every_mic_e_data_type_identifier_is_decoded() {
        for id in ['`', '\'', '\u{1c}', '\u{1d}'] {
            let info = format!("{id}(_fn\"Oj/");
            let frame = AprsTx::ui_frame("DL1ABC-9", "S32U6T", &[], &info);
            let packet = only(decode(
                AprsMode::Afsk1200,
                &keyed(AprsMode::Afsk1200, &frame),
            ));
            assert_eq!(packet.speed_kt, Some(20.0), "{id:?}");
            assert!(packet.lat.is_some(), "{id:?}");
        }
    }

    #[test]
    fn a_non_aprs_pid_still_yields_the_ax25_envelope() {
        let mut frame = AprsTx::ui_frame("DL1ABC", "CQ", &[], "");
        let pid = frame.len() - 1;
        frame[pid] = 0xCF;
        frame.extend_from_slice(b"!4807.38N/01131.00E>not aprs");
        let packet = only(decode(
            AprsMode::Afsk1200,
            &keyed(AprsMode::Afsk1200, &frame),
        ));
        assert_eq!(packet.source, "DL1ABC");
        assert_eq!(packet.destination, "CQ");
        assert_eq!(packet.info, "!4807.38N/01131.00E>not aprs");
        assert_eq!(packet.lat, None);
    }

    #[test]
    fn ambiguous_minutes_read_as_zero() {
        let frame = AprsTx::ui_frame("DL1ABC", "APRS", &[], "!4807.  N/01131.  E>ambiguous");
        let packet = only(decode(
            AprsMode::Afsk1200,
            &keyed(AprsMode::Afsk1200, &frame),
        ));
        assert!((packet.lat.unwrap() - (48.0 + 7.0 / 60.0)).abs() < 1e-6);
        assert!((packet.lon.unwrap() - (11.0 + 31.0 / 60.0)).abs() < 1e-6);
    }

    #[test]
    fn southern_and_western_hemispheres_are_negative() {
        let frame = AprsTx::ui_frame("VK2ABC", "APRS", &[], "!3352.50S/15112.30W#");
        let packet = only(decode(
            AprsMode::Afsk1200,
            &keyed(AprsMode::Afsk1200, &frame),
        ));
        assert!((packet.lat.unwrap() + 33.875).abs() < 1e-6);
        assert!((packet.lon.unwrap() + 151.205).abs() < 1e-6);
        assert_eq!(packet.symbol.as_deref(), Some("/#"));
        assert_eq!(packet.comment, None);
    }

    #[test]
    fn negative_altitude_is_read_as_signed_feet() {
        let frame = AprsTx::ui_frame("DL1ABC", "APRS", &[], "!4807.38N/01131.00E>/A=-00042 deep");
        let packet = only(decode(
            AprsMode::Afsk1200,
            &keyed(AprsMode::Afsk1200, &frame),
        ));
        assert_eq!(packet.altitude_ft, Some(-42));
    }

    #[test]
    fn apply_switches_the_physical_layer() {
        let mut chan = channel(AprsMode::Afsk1200);
        chan.apply(settings(ChannelParams::Aprs(AprsParams {
            mode: AprsMode::G3ruh9600,
            ..AprsParams::default()
        })))
        .unwrap();
        let mut out = ChannelOutputs::default();
        chan.process(&keyed(AprsMode::G3ruh9600, &position_frame()), &mut out);
        assert_position_packet(&only(packets(&out)));
    }

    #[test]
    fn noise_alone_produces_no_frames() {
        let iq = crate::testutil::complex_noise(0x1234_5678, 0.5, 240_000);
        assert!(decode(AprsMode::Afsk1200, &iq).is_empty());
        assert!(decode(AprsMode::G3ruh9600, &iq).is_empty());
    }

    #[test]
    fn both_layers_decode_through_wideband_noise() {
        for (mode, mut iq) in [
            (
                AprsMode::Afsk1200,
                keyed(AprsMode::Afsk1200, &position_frame()),
            ),
            (
                AprsMode::G3ruh9600,
                keyed(AprsMode::G3ruh9600, &position_frame()),
            ),
        ] {
            crate::testgen::add_noise(&mut iq, 0x5eed_1234, 0.4);
            assert_position_packet(&only(decode(mode, &iq)));
        }
    }

    #[test]
    fn out_of_range_bandwidth_is_rejected() {
        for bad in [0.0, -1.0, 48_000.0, f64::NAN] {
            let built = AprsChannel::new(
                ChannelCtx { input_rate: RATE },
                settings(ChannelParams::Aprs(AprsParams {
                    mode: AprsMode::Afsk1200,
                    bandwidth_hz: bad,
                })),
            );
            assert!(
                matches!(built, Err(ChannelError::InvalidSettings(_))),
                "bandwidth {bad} must be rejected"
            );
        }
    }

    #[test]
    fn mismatched_params_variant_is_rejected() {
        let mut chan = channel(AprsMode::Afsk1200);
        let err = chan.apply(settings(ChannelParams::Nfm(NfmParams::default())));
        assert!(matches!(err, Err(ChannelError::InvalidSettings(_))));
        let built = AprsChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Nfm(NfmParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }

    #[test]
    fn wrong_input_rate_is_rejected() {
        let built = AprsChannel::new(
            ChannelCtx {
                input_rate: 240_000.0,
            },
            settings(ChannelParams::Aprs(AprsParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }

    #[test]
    fn ui_frame_lays_out_the_ax25_address_field() {
        let frame = AprsTx::ui_frame("DL1ABC-9", "APRS", &["WIDE1-1*"], "!test");
        assert_eq!(frame.len(), 3 * ADDRESS_LEN + 2 + 5);
        assert_eq!(&frame[..6], b"APRS  ".map(|c| c << 1));
        assert_eq!(frame[6] & 1, 0);
        assert_eq!(frame[13] & 1, 0);
        assert_eq!(frame[20] & 1, 1);
        assert_eq!(frame[13] >> 1 & 0x0F, 9);
        assert_eq!(frame[13] & 0x80, 0);
        assert_eq!(frame[20] >> 1 & 0x0F, 1);
        assert_eq!(frame[20] & 0x80, 0x80);
        assert_eq!(&frame[21..], b"\x03\xf0!test");
    }

    #[test]
    fn stuffing_breaks_every_run_of_five_ones() {
        let mut bits = Vec::new();
        let mut ones = 0;
        for _ in 0..4 {
            push_stuffed(&mut bits, true, &mut ones);
            push_stuffed(&mut bits, true, &mut ones);
            push_stuffed(&mut bits, true, &mut ones);
        }
        let longest = bits
            .chunk_by(|a, b| a == b)
            .filter(|run| run[0])
            .map(<[bool]>::len)
            .max()
            .unwrap();
        assert_eq!(longest, 5);
    }

    #[test]
    fn the_burst_keys_one_symbol_per_bit_at_a_constant_envelope() {
        let frame = AprsTx::ui_frame("DL1ABC", "APRS", &[], "!x");
        let ramp = Burst::new(RATE).ramp_len();
        for (mode, sps) in [(AprsMode::Afsk1200, 40), (AprsMode::G3ruh9600, 5)] {
            let bits = 8 * PREAMBLE_FLAGS + 8 * (TRAILING_FLAGS + 1) + stuffed_bits(&frame);
            let iq = keyed(mode, &frame);
            assert_eq!(iq.len(), bits * sps + ramp, "{mode:?} burst length");
            for (k, s) in iq[ramp..iq.len() - ramp].iter().enumerate() {
                assert!((s.norm() - 1.0).abs() < 1e-5, "{mode:?} envelope at {k}");
            }
        }
    }

    fn stuffed_bits(frame: &[u8]) -> usize {
        let mut bits = Vec::new();
        push_frame(&mut bits, frame, 0);
        bits.len() - 8
    }

    #[test]
    fn two_submitted_frames_ride_a_single_burst() {
        let mut tx = transmitter(AprsMode::Afsk1200);
        tx.submit(TxPayload::Frame(AprsTx::ui_frame(
            "DL1ABC-1",
            "APRS",
            &[],
            ">first",
        )))
        .unwrap();
        tx.submit(TxPayload::Frame(AprsTx::ui_frame(
            "DL1ABC-2",
            "APRS",
            &[],
            ">second",
        )))
        .unwrap();
        let iq = burst(&mut tx);
        let ramp = Burst::new(RATE).ramp_len();
        for s in &iq[ramp..iq.len() - ramp] {
            assert!((s.norm() - 1.0).abs() < 1e-5, "carrier dropped mid-burst");
        }
        let decoded = decode(AprsMode::Afsk1200, &iq);
        assert_eq!(decoded.len(), 2, "{decoded:#?}");
        assert_eq!(decoded[0].tnc2, "DL1ABC-1>APRS:>first");
        assert_eq!(decoded[1].tnc2, "DL1ABC-2>APRS:>second");
    }

    #[test]
    fn tx_apply_switches_the_physical_layer_and_drops_the_backlog() {
        let mut tx = transmitter(AprsMode::Afsk1200);
        tx.submit(TxPayload::Frame(position_frame())).unwrap();
        tx.apply(mode_settings(AprsMode::G3ruh9600)).unwrap();
        let mut block = [Complex::new(0.0, 0.0); 64];
        assert_eq!(tx.generate(&mut block), 0, "backlog survived the switch");

        tx.submit(TxPayload::Frame(position_frame())).unwrap();
        let iq = burst(&mut tx);
        assert_position_packet(&only(decode(AprsMode::G3ruh9600, &iq)));
    }

    #[test]
    fn tx_radiates_nothing_until_a_frame_is_submitted() {
        let mut tx = transmitter(AprsMode::Afsk1200);
        let mut block = [Complex::new(9.0, 9.0); 64];
        assert_eq!(tx.generate(&mut block), 0);
        assert_eq!(block[0], Complex::new(9.0, 9.0));
    }

    #[test]
    fn tx_refuses_a_payload_no_receiver_would_accept() {
        let mut tx = transmitter(AprsMode::Afsk1200);
        assert!(matches!(
            tx.submit(TxPayload::Audio(vec![0.0; 64])),
            Err(ChannelError::InvalidPayload(_))
        ));
        for len in [MIN_FRAME_BYTES - 3, MAX_FRAME_BYTES - 1] {
            assert!(
                matches!(
                    tx.submit(TxPayload::Frame(vec![0x41; len])),
                    Err(ChannelError::InvalidPayload(_))
                ),
                "{len} octets must be refused"
            );
        }
        let mut block = [Complex::new(0.0, 0.0); 16];
        assert_eq!(tx.generate(&mut block), 0);
    }

    #[test]
    fn tx_refuses_a_backlog_past_the_queue_bound() {
        let mut tx = transmitter(AprsMode::Afsk1200);
        let frame = position_frame();
        let refused = std::iter::repeat_with(|| tx.submit(TxPayload::Frame(frame.clone())))
            .take(1_000)
            .any(|r| matches!(r, Err(ChannelError::InvalidPayload(_))));
        assert!(refused, "the queue grew without bound");
    }

    #[test]
    fn tx_rejects_mismatched_params_and_input_rate() {
        let built = AprsTx::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Nfm(NfmParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
        let built = AprsTx::new(
            ChannelCtx {
                input_rate: 240_000.0,
            },
            mode_settings(AprsMode::Afsk1200),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }
}
