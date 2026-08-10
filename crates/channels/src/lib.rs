//! `sdrmm-channels` — the `ChannelRx` plugin surface (PLAN §8). Depends only on `dsp` + `wire`.
//! Phase-1 analog demodulators plus the wave-1 and wave-2 data decoders (PLAN §13): NFM, AM,
//! SSB, WFM mono (+RDS), POCSAG, ADS-B, AIS, APRS/AX.25, RTTY, Morse, NAVTEX, ACARS, sub-GHz. Each mode is one module whose
//! descriptor and constructor sit in the same [`REGISTRY`] row, so the "add channel" UI and
//! `create` dispatch cannot drift apart.

mod acars;
mod adsb;
mod ais;
mod am;
mod aprs;
mod morse;
mod navtex;
mod nfm;
mod pocsag;
mod rds;
mod rtty;
mod ssb;
mod subghz;
mod wfm;

#[cfg(test)]
mod testutil;

/// Reference modulators for the decoder tests, fixtures and end-to-end runs (PLAN §14).
/// Compiled for this crate's own tests, and for downstream crates that opt in with the
/// `test-signals` feature — never in a production build.
#[cfg(any(test, feature = "test-signals"))]
pub mod testgen;

pub use acars::AcarsChannel;
pub use adsb::AdsbChannel;
pub use ais::AisChannelRx;
pub use am::AmChannel;
pub use aprs::AprsChannel;
pub use morse::MorseChannel;
pub use navtex::NavtexChannel;
pub use nfm::NfmChannel;
use num_complex::Complex;
pub use pocsag::PocsagChannel;
pub use rtty::RttyChannel;
use sdrmm_dsp::{Agc, Decimator, FirC};
use sdrmm_wire::{ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, Sideband};
pub use ssb::SsbChannel;
pub use subghz::SubghzChannel;
pub use wfm::WfmChannel;

/// Every channel emits mono PCM at this rate; the engine's audio path is sized against it.
pub const AUDIO_RATE: u32 = 48_000;

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
    }
}

/// Errors raised while constructing or configuring a channel.
#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("unknown channel type: {0}")]
    UnknownType(String),
    #[error("invalid settings: {0}")]
    InvalidSettings(String),
}

/// Construction context passed to a channel: the IQ rate it receives after the DDC. The
/// engine decimates to the descriptor's `input_rate_hz` before construction; channels verify
/// and refuse anything else. Grows as the plugin API matures (PLAN §8).
#[derive(Clone, Copy, Debug)]
pub struct ChannelCtx {
    /// Sample rate of the decimated IQ stream the channel will `process`, in Hz.
    pub input_rate: f64,
}

/// Sink a channel writes into each `process` call: demodulated audio, typed events, and
/// low-rate IQ taps for the analyzer (PLAN §8). Buffers are reused across calls by the host.
#[derive(Default)]
pub struct ChannelOutputs {
    /// Interleaved/mono PCM plus its sample rate, when the channel produced audio this block.
    pub audio_pcm: Vec<f32>,
    pub audio_rate: u32,
    /// Typed decoder frames produced this block (PLAN §5). The host stamps them with time and
    /// frequency; a decoder never formats or serializes on the DSP thread.
    pub events: Vec<DecoderEvent>,
    /// Decimated IQ for scope/constellation panels.
    pub iq_tap: Vec<Complex<f32>>,
}

impl ChannelOutputs {
    /// Clear all buffers without freeing capacity, ready for the next block.
    pub fn reset(&mut self) {
        self.audio_pcm.clear();
        self.audio_rate = 0;
        self.events.clear();
        self.iq_tap.clear();
    }
}

/// A receive channel: consumes decimated IQ, produces audio/events/taps (PLAN §8).
/// `offset_hz` and `squelch_db` in [`ChannelSettings`] are host concerns (DDC tuning and
/// gating happen in the engine); channels read only their mode params.
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

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs);
}

struct Registration {
    descriptor: fn() -> &'static ChannelDescriptor,
    create: fn(ChannelCtx, ChannelSettings) -> Result<Box<dyn ChannelRx>, ChannelError>,
}

fn boxed<C: ChannelRx + 'static>(
    ctx: ChannelCtx,
    settings: ChannelSettings,
) -> Result<Box<dyn ChannelRx>, ChannelError> {
    Ok(Box::new(C::new(ctx, settings)?))
}

