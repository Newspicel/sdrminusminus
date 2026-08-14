mod acars;
mod adsb;
mod ais;
mod am;
mod aprs;
mod atv;
mod dv;
mod morse;
mod navtex;
mod nfm;
mod pocsag;
mod rds;
mod rtty;
mod ssb;
mod subghz;
pub mod tone_squelch;
mod tx;
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
pub use dv::{
    DmrChannel, DpmrChannel, DstarChannel, M17Channel, NxdnChannel, P25Channel, YsfChannel,
};
pub use morse::MorseChannel;
pub use navtex::NavtexChannel;
pub use nfm::{NfmChannel, NfmTx};
use num_complex::Complex;
pub use pocsag::PocsagChannel;
pub use rtty::RttyChannel;
use sdrmm_dsp::{Agc, Decimator, FirC};
use sdrmm_wire::{ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, Sideband};
pub use ssb::{SsbChannel, SsbTx};
pub use subghz::SubghzChannel;
pub use wfm::WfmChannel;

/// Every channel emits PCM at this rate; the engine's audio path is sized against it.
pub const AUDIO_RATE: u32 = 48_000;

/// How many interleaved channels this mode's audio comes out as — WFM in stereo is the one
/// two-channel mode. The engine builds its Opus encoder from this rather than from what a
/// block happens to contain, so a squelched channel emits silence in the right layout; a mode
/// whose `process` disagreed with it would interleave garbage.
#[must_use]
pub fn audio_channels(params: &ChannelParams) -> u8 {
    match params {
        ChannelParams::Wfm(p) if p.stereo => 2,
        _ => 1,
    }
}

/// Opus integer-API decoders (and the browser DAC) hard-clip PCM beyond ±1.0, so every demod
/// bounds its final output here — overshoot (open-squelch FM noise, over-deviated carriers)
/// must never leave the channel as out-of-range samples.
pub(crate) fn clamp_full_scale(pcm: &mut [f32]) {
    for s in pcm {
        *s = s.clamp(-1.0, 1.0);
    }
}

/// The RF band a channel occupies relative to its offset, in Hz (`(low, high)`): symmetric
/// at the configured bandwidth for NFM/AM, at the descriptor nominal for WFM; SSB occupies
/// one sideband only. Drives the engine's passband-fit validation, so it must track what
/// [`channel_filter`] actually selects.
#[must_use]
pub fn occupied_band(params: &ChannelParams) -> (f64, f64) {
    match params {
        ChannelParams::Nfm(p) => (-p.bandwidth_hz / 2.0, p.bandwidth_hz / 2.0),
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
        ChannelParams::Dmr(_) => dv::dmr::occupied_band(),
        ChannelParams::Dstar(_) => dv::dstar::occupied_band(),
        ChannelParams::Ysf(_) => dv::ysf::occupied_band(),
        ChannelParams::Nxdn(p) => dv::nxdn::occupied_band(p),
        ChannelParams::P25(_) => dv::p25::occupied_band(),
        ChannelParams::Dpmr(_) => dv::dpmr::occupied_band(),
        ChannelParams::M17(_) => dv::m17::occupied_band(),
    }
}

/// Channel-selection filter the engine host runs on the DDC output ahead of squelch and
/// demod, so the gate measures in-channel power and adjacent-channel energy never reaches
/// the detector. Symmetric modes use a real-tap FIR (half the MACs of a complex one); SSB
/// needs the one-sided [`FirC`] mirroring its demod passband.
pub enum ChannelFilter {
    Symmetric(Decimator),
    Sideband(FirC),
    /// No extra selectivity: the DDC's anti-alias response already bounds the band, and the
    /// mode needs the full rise time (ADS-B's 0.5 µs pulses).
    Passthrough,
}

impl ChannelFilter {
    /// Replaces `out` with one filtered sample per input sample.
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

/// Build the channel filter for `params` at the mode's descriptor input rate, applying the
/// same bandwidth bounds the demod constructors enforce.
pub fn channel_filter(params: &ChannelParams) -> Result<ChannelFilter, ChannelError> {
    match params {
        ChannelParams::Nfm(p) => nfm::channel_filter(p),
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
        ChannelParams::Dmr(_) => Ok(dv::dmr::channel_filter()),
        ChannelParams::Dstar(_) => Ok(dv::dstar::channel_filter()),
        ChannelParams::Ysf(_) => Ok(dv::ysf::channel_filter()),
        ChannelParams::Nxdn(p) => Ok(dv::nxdn::channel_filter(p)),
        ChannelParams::P25(_) => Ok(dv::p25::channel_filter()),
        ChannelParams::Dpmr(_) => Ok(dv::dpmr::channel_filter()),
        ChannelParams::M17(_) => Ok(dv::m17::channel_filter()),
    }
}

/// Errors raised while constructing or configuring a channel.
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
    /// Sample rate of the channel's IQ stream, in Hz.
    pub input_rate: f64,
}

