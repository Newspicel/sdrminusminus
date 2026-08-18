use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    audio::AudioProcessing,
    network::NetworkExportStatus,
    state::{AudioRecordingStatus, RecordingStatus},
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChannelDescriptor {
    pub type_id: String,
    pub name: String,
    pub bandwidth_hz: f64,
    pub input_rate_hz: f64,
    #[serde(default = "default_has_audio")]
    pub has_audio: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoder_kind: Option<String>,
    #[serde(default)]
    pub has_video: bool,
    #[serde(default)]
    pub exact_rate_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_rate_max_hz: Option<f64>,
    #[serde(default)]
    pub can_transmit: bool,
    #[serde(default)]
    pub needs_position: bool,
}

impl ChannelDescriptor {
    #[must_use]
    pub fn native_rate_range(&self) -> Option<(f64, f64)> {
        self.native_rate_max_hz.map(|max| (self.input_rate_hz, max))
    }
}

fn default_has_audio() -> bool {
    true
}

impl Default for ChannelDescriptor {
    fn default() -> Self {
        Self {
            type_id: String::new(),
            name: String::new(),
            bandwidth_hz: 0.0,
            input_rate_hz: 0.0,
            has_audio: default_has_audio(),
            decoder_kind: None,
            has_video: false,
            exact_rate_only: false,
            native_rate_max_hz: None,
            can_transmit: false,
            needs_position: false,
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

fn default_deemphasis_us() -> f32 {
    50.0
}

fn default_stereo() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NfmToneMode {
    #[default]
    Off,
    Detect,
    Ctcss,
    Dcs,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NfmScramblerMode {
    #[default]
    Off,
    Inversion,
    Auto,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct NfmParams {
    #[serde(default = "default_nfm_bandwidth_hz")]
    pub bandwidth_hz: f64,
    #[serde(default)]
    pub tone_mode: NfmToneMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctcss_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dcs_code: Option<u16>,
    #[serde(default)]
    pub scrambler_mode: NfmScramblerMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inversion_hz: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SelcallSystem {
    #[default]
    Ccir1,
    Zvei1,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SelcallParams {
    #[serde(default)]
    pub system: SelcallSystem,
}

impl Default for NfmParams {
    fn default() -> Self {
        Self {
            bandwidth_hz: default_nfm_bandwidth_hz(),
            tone_mode: NfmToneMode::default(),
            ctcss_hz: None,
            dcs_code: None,
            scrambler_mode: NfmScramblerMode::default(),
            inversion_hz: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AmParams {
    #[serde(default = "default_am_bandwidth_hz")]
    pub bandwidth_hz: f64,
}

impl Default for AmParams {
    fn default() -> Self {
        Self {
            bandwidth_hz: default_am_bandwidth_hz(),
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
}

impl Default for SsbParams {
    fn default() -> Self {
        Self {
            sideband: Sideband::default(),
            bandwidth_hz: default_ssb_bandwidth_hz(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WfmParams {
    #[serde(default = "default_deemphasis_us")]
    pub deemphasis_us: f32,
    #[serde(default = "default_stereo")]
    pub stereo: bool,
}

impl Default for WfmParams {
    fn default() -> Self {
        Self {
            deemphasis_us: default_deemphasis_us(),
            stereo: default_stereo(),
        }
    }
}

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

fn default_pager_bandwidth_hz() -> f64 {
    12_500.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct FlexParams {
    #[serde(default = "default_pager_bandwidth_hz")]
    pub bandwidth_hz: f64,
    #[serde(default)]
    pub invert: bool,
}

impl Default for FlexParams {
    fn default() -> Self {
        Self {
            bandwidth_hz: default_pager_bandwidth_hz(),
            invert: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ErmesParams {
    #[serde(default = "default_pager_bandwidth_hz")]
    pub bandwidth_hz: f64,
    #[serde(default)]
    pub invert: bool,
}

impl Default for ErmesParams {
    fn default() -> Self {
        Self {
            bandwidth_hz: default_pager_bandwidth_hz(),
            invert: false,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AdsbParams {
    #[serde(default = "default_true")]
    pub crc_fix: bool,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AisChannel {
    #[default]
    A,
    B,
}

impl AisChannel {
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AprsMode {
    #[default]
    Afsk1200,
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
    #[serde(default = "default_rtty_shift_hz")]
    pub shift_hz: f64,
    #[serde(default)]
    pub stop_bits: RttyStopBits,
    #[serde(default)]
    pub invert: bool,
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
    #[serde(default = "default_morse_bandwidth_hz")]
    pub bandwidth_hz: f64,
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

fn default_cw_skimmer_bandwidth_hz() -> f64 {
    24_000.0
}

fn default_cw_skimmer_threshold_db() -> f32 {
    10.0
}

fn default_cw_skimmer_max_signals() -> u16 {
    32
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CwSkimmerParams {
    #[serde(default = "default_cw_skimmer_bandwidth_hz")]
    pub bandwidth_hz: f64,
    #[serde(default = "default_cw_skimmer_threshold_db")]
    pub threshold_db: f32,
    #[serde(default = "default_cw_skimmer_max_signals")]
    pub max_signals: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wpm: Option<f32>,
}

impl Default for CwSkimmerParams {
    fn default() -> Self {
        Self {
            bandwidth_hz: default_cw_skimmer_bandwidth_hz(),
            threshold_db: default_cw_skimmer_threshold_db(),
            max_signals: default_cw_skimmer_max_signals(),
            wpm: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NavtexParams {
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubghzModulation {
    #[default]
    Ook,
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
    #[serde(default = "default_subghz_bandwidth_hz")]
    pub bandwidth_hz: f64,
    #[serde(default = "default_subghz_min_pulse_us")]
    pub min_pulse_us: u32,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AtvModulation {
    #[default]
    Am,
    Fm,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AtvStandard {
    #[default]
    Ccir625,
    Eia525,
    SystemA405,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AtvColor {
    #[default]
    Monochrome,
    Pal,
    Ntsc,
}

impl AtvStandard {
    #[must_use]
    pub fn lines(self) -> u16 {
        match self {
            Self::Ccir625 => 625,
            Self::Eia525 => 525,
            Self::SystemA405 => 405,
        }
    }

    #[must_use]
    pub fn line_rate_hz(self) -> f64 {
        match self {
            Self::Ccir625 => 15_625.0,
            Self::Eia525 => 15_734.264,
            Self::SystemA405 => 10_125.0,
        }
    }

    #[must_use]
    pub fn frame_rate_hz(self) -> f64 {
        self.line_rate_hz() / f64::from(self.lines())
    }
}

fn default_atv_bandwidth_hz() -> f64 {
    1_500_000.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AtvParams {
    #[serde(default)]
    pub modulation: AtvModulation,
    #[serde(default)]
    pub standard: AtvStandard,
    #[serde(default = "default_atv_bandwidth_hz")]
    pub bandwidth_hz: f64,
    #[serde(default)]
    pub invert: bool,
    #[serde(default = "default_true")]
    pub interlace: bool,
    #[serde(default)]
    pub color: AtvColor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound_subcarrier_hz: Option<f64>,
}

impl Default for AtvParams {
    fn default() -> Self {
        Self {
            modulation: AtvModulation::default(),
            standard: AtvStandard::default(),
            bandwidth_hz: default_atv_bandwidth_hz(),
            invert: false,
            interlace: true,
            color: AtvColor::default(),
            sound_subcarrier_hz: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SstvMode {
    Robot36,
    Robot72,
    MartinM1,
    MartinM2,
    ScottieS1,
    ScottieS2,
    ScottieDx,
    Pd50,
    Pd90,
    Pd120,
    Pd180,
    Sc2180,
}

impl SstvMode {
    pub const ALL: [Self; 12] = [
        Self::Robot36,
        Self::Robot72,
        Self::MartinM1,
        Self::MartinM2,
        Self::ScottieS1,
        Self::ScottieS2,
        Self::ScottieDx,
        Self::Pd50,
        Self::Pd90,
        Self::Pd120,
        Self::Pd180,
        Self::Sc2180,
    ];

    #[must_use]
    pub fn vis(self) -> u8 {
        match self {
            Self::Robot36 => 8,
            Self::Robot72 => 12,
            Self::MartinM2 => 40,
            Self::MartinM1 => 44,
            Self::Sc2180 => 55,
            Self::ScottieS2 => 56,
            Self::ScottieS1 => 60,
            Self::ScottieDx => 76,
            Self::Pd50 => 93,
            Self::Pd120 => 95,
            Self::Pd180 => 96,
            Self::Pd90 => 99,
        }
    }

    #[must_use]
    pub fn from_vis(vis: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.vis() == vis)
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Robot36 => "Robot 36",
            Self::Robot72 => "Robot 72",
            Self::MartinM1 => "Martin M1",
            Self::MartinM2 => "Martin M2",
            Self::ScottieS1 => "Scottie S1",
            Self::ScottieS2 => "Scottie S2",
            Self::ScottieDx => "Scottie DX",
            Self::Pd50 => "PD50",
            Self::Pd90 => "PD90",
            Self::Pd120 => "PD120",
            Self::Pd180 => "PD180",
            Self::Sc2180 => "Wraase SC2-180",
        }
    }

    #[must_use]
    pub fn size(self) -> (u16, u16) {
        match self {
            Self::Robot36 | Self::Robot72 => (320, 240),
            Self::MartinM1
            | Self::MartinM2
            | Self::ScottieS1
            | Self::ScottieS2
            | Self::ScottieDx
            | Self::Pd50
            | Self::Pd90
            | Self::Sc2180 => (320, 256),
            Self::Pd120 | Self::Pd180 => (640, 496),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SstvParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<SstvMode>,
    #[serde(default = "default_true")]
    pub slant_correction: bool,
    #[serde(default = "default_true")]
    pub keep_partial: bool,
}

impl Default for SstvParams {
    fn default() -> Self {
        Self {
            mode: None,
            slant_correction: true,
            keep_partial: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DabMode {
    #[default]
    Auto,
    Dab,
    DabPlus,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DabParams {
    #[serde(default)]
    pub mode: DabMode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DatvStandard {
    #[default]
    DvbS,
    DvbS2,
}

fn default_datv_symbol_rate() -> f64 {
    333_000.0
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DatvParams {
    #[serde(default)]
    pub standard: DatvStandard,
    #[serde(default = "default_datv_symbol_rate")]
    pub symbol_rate: f64,
}

impl Default for DatvParams {
    fn default() -> Self {
        Self {
            standard: DatvStandard::default(),
            symbol_rate: default_datv_symbol_rate(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DrmMode {
    #[default]
    Auto,
    Drm30,
    DrmPlus,
}

fn default_drm_bandwidth_hz() -> f64 {
    100_000.0
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DrmParams {
    #[serde(default)]
    pub mode: DrmMode,
    #[serde(default = "default_drm_bandwidth_hz")]
    pub bandwidth_hz: f64,
}

impl Default for DrmParams {
    fn default() -> Self {
        Self {
            mode: DrmMode::default(),
            bandwidth_hz: default_drm_bandwidth_hz(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DmrSlots {
    #[default]
    Both,
    One,
    Two,
}

impl DmrSlots {
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
    #[serde(default)]
    pub ignore_crc: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NxdnBandwidth {
    #[default]
    Narrow,
    Wide,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct NxdnParams {
    #[serde(default)]
    pub bandwidth: NxdnBandwidth,
}

macro_rules! empty_params {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {$(
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
        pub struct $name {}
    )*};
}

empty_params! {
    DstarParams,
    YsfParams,
    P25Params,
    DpmrParams,
    M17Params,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FreeDvMode {
    #[default]
    Mode1600,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FreeDvParams {
    #[serde(default)]
    pub mode: FreeDvMode,
    #[serde(default)]
    pub sideband: Sideband,
}

pub const MIN_IDENT_BANDWIDTH_HZ: f64 = 1_000.0;
pub const MAX_IDENT_BANDWIDTH_HZ: f64 = 192_000.0;
pub const MIN_IDENT_INTERVAL_MS: u32 = 250;
pub const MAX_IDENT_INTERVAL_MS: u32 = 10_000;
pub const MIN_IDENT_THRESHOLD_DB: f32 = 3.0;
pub const MAX_IDENT_THRESHOLD_DB: f32 = 40.0;

fn default_ident_bandwidth_hz() -> f64 {
    MAX_IDENT_BANDWIDTH_HZ
}

fn default_ident_interval_ms() -> u32 {
    1_000
}

fn default_ident_threshold_db() -> f32 {
    8.0
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct IdentParams {
    #[serde(default = "default_ident_bandwidth_hz")]
    pub bandwidth_hz: f64,
    #[serde(default = "default_ident_interval_ms")]
    pub interval_ms: u32,
    #[serde(default = "default_ident_threshold_db")]
    pub threshold_db: f32,
}

impl Default for IdentParams {
    fn default() -> Self {
        Self {
            bandwidth_hz: default_ident_bandwidth_hz(),
            interval_ms: default_ident_interval_ms(),
            threshold_db: default_ident_threshold_db(),
        }
    }
}

fn default_wsjt_low_hz() -> f32 {
    200.0
}

fn default_wsjt_high_hz() -> f32 {
    3_000.0
}

fn default_wsjt_candidates() -> u16 {
    50
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WsjtParams {
    #[serde(default = "default_wsjt_low_hz")]
    pub audio_low_hz: f32,
    #[serde(default = "default_wsjt_high_hz")]
    pub audio_high_hz: f32,
    #[serde(default = "default_wsjt_candidates")]
    pub max_candidates: u16,
}

impl Default for WsjtParams {
    fn default() -> Self {
        Self {
            audio_low_hz: default_wsjt_low_hz(),
            audio_high_hz: default_wsjt_high_hz(),
            max_candidates: default_wsjt_candidates(),
        }
    }
}

fn default_wspr_low_hz() -> f32 {
    1_400.0
}

fn default_wspr_high_hz() -> f32 {
    1_600.0
}

fn default_wspr_candidates() -> u16 {
    200
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WsprParams {
    #[serde(default = "default_wspr_low_hz")]
    pub audio_low_hz: f32,
    #[serde(default = "default_wspr_high_hz")]
    pub audio_high_hz: f32,
    #[serde(default = "default_wspr_candidates")]
    pub max_candidates: u16,
}

impl Default for WsprParams {
    fn default() -> Self {
        Self {
            audio_low_hz: default_wspr_low_hz(),
            audio_high_hz: default_wspr_high_hz(),
            max_candidates: default_wspr_candidates(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PskBaud {
    #[default]
    Psk31,
    Psk63,
    Psk125,
    Psk250,
}

impl PskBaud {
    #[must_use]
    pub fn rate(self) -> f64 {
        match self {
            Self::Psk31 => 31.25,
            Self::Psk63 => 62.5,
            Self::Psk125 => 125.0,
            Self::Psk250 => 250.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PskParams {
    #[serde(default)]
    pub baud: PskBaud,
    #[serde(default)]
    pub invert: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RadioClockStandard {
    #[default]
    Dcf77,
    Wwvb,
    Msf,
    Jjy,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RadioClockParams {
    #[serde(default)]
    pub standard: RadioClockStandard,
    #[serde(default)]
    pub invert: bool,
}

fn default_gnss_prn() -> u8 {
    1
}

fn default_gnss_doppler_hz() -> u32 {
    10_000
}

fn default_gnss_threshold() -> f32 {
    2.5
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct GnssParams {
    #[serde(default = "default_gnss_prn")]
    pub prn: u8,
    #[serde(default = "default_gnss_doppler_hz")]
    pub doppler_hz: u32,
    #[serde(default = "default_gnss_threshold")]
    pub threshold: f32,
}

impl Default for GnssParams {
    fn default() -> Self {
        Self {
            prn: default_gnss_prn(),
            doppler_hz: default_gnss_doppler_hz(),
            threshold: default_gnss_threshold(),
        }
    }
}

pub const MIN_NAVAID_REPORT_MS: u32 = 250;
pub const MAX_NAVAID_REPORT_MS: u32 = 5_000;

fn default_navaid_report_ms() -> u32 {
    500
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct VorParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station_lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station_lon: Option<f64>,
    #[serde(default)]
    pub magnetic_declination_deg: f64,
    #[serde(default = "default_navaid_report_ms")]
    #[schema(minimum = 250, maximum = 5000)]
    pub report_ms: u32,
}

impl Default for VorParams {
    fn default() -> Self {
        Self {
            station: None,
            station_lat: None,
            station_lon: None,
            magnetic_declination_deg: 0.0,
            report_ms: default_navaid_report_ms(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum IlsComponent {
    #[default]
    Localizer,
    Glideslope,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct IlsParams {
    #[serde(default)]
    pub component: IlsComponent,
    #[serde(default = "default_navaid_report_ms")]
    #[schema(minimum = 250, maximum = 5000)]
    pub report_ms: u32,
}

impl Default for IlsParams {
    fn default() -> Self {
        Self {
            component: IlsComponent::default(),
            report_ms: default_navaid_report_ms(),
        }
    }
}

empty_params! {
    DscParams,
    InmarsatStdcParams,
    InmarsatAeroParams,
    Vdl2Params,
    HfdlParams,
    IridiumParams,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", content = "settings", rename_all = "snake_case")]
pub enum ChannelParams {
    Nfm(NfmParams),
    Selcall(SelcallParams),
    Am(AmParams),
    Ssb(SsbParams),
    Wfm(WfmParams),
    Pocsag(PocsagParams),
    Flex(FlexParams),
    Ermes(ErmesParams),
    Adsb(AdsbParams),
    Ais(AisParams),
    Aprs(AprsParams),
    Rtty(RttyParams),
    Morse(MorseParams),
    CwSkimmer(CwSkimmerParams),
    Navtex(NavtexParams),
    Acars(AcarsParams),
    Subghz(SubghzParams),
    Atv(AtvParams),
    Sstv(SstvParams),
    Dab(DabParams),
    Datv(DatvParams),
    Drm(DrmParams),
    Dmr(DmrParams),
    Dstar(DstarParams),
    Ysf(YsfParams),
    Nxdn(NxdnParams),
    P25(P25Params),
    Dpmr(DpmrParams),
    M17(M17Params),
    Ft8(WsjtParams),
    Ft4(WsjtParams),
    Psk(PskParams),
    Wspr(WsprParams),
    Freedv(FreeDvParams),
    Ident(IdentParams),
    RadioClock(RadioClockParams),
    Gnss(GnssParams),
    Vor(VorParams),
    Ils(IlsParams),
    Dsc(DscParams),
    InmarsatStdc(InmarsatStdcParams),
    InmarsatAero(InmarsatAeroParams),
    Vdl2(Vdl2Params),
    Hfdl(HfdlParams),
    Iridium(IridiumParams),
}

impl ChannelParams {
    #[must_use]
    pub fn type_id(&self) -> &'static str {
        match self {
            Self::Nfm(_) => "nfm",
            Self::Selcall(_) => "selcall",
            Self::Am(_) => "am",
            Self::Ssb(_) => "ssb",
            Self::Wfm(_) => "wfm",
            Self::Pocsag(_) => "pocsag",
            Self::Flex(_) => "flex",
            Self::Ermes(_) => "ermes",
            Self::Adsb(_) => "adsb",
            Self::Ais(_) => "ais",
            Self::Aprs(_) => "aprs",
            Self::Rtty(_) => "rtty",
            Self::Morse(_) => "morse",
            Self::CwSkimmer(_) => "cw_skimmer",
            Self::Navtex(_) => "navtex",
            Self::Acars(_) => "acars",
            Self::Subghz(_) => "subghz",
            Self::Atv(_) => "atv",
            Self::Sstv(_) => "sstv",
            Self::Dab(_) => "dab",
            Self::Datv(_) => "datv",
            Self::Drm(_) => "drm",
            Self::Dmr(_) => "dmr",
            Self::Dstar(_) => "dstar",
            Self::Ysf(_) => "ysf",
            Self::Nxdn(_) => "nxdn",
            Self::P25(_) => "p25",
            Self::Dpmr(_) => "dpmr",
            Self::M17(_) => "m17",
            Self::Ft8(_) => "ft8",
            Self::Ft4(_) => "ft4",
            Self::Psk(_) => "psk",
            Self::Wspr(_) => "wspr",
            Self::Freedv(_) => "freedv",
            Self::Ident(_) => "ident",
            Self::RadioClock(_) => "radio_clock",
            Self::Gnss(_) => "gnss",
            Self::Vor(_) => "vor",
            Self::Ils(_) => "ils",
            Self::Dsc(_) => "dsc",
            Self::InmarsatStdc(_) => "inmarsat_stdc",
            Self::InmarsatAero(_) => "inmarsat_aero",
            Self::Vdl2(_) => "vdl2",
            Self::Hfdl(_) => "hfdl",
            Self::Iridium(_) => "iridium",
        }
    }
}

pub const MIN_SQUELCH_AUTO_MARGIN_DB: f32 = 2.0;
pub const MAX_SQUELCH_AUTO_MARGIN_DB: f32 = 40.0;

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct ChannelSettings {
    #[serde(default)]
    pub offset_hz: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub squelch_db: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub squelch_auto_db: Option<f32>,
    pub params: ChannelParams,
    #[serde(default)]
    pub audio: AudioProcessing,
}

impl ChannelSettings {
    #[must_use]
    pub fn default_for(type_id: &str) -> Option<Self> {
        Some(Self {
            offset_hz: 0.0,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::default_for(type_id)?,
            audio: AudioProcessing::default_for(type_id),
        })
    }
}

impl<'de> Deserialize<'de> for ChannelSettings {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Stated {
            #[serde(default)]
            offset_hz: f64,
            #[serde(default)]
            squelch_db: Option<f32>,
            #[serde(default)]
            squelch_auto_db: Option<f32>,
            params: ChannelParams,
            #[serde(default)]
            audio: Option<AudioProcessing>,
        }
        let stated = Stated::deserialize(deserializer)?;
        let audio = stated
            .audio
            .unwrap_or_else(|| AudioProcessing::default_for(stated.params.type_id()));
        Ok(Self {
            offset_hz: stated.offset_hz,
            squelch_db: stated.squelch_db,
            squelch_auto_db: stated.squelch_auto_db,
            params: stated.params,
            audio,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChannelInfo {
    pub id: u32,
    #[serde(default)]
    pub stream: u32,
    pub settings: ChannelSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_recording: Option<AudioRecordingStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseband_recording: Option<RecordingStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_export: Option<NetworkExportStatus>,
}
