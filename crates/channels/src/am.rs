//! AM envelope detector: 48 kHz IQ → magnitude → DC block → lowpass → optional AGC.

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Agc, DcBlocker, Decimator, RealDecimator, design_lowpass};
use sdrmm_wire::{AmParams, ChannelDescriptor, ChannelParams, ChannelSettings};

use crate::{
    AUDIO_RATE, ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, audio_agc,
    check_input_rate, clamp_full_scale,
};

const AUDIO_TAPS: usize = 129;
const CHANNEL_TAPS: usize = 129;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "am".to_owned(),
    name: "AM".to_owned(),
    bandwidth_hz: 10_000.0,
    input_rate_hz: 48_000.0,
    has_audio: true,
    decoder_kind: None,
    ..ChannelDescriptor::default()
});

pub struct AmChannel {
    dc: DcBlocker,
    audio_lp: RealDecimator,
    agc: Option<Agc>,
    mag_buf: Vec<f32>,
}

fn params(settings: &ChannelSettings) -> Result<&AmParams, ChannelError> {
    match &settings.params {
        ChannelParams::Am(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "am channel got {} params",
            other.type_id()
        ))),
    }
}

fn check_bandwidth(p: &AmParams) -> Result<(), ChannelError> {
    let rate = DESCRIPTOR.input_rate_hz;
    if p.bandwidth_hz.is_finite() && p.bandwidth_hz > 0.0 && p.bandwidth_hz < rate {
        Ok(())
    } else {
        Err(ChannelError::InvalidSettings(format!(
            "am bandwidth must be in (0, {rate}) Hz, got {}",
            p.bandwidth_hz
        )))
    }
}

pub(crate) fn channel_filter(p: &AmParams) -> Result<ChannelFilter, ChannelError> {
    check_bandwidth(p)?;
    let cutoff = p.bandwidth_hz / 2.0 / DESCRIPTOR.input_rate_hz;
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(CHANNEL_TAPS, cutoff),
        1,
    )))
}

// A `bandwidth_hz`-wide AM signal carries audio to bandwidth/2, so the post-detection
// lowpass is the matched half of the host's RF channel filter, not a duplicate of it.
// `dsp` has no factor-1 real-FIR runner; `RealDecimator` at 1:1 is exactly that.
fn audio_lowpass(p: &AmParams) -> Result<RealDecimator, ChannelError> {
    check_bandwidth(p)?;
    let cutoff = p.bandwidth_hz / 2.0 / DESCRIPTOR.input_rate_hz;
    Ok(RealDecimator::new(&design_lowpass(AUDIO_TAPS, cutoff), 1))
}

impl AmChannel {
    fn set_agc(&mut self, enabled: bool) {
        if enabled {
            if self.agc.is_none() {
                self.agc = Some(audio_agc());
            }
        } else {
            self.agc = None;
        }
    }
}

impl ChannelRx for AmChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        let audio_lp = audio_lowpass(p)?;
        let mut chan = Self {
            dc: DcBlocker::new(),
            audio_lp,
            agc: None,
            mag_buf: Vec::new(),
        };
        chan.set_agc(p.agc);
        Ok(chan)
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        self.audio_lp = audio_lowpass(p)?;
        self.set_agc(p.agc);
        Ok(())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.mag_buf.clear();
        self.mag_buf.extend(iq.iter().map(|x| x.norm()));
        self.dc.process(&mut self.mag_buf);
        self.audio_lp.process(&self.mag_buf, &mut out.audio_pcm);
        if let Some(agc) = self.agc.as_mut() {
            agc.process(&mut out.audio_pcm);
        }
        clamp_full_scale(&mut out.audio_pcm);
        if !out.audio_pcm.is_empty() {
            out.audio_rate = AUDIO_RATE;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use sdrmm_wire::WfmParams;

    use super::*;
    use crate::testutil::{dominant_tone, rms, run_ragged, settings};

    const RATE: f64 = 48_000.0;

    fn am_iq(depth: f32, f_mod: f64, len: usize) -> Vec<Complex<f32>> {
        (0..len)
            .map(|k| {
                let env = 1.0 + depth * (TAU * f_mod * k as f64 / RATE).cos() as f32;
                Complex::new(env, 0.0)
            })
            .collect()
    }

    fn channel(p: AmParams) -> AmChannel {
        AmChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Am(p)),
        )
        .unwrap()
    }

    #[test]
    fn demodulates_1_khz_tone_over_ragged_blocks() {
        let mut chan = channel(AmParams {
            bandwidth_hz: 10_000.0,
            agc: false,
        });
        let audio = run_ragged(&mut chan, &am_iq(0.5, 1_000.0, 48_000));
        let window = &audio[4_000..16_000];
        let (freq, ratio) = dominant_tone(window, RATE);
        assert!((995.0..1_005.0).contains(&freq), "dominant {freq} Hz");
        assert!(ratio > 10.0, "tone-to-rest ratio {ratio}");
        // 50 % depth → 0.5-amplitude tone once the carrier DC is blocked.
        let amplitude = rms(window);
        assert!((0.32..0.39).contains(&amplitude), "rms {amplitude}");
    }

    #[test]
    fn agc_levels_audio_to_the_shared_target() {
        let mut chan = channel(AmParams {
            bandwidth_hz: 10_000.0,
            agc: true,
        });
        let audio = run_ragged(&mut chan, &am_iq(0.5, 1_000.0, 48_000));
        // Shared audio AGC target is 0.25 RMS; allow ±3 dB after convergence.
        let amplitude = rms(&audio[40_000..47_000]);
        assert!((0.18..0.36).contains(&amplitude), "rms {amplitude}");
    }

    #[test]
    fn apply_reconfigures_bandwidth_and_agc() {
        let mut chan = channel(AmParams {
            bandwidth_hz: 10_000.0,
            agc: true,
        });
        chan.apply(settings(ChannelParams::Am(AmParams {
            bandwidth_hz: 6_000.0,
            agc: false,
        })))
        .unwrap();
        let audio = run_ragged(&mut chan, &am_iq(0.5, 1_000.0, 48_000));
        let window = &audio[4_000..16_000];
        let (freq, ratio) = dominant_tone(window, RATE);
        assert!((995.0..1_005.0).contains(&freq), "dominant {freq} Hz");
        assert!(ratio > 10.0, "tone-to-rest ratio {ratio}");
        let amplitude = rms(window);
        assert!((0.32..0.39).contains(&amplitude), "rms {amplitude}");
    }

    #[test]
    fn mismatched_params_variant_is_rejected() {
        let mut chan = channel(AmParams::default());
        let err = chan.apply(settings(ChannelParams::Wfm(WfmParams::default())));
        assert!(matches!(err, Err(ChannelError::InvalidSettings(_))));
    }
}