/// One picture a video channel scanned out: 8-bit luma, row-major from the top line, exactly
/// `width · height` bytes. Grayscale because that is what an analog raster carries once the
/// colour subcarrier is left alone (: ATV decodes luma).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VideoPicture {
    pub width: u16,
    pub height: u16,
    pub luma: Vec<u8>,
}

#[derive(Default)]
pub struct ChannelOutputs {
    /// PCM plus its sample rate, when the channel produced audio this block. Interleaved at
    /// [`audio_channels`] for the channel's params — a whole number of sample frames.
    pub audio_pcm: Vec<f32>,
    pub audio_rate: u32,
    /// Typed decoder frames produced this block. The host stamps them with time and
    /// frequency; a decoder never formats or serializes on the DSP thread.
    pub events: Vec<DecoderEvent>,
    /// Pictures completed this block. A `Vec` rather than one slot because a block is sized by
    /// the device's USB transfers, not by the raster: nothing stops one from spanning two
    /// fields, and the second picture must not be the one that silently vanishes.
    pub video: Vec<VideoPicture>,
    /// Decimated IQ for scope/constellation panels.
    pub iq_tap: Vec<Complex<f32>>,
}

impl ChannelOutputs {
    /// Clear all buffers without freeing capacity, ready for the next block.
    pub fn reset(&mut self) {
        self.audio_pcm.clear();
        self.audio_rate = 0;
        self.events.clear();
        self.video.clear();
        self.iq_tap.clear();
    }
}

pub trait ChannelRx: Send {
    /// Static description that drives the "add channel" UI. Object-safe callers use the
    /// registry; this associated fn is for the concrete type.
    fn descriptor() -> &'static ChannelDescriptor
    where
        Self: Sized;

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError>
    where
        Self: Sized;

    /// Reconfigure mode params in place. A params variant of another channel type is an
    /// [`ChannelError::InvalidSettings`] — the engine rebuilds the pipeline on type change.
    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError>;

    /// The host moved the channel to a different frequency. A demod carries no state tied to
    /// a particular station, so it ignores this; a decoder does — whatever it has accreted
    /// describes the signal it just left and must not follow the channel to the next one.
    /// Retunes arrive here rather than through [`ChannelRx::apply`], which the host does not
    /// call for an offset-only change.
    fn retuned(&mut self) {}

    /// Whether the host must keep feeding this channel while the squelch is closed.
    ///
    /// A decoder measures time in the samples it has processed — its bit clock, its element
    /// timing, its inter-frame gaps — so skipping the gated span would splice those spans out
    /// of a stream that never had a gap in it. `true`, the default, is right for every one of
    /// them, and the host reads it only for the types that emit events at all.
    ///
    /// A channel whose events describe something *inside* a carrier says otherwise when it has
    /// nothing to describe: there is no subaudible tone under a closed squelch, and running
    /// the demodulator on silence to find that out is what the squelch exists to avoid.
    fn needs_gated_input(&self) -> bool {
        true
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs);
}

/// What a transmit channel was handed to send. The mirror of [`ChannelParams`]: one variant per
/// class of thing a mode carries, and a channel refuses the variants it does not speak.
///
/// Frames are bytes rather than text because the framing — addresses, stuffing, checksums — is
/// the protocol module's job, shared with the decoder that parses it back.
pub enum TxPayload {
    /// Mono PCM at [`AUDIO_RATE`] in ±1.0, for the analog modes. Out-of-range samples are
    /// clamped rather than allowed to over-deviate the carrier.
    Audio(Vec<f32>),
    /// A finished protocol frame, for the data modes.
    Frame(Vec<u8>),
}

pub trait ChannelTx: Send {
    /// The same descriptor the receive side publishes — one type id, one set of rates, whichever
    /// direction is being built.
    fn descriptor() -> &'static ChannelDescriptor
    where
        Self: Sized;

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError>
    where
        Self: Sized;

    /// Reconfigure mode params in place, as [`ChannelRx::apply`] does.
    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError>;

