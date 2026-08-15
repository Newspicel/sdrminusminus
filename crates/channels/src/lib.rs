mod acars;
mod adsb;
mod ais;
mod am;
mod aprs;
mod atv;
pub mod audio_chain;
mod dab;
mod datv;
mod drm;
mod dv;
mod gnss;
mod ident;
mod morse;
mod navtex;
mod nfm;
mod pocsag;
mod psk;
mod radio_clock;
mod rds;
mod rtty;
mod selcall;
mod ssb;
mod sstv;
mod subghz;
pub mod tone_squelch;
mod tx;
mod weak_signal;
mod wfm;

#[cfg(test)]
mod testutil;

#[cfg(any(test, feature = "test-signals"))]
pub mod testgen;

pub use acars::AcarsChannel;
pub use adsb::AdsbChannel;
pub use ais::AisChannelRx;
pub use am::{AmChannel, AmTx};
pub use aprs::{AprsChannel, AprsTx, MicE, MicEBit};
pub use atv::AtvChannel;
pub use audio_chain::{AudioChain, ClickProfile};
pub use dab::DabChannel;
pub use datv::DatvChannel;
pub use drm::DrmChannel;
pub use dv::{
    DmrChannel, DpmrChannel, DstarChannel, FreeDvChannel, M17Channel, NxdnChannel, P25Channel,
    YsfChannel,
};
pub use gnss::GnssChannel;
pub use ident::IdentChannel;
pub use morse::MorseChannel;
pub use navtex::NavtexChannel;
pub use nfm::{NfmChannel, NfmTx};
use num_complex::Complex;
pub use pocsag::PocsagChannel;
pub use psk::{Psk31Channel, Psk63Channel};
pub use radio_clock::RadioClockChannel;
pub use rtty::RttyChannel;
use sdrmm_dsp::{Decimator, FirC};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, PositionFix, Sideband,
};
pub use selcall::SelcallChannel;
pub use ssb::{SsbChannel, SsbTx};
pub use sstv::SstvChannel;
pub use subghz::SubghzChannel;
pub use weak_signal::{Ft4Channel, Ft8Channel, WsprChannel};
pub use wfm::WfmChannel;

pub const AUDIO_RATE: u32 = 48_000;

#[must_use]
pub fn audio_channels(params: &ChannelParams) -> u8 {
    match params {
        ChannelParams::Wfm(p) if p.stereo => 2,
        _ => 1,
    }
}

pub(crate) fn clamp_full_scale(pcm: &mut [f32]) {
    for s in pcm {
        *s = s.clamp(-1.0, 1.0);
    }
}