/// One row per demod module; both columns come from the same concrete type, so the
/// descriptor list and the `create` dispatch share a single source (PLAN §8).
const REGISTRY: &[Registration] = &[
    Registration {
        descriptor: NfmChannel::descriptor,
        create: boxed::<NfmChannel>,
    },
    Registration {
        descriptor: AmChannel::descriptor,
        create: boxed::<AmChannel>,
    },
    Registration {
        descriptor: SsbChannel::descriptor,
        create: boxed::<SsbChannel>,
    },
    Registration {
        descriptor: WfmChannel::descriptor,
        create: boxed::<WfmChannel>,
    },
    Registration {
        descriptor: PocsagChannel::descriptor,
        create: boxed::<PocsagChannel>,
    },
    Registration {
        descriptor: AdsbChannel::descriptor,
        create: boxed::<AdsbChannel>,
    },
    Registration {
        descriptor: AisChannelRx::descriptor,
        create: boxed::<AisChannelRx>,
    },
    Registration {
        descriptor: AprsChannel::descriptor,
        create: boxed::<AprsChannel>,
    },
    Registration {
        descriptor: RttyChannel::descriptor,
        create: boxed::<RttyChannel>,
    },
    Registration {
        descriptor: MorseChannel::descriptor,
        create: boxed::<MorseChannel>,
    },
    Registration {
        descriptor: NavtexChannel::descriptor,
        create: boxed::<NavtexChannel>,
    },
    Registration {
        descriptor: AcarsChannel::descriptor,
        create: boxed::<AcarsChannel>,
    },
    Registration {
        descriptor: SubghzChannel::descriptor,
        create: boxed::<SubghzChannel>,
    },
];

/// Descriptors for every compiled-in channel type (PLAN §8: static registry).
///
/// `exact_rate_only` is derived here rather than written into each descriptor, from the same two
/// functions the engine's admission check uses: the answer must not be able to disagree with the
/// refusal it predicts.
#[must_use]
pub fn descriptors() -> Vec<ChannelDescriptor> {
    REGISTRY
        .iter()
        .map(|r| {
            let mut descriptor = (r.descriptor)().clone();
            descriptor.exact_rate_only = exact_rate_only(&descriptor);
            descriptor
        })
        .collect()
}

/// Whether this type can only run with the device at exactly its input rate (PLAN §18): a mode
/// occupying its full output rate leaves the DDC no guard band, so no resampled path can carry
/// it. ADS-B is the one such mode today.
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
    let type_id = settings.params.type_id();
    let registration = REGISTRY
        .iter()
        .find(|r| (r.descriptor)().type_id == type_id)
        .ok_or_else(|| ChannelError::UnknownType(type_id.to_owned()))?;
    (registration.create)(ctx, settings.clone())
}

pub(crate) fn check_input_rate(
    ctx: ChannelCtx,
    descriptor: &ChannelDescriptor,
) -> Result<(), ChannelError> {
    // A native-rate channel is handed the device's own samples, so what it checks is a range.
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
        AcarsParams, AdsbParams, AisParams, AmParams, AprsParams, ChannelParams, MorseParams,
        NavtexParams, NfmParams, PocsagParams, RttyParams, SsbParams, SubghzParams, WfmParams,
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
            other => panic!("unexpected type id {other}"),
        }
    }

    #[test]
    fn descriptors_are_unique_and_complete() {
        let all = descriptors();
        assert_eq!(all.len(), 13);
        let ids: HashSet<&str> = all.iter().map(|d| d.type_id.as_str()).collect();
        assert_eq!(
            ids,
            HashSet::from([
                "nfm", "am", "ssb", "wfm", "pocsag", "adsb", "ais", "aprs", "rtty", "morse",
                "navtex", "acars", "subghz",
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
                other => panic!("unexpected type id {other}"),
            };
            assert_eq!(d.bandwidth_hz, bandwidth, "{}", d.type_id);
            assert_eq!(d.input_rate_hz, rate, "{}", d.type_id);
            assert!(!d.name.is_empty(), "{}", d.type_id);
            // Every channel type must be useful for something: audio, decoded frames, or
            // both (WFM+RDS is the only current "both").
            assert!(
                d.has_audio || d.decoder_kind.is_some(),
                "{} produces neither audio nor decoder events",
                d.type_id
            );
            assert_eq!(
                d.has_audio,
                matches!(d.type_id.as_str(), "nfm" | "am" | "ssb" | "wfm"),
                "{} audio flag does not match its mode class",
                d.type_id
            );
        }
    }

    /// The two rate rules the canvas draws (PLAN §18, amended). ADS-B is the one type handed the
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
                bandwidth_hz: 25_000.0
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
