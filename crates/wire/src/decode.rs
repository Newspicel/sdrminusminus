use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::RadioClockStandard;

/// RDS state after a group changed it (: 57 kHz BPSK, group/AF/RT decode). RDS is a
/// slowly-accreting picture rather than a stream of independent frames, so an event is the
/// current best view of the station, emitted only when a field actually changed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RdsUpdate {
    /// Programme Identification, as the 4 hex digits everyone quotes it by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pi: Option<String>,
    /// Programme Service name (8 chars), once every segment has been seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ps: Option<String>,
    /// RadioText (up to 64 chars), once the A/B flag closes a message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radiotext: Option<String>,
    /// Programme Type code (0–31).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pty: Option<u8>,
    /// Programme Type name for [`RdsUpdate::pty`] under the RDS (EU) table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pty_name: Option<String>,
    /// Traffic Programme flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tp: Option<bool>,
    /// Traffic Announcement flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ta: Option<bool>,
    /// Music (true) / Speech (false) switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub music: Option<bool>,
    /// Alternative frequencies in Hz, as advertised in group 0A.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alt_freqs_hz: Vec<f64>,
    /// Groups accepted since the channel started.
    pub groups: u64,
    /// Blocks rejected by the syndrome check since the channel started.
    pub block_errors: u64,
}

/// POCSAG message class (: 512/1200/2400 baud pagers).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PocsagPayload {
    /// Function-only page with no data codewords.
    Tone,
    /// BCD digits (function 0 by convention).
    Numeric,
    /// 7-bit ASCII (function 3 by convention).
    Alpha,
}

/// One decoded POCSAG page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PocsagMessage {
    /// 21-bit receiver address (RIC).
    pub address: u32,
    /// Function bits 0–3 (the "A/B/C/D" a pager shows).
    pub function: u8,
    /// Bit rate the batch was decoded at.
    pub baud: u16,
    pub payload: PocsagPayload,
    pub text: String,
    /// Single-bit errors the BCH(31,21) decoder repaired across the message's codewords.
    pub errors_corrected: u32,
}

/// One decoded Mode S / ADS-B frame (: preamble correlation + Mode S CRC).
/// Fields are `Option` because which ones a frame carries depends on its type code — a
/// position frame has no callsign, an identification frame has no altitude.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AdsbMessage {
    /// ICAO 24-bit address as 6 hex digits — the aircraft's identity across frames.
    pub icao: String,
    /// Downlink Format (17 = ADS-B extended squitter, 11 = all-call reply).
    pub df: u8,
    /// Extended-squitter type code, when the frame has an ME field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_code: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callsign: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub altitude_ft: Option<i32>,
    /// Latitude in degrees, once a CPR even/odd pair has been solved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground_speed_kt: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertical_rate_fpm: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub squawk: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_ground: Option<bool>,
    /// The raw frame as hex — the interop format every Mode S tool speaks.
    pub raw: String,
}

/// One decoded AIS message (: GMSK/NRZI over HDLC framing).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AisMessage {
    /// Maritime Mobile Service Identity — the vessel's identity across messages.
    pub mmsi: u32,
    /// ITU-R M.1371 message type (1–3 position report, 5 static data, 18/19 class B …).
    pub msg_type: u8,
    /// Which of the two AIS channels the burst arrived on (`A` = 161.975 MHz).
    pub ais_channel: char,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_sign: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    /// Speed over ground in knots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sog_kt: Option<f64>,
    /// Course over ground in degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cog_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_deg: Option<u16>,
    /// Navigational status code (0 = under way using engine …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nav_status: Option<u8>,
    /// The `!AIVDM` sentence — the interop format every AIS tool speaks.
    pub nmea: String,
}

/// One decoded AX.25 frame, with the APRS fields parsed out when the info field carries them
/// (: AFSK1200 + 9600 G3RUH).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AprsPacket {
    /// Source callsign with SSID, e.g. `DL1ABC-9`.
    pub source: String,
    pub destination: String,
    /// Digipeater path, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<String>,
    /// Raw information field.
    pub info: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    /// APRS symbol as `table` + `code`, e.g. `/>` for a car.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub course_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_kt: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub altitude_ft: Option<i32>,
    /// The Mic-E message the operator selected, named (APRS 1.0.1 ch. 10): one of the 7
    /// standard messages, one of the 7 custom ones, or `Emergency`. `Unknown` is the spec's
    /// own word for a packet whose three message bits mix the standard and custom tables.
    /// Absent on every packet that is not Mic-E.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mic_e_message: Option<String>,
    /// TNC2 monitor line (`SRC>DEST,PATH:info`) — the interop format.
    pub tnc2: String,
}

/// A run of decoded RTTY characters (: Baudot over FSK).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RttyText {
    pub text: String,
}

/// A run of decoded Morse characters plus the speed the tracker settled on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct MorseText {
    pub text: String,
    /// Estimated sending speed in words per minute (PARIS standard).
    pub wpm: f32,
}

/// One complete five-tone selective call after the repeat marker has been expanded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SelcallSequence {
    pub system: crate::channel::SelcallSystem,
    /// Five decoded digits/group symbols. Consecutive equal digits are represented literally,
    /// not by the on-air repeat marker.
    pub code: String,
    /// Median detected tone duration, rounded to milliseconds.
    pub tone_ms: u32,
}