#[must_use]
pub fn occupied_band(params: &ChannelParams) -> (f64, f64) {
    match params {
        ChannelParams::Nfm(p) => (-p.bandwidth_hz / 2.0, p.bandwidth_hz / 2.0),
        ChannelParams::Selcall(_) => selcall::occupied_band(),
        ChannelParams::Am(p) => (-p.bandwidth_hz / 2.0, p.bandwidth_hz / 2.0),
        ChannelParams::Ssb(p) => match p.sideband {
            Sideband::Usb => (ssb::PASSBAND_LOW_HZ, p.bandwidth_hz),
            Sideband::Lsb => (-p.bandwidth_hz, -ssb::PASSBAND_LOW_HZ),
        },
        ChannelParams::Wfm(_) => {
            let half = WfmChannel::descriptor().bandwidth_hz / 2.0;
            (-half, half)
        }
        ChannelParams::Pocsag(p) => pocsag::occupied_band(p),
        ChannelParams::Adsb(_) => adsb::occupied_band(),
        ChannelParams::Ais(p) => ais::occupied_band(p),
        ChannelParams::Aprs(p) => aprs::occupied_band(p),
        ChannelParams::Rtty(p) => rtty::occupied_band(p),
        ChannelParams::Morse(p) => morse::occupied_band(p),
        ChannelParams::Navtex(_) => navtex::occupied_band(),
        ChannelParams::Acars(p) => acars::occupied_band(p),
        ChannelParams::Subghz(p) => subghz::occupied_band(p),
        ChannelParams::Atv(p) => atv::occupied_band(p),
        ChannelParams::Sstv(p) => sstv::occupied_band(p),
        ChannelParams::Dab(_) => dab::occupied_band(),
        ChannelParams::Datv(p) => datv::occupied_band(p),
        ChannelParams::Drm(p) => drm::occupied_band(p),
        ChannelParams::Dmr(_) => dv::dmr::occupied_band(),
        ChannelParams::Dstar(_) => dv::dstar::occupied_band(),
        ChannelParams::Ysf(_) => dv::ysf::occupied_band(),
        ChannelParams::Nxdn(p) => dv::nxdn::occupied_band(p),
        ChannelParams::P25(_) => dv::p25::occupied_band(),
        ChannelParams::Dpmr(_) => dv::dpmr::occupied_band(),
        ChannelParams::M17(_) => dv::m17::occupied_band(),
        ChannelParams::Ft8(_) | ChannelParams::Ft4(_) | ChannelParams::Wspr(_) => {
            weak_signal::occupied_band(params)
        }
        ChannelParams::Psk31(_) | ChannelParams::Psk63(_) => psk::occupied_band(params),
        ChannelParams::Freedv(p) => dv::freedv::occupied_band(p),
        ChannelParams::Ident(p) => ident::occupied_band(p),
        ChannelParams::RadioClock(_) => radio_clock::occupied_band(),
        ChannelParams::Gnss(_) => gnss::occupied_band(),
    }
}

pub enum ChannelFilter {
    Symmetric(Decimator),
    Sideband(FirC),
    Passthrough,
}

impl ChannelFilter {
    pub fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        match self {
            Self::Symmetric(f) => f.process(input, out),
            Self::Sideband(f) => f.process(input, out),
            Self::Passthrough => {
                out.clear();
                out.extend_from_slice(input);
            }
        }
    }
}

pub fn channel_filter(params: &ChannelParams) -> Result<ChannelFilter, ChannelError> {
    match params {
        ChannelParams::Nfm(p) => nfm::channel_filter(p),
        ChannelParams::Selcall(_) => Ok(selcall::channel_filter()),
        ChannelParams::Am(p) => am::channel_filter(p),
        ChannelParams::Ssb(p) => Ok(ChannelFilter::Sideband(ssb::sideband_filter(p)?)),
        ChannelParams::Wfm(_) => Ok(wfm::channel_filter()),
        ChannelParams::Pocsag(p) => pocsag::channel_filter(p),
        ChannelParams::Adsb(_) => Ok(adsb::channel_filter()),
        ChannelParams::Ais(p) => ais::channel_filter(p),
        ChannelParams::Aprs(p) => aprs::channel_filter(p),
        ChannelParams::Rtty(p) => rtty::channel_filter(p),
        ChannelParams::Morse(p) => morse::channel_filter(p),
        ChannelParams::Navtex(_) => Ok(navtex::channel_filter()),
        ChannelParams::Acars(p) => acars::channel_filter(p),
        ChannelParams::Subghz(p) => subghz::channel_filter(p),
        ChannelParams::Atv(p) => atv::channel_filter(p),
        ChannelParams::Sstv(p) => sstv::channel_filter(p),
        ChannelParams::Dab(_) => Ok(dab::channel_filter()),
        ChannelParams::Datv(p) => datv::channel_filter(p),
        ChannelParams::Drm(p) => drm::channel_filter(p),
        ChannelParams::Dmr(_) => Ok(dv::dmr::channel_filter()),
        ChannelParams::Dstar(_) => Ok(dv::dstar::channel_filter()),
        ChannelParams::Ysf(_) => Ok(dv::ysf::channel_filter()),
        ChannelParams::Nxdn(p) => Ok(dv::nxdn::channel_filter(p)),
        ChannelParams::P25(_) => Ok(dv::p25::channel_filter()),
        ChannelParams::Dpmr(_) => Ok(dv::dpmr::channel_filter()),
        ChannelParams::M17(_) => Ok(dv::m17::channel_filter()),
        ChannelParams::Ft8(_) | ChannelParams::Ft4(_) | ChannelParams::Wspr(_) => {
            weak_signal::channel_filter(params)
        }
        ChannelParams::Psk31(_) | ChannelParams::Psk63(_) => psk::channel_filter(params),
        ChannelParams::Freedv(p) => dv::freedv::channel_filter(p),
        ChannelParams::Ident(p) => ident::channel_filter(p),
        ChannelParams::RadioClock(_) => Ok(radio_clock::channel_filter()),
        ChannelParams::Gnss(_) => Ok(gnss::channel_filter()),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("unknown channel type: {0}")]
    UnknownType(String),
    #[error("invalid settings: {0}")]
    InvalidSettings(String),
    #[error("{0} has no modulator")]
    NoTransmitter(String),
    #[error("invalid payload: {0}")]
    InvalidPayload(String),
}

