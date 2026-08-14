//! Composition: one [`Channel`] applying any subset of the impairment axes in a canonical
//! order, built from a [`ChannelSpec`]. The sweep and limits runners hold a spec, set exactly
//! one axis (or a named composite profile), and hand the built channel a clean modulator
//! output — so "the CFO limits row" and "the CFO impairment" cannot mean different things.
use num_complex::Complex;

use super::{
    Awgn, BurstModel, Cfo, Clipping, ClockError, DcOffset, Drift, Impairment, Interferer,
    IqImbalance, Multipath, PhaseNoise, Quantiser, TimingJitter, TimingOffset,
};
use crate::ber::rng::Rng;

/// The axes a composed channel may carry; `None` axes are identity. Fields are public so a
/// limits runner can introspect what a profile contains; the builder methods exist so setting
/// one axis reads as one line at the call site.
#[derive(Clone, Copy, Debug, Default)]
pub struct ChannelSpec {
    pub burst: Option<BurstModel>,
    pub multipath: Option<Multipath>,
    pub cfo: Option<Cfo>,
    pub drift: Option<Drift>,
    pub phase_noise: Option<PhaseNoise>,
    pub clock: Option<ClockError>,
    pub timing_offset: Option<TimingOffset>,
    pub timing_jitter: Option<TimingJitter>,
    pub iq_imbalance: Option<IqImbalance>,
    pub dc_offset: Option<DcOffset>,
    pub cochannel: Option<Interferer>,
    pub adjacent: Option<Interferer>,
    pub clipping: Option<Clipping>,
    pub quantiser: Option<Quantiser>,
    pub awgn: Option<Awgn>,
}

macro_rules! setter {
    ($(#[$doc:meta])* $name:ident: $ty:ty) => {
        $(#[$doc])*
        #[must_use]
        pub fn $name(mut self, value: $ty) -> Self {
            self.$name = Some(value);
            self
        }
    };
}

impl ChannelSpec {
    setter!(burst: BurstModel);
    setter!(multipath: Multipath);
    setter!(cfo: Cfo);
    setter!(drift: Drift);
    setter!(phase_noise: PhaseNoise);
    setter!(clock: ClockError);
    setter!(timing_offset: TimingOffset);
    setter!(timing_jitter: TimingJitter);
    setter!(iq_imbalance: IqImbalance);
    setter!(dc_offset: DcOffset);
    setter!(cochannel: Interferer);
    setter!(adjacent: Interferer);
    setter!(clipping: Clipping);
    setter!(quantiser: Quantiser);
    setter!(awgn: Awgn);

    #[must_use]
    pub fn build(self) -> Channel {
        Channel { spec: self }
    }
}

/// A built channel: applies its spec's axes in the canonical order documented at module
/// level. It is itself an [`Impairment`], so composites nest wherever a single axis fits.
#[derive(Clone, Copy, Debug)]
pub struct Channel {
    spec: ChannelSpec,
}

impl Channel {
    #[must_use]
    pub fn spec(&self) -> &ChannelSpec {
        &self.spec
    }
}

impl Impairment for Channel {
    fn apply(&self, x: &mut Vec<Complex<f32>>, rng: &mut Rng) {
        let s = &self.spec;
        let stages: [Option<&dyn Impairment>; 15] = [
            s.burst.as_ref().map(|i| i as &dyn Impairment),
            s.multipath.as_ref().map(|i| i as &dyn Impairment),
            s.cfo.as_ref().map(|i| i as &dyn Impairment),
            s.drift.as_ref().map(|i| i as &dyn Impairment),
            s.phase_noise.as_ref().map(|i| i as &dyn Impairment),
            s.clock.as_ref().map(|i| i as &dyn Impairment),
            s.timing_offset.as_ref().map(|i| i as &dyn Impairment),
            s.timing_jitter.as_ref().map(|i| i as &dyn Impairment),
            s.iq_imbalance.as_ref().map(|i| i as &dyn Impairment),
            s.dc_offset.as_ref().map(|i| i as &dyn Impairment),
            s.cochannel.as_ref().map(|i| i as &dyn Impairment),
            s.adjacent.as_ref().map(|i| i as &dyn Impairment),
            s.clipping.as_ref().map(|i| i as &dyn Impairment),
            s.quantiser.as_ref().map(|i| i as &dyn Impairment),
            s.awgn.as_ref().map(|i| i as &dyn Impairment),
        ];
        for stage in stages.into_iter().flatten() {
            stage.apply(x, rng);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ChannelSpec;
    use crate::ber::{
        impair::{
            Awgn, Cfo, Clipping, ClockError, Impairment, rms,
            testutil::{tone, white},
        },
        rng::Rng,
    };

    /// A single-axis channel is exactly that axis — the composition adds nothing of its own.
    #[test]
    fn single_axis_channel_equals_the_bare_impairment() {
        let cfo = Cfo::from_cycles_per_sample(0.01);
        let mut via_channel = tone(0.05, 4096);
        let mut bare = via_channel.clone();
        ChannelSpec::default()
            .cfo(cfo)
            .build()
            .apply(&mut via_channel, &mut Rng::new(3));
        cfo.apply(&mut bare, &mut Rng::new(3));
        assert_eq!(via_channel, bare);
    }

    /// Order is observable and canonical: AWGN lands *after* clipping, so a clipped channel
    /// with noise still has peaks past the clip limit. The reverse order would bound the
    /// noise too, and every stated Eb/N0 would quietly lie under clipping sweeps.
    #[test]
    fn awgn_is_applied_after_clipping() {
        let mut x = white(&mut Rng::new(0x0a0), 50_000);
        let limit = rms(&x); // 0 dB overdrive clips at the RMS
        let spec = ChannelSpec::default()
            .clipping(Clipping::new(0.0))
            .awgn(Awgn::with_sigma(0.5));
        spec.build().apply(&mut x, &mut Rng::new(0x0a1));
        let max = x.iter().map(|s| f64::from(s.norm())).fold(0.0f64, f64::max);
        assert!(max > limit * 1.05, "max {max} vs clip limit {limit}");
    }

    /// Length-changing stages compose: a clock-error channel changes the length exactly as
    /// the bare resampler does, and the stages after it operate on the new length.
    #[test]
    fn length_change_propagates_through_the_composition() {
        let mut x = tone(0.03, 100_000);
        ChannelSpec::default()
            .clock(ClockError::new(500.0))
            .awgn(Awgn::with_sigma(0.01))
            .build()
            .apply(&mut x, &mut Rng::new(9));
        assert_eq!(x.len(), 100_050);
    }

    /// The whole composition replays from its seed — the reproducibility contract every
    /// committed curve rests on.
    #[test]
    fn composition_is_deterministic_from_the_seed() {
        let spec = ChannelSpec::default()
            .cfo(Cfo::from_cycles_per_sample(0.002))
            .clipping(Clipping::new(6.0))
            .awgn(Awgn::with_sigma(0.3));
        let mut a = tone(0.04, 20_000);
        let mut b = a.clone();
        spec.build().apply(&mut a, &mut Rng::new(0x5eed));
        spec.build().apply(&mut b, &mut Rng::new(0x5eed));
        assert_eq!(a, b);
    }
}
