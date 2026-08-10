//! SSB: 48 kHz IQ → one-sided complex band filter → real part → optional AGC.

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Agc, FirC, design_lowpass};
use sdrmm_wire::{ChannelDescriptor, ChannelParams, ChannelSettings, Sideband, SsbParams};

use crate::{
    AUDIO_RATE, ChannelCtx, ChannelError, ChannelOutputs, ChannelRx, audio_agc, check_input_rate,
    clamp_full_scale,
};

/// Lower passband edge: keeps demodulator DC and rumble out of the audio.
pub(crate) const PASSBAND_LOW_HZ: f64 = 100.0;
/// Long prototype for a ~500 Hz transition — the opposite sideband must be gone, not damped.
const FILTER_TAPS: usize = 257;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "ssb".to_owned(),
    name: "SSB".to_owned(),
    bandwidth_hz: 3_000.0,
    input_rate_hz: 48_000.0,
    has_audio: true,
    decoder_kind: None,
    ..ChannelDescriptor::default()
});

pub struct SsbChannel {
    filter: FirC,
    agc: Option<Agc>,
    filt_buf: Vec<Complex<f32>>,
}

fn params(settings: &ChannelSettings) -> Result<&SsbParams, ChannelError> {
    match &settings.params {
        ChannelParams::Ssb(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "ssb channel got {} params",
            other.type_id()
        ))),
    }
}

pub(crate) fn sideband_filter(p: &SsbParams) -> Result<FirC, ChannelError> {
    let rate = DESCRIPTOR.input_rate_hz;
    if !(p.bandwidth_hz.is_finite()
        && p.bandwidth_hz > PASSBAND_LOW_HZ
        && p.bandwidth_hz < rate / 2.0)
    {
        return Err(ChannelError::InvalidSettings(format!(
            "ssb bandwidth must be in ({PASSBAND_LOW_HZ}, {}) Hz, got {}",
            rate / 2.0,
            p.bandwidth_hz
        )));
    }
    let half_width = (p.bandwidth_hz - PASSBAND_LOW_HZ) / 2.0;
    let center = (p.bandwidth_hz + PASSBAND_LOW_HZ) / 2.0;
    let prototype = design_lowpass(FILTER_TAPS, half_width / rate);
    let center_norm = match p.sideband {
        Sideband::Usb => center,
        Sideband::Lsb => -center,
    } / rate;
    Ok(FirC::from_lowpass(&prototype, center_norm))
}

impl SsbChannel {
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

impl ChannelRx for SsbChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        let filter = sideband_filter(p)?;
        let mut chan = Self {
            filter,
            agc: None,
            filt_buf: Vec::new(),
        };
        chan.set_agc(p.agc);
        Ok(chan)
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        self.filter = sideband_filter(p)?;
        self.set_agc(p.agc);
        Ok(())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.filter.process(iq, &mut self.filt_buf);
        out.audio_pcm.clear();
        // The received baseband is already one-sided at full amplitude (a unit RF tone
        // arrives as a unit complex exponential), so Re() alone is the product-detector
        // output — any extra gain would put strong stations past full scale.
        out.audio_pcm.extend(self.filt_buf.iter().map(|x| x.re));
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
    use sdrmm_wire::NfmParams;

    use super::*;
    use crate::testutil::{complex_tone, dominant_tone, rms, run_ragged, settings};

    const RATE: f64 = 48_000.0;

    fn channel(sideband: Sideband) -> SsbChannel {
        // AGC off: amplitude and rejection assertions need the raw filter output.
        SsbChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Ssb(SsbParams {
                sideband,
                bandwidth_hz: 2_700.0,
                agc: false,
            })),
        )
        .unwrap()
    }

    #[test]
    fn usb_demodulates_positive_tone_over_ragged_blocks() {
        let mut chan = channel(Sideband::Usb);
        let audio = run_ragged(&mut chan, &complex_tone(1_000.0 / RATE, 48_000));
        let window = &audio[2_000..14_000];
        let (freq, ratio) = dominant_tone(window, RATE);
        assert!((995.0..1_005.0).contains(&freq), "dominant {freq} Hz");
        assert!(ratio > 10.0, "tone-to-rest ratio {ratio}");
        // A unit one-sided tone demodulates to a unit-amplitude cosine (rms ≈ 0.707) —
        // full-scale RF maps to full-scale PCM, never beyond.
        let amplitude = rms(window);
        assert!((0.62..0.78).contains(&amplitude), "rms {amplitude}");
    }

    #[test]
    fn lsb_demodulates_negative_tone() {
        let mut chan = channel(Sideband::Lsb);
        let audio = run_ragged(&mut chan, &complex_tone(-1_000.0 / RATE, 48_000));
        let (freq, ratio) = dominant_tone(&audio[2_000..14_000], RATE);
        assert!((995.0..1_005.0).contains(&freq), "dominant {freq} Hz");
        assert!(ratio > 10.0, "tone-to-rest ratio {ratio}");
    }

    #[test]
    fn lsb_rejects_a_usb_tone_by_more_than_20_db() {
        let input = complex_tone(1_000.0 / RATE, 48_000);
        let mut usb = channel(Sideband::Usb);
        let usb_rms = rms(&run_ragged(&mut usb, &input)[2_000..14_000]);
        let mut lsb = channel(Sideband::Lsb);
        let lsb_rms = rms(&run_ragged(&mut lsb, &input)[2_000..14_000]);
        assert!(
            lsb_rms < usb_rms / 10.0,
            "lsb rms {lsb_rms} vs usb rms {usb_rms}"
        );
    }

    #[test]
    fn apply_narrower_bandwidth_keeps_demodulating() {
        let mut chan = channel(Sideband::Usb);
        chan.apply(settings(ChannelParams::Ssb(SsbParams {
            sideband: Sideband::Usb,
            bandwidth_hz: 2_000.0,
            agc: false,
        })))
        .unwrap();
        let audio = run_ragged(&mut chan, &complex_tone(1_000.0 / RATE, 48_000));
        let (freq, ratio) = dominant_tone(&audio[2_000..14_000], RATE);
        assert!((995.0..1_005.0).contains(&freq), "dominant {freq} Hz");
        assert!(ratio > 10.0, "tone-to-rest ratio {ratio}");
    }

    #[test]
    fn mismatched_params_variant_is_rejected() {
        let mut chan = channel(Sideband::Usb);
        let err = chan.apply(settings(ChannelParams::Nfm(NfmParams::default())));
        assert!(matches!(err, Err(ChannelError::InvalidSettings(_))));
    }
}