#[derive(Clone, Copy, Debug)]
pub struct ChannelCtx {
    pub input_rate: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VideoPicture {
    pub width: u16,
    pub height: u16,
    pub luma: Vec<u8>,
    pub rgb: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedImage {
    pub source: &'static str,
    pub mode: String,
    pub complete: bool,
    pub lines: u16,
    pub picture: VideoPicture,
}

#[derive(Default)]
pub struct ChannelOutputs {
    pub audio_pcm: Vec<f32>,
    pub audio_rate: u32,
    pub events: Vec<DecoderEvent>,
    pub video: Vec<VideoPicture>,
    pub images: Vec<DecodedImage>,
    pub iq_tap: Vec<Complex<f32>>,
}

impl ChannelOutputs {
    pub fn reset(&mut self) {
        self.audio_pcm.clear();
        self.audio_rate = 0;
        self.events.clear();
        self.video.clear();
        self.images.clear();
        self.iq_tap.clear();
    }
}

pub trait ChannelRx: Send {
    fn descriptor() -> &'static ChannelDescriptor
    where
        Self: Sized;

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError>
    where
        Self: Sized;

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError>;

    fn retuned(&mut self) {}

    fn position_changed(&mut self, _fix: Option<&PositionFix>) {}

    fn needs_gated_input(&self) -> bool {
        true
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs);
}

pub enum TxPayload {
    Audio(Vec<f32>),
    Frame(Vec<u8>),
}

pub trait ChannelTx: Send {
    fn descriptor() -> &'static ChannelDescriptor
    where
        Self: Sized;

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError>
    where
        Self: Sized;

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError>;

    fn submit(&mut self, payload: TxPayload) -> Result<(), ChannelError>;

    fn generate(&mut self, out: &mut [Complex<f32>]) -> usize;
}

type CreateRx = fn(ChannelCtx, ChannelSettings) -> Result<Box<dyn ChannelRx>, ChannelError>;
type CreateTx = fn(ChannelCtx, ChannelSettings) -> Result<Box<dyn ChannelTx>, ChannelError>;

struct Registration {
    descriptor: fn() -> &'static ChannelDescriptor,
    create: CreateRx,
    create_tx: Option<CreateTx>,
}

fn boxed<C: ChannelRx + 'static>(
    ctx: ChannelCtx,
    settings: ChannelSettings,
) -> Result<Box<dyn ChannelRx>, ChannelError> {
    Ok(Box::new(C::new(ctx, settings)?))
}

fn boxed_tx<C: ChannelTx + 'static>(
    ctx: ChannelCtx,
    settings: ChannelSettings,
) -> Result<Box<dyn ChannelTx>, ChannelError> {
    Ok(Box::new(C::new(ctx, settings)?))
}

