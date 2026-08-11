//! Channel model. Typed per-channel settings (PLAN §8): each demod owns a params struct
//! here, tagged into [`ChannelParams`] so the client gets a discriminated union.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Static description of a channel type, surfaced to drive the "add channel" UI (PLAN §8).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChannelDescriptor {
    /// Stable type id, e.g. `"nfm"`, `"am"`, `"ssb"`, `"wfm"`.
    pub type_id: String,
    /// Display name, e.g. `"NFM"`, `"WFM (mono)"`.
    pub name: String,
    /// Nominal RF bandwidth the channel needs, in Hz.
    pub bandwidth_hz: f64,
    /// IQ rate the demod expects from the DDC, in Hz.
    pub input_rate_hz: f64,
    /// Whether the channel produces listenable audio. Data decoders (PLAN §13 wave 1) do
    /// not, so the client hides their audio controls instead of offering a silent stream.
    /// Defaults to `true` so a snapshot from an older peer keeps the pre-M4 behaviour.
    #[serde(default = "default_has_audio")]
    pub has_audio: bool,
    /// [`crate::DecoderEvent::kind`] this channel emits, when it is a decoder — the client
    /// uses it to pick the panel that renders the events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoder_kind: Option<String>,
    /// The type occupies its whole channel rate, so it runs only with the device tuned to
    /// exactly `input_rate_hz` — a resampling DDC has no guard band left to give it (PLAN §18).
    /// Reported so the canvas can refuse the wire where the operator draws it, naming the rate
    /// that works, instead of letting the engine reject it after the fact. Defaults to `false`,
    /// which is every type that leaves a guard band.
    #[serde(default)]
    pub exact_rate_only: bool,
    /// Set when the channel is handed the device's **own** samples — mixed to its offset, never
    /// resampled. `input_rate_hz` is then the lowest device rate it can run at and this the
    /// highest, so a receiver is set anywhere in that range rather than to one exact number.
    ///
    /// ADS-B is the one such type (PLAN §18, amended): a 0.5 µs pulse is a single sample at
    /// 2 Msps, so any rate conversion splits it across two and nothing decodes — the decoder
    /// meets the radio at its rate instead. Mutually exclusive with `exact_rate_only`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_rate_max_hz: Option<f64>,
    /// The type ships a modulator as well as a demodulator, so a transmit channel of it can be
    /// built (PLAN §20). Says only that the waveform exists — whether it may ever reach an
    /// antenna is the authorized-use gate's question, and that gate has not been built.
    /// Defaults to `false`, which is every receive-only type.
    #[serde(default)]
    pub can_transmit: bool,
}

impl ChannelDescriptor {
    /// The device rates this type will run at, or `None` if it takes the one rate the DDC can
    /// resample to. Both ends inclusive.
    #[must_use]
    pub fn native_rate_range(&self) -> Option<(f64, f64)> {
        self.native_rate_max_hz.map(|max| (self.input_rate_hz, max))
    }
}

fn default_has_audio() -> bool {
    true
}

/// The neutral descriptor a channel module fills in. Its values are the serde defaults, so a
/// descriptor built from a literal and one parsed from JSON agree on every field nobody set —
/// `has_audio` in particular, which is `true` for the audio channels that pre-date the flag.
impl Default for ChannelDescriptor {
    fn default() -> Self {
        Self {
            type_id: String::new(),
            name: String::new(),
            bandwidth_hz: 0.0,
            input_rate_hz: 0.0,
            has_audio: default_has_audio(),
            decoder_kind: None,
            exact_rate_only: false,
            native_rate_max_hz: None,
            can_transmit: false,
        }
    }
}

fn default_nfm_bandwidth_hz() -> f64 {
    12_500.0
}

fn default_am_bandwidth_hz() -> f64 {
    10_000.0
}

fn default_ssb_bandwidth_hz() -> f64 {
    2_700.0
}

fn default_agc() -> bool {
    true
}

