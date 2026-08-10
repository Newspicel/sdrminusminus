//! AX.25 / APRS decoder (PLAN §13 P2): Bell 202 AFSK1200 and 9600 baud G3RUH.
//!
//! Both physical layers hand the same thing to the link layer — NRZI line levels — so the
//! frame path below them is shared: NRZI decode, HDLC deframing, CRC-16/X.25, then the AX.25
//! address/control/PID split (AX.25 2.2) and a best-effort parse of the APRS information
//! field (APRS Protocol Reference 1.0.1).

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{
    BitSync, DcBlocker, Decimator, Descrambler, FmDemod, HdlcDeframer, NrziDecoder, RealDecimator,
    ToneCorrelator, design_lowpass, hdlc_fcs_ok,
};
use sdrmm_wire::{
    AprsMode, AprsPacket, AprsParams, ChannelDescriptor, ChannelParams, ChannelSettings,
    DecoderEvent,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const CHANNEL_TAPS: usize = 129;

/// Bell 202 mark/space and the 1200 baud rate every VHF packet station keys (AX.25 2.2 §2.1).
pub(crate) const AFSK_MARK_HZ: f64 = 1_200.0;
pub(crate) const AFSK_SPACE_HZ: f64 = 2_200.0;
pub(crate) const AFSK_BAUD: f64 = 1_200.0;
/// G3RUH direct-FSK rate (AX.25 2.2 §2.1).
pub(crate) const G3RUH_BAUD: f64 = 9_600.0;

/// Deviation the discriminator is scaled to, and the deviation the reference modulator keys.
/// Both layers only ever look at a sign or a ratio downstream, so the value's only job is to
/// keep a legal ±5 kHz signal inside the discriminator's linear range.
pub(crate) const DEVIATION_HZ: f64 = 3_000.0;

/// Post-discriminator lowpass for 9600 baud. The NRZ main lobe reaches the baud rate, so the
/// cutoff sits just below it, and the tap count stays short: at five samples per symbol a
/// sharp filter's ringing is inter-symbol interference, not selectivity.
const G3RUH_CUTOFF_HZ: f64 = 7_200.0;
const G3RUH_TAPS: usize = 15;

/// Address field entry: 6 shifted callsign characters plus an SSID octet (AX.25 2.2 §3.12).
pub(crate) const ADDRESS_LEN: usize = 7;
/// Destination, source and at most 8 digipeaters (AX.25 2.2 §3.12.2).
const MAX_ADDRESSES: usize = 10;
/// Unnumbered Information control field (AX.25 2.2 §4.3.3.6); bit 4 is the poll/final bit and
/// carries no meaning for a monitor.
pub(crate) const CONTROL_UI: u8 = 0x03;
const CONTROL_PF: u8 = 0x10;
/// "No layer 3 protocol implemented" — the PID APRS rides on (APRS 1.0.1 ch. 4).
pub(crate) const PID_NO_LAYER3: u8 = 0xF0;

/// Two addresses, a control octet and the FCS is the shortest thing that can be an AX.25
/// frame; the longest is 10 addresses, control, PID, a 256-octet information field and the FCS.
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

/// The physical layer: everything between the discriminator and the NRZI line levels.
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
                // A window of `rate / (space − mark)` samples spaces the sliding-DFT bins by
                // exactly the tone split, so each correlator sits on the other tone's null.
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

    /// Append one line level per recovered bit. G3RUH descrambles here so that both variants
    /// hand the link layer the same NRZI line, whatever the modulation was.
    fn levels(&mut self, discriminated: &[f32], out: &mut Vec<bool>) {
        match self {
            Self::Afsk { mark, space, sync } => {
                for &s in discriminated {
                    // The two magnitudes are equal on a tone halfway between them, so their
                    // difference is the sliced baseband with its decision threshold at zero.
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

/// Occupied RF band relative to the channel offset, in Hz.
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
        // Bandwidth is the host's channel filter; only the physical layer lives here, so a
        // frame in flight survives everything except an actual modulation change.
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

/// One decoded address field entry.
struct Address {
    /// Callsign with its SSID suffix, e.g. `DL1ABC-9`.
    call: String,
    /// Extension bit: this was the last entry in the address field.
    last: bool,
    /// "Has been repeated" — bit 7 of a digipeater's SSID octet (AX.25 2.2 §3.12.2).
    repeated: bool,
}

fn decode_address(field: &[u8]) -> Option<Address> {
    let (&ssid_octet, chars) = field.split_last()?;
    let mut call = String::with_capacity(9);
    let mut padded = false;
    for &octet in chars {
        // Bit 0 of every address octet but the last is the HDLC extension bit and reads 0.
        if octet & 1 != 0 {
            return None;
        }
        let c = char::from(octet >> 1);
        if c == ' ' {
            padded = true;
            continue;
        }
        // Callsigns are right-padded with spaces, so a character after one is not a callsign.
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

/// I frames (bit 0 clear) and UI frames carry a PID octet; S frames and the other U frames do
/// not (AX.25 2.2 §4.3).
fn carries_pid(control: u8) -> bool {
    control & 1 == 0 || control & !CONTROL_PF == CONTROL_UI
}

/// AX.25 does not constrain the information field to UTF-8 and APRS routinely carries raw
/// 8-bit octets (Mic-E, telemetry, weather), so each octet maps to the code point of the same
/// value instead of being lossily re-decoded.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

/// Split an FCS-checked HDLC frame into an AX.25 packet. `None` when the address field is not
/// an address field — the FCS admits roughly one framing artefact in 65536.
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

/// The TNC2 monitor line every APRS tool speaks: `SRC>DEST,PATH1,PATH2:info`.
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

/// A parsed position report.
struct Report {
    lat: f64,
    lon: f64,
    /// Symbol table identifier followed by the symbol code, e.g. `/>`.
    symbol: String,
    course_deg: Option<f64>,
    speed_kt: Option<f64>,
    comment: Option<String>,
    altitude_ft: Option<i32>,
}

/// Timestamp preceding a position in the `/` and `@` report formats: 6 digits plus the format
/// character (APRS 1.0.1 ch. 6).
const TIMESTAMP_LEN: usize = 7;

/// Fill in whatever the information field's data type identifier lets us read. Anything not
/// recognised (status, messages, telemetry, Mic-E) leaves the packet with its raw info only.
fn apply_aprs(info: &[u8], packet: &mut AprsPacket) {
    let Some((kind, rest)) = info.split_first() else {
        return;
    };
    let body = match kind {
        b'!' | b'=' => Some(rest),
        b'/' | b'@' => rest.get(TIMESTAMP_LEN..),
        _ => None,
    };
    let Some(report) = body.and_then(parse_position) else {
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

/// A leading digit is the first degree digit of an uncompressed report; every compressed one
/// starts with its symbol table identifier, which is never a digit (APRS 1.0.1 ch. 9).
fn parse_position(body: &[u8]) -> Option<Report> {
    if body.first()?.is_ascii_digit() {
        uncompressed_position(body)
    } else {
        compressed_position(body)
    }
}

/// `DDMM.mmN/DDDMM.mmW$`: 8 latitude octets, the symbol table, 9 longitude octets and the
/// symbol code (APRS 1.0.1 ch. 6).
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

/// Compressed report: symbol table, base-91 latitude and longitude, symbol code, the two
/// course/speed octets and the compression-type octet (APRS 1.0.1 ch. 9).
const COMPRESSED_LEN: usize = 13;
/// Printable base-91 alphabet, `!` through `{`.
const BASE91_MIN: u8 = b'!';
const BASE91_MAX: u8 = b'{';
/// Full-scale base-91 divisors for the 4-digit latitude and longitude fields.
const COMPRESSED_LAT_SCALE: f64 = 380_926.0;
const COMPRESSED_LON_SCALE: f64 = 190_463.0;

fn compressed_position(body: &[u8]) -> Option<Report> {
    let field = body.get(..COMPRESSED_LEN)?;
    let table = *field.first()?;
    // A compressed report's table identifier is `/`, `\`, or an overlay: `A`–`Z` for a
    // digit/letter overlay, `a`–`j` for the overlaid digits 0–9.
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
    // The compression-type byte says what the cs pair means. Reading it as course/speed
    // regardless would fabricate both for a GGA-sourced report, whose cs carries altitude
    // (APRS 1.0.1 ch. 9, "Compressed Position Report Data Formats").
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
        // An explicit `/A=` in the comment is the more precise statement of the two.
        altitude_ft: comment_alt_ft.or(compressed_alt_ft),
    })
}

/// Bits 3–4 of the compression-type byte are the NMEA source; `0b10` is GGA, and only then do
/// the two cs bytes carry altitude instead of course and speed.
const COMPRESSED_TYPE_NMEA_MASK: u8 = 0b0001_1000;
const COMPRESSED_TYPE_NMEA_GGA: u8 = 0b0001_0000;

/// Split the compressed `c`/`s` pair according to the compression-type byte `t`, returning
/// `(course, speed, altitude)`. Only one of course/speed and altitude is ever present.
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

/// GGA-sourced compressed altitude: `1.002^(c·91 + s)` feet (APRS 1.0.1 ch. 9).
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

/// The half-open range is the whole rule: below it, `c` = space marks the field as carrying no
/// course/speed, and at its top `c` = `{` makes `s` a pre-calculated radio range rather than a
/// speed (APRS 1.0.1 ch. 9).
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

/// `MM.mm`. Position ambiguity blanks minute digits from the right and a blanked digit is
/// defined to read as zero (APRS 1.0.1 ch. 6), which is what every receiver does with it.
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

/// `CSE/SPD`: course in degrees and speed in knots, immediately after the symbol code
/// (APRS 1.0.1 ch. 7). Returns the remainder so the caller can treat it as the comment —
/// the same seven octets also hold `PHGnnnn`/`RNGnnnn`/`DFSnnnn`, which the digit test rejects.
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

/// `/A=nnnnnn` in feet, anywhere in the comment (APRS 1.0.1 ch. 6).
const ALTITUDE_TAG: &[u8] = b"/A=";
const ALTITUDE_DIGITS: usize = 6;

/// Trim the trailing whitespace transmitters pad with, then read the comment and the altitude
/// it may embed.
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
    // Stations below sea level send a minus sign in place of the leading digit.
    match field.split_first()? {
        (b'-', digits) => ascii_u32(digits)
            .and_then(|v| i32::try_from(v).ok())
            .map(|v| -v),
        _ => ascii_u32(field).and_then(|v| i32::try_from(v).ok()),
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::NfmParams;

    use super::*;
    use crate::{
        testgen::aprs::{Fcs, afsk1200, afsk1200_frames, g3ruh9600, ui_frame},
        testutil::settings,
    };

    const RATE: f64 = 48_000.0;

    fn channel(mode: AprsMode) -> AprsChannel {
        AprsChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Aprs(AprsParams {
                mode,
                ..AprsParams::default()
            })),
        )
        .unwrap()
    }

    fn decode(mode: AprsMode, iq: &[Complex<f32>]) -> Vec<AprsPacket> {
        let mut chan = channel(mode);
        let mut out = ChannelOutputs::default();
        chan.process(iq, &mut out);
        packets(&out)
    }

    /// Feed the channel in deliberately ragged blocks, as the engine's host does.
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
        ui_frame("DL1ABC-9", "APRS", &["WIDE1-1*", "WIDE2-1"], POSITION_INFO)
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
        let iq = afsk1200(&position_frame(), RATE);
        assert_position_packet(&only(decode(AprsMode::Afsk1200, &iq)));
    }

    #[test]
    fn g3ruh9600_round_trips_the_same_frame() {
        let iq = g3ruh9600(&position_frame(), RATE);
        assert_position_packet(&only(decode(AprsMode::G3ruh9600, &iq)));
    }

    #[test]
    fn ragged_blocks_match_one_shot_exactly() {
        for (mode, iq) in [
            (AprsMode::Afsk1200, afsk1200(&position_frame(), RATE)),
            (AprsMode::G3ruh9600, g3ruh9600(&position_frame(), RATE)),
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
        // APRS 1.0.1 ch. 9: 49°30.00'N 72°45.00'W, car symbol, course 88°, speed 36.2 kt.
        let frame = ui_frame("DL1ABC", "APRS", &[], "!/5L!!<*e7>7P[Compressed");
        let packet = only(decode(AprsMode::Afsk1200, &afsk1200(&frame, RATE)));
        let (lat, lon) = (packet.lat.unwrap(), packet.lon.unwrap());
        assert!((lat - 49.5).abs() < 1e-4, "lat {lat}");
        assert!((lon + 72.75).abs() < 1e-4, "lon {lon}");
        assert_eq!(packet.symbol.as_deref(), Some("/>"));
        assert_eq!(packet.course_deg, Some(88.0));
        let speed = packet.speed_kt.unwrap();
        assert!((speed - 36.2).abs() < 0.1, "speed {speed}");
        assert_eq!(packet.comment.as_deref(), Some("Compressed"));
    }

    /// APRS 1.0.1 ch. 9: with a GGA-sourced compression type the cs pair is altitude, not
    /// course and speed. Reading it as course/speed invents a heading for a balloon and throws
    /// its altitude away.
    #[test]
    fn compressed_gga_report_carries_altitude_instead_of_course_and_speed() {
        // The spec's worked example: cs = "S]" with type byte "T" -> 10 004 feet.
        let frame = ui_frame("DL1ABC", "APRS", &[], "!/5L!!<*e7>S]T");
        let packet = only(decode(AprsMode::Afsk1200, &afsk1200(&frame, RATE)));
        assert_eq!(packet.course_deg, None, "course must not be invented");
        assert_eq!(packet.speed_kt, None, "speed must not be invented");
        let alt = packet.altitude_ft.expect("altitude from the cs pair");
        assert!((alt - 10_004).abs() <= 1, "altitude {alt} ft");
    }

    /// A `/A=` in the comment is the more precise of the two statements and wins.
    #[test]
    fn explicit_altitude_overrides_the_compressed_one() {
        let frame = ui_frame("DL1ABC", "APRS", &[], "!/5L!!<*e7>S]T/A=001234");
        let packet = only(decode(AprsMode::Afsk1200, &afsk1200(&frame, RATE)));
        assert_eq!(packet.altitude_ft, Some(1_234));
    }

    #[test]
    fn timestamped_report_skips_the_timestamp() {
        let frame = ui_frame("DL1ABC", "APRS", &[], "@092345z4807.38N/01131.00E>Zulu");
        let packet = only(decode(AprsMode::Afsk1200, &afsk1200(&frame, RATE)));
        assert!((packet.lat.unwrap() - 48.123).abs() < 1e-4);
        assert_eq!(packet.comment.as_deref(), Some("Zulu"));
    }

    #[test]
    fn a_corrupt_fcs_emits_nothing() {
        let iq = afsk1200_frames(&[&position_frame()], Fcs::Corrupt, RATE);
        assert!(decode(AprsMode::Afsk1200, &iq).is_empty());
    }

    #[test]
    fn long_runs_of_ones_in_the_info_field_survive_stuffing() {
        // 0x7F is seven set bits LSB-first — an abort if the transmitter failed to stuff it —
        // and 0x7E is six, the shortest run the rule applies to.
        let info = ">stuffing \u{7f}\u{7f}~~ test";
        let frame = ui_frame("DL1ABC-1", "APRS", &[], info);
        let packet = only(decode(AprsMode::Afsk1200, &afsk1200(&frame, RATE)));
        assert_eq!(packet.info, info);
        assert_eq!(packet.lat, None);
    }

    #[test]
    fn two_frames_sharing_a_flag_both_decode() {
        let first = ui_frame("DL1ABC-1", "APRS", &[], ">first");
        let second = ui_frame("DL1ABC-2", "APRS", &["WIDE1-1"], ">second");
        let iq = afsk1200_frames(&[&first, &second], Fcs::Valid, RATE);
        let decoded = decode(AprsMode::Afsk1200, &iq);
        assert_eq!(decoded.len(), 2, "{decoded:#?}");
        assert_eq!(decoded[0].tnc2, "DL1ABC-1>APRS:>first");
        assert_eq!(decoded[1].tnc2, "DL1ABC-2>APRS,WIDE1-1:>second");
    }

    #[test]
    fn mic_e_decodes_as_a_frame_without_a_position() {
        // Mic-E encodes the position in the destination callsign and a binary information
        // field; M4 decodes the frame but deliberately does not parse it.
        let frame = ui_frame("DL1ABC-7", "S32U6T", &["WIDE2-2"], "`(_fn\"Oj/");
        let packet = only(decode(AprsMode::Afsk1200, &afsk1200(&frame, RATE)));
        assert_eq!(packet.source, "DL1ABC-7");
        assert_eq!(packet.destination, "S32U6T");
        assert_eq!(packet.path, ["WIDE2-2"]);
        assert_eq!(packet.info, "`(_fn\"Oj/");
        assert_eq!(packet.tnc2, "DL1ABC-7>S32U6T,WIDE2-2:`(_fn\"Oj/");
        assert_eq!(packet.lat, None);
        assert_eq!(packet.lon, None);
        assert_eq!(packet.symbol, None);
    }

    #[test]
    fn a_non_aprs_pid_still_yields_the_ax25_envelope() {
        let mut frame = ui_frame("DL1ABC", "CQ", &[], "");
        // Swap the PID for one that is not "no layer 3": still AX.25, no APRS parsing.
        let pid = frame.len() - 1;
        frame[pid] = 0xCF;
        frame.extend_from_slice(b"!4807.38N/01131.00E>not aprs");
        let packet = only(decode(AprsMode::Afsk1200, &afsk1200(&frame, RATE)));
        assert_eq!(packet.source, "DL1ABC");
        assert_eq!(packet.destination, "CQ");
        assert_eq!(packet.info, "!4807.38N/01131.00E>not aprs");
        assert_eq!(packet.lat, None);
    }

    #[test]
    fn ambiguous_minutes_read_as_zero() {
        let frame = ui_frame("DL1ABC", "APRS", &[], "!4807.  N/01131.  E>ambiguous");
        let packet = only(decode(AprsMode::Afsk1200, &afsk1200(&frame, RATE)));
        assert!((packet.lat.unwrap() - (48.0 + 7.0 / 60.0)).abs() < 1e-6);
        assert!((packet.lon.unwrap() - (11.0 + 31.0 / 60.0)).abs() < 1e-6);
    }

    #[test]
    fn southern_and_western_hemispheres_are_negative() {
        let frame = ui_frame("VK2ABC", "APRS", &[], "!3352.50S/15112.30W#");
        let packet = only(decode(AprsMode::Afsk1200, &afsk1200(&frame, RATE)));
        assert!((packet.lat.unwrap() + 33.875).abs() < 1e-6);
        assert!((packet.lon.unwrap() + 151.205).abs() < 1e-6);
        assert_eq!(packet.symbol.as_deref(), Some("/#"));
        assert_eq!(packet.comment, None);
    }

    #[test]
    fn negative_altitude_is_read_as_signed_feet() {
        let frame = ui_frame("DL1ABC", "APRS", &[], "!4807.38N/01131.00E>/A=-00042 deep");
        let packet = only(decode(AprsMode::Afsk1200, &afsk1200(&frame, RATE)));
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
        chan.process(&g3ruh9600(&position_frame(), RATE), &mut out);
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
        // Noise in ±0.4 on a unit-amplitude carrier, with no channel filter in front of the
        // demodulator — a weaker signal than any station a receiver would bother logging.
        for (mode, mut iq) in [
            (AprsMode::Afsk1200, afsk1200(&position_frame(), RATE)),
            (AprsMode::G3ruh9600, g3ruh9600(&position_frame(), RATE)),
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
}