const REGISTRY: &[Registration] = &[
    Registration {
        descriptor: NfmChannel::descriptor,
        create: boxed::<NfmChannel>,
        create_tx: Some(boxed_tx::<NfmTx>),
    },
    Registration {
        descriptor: SelcallChannel::descriptor,
        create: boxed::<SelcallChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: AmChannel::descriptor,
        create: boxed::<AmChannel>,
        create_tx: Some(boxed_tx::<AmTx>),
    },
    Registration {
        descriptor: SsbChannel::descriptor,
        create: boxed::<SsbChannel>,
        create_tx: Some(boxed_tx::<SsbTx>),
    },
    Registration {
        descriptor: WfmChannel::descriptor,
        create: boxed::<WfmChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: PocsagChannel::descriptor,
        create: boxed::<PocsagChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: AdsbChannel::descriptor,
        create: boxed::<AdsbChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: AisChannelRx::descriptor,
        create: boxed::<AisChannelRx>,
        create_tx: None,
    },
    Registration {
        descriptor: AprsChannel::descriptor,
        create: boxed::<AprsChannel>,
        create_tx: Some(boxed_tx::<AprsTx>),
    },
    Registration {
        descriptor: RttyChannel::descriptor,
        create: boxed::<RttyChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: MorseChannel::descriptor,
        create: boxed::<MorseChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: NavtexChannel::descriptor,
        create: boxed::<NavtexChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: AcarsChannel::descriptor,
        create: boxed::<AcarsChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: SubghzChannel::descriptor,
        create: boxed::<SubghzChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: AtvChannel::descriptor,
        create: boxed::<AtvChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: SstvChannel::descriptor,
        create: boxed::<SstvChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: DabChannel::descriptor,
        create: boxed::<DabChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: DatvChannel::descriptor,
        create: boxed::<DatvChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: DrmChannel::descriptor,
        create: boxed::<DrmChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: DmrChannel::descriptor,
        create: boxed::<DmrChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: DstarChannel::descriptor,
        create: boxed::<DstarChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: YsfChannel::descriptor,
        create: boxed::<YsfChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: NxdnChannel::descriptor,
        create: boxed::<NxdnChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: P25Channel::descriptor,
        create: boxed::<P25Channel>,
        create_tx: None,
    },
    Registration {
        descriptor: DpmrChannel::descriptor,
        create: boxed::<DpmrChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: M17Channel::descriptor,
        create: boxed::<M17Channel>,
        create_tx: None,
    },
    Registration {
        descriptor: FreeDvChannel::descriptor,
        create: boxed::<FreeDvChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: Ft8Channel::descriptor,
        create: boxed::<Ft8Channel>,
        create_tx: None,
    },
    Registration {
        descriptor: Ft4Channel::descriptor,
        create: boxed::<Ft4Channel>,
        create_tx: None,
    },
    Registration {
        descriptor: Psk31Channel::descriptor,
        create: boxed::<Psk31Channel>,
        create_tx: None,
    },
    Registration {
        descriptor: Psk63Channel::descriptor,
        create: boxed::<Psk63Channel>,
        create_tx: None,
    },
    Registration {
        descriptor: WsprChannel::descriptor,
        create: boxed::<WsprChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: IdentChannel::descriptor,
        create: boxed::<IdentChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: RadioClockChannel::descriptor,
        create: boxed::<RadioClockChannel>,
        create_tx: None,
    },
    Registration {
        descriptor: GnssChannel::descriptor,
        create: boxed::<GnssChannel>,
        create_tx: None,
    },
];

#[must_use]
pub fn descriptors() -> Vec<ChannelDescriptor> {
    REGISTRY
        .iter()
        .map(|r| {
            let mut descriptor = (r.descriptor)().clone();
            descriptor.exact_rate_only = exact_rate_only(&descriptor);
            descriptor.can_transmit = r.create_tx.is_some();
            descriptor
        })
        .collect()
}

fn exact_rate_only(descriptor: &ChannelDescriptor) -> bool {
    if descriptor.native_rate_max_hz.is_some() {
        return false;
    }
    let Some(params) = ChannelParams::default_for(&descriptor.type_id) else {
        return false;
    };
    let (low, high) = occupied_band(&params);
    high - low >= sdrmm_dsp::resamplable_bandwidth_hz(descriptor.input_rate_hz)
}

