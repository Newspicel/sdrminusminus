//! WFM mono: 240 kHz IQ → quadrature discriminator → de-emphasis → 5:1 decimate to 48 kHz.

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, Deemphasis, FmDemod, RealDecimator, design_lowpass};
use sdrmm_wire::{ChannelDescriptor, ChannelParams, ChannelSettings, WfmParams};

use crate::{
    AUDIO_RATE, ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx,
    check_input_rate, clamp_full_scale,
};

const DEVIATION_HZ: f64 = 75_000.0;
/// Mono audio ends at 15 kHz; everything above (pilot, stereo subcarrier, RDS) is cut.
const AUDIO_CUTOFF_HZ: f64 = 15_000.0;
const DECIM_FACTOR: usize = 5;
const DECIM_TAPS: usize = 127;
/// The ±100 kHz channel edge sits at 0.417 of the 240 kHz rate; 65 taps put the stopband
/// just inside Nyquist while keeping the per-sample cost sane at this rate.
const CHANNEL_TAPS: usize = 65;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "wfm".to_owned(),
    name: "WFM (mono)".to_owned(),
    bandwidth_hz: 200_000.0,
    input_rate_hz: 240_000.0,
});

pub struct WfmChannel {
    demod: FmDemod,
    deemphasis: Deemphasis,
    decim: RealDecimator,
    demod_buf: Vec<f32>,
}

fn params(settings: &ChannelSettings) -> Result<&WfmParams, ChannelError> {
    match &settings.params {
        ChannelParams::Wfm(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "wfm channel got {} params",
            other.type_id()
        ))),
    }
}

/// WFM has no bandwidth knob; the channel filter is fixed at the descriptor nominal.
pub(crate) fn channel_filter() -> ChannelFilter {
    let cutoff = DESCRIPTOR.bandwidth_hz / 2.0 / DESCRIPTOR.input_rate_hz;
    ChannelFilter::Symmetric(Decimator::new(&design_lowpass(CHANNEL_TAPS, cutoff), 1))
}

fn deemphasis(p: &WfmParams) -> Result<Deemphasis, ChannelError> {
    if !(p.deemphasis_us.is_finite() && p.deemphasis_us > 0.0) {
        return Err(ChannelError::InvalidSettings(format!(
            "wfm deemphasis must be positive, got {} µs",
            p.deemphasis_us
        )));
    }
    Ok(Deemphasis::new(DESCRIPTOR.input_rate_hz, p.deemphasis_us))
}

impl ChannelRx for WfmChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let deemphasis = deemphasis(params(&settings)?)?;
        Ok(Self {
            demod: FmDemod::new(ctx.input_rate, DEVIATION_HZ),
            deemphasis,
            decim: RealDecimator::new(
                &design_lowpass(DECIM_TAPS, AUDIO_CUTOFF_HZ / ctx.input_rate),
                DECIM_FACTOR,
            ),
            demod_buf: Vec::new(),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        self.deemphasis = deemphasis(params(&settings)?)?;
        Ok(())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.demod.process(iq, &mut self.demod_buf);
        self.deemphasis.process(&mut self.demod_buf);
        self.decim.process(&self.demod_buf, &mut out.audio_pcm);
        clamp_full_scale(&mut out.audio_pcm);
        if !out.audio_pcm.is_empty() {
            out.audio_rate = AUDIO_RATE;
        }
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::SsbParams;

    use super::*;
    use crate::testutil::{dominant_tone, fm_iq, rms, run_ragged, settings};

    const RATE: f64 = 240_000.0;

    fn channel(deemphasis_us: f32) -> WfmChannel {
        WfmChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Wfm(WfmParams { deemphasis_us })),
        )
        .unwrap()
    }

    #[test]
    fn demodulates_1_khz_tone_at_48_khz_over_ragged_blocks() {
        let mut chan = channel(50.0);
        let audio = run_ragged(&mut chan, &fm_iq(RATE, 1_000.0, DEVIATION_HZ, 240_000));
        let window = &audio[2_000..14_000];
        let (freq, ratio) = dominant_tone(window, f64::from(AUDIO_RATE));
        assert!((995.0..1_005.0).contains(&freq), "dominant {freq} Hz");
        assert!(ratio > 10.0, "tone-to-rest ratio {ratio}");
        // Unit demod cosine through 50 µs de-emphasis: |H(1 kHz)| ≈ 0.954 → RMS ≈ 0.67.
        let amplitude = rms(window);
        assert!((0.6..0.74).contains(&amplitude), "rms {amplitude}");
    }

    #[test]
    fn output_length_is_exactly_one_fifth_of_input() {
        let mut chan = channel(50.0);
        let iq = fm_iq(RATE, 1_000.0, DEVIATION_HZ, 240_000);
        let audio = run_ragged(&mut chan, &iq);
        assert_eq!(audio.len(), iq.len() / DECIM_FACTOR);
    }

    #[test]
    fn apply_75_us_deemphasis_keeps_demodulating() {
        let mut chan = channel(50.0);
        chan.apply(settings(ChannelParams::Wfm(WfmParams {
            deemphasis_us: 75.0,
        })))
        .unwrap();
        let audio = run_ragged(&mut chan, &fm_iq(RATE, 1_000.0, DEVIATION_HZ, 240_000));
        let (freq, ratio) = dominant_tone(&audio[2_000..14_000], f64::from(AUDIO_RATE));
        assert!((995.0..1_005.0).contains(&freq), "dominant {freq} Hz");
        assert!(ratio > 10.0, "tone-to-rest ratio {ratio}");
    }

    #[test]
    fn mismatched_params_variant_is_rejected() {
        let mut chan = channel(50.0);
        let err = chan.apply(settings(ChannelParams::Ssb(SsbParams::default())));
        assert!(matches!(err, Err(ChannelError::InvalidSettings(_))));
    }

    #[test]
    fn wrong_input_rate_is_rejected() {
        let built = WfmChannel::new(
            ChannelCtx {
                input_rate: 48_000.0,
            },
            settings(ChannelParams::Wfm(WfmParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }
}
