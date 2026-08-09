//! NFM voice: 48 kHz IQ → quadrature discriminator → voice-band lowpass. RF selectivity at
//! `bandwidth_hz` is the host's channel filter (see [`crate::channel_filter`]); here the
//! bandwidth only sets the deviation the discriminator is scaled to.

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, FmDemod, RealDecimator, design_lowpass};
use sdrmm_wire::{ChannelDescriptor, ChannelParams, ChannelSettings, NfmParams};

use crate::{
    AUDIO_RATE, ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx,
    check_input_rate, clamp_full_scale,
};

/// Fixed post-demod audio cutoff — voice content ends here regardless of channel spacing.
const VOICE_CUTOFF_HZ: f64 = 3_400.0;
const AUDIO_TAPS: usize = 129;
const CHANNEL_TAPS: usize = 129;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "nfm".to_owned(),
    name: "NFM".to_owned(),
    bandwidth_hz: 12_500.0,
    input_rate_hz: 48_000.0,
});

pub struct NfmChannel {
    demod: FmDemod,
    audio_lp: RealDecimator,
    demod_buf: Vec<f32>,
}

fn params(settings: &ChannelSettings) -> Result<&NfmParams, ChannelError> {
    match &settings.params {
        ChannelParams::Nfm(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "nfm channel got {} params",
            other.type_id()
        ))),
    }
}

fn check_bandwidth(p: &NfmParams) -> Result<(), ChannelError> {
    let rate = DESCRIPTOR.input_rate_hz;
    if p.bandwidth_hz.is_finite() && p.bandwidth_hz > 0.0 && p.bandwidth_hz < rate {
        Ok(())
    } else {
        Err(ChannelError::InvalidSettings(format!(
            "nfm bandwidth must be in (0, {rate}) Hz, got {}",
            p.bandwidth_hz
        )))
    }
}

/// Deviation follows the channel plan — ±2.5 kHz on 12.5 kHz spacing, ±5 kHz on 25 kHz —
/// so a fixed bandwidth/5 ratio covers both standards.
fn deviation_hz(p: &NfmParams) -> f64 {
    p.bandwidth_hz / 5.0
}

pub(crate) fn channel_filter(p: &NfmParams) -> Result<ChannelFilter, ChannelError> {
    check_bandwidth(p)?;
    let cutoff = p.bandwidth_hz / 2.0 / DESCRIPTOR.input_rate_hz;
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(CHANNEL_TAPS, cutoff),
        1,
    )))
}

// `dsp` has no factor-1 real-FIR runner; `RealDecimator` at 1:1 is exactly that.
fn audio_lowpass() -> RealDecimator {
    RealDecimator::new(
        &design_lowpass(AUDIO_TAPS, VOICE_CUTOFF_HZ / f64::from(AUDIO_RATE)),
        1,
    )
}

impl ChannelRx for NfmChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        check_bandwidth(p)?;
        Ok(Self {
            demod: FmDemod::new(ctx.input_rate, deviation_hz(p)),
            audio_lp: audio_lowpass(),
            demod_buf: Vec::new(),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        check_bandwidth(p)?;
        self.demod = FmDemod::new(DESCRIPTOR.input_rate_hz, deviation_hz(p));
        Ok(())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.demod.process(iq, &mut self.demod_buf);
        self.audio_lp.process(&self.demod_buf, &mut out.audio_pcm);
        clamp_full_scale(&mut out.audio_pcm);
        if !out.audio_pcm.is_empty() {
            out.audio_rate = AUDIO_RATE;
        }
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::AmParams;

    use super::*;
    use crate::testutil::{complex_noise, dominant_tone, fm_iq, rms, run_ragged, settings};

    const RATE: f64 = 48_000.0;
    /// Deviation matching the default 12.5 kHz bandwidth (bandwidth/5).
    const DEVIATION_HZ: f64 = 2_500.0;

    fn channel() -> NfmChannel {
        NfmChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Nfm(NfmParams::default())),
        )
        .unwrap()
    }

    #[test]
    fn demodulates_1_khz_tone_over_ragged_blocks() {
        let mut chan = channel();
        let audio = run_ragged(&mut chan, &fm_iq(RATE, 1_000.0, DEVIATION_HZ, 48_000));
        let window = &audio[2_000..14_000];
        let (freq, ratio) = dominant_tone(window, RATE);
        assert!((995.0..1_005.0).contains(&freq), "dominant {freq} Hz");
        assert!(ratio > 10.0, "tone-to-rest ratio {ratio}");
        // Full-scale deviation demodulates to a unit-amplitude cosine.
        let amplitude = rms(window);
        assert!((0.6..0.8).contains(&amplitude), "rms {amplitude}");
    }

    #[test]
    fn apply_wide_bandwidth_rescales_deviation() {
        let mut chan = channel();
        chan.apply(settings(ChannelParams::Nfm(NfmParams {
            bandwidth_hz: 25_000.0,
        })))
        .unwrap();
        // A ±5 kHz-deviation signal (standard for 25 kHz channels) must land at unit
        // amplitude, not the ±2.0 a fixed 2.5 kHz scale would produce.
        let audio = run_ragged(&mut chan, &fm_iq(RATE, 1_000.0, 5_000.0, 48_000));
        let window = &audio[2_000..14_000];
        let (freq, ratio) = dominant_tone(window, RATE);
        assert!((995.0..1_005.0).contains(&freq), "dominant {freq} Hz");
        assert!(ratio > 10.0, "tone-to-rest ratio {ratio}");
        let amplitude = rms(window);
        assert!((0.6..0.8).contains(&amplitude), "rms {amplitude}");
    }

    #[test]
    fn carrier_free_noise_stays_within_full_scale() {
        // Discriminator noise with no carrier reaches ±(π·rate)/(2π·deviation) ≈ ±9.6 before
        // the audio filter; the final clamp must bound what leaves the channel.
        let mut chan = channel();
        let audio = run_ragged(&mut chan, &complex_noise(0x1234_5678, 0.01, 48_000));
        assert!(!audio.is_empty());
        for (i, &s) in audio.iter().enumerate() {
            assert!((-1.0..=1.0).contains(&s), "sample {i} out of range: {s}");
        }
    }

    #[test]
    fn out_of_range_bandwidth_is_rejected() {
        for bad in [0.0, -1.0, 48_000.0, f64::NAN] {
            let built = NfmChannel::new(
                ChannelCtx { input_rate: RATE },
                settings(ChannelParams::Nfm(NfmParams { bandwidth_hz: bad })),
            );
            assert!(
                matches!(built, Err(ChannelError::InvalidSettings(_))),
                "bandwidth {bad} must be rejected"
            );
        }
    }

    #[test]
    fn mismatched_params_variant_is_rejected() {
        let mut chan = channel();
        let err = chan.apply(settings(ChannelParams::Am(AmParams::default())));
        assert!(matches!(err, Err(ChannelError::InvalidSettings(_))));
        let built = NfmChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Am(AmParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }

    #[test]
    fn wrong_input_rate_is_rejected() {
        let built = NfmChannel::new(
            ChannelCtx {
                input_rate: 240_000.0,
            },
            settings(ChannelParams::Nfm(NfmParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }
}