pub fn create(
    ctx: ChannelCtx,
    settings: &ChannelSettings,
) -> Result<Box<dyn ChannelRx>, ChannelError> {
    (find(settings)?.create)(ctx, settings.clone())
}

pub fn create_tx(
    ctx: ChannelCtx,
    settings: &ChannelSettings,
) -> Result<Box<dyn ChannelTx>, ChannelError> {
    let create = find(settings)?
        .create_tx
        .ok_or_else(|| ChannelError::NoTransmitter(settings.params.type_id().to_owned()))?;
    create(ctx, settings.clone())
}

fn find(settings: &ChannelSettings) -> Result<&'static Registration, ChannelError> {
    let type_id = settings.params.type_id();
    REGISTRY
        .iter()
        .find(|r| (r.descriptor)().type_id == type_id)
        .ok_or_else(|| ChannelError::UnknownType(type_id.to_owned()))
}

pub(crate) fn check_input_rate(
    ctx: ChannelCtx,
    descriptor: &ChannelDescriptor,
) -> Result<(), ChannelError> {
    if let Some((low, high)) = descriptor.native_rate_range() {
        return if (low..=high).contains(&ctx.input_rate) {
            Ok(())
        } else {
            Err(ChannelError::InvalidSettings(format!(
                "{} runs at {low}–{high} Hz, engine supplied {} Hz",
                descriptor.type_id, ctx.input_rate
            )))
        };
    }
    if ctx.input_rate == descriptor.input_rate_hz {
        Ok(())
    } else {
        Err(ChannelError::InvalidSettings(format!(
            "{} expects {} Hz input, engine supplied {} Hz",
            descriptor.type_id, descriptor.input_rate_hz, ctx.input_rate
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use sdrmm_wire::{
        AcarsParams, AdsbParams, AisParams, AmParams, AprsParams, AtvColor, AtvParams,
        ChannelParams, DabParams, DatvParams, DmrParams, DpmrParams, DrmParams, DstarParams,
        FreeDvParams, GnssParams, IdentParams, M17Params, MorseParams, NavtexParams, NfmParams,
        NxdnParams, P25Params, PocsagParams, PskParams, RadioClockParams, RttyParams,
        SelcallParams, SsbParams, SstvParams, SubghzParams, WfmParams, WsjtParams, WsprParams,
        YsfParams,
    };

    use super::*;
    use crate::testutil::settings;

    fn default_params(type_id: &str) -> ChannelParams {
        match type_id {
            "nfm" => ChannelParams::Nfm(NfmParams::default()),
            "selcall" => ChannelParams::Selcall(SelcallParams::default()),
            "am" => ChannelParams::Am(AmParams::default()),
            "ssb" => ChannelParams::Ssb(SsbParams::default()),
            "wfm" => ChannelParams::Wfm(WfmParams::default()),
            "pocsag" => ChannelParams::Pocsag(PocsagParams::default()),
            "adsb" => ChannelParams::Adsb(AdsbParams::default()),
            "ais" => ChannelParams::Ais(AisParams::default()),
            "aprs" => ChannelParams::Aprs(AprsParams::default()),
            "rtty" => ChannelParams::Rtty(RttyParams::default()),
            "morse" => ChannelParams::Morse(MorseParams::default()),
            "navtex" => ChannelParams::Navtex(NavtexParams::default()),
            "acars" => ChannelParams::Acars(AcarsParams::default()),
            "subghz" => ChannelParams::Subghz(SubghzParams::default()),
            "atv" => ChannelParams::Atv(AtvParams::default()),
            "sstv" => ChannelParams::Sstv(SstvParams::default()),
            "dab" => ChannelParams::Dab(DabParams::default()),
            "datv" => ChannelParams::Datv(DatvParams::default()),
            "drm" => ChannelParams::Drm(DrmParams::default()),
            "dmr" => ChannelParams::Dmr(DmrParams::default()),
            "dstar" => ChannelParams::Dstar(DstarParams::default()),
            "ysf" => ChannelParams::Ysf(YsfParams::default()),
            "nxdn" => ChannelParams::Nxdn(NxdnParams::default()),
            "p25" => ChannelParams::P25(P25Params::default()),
            "dpmr" => ChannelParams::Dpmr(DpmrParams::default()),
            "m17" => ChannelParams::M17(M17Params::default()),
            "ft8" => ChannelParams::Ft8(WsjtParams::default()),
            "ft4" => ChannelParams::Ft4(WsjtParams::default()),
            "psk31" => ChannelParams::Psk31(PskParams::default()),
            "psk63" => ChannelParams::Psk63(PskParams::default()),
            "wspr" => ChannelParams::Wspr(WsprParams::default()),
            "freedv" => ChannelParams::Freedv(FreeDvParams::default()),
            "ident" => ChannelParams::Ident(IdentParams::default()),
            "radio_clock" => ChannelParams::RadioClock(RadioClockParams::default()),
            "gnss" => ChannelParams::Gnss(GnssParams::default()),
            other => panic!("unexpected type id {other}"),
        }
    }

    #[test]
    fn descriptors_are_unique_and_complete() {
        let all = descriptors();
        assert_eq!(all.len(), 35);
        let ids: HashSet<&str> = all.iter().map(|d| d.type_id.as_str()).collect();
        assert_eq!(
            ids,
            HashSet::from([
                "nfm",
                "selcall",
                "am",
                "ssb",
                "wfm",
                "pocsag",
                "adsb",
                "ais",
                "aprs",
                "rtty",
                "morse",
                "navtex",
                "acars",
                "subghz",
                "atv",
                "sstv",
                "dab",
                "datv",
                "drm",
                "dmr",
                "dstar",
                "ysf",
                "nxdn",
                "p25",
                "dpmr",
                "m17",
                "freedv",
                "ft8",
                "ft4",
                "psk31",
                "psk63",
                "wspr",
                "ident",
                "radio_clock",
                "gnss",
            ])
        );
        for d in &all {
            let (bandwidth, rate) = match d.type_id.as_str() {
                "nfm" => (12_500.0, 48_000.0),
                "selcall" => (12_500.0, 48_000.0),
                "am" => (10_000.0, 48_000.0),
                "ssb" => (3_000.0, 48_000.0),
                "wfm" => (200_000.0, 240_000.0),
                "pocsag" => (12_500.0, 48_000.0),
                "adsb" => (2_000_000.0, 2_000_000.0),
                "ais" => (25_000.0, 48_000.0),
                "aprs" => (12_500.0, 48_000.0),
                "rtty" => (1_000.0, 8_000.0),
                "morse" => (400.0, 8_000.0),
                "navtex" => (600.0, 8_000.0),
                "acars" => (12_500.0, 48_000.0),
                "subghz" => (150_000.0, 250_000.0),
                "atv" => (1_500_000.0, 2_000_000.0),
                "sstv" => (1_600.0, 16_000.0),
                "dab" => (1_536_000.0, 2_048_000.0),
                "datv" => (1_500_000.0, 2_000_000.0),
                "drm" => (100_000.0, 192_000.0),
                "dmr" | "ysf" | "p25" => (12_500.0, 48_000.0),
                "dstar" | "nxdn" | "dpmr" => (6_250.0, 48_000.0),
                "m17" => (9_000.0, 48_000.0),
                "ft8" | "ft4" | "wspr" => (3_200.0, 12_000.0),
                "psk31" => (80.0, 8_000.0),
                "psk63" => (160.0, 8_000.0),
                "freedv" => (1_400.0, 8_000.0),
                "ident" => (192_000.0, 240_000.0),
                "radio_clock" => (200.0, 2_000.0),
                "gnss" => (2_046_000.0, 2_048_000.0),
                other => panic!("unexpected type id {other}"),
            };
            assert_eq!(d.bandwidth_hz, bandwidth, "{}", d.type_id);
            assert_eq!(d.input_rate_hz, rate, "{}", d.type_id);
            assert!(!d.name.is_empty(), "{}", d.type_id);
            assert!(
                d.has_audio || d.decoder_kind.is_some() || d.has_video,
                "{} produces neither audio, decoder events nor video",
                d.type_id
            );
            assert_eq!(
                d.has_audio,
                matches!(
                    d.type_id.as_str(),
                    "nfm"
                        | "am"
                        | "ssb"
                        | "wfm"
                        | "atv"
                        | "dmr"
                        | "dstar"
                        | "ysf"
                        | "nxdn"
                        | "p25"
                        | "dpmr"
                        | "m17"
                        | "freedv"
                ),
                "{} audio flag does not match its mode class",
                d.type_id
            );
            assert_eq!(
                d.has_video,
                matches!(d.type_id.as_str(), "atv" | "sstv"),
                "{} video flag does not match its mode class",
                d.type_id
            );
        }
    }

    #[test]
    fn audio_channels_matches_the_frames_each_mode_produces() {
        const LEN: usize = 96_000;
        for d in descriptors().into_iter().filter(|d| d.has_audio) {
            let params = default_params(&d.type_id);
            let channels = usize::from(audio_channels(&params));
            let ctx = ChannelCtx {
                input_rate: d.input_rate_hz,
            };
            let mut chan = create(ctx, &settings(params)).expect("builds");
            let audio = crate::testutil::run_ragged(
                chan.as_mut(),
                &crate::testutil::complex_noise(7, 0.5, LEN),
            );
            assert_eq!(
                audio.len() % channels,
                0,
                "{} emitted a partial sample frame",
                d.type_id
            );
            if d.decoder_kind.as_deref() == Some("dv") {
                continue;
            }
            if d.type_id == "atv" && audio.is_empty() {
                continue;
            }
            let frames = audio.len() / channels;
            let expected = (LEN as f64 * f64::from(AUDIO_RATE) / d.input_rate_hz) as usize;
            assert!(
                frames <= expected && expected - frames < 200,
                "{} produced {frames} frames of {channels}-channel audio, expected ~{expected}",
                d.type_id
            );
        }
    }

    #[test]
    fn native_rate_modes_match_their_required_device_rates() {
        for d in descriptors() {
            let expected = match d.type_id.as_str() {
                "adsb" => Some((2_000_000.0, 4_000_000.0)),
                "atv" => Some((2_000_000.0, 20_000_000.0)),
                "gnss" => Some((2_048_000.0, 2_048_000.0)),
                _ => None,
            };
            assert_eq!(
                d.native_rate_range(),
                expected,
                "{} native rate range",
                d.type_id
            );
            assert!(
                !(d.exact_rate_only && d.native_rate_max_hz.is_some()),
                "{} claims both rate rules",
                d.type_id
            );
            assert!(!d.exact_rate_only, "{} exact-rate flag", d.type_id);
        }
    }

    #[test]
    fn create_builds_every_registered_type() {
        for d in descriptors() {
            let ctx = ChannelCtx {
                input_rate: d.input_rate_hz,
            };
            let built = create(ctx, &settings(default_params(&d.type_id)));
            assert!(built.is_ok(), "{}: {:?}", d.type_id, built.err());
        }
    }

    #[test]
    fn can_transmit_matches_what_create_tx_will_build() {
        for d in descriptors() {
            let ctx = ChannelCtx {
                input_rate: d.input_rate_hz,
            };
            let built = create_tx(ctx, &settings(default_params(&d.type_id)));
            assert_eq!(
                built.is_ok(),
                d.can_transmit,
                "{}: can_transmit {} but create_tx said {:?}",
                d.type_id,
                d.can_transmit,
                built.err()
            );
            if !d.can_transmit {
                assert!(
                    matches!(built, Err(ChannelError::NoTransmitter(_))),
                    "{} must refuse by naming the missing modulator",
                    d.type_id
                );
            }
        }
    }

    #[test]
    fn only_the_modes_with_a_modulator_transmit() {
        let transmitting: Vec<String> = descriptors()
            .into_iter()
            .filter(|d| d.can_transmit)
            .map(|d| d.type_id)
            .collect();
        assert_eq!(transmitting, ["nfm", "am", "ssb", "aprs"]);
    }

    #[test]
    fn create_rejects_mismatched_input_rate() {
        let ctx = ChannelCtx {
            input_rate: 96_000.0,
        };
        let err = create(ctx, &settings(ChannelParams::Nfm(NfmParams::default())));
        assert!(matches!(err, Err(ChannelError::InvalidSettings(_))));
    }

    #[test]
    fn occupied_band_tracks_params_and_sideband() {
        assert_eq!(
            occupied_band(&ChannelParams::Nfm(NfmParams {
                bandwidth_hz: 25_000.0,
                ..NfmParams::default()
            })),
            (-12_500.0, 12_500.0)
        );
        assert_eq!(
            occupied_band(&ChannelParams::Am(AmParams {
                bandwidth_hz: 8_000.0,
            })),
            (-4_000.0, 4_000.0)
        );
        assert_eq!(
            occupied_band(&ChannelParams::Ssb(SsbParams {
                sideband: Sideband::Usb,
                bandwidth_hz: 10_000.0,
            })),
            (100.0, 10_000.0)
        );
        assert_eq!(
            occupied_band(&ChannelParams::Ssb(SsbParams {
                sideband: Sideband::Lsb,
                bandwidth_hz: 10_000.0,
            })),
            (-10_000.0, -100.0)
        );
        assert_eq!(
            occupied_band(&ChannelParams::Wfm(WfmParams::default())),
            (-100_000.0, 100_000.0)
        );
        assert_eq!(
            occupied_band(&ChannelParams::Atv(AtvParams {
                color: AtvColor::Pal,
                sound_subcarrier_hz: Some(5_500_000.0),
                ..AtvParams::default()
            })),
            (-5_033_618.75, 5_565_000.0)
        );
    }

    fn filter_rms(filter: &mut ChannelFilter, freq_norm: f64) -> f32 {
        let mut out = Vec::new();
        filter.process(&crate::testutil::complex_tone(freq_norm, 8_192), &mut out);
        let settled = &out[512..];
        (settled.iter().map(|v| f64::from(v.norm_sqr())).sum::<f64>() / settled.len() as f64).sqrt()
            as f32
    }

    #[test]
    fn channel_filter_passes_in_channel_and_rejects_adjacent() {
        let mut f = channel_filter(&ChannelParams::Nfm(NfmParams::default())).unwrap();
        let pass = filter_rms(&mut f, 1_000.0 / 48_000.0);
        assert!((0.9..1.05).contains(&pass), "in-channel rms {pass}");
        let mut f = channel_filter(&ChannelParams::Nfm(NfmParams::default())).unwrap();
        let reject = filter_rms(&mut f, 15_000.0 / 48_000.0);
        assert!(reject < 0.01, "adjacent leak rms {reject}");

        let ssb = ChannelParams::Ssb(SsbParams::default());
        let mut f = channel_filter(&ssb).unwrap();
        let pass = filter_rms(&mut f, 1_000.0 / 48_000.0);
        assert!((0.9..1.05).contains(&pass), "usb rms {pass}");
        let mut f = channel_filter(&ssb).unwrap();
        let reject = filter_rms(&mut f, -1_000.0 / 48_000.0);
        assert!(reject < 0.01, "lsb leak rms {reject}");
    }

    #[test]
    fn channel_filter_rejects_out_of_range_bandwidth() {
        for params in [
            ChannelParams::Nfm(NfmParams {
                bandwidth_hz: f64::NAN,
                ..NfmParams::default()
            }),
            ChannelParams::Am(AmParams { bandwidth_hz: 0.0 }),
            ChannelParams::Ssb(SsbParams {
                sideband: Sideband::Usb,
                bandwidth_hz: 50.0,
            }),
        ] {
            assert!(
                matches!(
                    channel_filter(&params),
                    Err(ChannelError::InvalidSettings(_))
                ),
                "{} must be rejected",
                params.type_id()
            );
        }
    }
}