/// One NAVTEX broadcast (: SITOR-B over 100 baud FSK). The `ZCZC B1B2B3B4` header is
/// parsed out because that is what a receiver filters on — station, subject and serial are how
/// a ship decides whether it has already seen this message.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct NavtexMessage {
    /// B1 — transmitting station within the NAVAREA.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station: Option<char>,
    /// B2 — subject indicator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<char>,
    /// Plain-language meaning of [`NavtexMessage::subject`] (ITU-R M.540 Annex 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_name: Option<String>,
    /// B3B4 — serial number, 00–99, which a receiver uses to suppress repeats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<u8>,
    pub text: String,
    /// Characters the mode-B time diversity repaired from the repeat copy.
    pub errors_corrected: u32,
    /// True when the broadcast ended with `NNNN`; false when the carrier or the phasing was
    /// lost and the partial text was flushed instead.
    pub complete: bool,
}

impl NavtexMessage {
    /// The `B1B2B3B4` group as broadcast, when the header was received.
    #[must_use]
    pub fn header(&self) -> Option<String> {
        let (station, subject, serial) = (self.station?, self.subject?, self.serial?);
        Some(format!("{station}{subject}{serial:02}"))
    }

    /// Plain-language meaning of a B2 subject indicator (ITU-R M.540 Annex 2). `None` for the
    /// letters the standard leaves unassigned — naming those would invent authority the
    /// broadcast does not carry.
    #[must_use]
    pub fn subject_name(subject: char) -> Option<&'static str> {
        Some(match subject.to_ascii_uppercase() {
            'A' => "Navigational warning",
            'B' => "Meteorological warning",
            'C' => "Ice report",
            'D' => "Search and rescue / piracy",
            'E' => "Meteorological forecast",
            'F' => "Pilot service",
            'G' => "AIS",
            'H' => "LORAN",
            'J' => "SATNAV",
            'K' => "Other electronic navaid",
            'L' => "Navigational warning (additional)",
            'T' => "Test transmission",
            'V' => "Notice to fishermen",
            'W' => "Environmental",
            'X' | 'Y' => "Special service",
            'Z' => "No message on hand",
            _ => return None,
        })
    }
}

/// One ACARS block (: MSK 2400 bit/s over AM, ARINC 618 framing). Field names follow
/// the standard's, so a message here reads the same as in every other ACARS tool.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AcarsMessage {
    /// Mode character — `2` is VHF category A.
    pub mode: char,
    /// Aircraft registration with the standard `.` padding removed.
    pub registration: String,
    /// Technical acknowledgement: the block being acknowledged, or `None` for a NAK.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack: Option<char>,
    /// Two-character label identifying the message type (`5Z`, `H1`, `_d` …).
    pub label: String,
    /// Block identifier. `0`–`9` marks a downlink (aircraft to ground).
    pub block_id: char,
    pub downlink: bool,
    /// Message sequence number, on downlinks only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq_no: Option<String>,
    /// Flight number, on downlinks only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flight: Option<String>,
    pub text: String,
    /// The block ended with ETB rather than ETX: another block of the same message follows.
    pub more: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubghzEncoding {
    /// Pulse-width coding: a short-long cell is one bit value, long-short the other. The
    /// family PT2262/EV1527/Princeton and most cheap remotes speak. The chip is *not*
    /// identified — an EV1527's 24 data bits and a PT2262's 12 tri-state symbols are the same
    /// pulse train, so both readings are offered and the operator picks.
    Pwm,
    /// Manchester: each bit is a mid-cell transition.
    Manchester,
    /// Nothing matched; only the raw edge timings are reported.
    #[default]
    Raw,
}

/// One sub-GHz burst: a remote, a sensor, a TPMS. Repeats of the same payload inside a short
/// window collapse into one frame with a count, because every one of these devices sends its
/// payload several times and a log with eight identical rows is a worse log.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SubghzFrame {
    pub modulation: crate::channel::SubghzModulation,
    pub encoding: SubghzEncoding,
    /// Decoded payload length in bits; 0 for a raw capture.
    pub bits: u32,
    /// Payload as hex, MSB first, left-padded to whole bytes.
    pub data: String,
    /// EV1527 reading of a 24-bit payload: the 20-bit transmitter address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<u32>,
    /// EV1527 reading of a 24-bit payload: the 4 button bits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button: Option<u8>,
    /// PT2262 reading: 12 tri-state symbols as `0`, `1` and `F`, when every bit pair is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tri_state: Option<String>,
    /// The base pulse period the frame was measured against, in µs.
    pub short_us: u32,
    /// How many times the identical payload arrived inside the collapse window.
    pub repeats: u32,
    /// Raw pulse/gap durations in µs, pulse first — what a Flipper shows for a signal it
    /// cannot name. Truncated, so this is for inspection, not replay.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timings_us: Vec<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ToneSquelchStatus {
    /// The CTCSS tone present, in Hz.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctcss_hz: Option<f64>,
    /// The DCS code present, as the three octal digits a radio displays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dcs_code: Option<u16>,
    /// Whether audio is passing. Always true unless the channel was told to gate on a tone:
    /// [`crate::NfmToneMode::Detect`] reports what is there without acting on it.
    pub open: bool,
}