    fn submit(&mut self, payload: TxPayload) -> Result<(), ChannelError>;

    /// Hot path: fill `out` with the next samples of the burst, returning how many were written
    /// to its head. No allocation, no locks — the same contract [`ChannelRx::process`] runs
    /// under.
    ///
    /// A short fill ends the burst: the modulator wrote its last sample and has ramped the
    /// carrier down. `0` means there is nothing queued, so the host may drop the transmitter.
    /// Submitting again starts a new burst rather than resuming this one.
    fn generate(&mut self, out: &mut [Complex<f32>]) -> usize;
}

type CreateRx = fn(ChannelCtx, ChannelSettings) -> Result<Box<dyn ChannelRx>, ChannelError>;
type CreateTx = fn(ChannelCtx, ChannelSettings) -> Result<Box<dyn ChannelTx>, ChannelError>;

struct Registration {
    descriptor: fn() -> &'static ChannelDescriptor,
    create: CreateRx,
    /// Populated for the modes that ship a modulator too; `None` is a receive-only type, and is
    /// what [`ChannelDescriptor::can_transmit`] is derived from so the two cannot disagree.
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
];

/// Descriptors for every compiled-in channel type (: static registry).
///
/// `exact_rate_only` and `can_transmit` are derived here rather than written into each
/// descriptor, from the same registry columns the dispatch below reads: neither answer must be
/// able to disagree with the refusal it predicts.
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
    // A native-rate type never meets a resampling DDC at all: it is handed the device's own
    // samples, so the guard band this asks about is not in its path.
    if descriptor.native_rate_max_hz.is_some() {
        return false;
    }
    let Some(params) = ChannelParams::default_for(&descriptor.type_id) else {
        return false;
    };
    let (low, high) = occupied_band(&params);
    high - low >= sdrmm_dsp::resamplable_bandwidth_hz(descriptor.input_rate_hz)
}

/// Build the channel matching `settings.params`.
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

