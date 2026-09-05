use num_complex::Complex;

use super::{
    Awgn, BurstModel, Cfo, Clipping, ClockError, DcOffset, Drift, Impairment, Interferer,
    IqImbalance, Multipath, PhaseNoise, Quantiser, TimingJitter, TimingOffset,
};
use crate::ber::rng::Rng;

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

    #[test]
    fn awgn_is_applied_after_clipping() {
        let mut x = white(&mut Rng::new(0x0a0), 50_000);
        let limit = rms(&x);
        let spec = ChannelSpec::default()
            .clipping(Clipping::new(0.0))
            .awgn(Awgn::with_sigma(0.5));
        spec.build().apply(&mut x, &mut Rng::new(0x0a1));
        let max = x.iter().map(|s| f64::from(s.norm())).fold(0.0f64, f64::max);
        assert!(max > limit * 1.05, "max {max} vs clip limit {limit}");
    }

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