/// The modulation family a [`IdentReport`] settled on.
///
/// Coarse on purpose. What a receiver can read off an unknown waveform is how its envelope, its
/// instantaneous frequency and its spectrum behave, and those separate *families* — they do not
/// separate the variants inside one, which is what the protocol candidates are for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Modulation {
    /// Nothing rose above the noise floor.
    #[default]
    None,
    /// A carrier with nothing on it.
    Carrier,
    /// A carrier switched on and off — OOK/ASK, and the pulse modulations that look like it.
    Ook,
    /// Amplitude modulation with its carrier present.
    Am,
    /// One sideband, carrier suppressed.
    Ssb,
    /// Analog frequency modulation: constant envelope, one broad continuum of frequencies.
    Fm,
    /// Two-level frequency shift keying, GFSK and GMSK included.
    Fsk2,
    /// Four-level FSK — the C4FM the land-mobile digital modes transmit.
    Fsk4,
    /// Two-phase shift keying.
    Psk2,
    /// Four-phase shift keying.
    Psk4,
    /// Constant envelope, flat spectrum, no symbol structure to find: OFDM, spread spectrum,
    /// and anything else that transmits something shaped like noise.
    NoiseLike,
    /// Something is there, and none of the tests agreed on what.
    Unknown,
}

impl Modulation {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "no signal",
            Self::Carrier => "unmodulated carrier",
            Self::Ook => "OOK",
            Self::Am => "AM",
            Self::Ssb => "SSB",
            Self::Fm => "FM",
            Self::Fsk2 => "2-FSK",
            Self::Fsk4 => "4-FSK",
            Self::Psk2 => "BPSK",
            Self::Psk4 => "QPSK",
            Self::NoiseLike => "noise-like",
            Self::Unknown => "unknown",
        }
    }

    /// Whether the identifier found anything to describe.
    #[must_use]
    pub const fn is_signal(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// The measurements a classification was made from, carried so the decision can be checked
/// rather than taken on trust — an operator staring at an unfamiliar signal needs the numbers,
/// not just the verdict.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct IdentFeatures {
    /// Standard deviation of the envelope over its own mean. Zero for a constant-envelope
    /// mode, large for anything that carries information in amplitude.
    pub envelope_variation: f32,
    /// Fraction of the observation the carrier was keyed on. 1.0 for a continuous transmission.
    pub duty: f32,
    /// Keyed-on level over keyed-off level, in dB — how deep the keying goes, which is what
    /// separates a switched carrier from one that is merely fading.
    pub keying_depth_db: f32,
    /// Power imbalance about the strongest spectral line, `(upper − lower) / total`. Zero for
    /// a symmetric spectrum (AM, FM, most digital modes), ±1 for a single sideband.
    pub spectral_asymmetry: f32,
    /// The strongest spectral line over the median of the occupied band, in dB — how much of
    /// a carrier there is.
    pub carrier_db: f32,
    /// Wiener entropy of the occupied band: 1.0 is white, 0.0 is a single tone.
    pub spectral_flatness: f32,
    /// Instantaneous-frequency levels the discriminator resolved: 2 or 4 for keyed modes, 1
    /// for a carrier, 0 when the distribution is a continuum.
    pub frequency_levels: u8,
    /// Spread of the instantaneous frequency, in Hz — the deviation of an analog FM signal,
    /// and roughly the outer deviation of a keyed one.
    pub frequency_spread_hz: f64,
    /// Strength of the spectral line the squared signal produces, over its own floor, in dB.
    /// A BPSK signal makes one; so does MSK, which is why the FSK tests run first.
    pub square_line_db: f32,
    /// The same for the fourth power, which is what QPSK makes.
    pub quartic_line_db: f32,
}

/// One protocol the waveform could be, and what says so.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ProtocolMatch {
    pub name: String,
    /// The channel type that decodes it, when this build has one — what the client offers to
    /// switch the channel to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_id: Option<String>,
    /// How well the measurements fit the protocol's signature, 0 to 1.
    pub score: f32,
    /// Set when the protocol's own framing was found in the signal, not merely resembled. A
    /// confirmed match is an answer; an unconfirmed one is a shortlist entry.
    #[serde(default)]
    pub confirmed: bool,
    /// The evidence, in a phrase: what matched, or what was recognised.
    pub why: String,
}

/// What the signal identifier made of the slice it was pointed at.
///
/// Emitted on a cadence rather than per frame: there is no frame here. Each report describes one
/// observation window, so a stream of them is a record of what was on the air and when.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct IdentReport {
    pub modulation: Modulation,
    /// How firmly the classifier held that answer, 0 to 1.
    pub confidence: f32,
    /// Which sideband, when the modulation is [`Modulation::Ssb`] and the signal sits wholly to
    /// one side of where the channel is tuned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sideband: Option<crate::channel::Sideband>,
    /// Occupied bandwidth, in Hz.
    pub bandwidth_hz: f64,
    /// Where the signal's centre sits relative to the channel's own offset, in Hz — how far
    /// the operator is off tune.
    pub center_offset_hz: f64,
    /// Signal-to-noise ratio in the occupied band, in dB.
    pub snr_db: f32,
    /// Symbols per second, when a symbol clock was found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_rate_hz: Option<f64>,
    /// Peak frequency deviation of a keyed or analog FM signal, in Hz.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deviation_hz: Option<f64>,
    /// Protocols that fit, best first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<ProtocolMatch>,
    pub features: IdentFeatures,
}

impl IdentReport {
    /// The candidate the report is actually asserting, if any: a confirmed match, or the best
    /// resemblance when nothing was confirmed.
    #[must_use]
    pub fn best(&self) -> Option<&ProtocolMatch> {
        self.candidates.first()
    }
}