/// Shared audio AGC: fast attack so voice peaks never blast, slow release so syllable gaps
/// don't pump.
pub(crate) fn audio_agc() -> Agc {
    Agc::new(f64::from(AUDIO_RATE), 0.25, 0.005, 0.2, 100.0)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use sdrmm_wire::{
        AcarsParams, AdsbParams, AisParams, AmParams, AprsParams, AtvParams, ChannelParams,
        DmrParams, DpmrParams, DstarParams, M17Params, MorseParams, NavtexParams, NfmParams,
        NxdnParams, P25Params, PocsagParams, RttyParams, SsbParams, SubghzParams, WfmParams,
        YsfParams,
    };

    use super::*;
    use crate::testutil::settings;

    fn default_params(type_id: &str) -> ChannelParams {
        match type_id {
            "nfm" => ChannelParams::Nfm(NfmParams::default()),
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
            "dmr" => ChannelParams::Dmr(DmrParams::default()),
            "dstar" => ChannelParams::Dstar(DstarParams::default()),
            "ysf" => ChannelParams::Ysf(YsfParams::default()),
            "nxdn" => ChannelParams::Nxdn(NxdnParams::default()),
            "p25" => ChannelParams::P25(P25Params::default()),
            "dpmr" => ChannelParams::Dpmr(DpmrParams::default()),
            "m17" => ChannelParams::M17(M17Params::default()),
            other => panic!("unexpected type id {other}"),
        }
    }

    #[test]
    fn descriptors_are_unique_and_complete() {
        let all = descriptors();
        assert_eq!(all.len(), 21);
        let ids: HashSet<&str> = all.iter().map(|d| d.type_id.as_str()).collect();
        assert_eq!(
            ids,
            HashSet::from([
                "nfm", "am", "ssb", "wfm", "pocsag", "adsb", "ais", "aprs", "rtty", "morse",
                "navtex", "acars", "subghz", "atv", "dmr", "dstar", "ysf", "nxdn", "p25", "dpmr",
                "m17",
            ])
        );
        for d in &all {
            let (bandwidth, rate) = match d.type_id.as_str() {
                "nfm" => (12_500.0, 48_000.0),
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
                "dmr" | "ysf" | "p25" => (12_500.0, 48_000.0),
                "dstar" | "nxdn" | "dpmr" => (6_250.0, 48_000.0),
                "m17" => (9_000.0, 48_000.0),
                other => panic!("unexpected type id {other}"),
            };
            assert_eq!(d.bandwidth_hz, bandwidth, "{}", d.type_id);
            assert_eq!(d.input_rate_hz, rate, "{}", d.type_id);
            assert!(!d.name.is_empty(), "{}", d.type_id);
            // Every channel type must be useful for something: audio, decoded frames, or a
            // picture. Several do more than one — WFM decodes RDS beside its audio, NFM the
            // subaudible tone under it.
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
                        | "dmr"
                        | "dstar"
                        | "ysf"
                        | "nxdn"
                        | "p25"
                        | "dpmr"
                        | "m17"
                ),
                "{} audio flag does not match its mode class",
                d.type_id
            );
            assert_eq!(
                d.has_video,
                d.type_id == "atv",
                "{} video flag does not match its mode class",
                d.type_id
            );
        }
    }

    /// [`audio_channels`] is what the engine sizes its Opus encoder and its squelched
    /// zero-fill from, so it has to be what the demodulator actually writes: a mode whose
    /// interleave disagreed would show up here as half — or double — the frames its rate
    /// implies, and would reach a listener as chipmunk audio, not as a wrong channel count.
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
            // Digital voice is burst gated: idle/noise deliberately produces no PCM. Each
            // mode's encoded-waveform test checks decoded duration at its own frame cadence.
            if d.decoder_kind.as_deref() == Some("dv") {
                continue;
            }
            let frames = audio.len() / channels;
            // Filter warm-up is the only shortfall allowed: no mode's group delay reaches
            // 200 output frames at these tap counts.
            let expected = (LEN as f64 * f64::from(AUDIO_RATE) / d.input_rate_hz) as usize;
            assert!(
                frames <= expected && expected - frames < 200,
                "{} produced {frames} frames of {channels}-channel audio, expected ~{expected}",
                d.type_id
            );
        }
    }

    /// The two rate rules the canvas draws. ADS-B is the one type handed the
    /// device's own samples, and *because* it is, no type is exact-rate any more: the flag and
    /// the range are mutually exclusive, and a type claiming both would leave the canvas telling
    /// the operator to set a rate the engine then refuses.
    #[test]
    fn only_adsb_runs_at_the_device_rate_and_nothing_is_exact_rate() {
        for d in descriptors() {
            assert_eq!(
                d.native_rate_range(),
                (d.type_id == "adsb").then_some((2_000_000.0, 4_000_000.0)),
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

    /// The flag the canvas would draw a transmit port from must be the same fact `create_tx`
    /// acts on — a type advertising a modulator it cannot build, or hiding one it can, is the
    /// drift the derived column exists to prevent.
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

    /// : the modes with a modulator so far — the analog voice trio and AX.25. This is a
    /// reminder to extend the list deliberately, not a cap.
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
                agc: false
            })),
            (-4_000.0, 4_000.0)
        );
        assert_eq!(
            occupied_band(&ChannelParams::Ssb(SsbParams {
                sideband: Sideband::Usb,
                bandwidth_hz: 10_000.0,
                agc: false
            })),
            (100.0, 10_000.0)
        );
        assert_eq!(
            occupied_band(&ChannelParams::Ssb(SsbParams {
                sideband: Sideband::Lsb,
                bandwidth_hz: 10_000.0,
                agc: false
            })),
            (-10_000.0, -100.0)
        );
        assert_eq!(
            occupied_band(&ChannelParams::Wfm(WfmParams::default())),
            (-100_000.0, 100_000.0)
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
        // NFM at the default 12.5 kHz: a 15 kHz tone sits inside the DDC's flat ±19.2 kHz
        // passband but outside the channel — it must be gone, not merely damped.
        let mut f = channel_filter(&ChannelParams::Nfm(NfmParams::default())).unwrap();
        let pass = filter_rms(&mut f, 1_000.0 / 48_000.0);
        assert!((0.9..1.05).contains(&pass), "in-channel rms {pass}");
        let mut f = channel_filter(&ChannelParams::Nfm(NfmParams::default())).unwrap();
        let reject = filter_rms(&mut f, 15_000.0 / 48_000.0);
        assert!(reject < 0.01, "adjacent leak rms {reject}");

        // SSB keeps its one-sided selection: the opposite sideband is rejected.
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
            ChannelParams::Am(AmParams {
                bandwidth_hz: 0.0,
                agc: false,
            }),
            ChannelParams::Ssb(SsbParams {
                sideband: Sideband::Usb,
                bandwidth_hz: 50.0,
                agc: false,
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