fn default_deemphasis_us() -> f32 {
    50.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct NfmParams {
    #[serde(default = "default_nfm_bandwidth_hz")]
    pub bandwidth_hz: f64,
}

impl Default for NfmParams {
    fn default() -> Self {
        Self {
            bandwidth_hz: default_nfm_bandwidth_hz(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AmParams {
    #[serde(default = "default_am_bandwidth_hz")]
    pub bandwidth_hz: f64,
    #[serde(default = "default_agc")]
    pub agc: bool,
}

impl Default for AmParams {
    fn default() -> Self {
        Self {
            bandwidth_hz: default_am_bandwidth_hz(),
            agc: default_agc(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Sideband {
    #[default]
    Usb,
    Lsb,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SsbParams {
    #[serde(default)]
    pub sideband: Sideband,
    #[serde(default = "default_ssb_bandwidth_hz")]
    pub bandwidth_hz: f64,
    #[serde(default = "default_agc")]
    pub agc: bool,
}

impl Default for SsbParams {
    fn default() -> Self {
        Self {
            sideband: Sideband::default(),
            bandwidth_hz: default_ssb_bandwidth_hz(),
            agc: default_agc(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WfmParams {
    /// De-emphasis time constant in µs (50 in most of the world, 75 in the Americas).
    #[serde(default = "default_deemphasis_us")]
    pub deemphasis_us: f32,
    /// Decode the 57 kHz RDS subcarrier alongside the audio (PLAN §13 P2). Off by default:
    /// it costs a second demod chain on the same channel.
    #[serde(default)]
    pub rds: bool,
}

impl Default for WfmParams {
    fn default() -> Self {
        Self {
            deemphasis_us: default_deemphasis_us(),
            rds: false,
        }
    }
}

/// Bit rate of a POCSAG transmission. Pagers on one frequency may use several, so `Auto`
/// (the default) locks onto whichever preamble it finds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PocsagBaud {
    #[default]
    Auto,
    B512,
    B1200,
    B2400,
}

impl PocsagBaud {
    /// The bit rates this setting admits, fastest first (the order the detector tries).
    #[must_use]
    pub fn rates(self) -> &'static [u16] {
        match self {
            Self::Auto => &[2_400, 1_200, 512],
            Self::B512 => &[512],
            Self::B1200 => &[1_200],
            Self::B2400 => &[2_400],
        }
    }
}

fn default_pocsag_bandwidth_hz() -> f64 {
    12_500.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PocsagParams {
    #[serde(default)]
    pub baud: PocsagBaud,
    #[serde(default = "default_pocsag_bandwidth_hz")]
    pub bandwidth_hz: f64,
    /// Swap mark and space: some transmitters (and some receiver chains) invert the
    /// discriminator polarity, which turns every codeword into noise.
    #[serde(default)]
    pub invert: bool,
}

impl Default for PocsagParams {
    fn default() -> Self {
        Self {
            baud: PocsagBaud::default(),
            bandwidth_hz: default_pocsag_bandwidth_hz(),
            invert: false,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AdsbParams {
    /// Repair single-bit errors the Mode S CRC localizes. Off trades sensitivity for a
    /// lower false-frame rate on a noisy antenna.
    #[serde(default = "default_true")]
    pub crc_fix: bool,
    /// Reference position for locally-decoded (single-frame) CPR positions, in degrees.
    /// Without one, a position needs a matching even/odd frame pair.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_lon: Option<f64>,
}

impl Default for AdsbParams {
    fn default() -> Self {
        Self {
            crc_fix: true,
            ref_lat: None,
            ref_lon: None,
        }
    }
}

/// Which of the two AIS channels a receiver is parked on. Only the label travels with the
/// decoded message — the tuning itself is the channel's `offset_hz`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AisChannel {
    /// 161.975 MHz.
    #[default]
    A,
    /// 162.025 MHz.
    B,
}

impl AisChannel {
    /// The `!AIVDM` channel letter for this AIS channel.
    #[must_use]
    pub fn letter(self) -> char {
        match self {
            Self::A => 'A',
            Self::B => 'B',
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AisParams {
    #[serde(default)]
    pub ais_channel: AisChannel,
}

/// AX.25 physical layer (PLAN §13: AFSK1200 + 9600 G3RUH).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AprsMode {
    /// Bell 202 AFSK (1200/2200 Hz) through an FM receiver — VHF APRS.
    #[default]
    Afsk1200,
    /// 9600 baud G3RUH: scrambled NRZI straight off the discriminator.
    G3ruh9600,
}

fn default_aprs_bandwidth_hz() -> f64 {
    12_500.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AprsParams {
    #[serde(default)]
    pub mode: AprsMode,
    #[serde(default = "default_aprs_bandwidth_hz")]
    pub bandwidth_hz: f64,
}

impl Default for AprsParams {
    fn default() -> Self {
        Self {
            mode: AprsMode::default(),
            bandwidth_hz: default_aprs_bandwidth_hz(),
        }
    }
}

fn default_rtty_baud() -> f64 {
    45.45
}

fn default_rtty_shift_hz() -> f64 {
    170.0
}

/// RTTY stop-bit length in bit periods. 45.45 baud amateur RTTY uses 1.5.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RttyStopBits {
    One,
    #[default]
    OneAndHalf,
    Two,
}

impl RttyStopBits {
    #[must_use]
    pub fn periods(self) -> f64 {
        match self {
            Self::One => 1.0,
            Self::OneAndHalf => 1.5,
            Self::Two => 2.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RttyParams {
    #[serde(default = "default_rtty_baud")]
    pub baud: f64,
    /// Mark/space separation in Hz (170 amateur, 450/850 commercial).
    #[serde(default = "default_rtty_shift_hz")]
    pub shift_hz: f64,
    #[serde(default)]
    pub stop_bits: RttyStopBits,
    /// Swap mark and space (equivalent to reversing the sideband).
    #[serde(default)]
    pub invert: bool,
    /// Return to the letters table after a space — the usual amateur convention, which
    /// recovers a stream that lost its shift character.
    #[serde(default = "default_true")]
    pub unshift_on_space: bool,
}

impl Default for RttyParams {
    fn default() -> Self {
        Self {
            baud: default_rtty_baud(),
            shift_hz: default_rtty_shift_hz(),
            stop_bits: RttyStopBits::default(),
            invert: false,
            unshift_on_space: true,
        }
    }
}

fn default_morse_bandwidth_hz() -> f64 {
    400.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct MorseParams {
    /// Width of the CW filter around the channel offset, in Hz.
    #[serde(default = "default_morse_bandwidth_hz")]
    pub bandwidth_hz: f64,
    /// Fixed sending speed in words per minute; `None` tracks the speed from the signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wpm: Option<f32>,
}

impl Default for MorseParams {
    fn default() -> Self {
        Self {
            bandwidth_hz: default_morse_bandwidth_hz(),
            wpm: None,
        }
    }
}

/// NAVTEX is a single-purpose broadcast: 100 baud, 170 Hz shift, CCIR 476 (ITU-R M.540), so
/// there is nothing to tune but the sideband the receiver happens to be on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NavtexParams {
    /// Swap mark and space (equivalent to reversing the sideband).
    #[serde(default)]
    pub invert: bool,
}

fn default_acars_bandwidth_hz() -> f64 {
    12_500.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AcarsParams {
    #[serde(default = "default_acars_bandwidth_hz")]
    pub bandwidth_hz: f64,
}

impl Default for AcarsParams {
    fn default() -> Self {
        Self {
            bandwidth_hz: default_acars_bandwidth_hz(),
        }
    }
}

/// How a sub-GHz device keys its carrier (PLAN §8b). The two need different front ends —
/// an envelope detector versus a discriminator — but produce the same pulse-width stream, so
/// everything above the detector is shared.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubghzModulation {
    /// On-off keying / ASK: garage remotes, doorbells, most 433 MHz sensors.
    #[default]
    Ook,
    /// Two-level FSK: TPMS sensors and the newer weather stations.
    Fsk,
}

fn default_subghz_bandwidth_hz() -> f64 {
    150_000.0
}

fn default_subghz_min_pulse_us() -> u32 {
    80
}

fn default_subghz_frame_gap_us() -> u32 {
    5_000
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SubghzParams {
    #[serde(default)]
    pub modulation: SubghzModulation,
    /// Detection bandwidth in Hz. Wide by radio standards on purpose: a SAW-controlled remote
    /// may sit tens of kHz off its nominal 433.92 MHz, and a filter narrow enough to be
    /// "correct" would simply miss it.
    #[serde(default = "default_subghz_bandwidth_hz")]
    pub bandwidth_hz: f64,
    /// Shortest keying edge accepted, in µs. Anything briefer is a noise spike, not a symbol.
    #[serde(default = "default_subghz_min_pulse_us")]
    pub min_pulse_us: u32,
    /// Silence that ends a frame, in µs. Must exceed the longest gap *inside* a frame — the
    /// PT2262/EV1527 sync gap is ~31 short periods, around 10 ms at a 320 µs clock.
    #[serde(default = "default_subghz_frame_gap_us")]
    pub frame_gap_us: u32,
}

impl Default for SubghzParams {
    fn default() -> Self {
        Self {
            modulation: SubghzModulation::default(),
            bandwidth_hz: default_subghz_bandwidth_hz(),
            min_pulse_us: default_subghz_min_pulse_us(),
            frame_gap_us: default_subghz_frame_gap_us(),
        }
    }
}

/// Which DMR timeslot a channel reports. Both slots share one 12.5 kHz carrier in 30 ms
/// alternation, so the receiver always hears both; this only decides what reaches the log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DmrSlots {
    #[default]
    Both,
    One,
    Two,
}

impl DmrSlots {
    /// Whether a burst decoded on `slot` (1 or 2) should be reported.
    #[must_use]
    pub fn accepts(self, slot: u8) -> bool {
        match self {
            Self::Both => true,
            Self::One => slot == 1,
            Self::Two => slot == 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DmrParams {
    #[serde(default)]
    pub slots: DmrSlots,
}

/// The two NXDN channel widths. They are different radios as far as the receiver is
/// concerned: 6.25 kHz halves both the symbol rate and the deviation of the 12.5 kHz one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NxdnBandwidth {
    /// 6.25 kHz, 2400 symbols per second — the common deployment.
    #[default]
    Narrow,
    /// 12.5 kHz, 4800 symbols per second.
    Wide,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct NxdnParams {
    #[serde(default)]
    pub bandwidth: NxdnBandwidth,
}

/// The digital-voice modes whose framing carries no options worth setting: everything about
/// them — symbol rate, deviation, channel width, sync patterns — is fixed by the mode.
macro_rules! empty_params {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {$(
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
        pub struct $name {}
    )*};
}

empty_params! {
    /// D-Star (GMSK, 4800 bit/s).
    DstarParams,
    /// System Fusion (C4FM, 4800 symbols/s).
    YsfParams,
    /// P25 Phase 1 (C4FM, 4800 symbols/s).
    P25Params,
    /// dPMR (C4FM, 2400 symbols/s, 6.25 kHz).
    DpmrParams,
    /// M17 (C4FM, 4800 symbols/s, RRC 0.5).
    M17Params,
}

/// Type-discriminated demod parameters. Adjacently tagged so the generated TS is a
/// discriminated union on `type`, and `{"type":"nfm","settings":{}}` deserializes with
/// every field at its default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", content = "settings", rename_all = "snake_case")]
pub enum ChannelParams {
    Nfm(NfmParams),
    Am(AmParams),
    Ssb(SsbParams),
    Wfm(WfmParams),
    Pocsag(PocsagParams),
    Adsb(AdsbParams),
    Ais(AisParams),
    Aprs(AprsParams),
    Rtty(RttyParams),
    Morse(MorseParams),
    Navtex(NavtexParams),
    Acars(AcarsParams),
    Subghz(SubghzParams),
    Dmr(DmrParams),
    Dstar(DstarParams),
    Ysf(YsfParams),
    Nxdn(NxdnParams),
    P25(P25Params),
    Dpmr(DpmrParams),
    M17(M17Params),
}

impl ChannelParams {
    /// The stable type id, matching [`ChannelDescriptor::type_id`].
    #[must_use]
    pub fn type_id(&self) -> &'static str {
        match self {
            Self::Nfm(_) => "nfm",
            Self::Am(_) => "am",
            Self::Ssb(_) => "ssb",
            Self::Wfm(_) => "wfm",
            Self::Pocsag(_) => "pocsag",
            Self::Adsb(_) => "adsb",
            Self::Ais(_) => "ais",
            Self::Aprs(_) => "aprs",
            Self::Rtty(_) => "rtty",
            Self::Morse(_) => "morse",
            Self::Navtex(_) => "navtex",
            Self::Acars(_) => "acars",
            Self::Subghz(_) => "subghz",
            Self::Dmr(_) => "dmr",
            Self::Dstar(_) => "dstar",
            Self::Ysf(_) => "ysf",
            Self::Nxdn(_) => "nxdn",
            Self::P25(_) => "p25",
            Self::Dpmr(_) => "dpmr",
            Self::M17(_) => "m17",
        }
    }
}

/// Per-channel settings: where the channel sits and how it demodulates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChannelSettings {
    /// Offset from the device center frequency, in Hz.
    #[serde(default)]
    pub offset_hz: f64,
    /// Squelch threshold in dBFS, measured on the channel-filtered IQ (the mode's occupied
    /// bandwidth, not the full DDC passband); `None` = squelch open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub squelch_db: Option<f32>,
    pub params: ChannelParams,
}

/// A live channel instance inside a device set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChannelInfo {
    pub id: u32,
    /// Which of the device's receive streams feeds this channel. Defaults to 0 because a peer
    /// that predates multi-stream devices names no stream and means the only one its radio has.
    #[serde(default)]
    pub stream: u32,
    pub settings: ChannelSettings,
}