/// Which digital-voice mode a [`DvFrame`] was heard on ( wave 3). One event type
/// serves all of them because the *question* is the same in every mode — who is talking, to
/// whom, on which network — and only the names for it differ.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DvMode {
    #[default]
    Dmr,
    Dstar,
    Ysf,
    Nxdn,
    P25,
    Dpmr,
    M17,
    #[serde(rename = "freedv")]
    FreeDv,
}

impl DvMode {
    /// Display name, as operators write it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Dmr => "DMR",
            Self::Dstar => "D-STAR",
            Self::Ysf => "YSF",
            Self::Nxdn => "NXDN",
            Self::P25 => "P25",
            Self::Dpmr => "dPMR",
            Self::M17 => "M17",
            Self::FreeDv => "FreeDV",
        }
    }
}

/// What the burst was carrying. Every mode distinguishes these four, whatever it calls them:
/// the frame that opens a transmission and names the parties, the voice frames that follow,
/// the one that closes it, and the signalling that travels outside a call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DvFrameKind {
    /// Call setup: DMR voice LC header, D-Star header, P25 header, M17 link setup.
    #[default]
    Header,
    /// A voice burst whose signalling named the call (late entry, or an embedded/link-control
    /// repeat). Voice frames carrying nothing new are not reported — a 20 ms heartbeat is not
    /// a log entry.
    Voice,
    /// End of transmission.
    Terminator,
    /// Signalling outside a call: a DMR CSBK, a P25 trunking block, an NXDN control message.
    Control,
    /// Payload data rather than voice: a DMR data header, a D-Star fast-data or slow-data
    /// text block, an M17 packet.
    Data,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Vendor {
    Standard,
    Etsi,
    Motorola,
    Hytera,
    Harris,
    Tait,
    JvcKenwood,
    Emc,
    RadioActivity,
    FlydeMicro,
    ProdEl,
    Unknown,
}

impl Vendor {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Etsi => "ETSI",
            Self::Motorola => "Motorola",
            Self::Hytera => "Hytera",
            Self::Harris => "Harris",
            Self::Tait => "Tait",
            Self::JvcKenwood => "JVCKENWOOD",
            Self::Emc => "EMC",
            Self::RadioActivity => "Radio Activity",
            Self::FlydeMicro => "Flyde Micro",
            Self::ProdEl => "PROD-EL",
            Self::Unknown => "unknown vendor",
        }
    }
}

/// Activity advertised for one DMR timeslot by a Short LC activity update.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DvSlotActivity {
    pub slot: u8,
    pub activity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_hash: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DvChannelDefinition {
    pub channel: u16,
    pub tx_hz: u64,
    pub rx_hz: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_code: Option<u8>,
}

/// What the signalling turned out to be. An observation, so it has no "auto" —
/// [`crate::DmrTrunkProtocol`] is what an operator asks the node for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DvTrunkProtocol {
    CapacityPlus,
    HyteraXpt,
    TierThree,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DvFrame {
    pub mode: DvMode,
    pub kind: DvFrameKind,
    /// TDMA timeslot, 1 or 2 — DMR only; every other mode here is single-slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<u8>,
    /// The mode's network discriminator, under whichever name it publishes: DMR colour code,
    /// NXDN/dPMR RAN or colour code, P25 NAC, YSF has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<Vendor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer_id: Option<u8>,
    /// True for a talkgroup call, false for a call addressed to one radio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_call: Option<bool>,
    /// Numeric source address — DMR/NXDN/dPMR radio ID, P25 source unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<u32>,
    /// Numeric destination: talkgroup for a group call, radio ID for a private one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<u32>,
    /// Source callsign — the modes that address by callsign rather than by number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_call: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_call: Option<String>,
    /// The repeater or reflector the call is routed through: D-Star RPT1/RPT2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// Set when the frame says its payload is encrypted. `Some(false)` is a positive statement
    /// that it is in the clear; `None` means the frame did not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub algorithm_id: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<u16>,
    /// P25 encryption message indicator as the 72-bit hexadecimal value on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_indicator: Option<String>,
    /// DMR talker alias assembled from its header and continuation LCs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub talker_alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_error_m: Option<u32>,
    /// Logical or absolute channel number named by trunking signalling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_definition: Option<DvChannelDefinition>,
    /// Set where the signalling names its own flavour, so a follower keys on a discriminant
    /// rather than on the wording of [`DvFrame::opcode`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trunk_protocol: Option<DvTrunkProtocol>,
    /// Whether the frame's own CRC matched. `Some(false)` survived only because the channel was
    /// told to ignore the check, so nothing may act on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crc_verified: Option<bool>,
    /// Capacity Plus logical slot number carrying the rest channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rest_channel: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_id: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site_id: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emergency: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub late_entry: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slot_activity: Vec<DvSlotActivity>,
    /// Decoded packet text, or hexadecimal when its application format is not understood.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// Name of the signalling opcode for a control frame — "group voice channel grant",
    /// "preamble", … — as its specification names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opcode: Option<String>,
    /// Free text the frame carried: a D-Star slow-data message, a YSF radio ID, an M17 meta
    /// field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Bit errors the frame's error-correcting codes repaired — the honest signal-quality
    /// readout, since a mode with no audio has no other.
    pub errors_corrected: u32,
}

impl DvFrame {
    /// A frame of `mode` and `kind` with nothing else known yet.
    #[must_use]
    pub fn new(mode: DvMode, kind: DvFrameKind) -> Self {
        Self {
            mode,
            kind,
            ..Self::default()
        }
    }

    /// How the parties read on one line: `TG 505 ← 2621001`, `DL1ABC → CQCQCQ`.
    #[must_use]
    pub fn parties(&self) -> Option<String> {
        let to = self.destination_call.clone().or_else(|| {
            self.destination.map(|d| match self.group_call {
                Some(false) => d.to_string(),
                _ => format!("TG {d}"),
            })
        });
        let from = self
            .source_call
            .clone()
            .or_else(|| self.source.map(|s| s.to_string()));
        match (to, from) {
            (Some(to), Some(from)) => Some(format!("{to} ← {from}")),
            (Some(to), None) => Some(to),
            (None, Some(from)) => Some(from),
            (None, None) => None,
        }
    }
}

/// One complete civil-time minute recovered from a long-wave radio-clock service.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RadioClockFrame {
    pub standard: RadioClockStandard,
    /// ISO 8601 civil time carried on air. DCF77, MSF and JJY include their UTC offset;
    /// WWVB is UTC.
    pub datetime: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utc_offset_minutes: Option<i16>,
    #[serde(default)]
    pub dst: bool,
    #[serde(default)]
    pub leap_warning: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dut1_seconds: Option<f32>,
    /// The 60 received symbols (`0`, `1`, `M` marker, `?` invalid).
    pub symbols: String,
}

/// GPS L1 C/A acquisition state or one parity-checked NAV subframe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct GnssFrame {
    pub prn: u8,
    pub doppler_hz: f32,
    pub code_phase_chips: f32,
    pub cn0_db_hz: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subframe: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tow_seconds: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week: Option<u16>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum DecoderEvent {
    Rds(RdsUpdate),
    Pocsag(PocsagMessage),
    Adsb(AdsbMessage),
    Ais(AisMessage),
    Aprs(AprsPacket),
    Rtty(RttyText),
    Morse(MorseText),
    Selcall(SelcallSequence),
    Navtex(NavtexMessage),
    Acars(AcarsMessage),
    Subghz(SubghzFrame),
    Tone(ToneSquelchStatus),
    Dv(DvFrame),
    Ident(IdentReport),
    RadioClock(RadioClockFrame),
    Gnss(GnssFrame),
}

impl DecoderEvent {
    /// Stable discriminator, matching the channel `type_id` that produces it.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Rds(_) => "rds",
            Self::Pocsag(_) => "pocsag",
            Self::Adsb(_) => "adsb",
            Self::Ais(_) => "ais",
            Self::Aprs(_) => "aprs",
            Self::Rtty(_) => "rtty",
            Self::Morse(_) => "morse",
            Self::Selcall(_) => "selcall",
            Self::Navtex(_) => "navtex",
            Self::Acars(_) => "acars",
            Self::Subghz(_) => "subghz",
            Self::Tone(_) => "tone",
            Self::Dv(_) => "dv",
            Self::Ident(_) => "ident",
            Self::RadioClock(_) => "radio_clock",
            Self::Gnss(_) => "gnss",
        }
    }

    /// One-line human summary: the log list's row text and the CSV export's `summary`
    /// column. Lives here so every consumer renders an event the same way.
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::Rds(r) => {
                let mut parts = Vec::new();
                if let Some(pi) = &r.pi {
                    parts.push(format!("PI {pi}"));
                }
                if let Some(ps) = &r.ps {
                    parts.push(ps.clone());
                }
                if let Some(pty) = &r.pty_name {
                    parts.push(pty.clone());
                }
                if let Some(rt) = &r.radiotext {
                    parts.push(rt.clone());
                }
                parts.join(" · ")
            }
            Self::Pocsag(p) => {
                if p.text.is_empty() {
                    format!("{} ({})", p.address, p.function)
                } else {
                    format!("{}: {}", p.address, p.text)
                }
            }
            Self::Adsb(a) => {
                let mut parts = vec![a.icao.clone()];
                if let Some(cs) = &a.callsign {
                    parts.push(cs.trim().to_owned());
                }
                if let Some(alt) = a.altitude_ft {
                    parts.push(format!("{alt} ft"));
                }
                if let (Some(lat), Some(lon)) = (a.lat, a.lon) {
                    parts.push(format!("{lat:.4}, {lon:.4}"));
                }
                parts.join(" · ")
            }
            Self::Ais(m) => {
                let mut parts = vec![m.mmsi.to_string()];
                if let Some(name) = &m.name {
                    parts.push(name.trim().to_owned());
                }
                if let (Some(lat), Some(lon)) = (m.lat, m.lon) {
                    parts.push(format!("{lat:.4}, {lon:.4}"));
                }
                parts.join(" · ")
            }
            // A Mic-E monitor line is packed binary, so the message named beside it is the
            // only part of the row a reader can act on.
            Self::Aprs(p) => match &p.mic_e_message {
                Some(message) => format!("{} · {message}", p.tnc2),
                None => p.tnc2.clone(),
            },
            Self::Rtty(t) => t.text.clone(),
            Self::Morse(m) => m.text.clone(),
            Self::Selcall(s) => format!(
                "{} · {}",
                match s.system {
                    crate::channel::SelcallSystem::Ccir1 => "CCIR-1",
                    crate::channel::SelcallSystem::Zvei1 => "ZVEI-1",
                },
                s.code
            ),
            Self::Navtex(n) => {
                let mut parts = Vec::new();
                if let Some(id) = n.header() {
                    parts.push(id);
                }
                if let Some(subject) = &n.subject_name {
                    parts.push(subject.clone());
                }
                parts.push(n.text.replace('\n', " ").trim().to_owned());
                parts.retain(|p| !p.is_empty());
                parts.join(" · ")
            }
            Self::Acars(a) => {
                let mut parts = vec![a.registration.clone()];
                if let Some(flight) = &a.flight {
                    parts.push(flight.trim().to_owned());
                }
                parts.push(format!("[{}]", a.label));
                let text = a.text.replace('\n', " ");
                let text = text.trim();
                if !text.is_empty() {
                    parts.push(text.to_owned());
                }
                parts.join(" · ")
            }
            Self::Tone(t) => {
                let mut parts = Vec::new();
                if let Some(hz) = t.ctcss_hz {
                    parts.push(format!("CTCSS {hz:.1} Hz"));
                }
                if let Some(code) = t.dcs_code {
                    parts.push(format!("DCS {code:03}"));
                }
                if parts.is_empty() {
                    parts.push("no tone".to_owned());
                }
                parts.push(if t.open { "open" } else { "muted" }.to_owned());
                parts.join(" · ")
            }
            Self::Subghz(f) => {
                let mut parts = vec![if f.bits == 0 {
                    format!("raw, {} edges", f.timings_us.len())
                } else {
                    format!("{} bit {}", f.bits, f.data)
                }];
                if let Some(address) = f.address {
                    parts.push(format!("addr {address:05X}"));
                }
                if let Some(button) = f.button {
                    parts.push(format!("btn {button:X}"));
                }
                if f.repeats > 1 {
                    parts.push(format!("×{}", f.repeats));
                }
                parts.join(" · ")
            }
            Self::Dv(f) => {
                let mut parts = vec![f.mode.label().to_owned()];
                if let Some(slot) = f.slot {
                    parts.push(format!("TS{slot}"));
                }
                if let Some(cc) = f.color_code {
                    parts.push(match f.mode {
                        DvMode::P25 => format!("NAC {cc:03X}"),
                        DvMode::Nxdn | DvMode::Dpmr => format!("RAN {cc}"),
                        _ => format!("CC {cc}"),
                    });
                }
                if let Some(parties) = f.parties() {
                    parts.push(parties);
                }
                if let Some(alias) = &f.talker_alias {
                    parts.push(alias.clone());
                }
                if let Some(vendor) = f.vendor {
                    parts.push(vendor.label().to_owned());
                }
                if let Some(via) = &f.via {
                    parts.push(format!("via {via}"));
                }
                if let Some(opcode) = &f.opcode {
                    parts.push(opcode.clone());
                }
                if f.encrypted == Some(true) {
                    parts.push(match (f.algorithm_id, f.key_id) {
                        (Some(algorithm), Some(key)) => {
                            format!("encrypted ALG {algorithm:02X} KID {key:04X}")
                        }
                        _ => "encrypted".to_owned(),
                    });
                }
                if let Some(text) = &f.text {
                    parts.push(text.clone());
                }
                parts.join(" · ")
            }
            Self::Ident(r) => {
                let mut parts = vec![r.modulation.label().to_owned()];
                if r.modulation.is_signal() {
                    parts.push(format!("{:.1} kHz", r.bandwidth_hz / 1_000.0));
                    if let Some(baud) = r.symbol_rate_hz {
                        parts.push(format!("{baud:.0} Bd"));
                    }
                    if let Some(deviation) = r.deviation_hz {
                        parts.push(format!("±{deviation:.0} Hz"));
                    }
                    parts.push(format!("{:.0} dB SNR", r.snr_db));
                }
                if let Some(best) = r.best() {
                    parts.push(if best.confirmed {
                        format!("{} (confirmed)", best.name)
                    } else {
                        format!("{} ({:.0}%)", best.name, best.score * 100.0)
                    });
                }
                parts.join(" · ")
            }
            Self::RadioClock(r) => {
                let mut parts = vec![
                    format!("{:?}", r.standard).to_uppercase(),
                    r.datetime.clone(),
                ];
                if r.leap_warning {
                    parts.push("leap warning".to_owned());
                }
                parts.join(" · ")
            }
            Self::Gnss(g) => {
                let mut parts = vec![
                    format!("GPS PRN {}", g.prn),
                    format!("{:+.0} Hz", g.doppler_hz),
                    format!("{:.1} dB-Hz", g.cn0_db_hz),
                ];
                if let Some(id) = g.subframe {
                    parts.push(format!("subframe {id}"));
                } else {
                    parts.push("acquired".to_owned());
                }
                if let Some(tow) = g.tow_seconds {
                    parts.push(format!("TOW {tow} s"));
                }
                parts.join(" · ")
            }
        }
    }

    #[must_use]
    pub fn position(&self) -> Option<(f64, f64)> {
        let (lat, lon) = match self {
            Self::Adsb(a) => (a.lat, a.lon),
            Self::Ais(m) => (m.lat, m.lon),
            Self::Aprs(p) => (p.lat, p.lon),
            Self::Dv(f) => (f.lat, f.lon),
            _ => (None, None),
        };
        lat.zip(lon)
    }

    /// Stable identity of the emitter within a decoder — aircraft ICAO, vessel MMSI, APRS
    /// callsign, pager address. Map layers and the log's "latest per station" view key on it.
    #[must_use]
    pub fn station(&self) -> Option<String> {
        match self {
            Self::Rds(r) => r.pi.clone(),
            Self::Pocsag(p) => Some(p.address.to_string()),
            Self::Adsb(a) => Some(a.icao.clone()),
            Self::Ais(m) => Some(m.mmsi.to_string()),
            Self::Aprs(p) => Some(p.source.clone()),
            // A NAVTEX station is identified by B1 alone only within its NAVAREA, but a
            // receiver hears one area at a time, so B1 is the identity that matters here.
            Self::Navtex(n) => n.station.map(String::from),
            Self::Acars(a) => Some(a.registration.clone()),
            // The transmitter's own address when the payload carries one; otherwise the
            // payload itself, which is what an unidentified remote is known by.
            Self::Subghz(f) => f
                .address
                .map(|a| format!("{a:05X}"))
                .or_else(|| (!f.data.is_empty()).then(|| f.data.clone())),
            Self::Dv(f) => f
                .source_call
                .clone()
                .or_else(|| f.source.map(|s| s.to_string())),
            Self::Gnss(g) => Some(format!("GPS-{}", g.prn)),
            Self::RadioClock(r) => Some(format!("{:?}", r.standard).to_uppercase()),
            Self::Rtty(_) | Self::Morse(_) | Self::Selcall(_) | Self::Tone(_) | Self::Ident(_) => {
                None
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DecodedRecord {
    pub device_set: u32,
    pub channel: u32,
    /// RFC3339 UTC.
    pub at: String,
    /// Absolute RF frequency of the channel when the frame arrived, in Hz.
    pub freq_hz: f64,
    pub event: DecoderEvent,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `{"kind": …, "data": …}` tagging is what the generated TS union discriminates on,
    /// and what the log table's `kind` column mirrors; lock it.
    #[test]
    fn decoder_event_is_adjacently_tagged() {
        let ev = DecoderEvent::Pocsag(PocsagMessage {
            address: 1_234_567,
            function: 3,
            baud: 1_200,
            payload: PocsagPayload::Alpha,
            text: "TEST".to_owned(),
            errors_corrected: 1,
        });
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["kind"], "pocsag");
        assert_eq!(json["data"]["address"], 1_234_567);
        assert_eq!(json["data"]["payload"], "alpha");
        assert_eq!(ev.kind(), "pocsag");

        let back: DecoderEvent = serde_json::from_value(json).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn kind_matches_the_serialized_tag() {
        for ev in [
            DecoderEvent::Rds(RdsUpdate::default()),
            DecoderEvent::Pocsag(PocsagMessage {
                address: 1,
                function: 0,
                baud: 512,
                payload: PocsagPayload::Tone,
                text: String::new(),
                errors_corrected: 0,
            }),
            DecoderEvent::Adsb(AdsbMessage::default()),
            DecoderEvent::Ais(AisMessage::default()),
            DecoderEvent::Aprs(AprsPacket::default()),
            DecoderEvent::Rtty(RttyText {
                text: String::new(),
            }),
            DecoderEvent::Morse(MorseText {
                text: String::new(),
                wpm: 0.0,
            }),
            DecoderEvent::Selcall(SelcallSequence {
                system: crate::channel::SelcallSystem::Ccir1,
                code: "12345".to_owned(),
                tone_ms: 100,
            }),
            DecoderEvent::Navtex(NavtexMessage::default()),
            DecoderEvent::Acars(AcarsMessage::default()),
            DecoderEvent::Subghz(SubghzFrame::default()),
            DecoderEvent::Tone(ToneSquelchStatus::default()),
            DecoderEvent::RadioClock(RadioClockFrame {
                standard: RadioClockStandard::Dcf77,
                datetime: "2026-08-15T12:34:00+02:00".to_owned(),
                utc_offset_minutes: Some(120),
                dst: true,
                leap_warning: false,
                dut1_seconds: None,
                symbols: String::new(),
            }),
            DecoderEvent::Gnss(GnssFrame {
                prn: 1,
                doppler_hz: 0.0,
                code_phase_chips: 0.0,
                cn0_db_hz: 40.0,
                subframe: None,
                tow_seconds: None,
                week: None,
                words: Vec::new(),
            }),
        ] {
            let json = serde_json::to_value(&ev).unwrap();
            assert_eq!(json["kind"], ev.kind());
        }
    }

    /// The B2 table is the only place a letter becomes a claim about what the broadcast is
    /// for, so it is transcribed here against ITU-R M.540 rather than spot-checked — and the
    /// unassigned letters must stay unnamed.
    #[test]
    fn navtex_subject_names_match_the_standard_table() {
        const NAMED: [(char, &str); 16] = [
            ('A', "Navigational warning"),
            ('B', "Meteorological warning"),
            ('C', "Ice report"),
            ('D', "Search and rescue / piracy"),
            ('E', "Meteorological forecast"),
            ('F', "Pilot service"),
            ('G', "AIS"),
            ('H', "LORAN"),
            ('J', "SATNAV"),
            ('K', "Other electronic navaid"),
            ('L', "Navigational warning (additional)"),
            ('T', "Test transmission"),
            ('V', "Notice to fishermen"),
            ('W', "Environmental"),
            ('X', "Special service"),
            ('Y', "Special service"),
        ];
        for (subject, name) in NAMED {
            assert_eq!(
                NavtexMessage::subject_name(subject),
                Some(name),
                "{subject}"
            );
        }
        assert_eq!(NavtexMessage::subject_name('Z'), Some("No message on hand"));
        for unassigned in ['I', 'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'U'] {
            assert_eq!(
                NavtexMessage::subject_name(unassigned),
                None,
                "{unassigned} is unassigned and must stay unnamed"
            );
        }
    }

    #[test]
    fn navtex_header_needs_the_whole_group() {
        let mut msg = NavtexMessage {
            station: Some('D'),
            subject: Some('A'),
            serial: Some(7),
            ..NavtexMessage::default()
        };
        assert_eq!(msg.header().as_deref(), Some("DA07"));
        msg.serial = None;
        assert_eq!(msg.header(), None);
    }

    /// A raw sub-GHz capture carries no bits, so the summary must describe the capture rather
    /// than print an empty payload.
    #[test]
    fn subghz_summary_describes_a_raw_capture() {
        let raw = DecoderEvent::Subghz(SubghzFrame {
            timings_us: vec![320, 960, 960, 320],
            repeats: 1,
            ..SubghzFrame::default()
        });
        assert_eq!(raw.summary(), "raw, 4 edges");
        assert_eq!(raw.station(), None);

        let decoded = DecoderEvent::Subghz(SubghzFrame {
            encoding: SubghzEncoding::Pwm,
            bits: 24,
            data: "A1B2C3".to_owned(),
            address: Some(0x0A1B2),
            button: Some(3),
            repeats: 4,
            ..SubghzFrame::default()
        });
        assert_eq!(decoded.summary(), "24 bit A1B2C3 · addr 0A1B2 · btn 3 · ×4");
        assert_eq!(decoded.station().as_deref(), Some("0A1B2"));
    }

    #[test]
    fn clock_and_gnss_summaries_keep_the_measurements_used_by_the_live_log() {
        let clock = DecoderEvent::RadioClock(RadioClockFrame {
            standard: RadioClockStandard::Dcf77,
            datetime: "2026-08-15T12:34:00+02:00".to_owned(),
            utc_offset_minutes: Some(120),
            dst: true,
            leap_warning: false,
            dut1_seconds: None,
            symbols: String::new(),
        });
        assert_eq!(clock.summary(), "DCF77 · 2026-08-15T12:34:00+02:00");
        assert_eq!(clock.station().as_deref(), Some("DCF77"));

        let gnss = DecoderEvent::Gnss(GnssFrame {
            prn: 7,
            doppler_hz: 1_000.0,
            code_phase_chips: 158.34,
            cn0_db_hz: 44.5,
            subframe: None,
            tow_seconds: None,
            week: None,
            words: Vec::new(),
        });
        assert_eq!(
            gnss.summary(),
            "GPS PRN 7 · +1000 Hz · 44.5 dB-Hz · acquired"
        );
        assert_eq!(gnss.station().as_deref(), Some("GPS-7"));
    }

    /// A Mic-E packet's TNC2 line is the packed binary it was sent as, so the log row names
    /// the message; every other APRS packet's line is already readable and is left alone.
    #[test]
    fn a_mic_e_summary_names_the_message_beside_the_monitor_line() {
        let mut packet = AprsPacket {
            tnc2: "DL1ABC-9>S32U6T:`(_fn\"Oj/".to_owned(),
            ..AprsPacket::default()
        };
        assert_eq!(
            DecoderEvent::Aprs(packet.clone()).summary(),
            "DL1ABC-9>S32U6T:`(_fn\"Oj/"
        );
        packet.mic_e_message = Some("Returning".to_owned());
        assert_eq!(
            DecoderEvent::Aprs(packet).summary(),
            "DL1ABC-9>S32U6T:`(_fn\"Oj/ · Returning"
        );
    }

    #[test]
    fn position_is_reported_only_when_both_coordinates_are_known() {
        let mut a = AdsbMessage {
            icao: "3C6444".to_owned(),
            lat: Some(52.5),
            ..AdsbMessage::default()
        };
        assert_eq!(DecoderEvent::Adsb(a.clone()).position(), None);
        a.lon = Some(13.4);
        assert_eq!(DecoderEvent::Adsb(a).position(), Some((52.5, 13.4)));
        assert_eq!(
            DecoderEvent::Rtty(RttyText {
                text: "CQ".to_owned()
            })
            .position(),
            None
        );
    }

    #[test]
    fn decoded_record_roundtrips() {
        let rec = DecodedRecord {
            device_set: 1,
            channel: 2,
            at: "2026-08-09T12:00:00Z".to_owned(),
            freq_hz: 1_090_000_000.0,
            event: DecoderEvent::Adsb(AdsbMessage {
                icao: "3C6444".to_owned(),
                df: 17,
                type_code: Some(4),
                callsign: Some("DLH123".to_owned()),
                raw: "8D3C6444".to_owned(),
                ..AdsbMessage::default()
            }),
        };
        let json = serde_json::to_value(&rec).unwrap();
        assert_eq!(json["event"]["kind"], "adsb");
        assert_eq!(json["freq_hz"], 1_090_000_000.0);
        let back: DecodedRecord = serde_json::from_value(json).unwrap();
        assert_eq!(back, rec);
    }
}
