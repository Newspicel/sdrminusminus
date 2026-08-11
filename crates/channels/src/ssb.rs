//! SSB: 48 kHz IQ → one-sided complex band filter → real part → optional AGC.
//!
//! [`SsbTx`] is the exciter that pairs with it, and deliberately by the other method: the
//! receiver filters one side of the spectrum and takes the real part, the transmitter builds
//! the analytic signal with a Hilbert transformer. Neither can hide the other's error.

use std::{
    f64::consts::{PI, TAU},
    sync::LazyLock,
};

use num_complex::Complex;
use sdrmm_dsp::{Agc, FirC, RealDecimator, design_bandpass, design_lowpass};
use sdrmm_wire::{ChannelDescriptor, ChannelParams, ChannelSettings, Sideband, SsbParams};

use crate::{
    AUDIO_RATE, ChannelCtx, ChannelError, ChannelOutputs, ChannelRx, ChannelTx, TxPayload,
    audio_agc, check_input_rate, clamp_full_scale,
    tx::{Burst, TxQueue},
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

/// Length of the exciter's Hilbert transformer. Odd, so the in-phase path is delayed by a
/// whole number of samples; long enough that the quadrature path holds its 90° across a voice
/// channel at 48 kHz — the response necessarily decays toward DC and Nyquist, which is the
/// other reason the audio is bandpassed to the passband before it reaches the transformer.
const HILBERT_TAPS: usize = 257;
/// The audio shaping ahead of the transformer is the receiver's sideband filter written as a
/// baseband bandpass: same length, same band edges, so the exciter transmits the passband the
/// receiver keeps rather than one the two have to be trusted to agree on.
const AUDIO_TAPS: usize = FILTER_TAPS;

/// Windowed Hilbert transformer: `2/(πn)` at odd offsets from the centre, zero elsewhere.
///
/// Blackman-windowed by hand rather than through `dsp`'s window: an exciter sharing a window
/// with the filters it is tested against could hide an error in either.
fn hilbert_taps() -> Vec<f32> {
    let center = (HILBERT_TAPS / 2) as i64;
    (0..HILBERT_TAPS)
        .map(|k| {
            let n = k as i64 - center;
            if n.unsigned_abs().is_multiple_of(2) {
                return 0.0;
            }
            let phase = TAU * k as f64 / (HILBERT_TAPS - 1) as f64;
            let window = 0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos();
            (window * 2.0 / (PI * n as f64)) as f32
        })
        .collect()
}

/// The in-phase path's matching delay, as a FIR whose only non-zero tap is the Hilbert
/// transformer's centre. Running both paths through the same kind of filter is what keeps them
/// sample-aligned across `submit` boundaries without a second piece of history to get wrong.
fn delay_taps() -> Vec<f32> {
    let mut taps = vec![0.0; HILBERT_TAPS];
    taps[HILBERT_TAPS / 2] = 1.0;
    taps
}

/// Audio shaping ahead of the transformer: the band [`crate::occupied_band`] says this channel
/// occupies, with both edges at −6 dB and nothing left a kilohertz past either. The low edge is
/// a skirt rather than a wall at this length — rumble below it is attenuated, not removed, and
/// what survives lands inside the transmitted sideband rather than in the suppressed one.
fn audio_bandpass(p: &SsbParams) -> Result<RealDecimator, ChannelError> {
    let rate = DESCRIPTOR.input_rate_hz;
    // Rejecting bandwidth here through the receiver's designer keeps one definition of what an
    // SSB channel may be set to.
    sideband_filter(p)?;
    Ok(RealDecimator::new(
        &design_bandpass(AUDIO_TAPS, PASSBAND_LOW_HZ / rate, p.bandwidth_hz / rate),
        1,
    ))
}

/// SSB exciter: queued voice → its analytic signal, one sideband at a time.
///
/// The phasing method a real exciter uses. Full-scale audio leaves it at unit envelope for a
/// single tone; denser audio whose analytic envelope would pass full scale is limited in
/// magnitude on the way in, which is the one place the transmitted sideband is allowed to be
/// distorted rather than allowed out of range.
pub struct SsbTx {
    /// The analytic signal, built at `submit` — `generate` only applies the burst envelope.
    queue: TxQueue<Complex<f32>>,
    sideband: Sideband,
    audio_bp: RealDecimator,
    hilbert: RealDecimator,
    delay: RealDecimator,
    shaped: Vec<f32>,
    in_phase: Vec<f32>,
    quadrature: Vec<f32>,
    burst: Burst,
}

impl ChannelTx for SsbTx {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        Ok(Self {
            queue: TxQueue::new(DESCRIPTOR.type_id.as_str(), f64::from(AUDIO_RATE)),
            sideband: p.sideband,
            audio_bp: audio_bandpass(p)?,
            hilbert: RealDecimator::new(&hilbert_taps(), 1),
            delay: RealDecimator::new(&delay_taps(), 1),
            shaped: Vec::new(),
            in_phase: Vec::new(),
            quadrature: Vec::new(),
            burst: Burst::new(ctx.input_rate),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        self.audio_bp = audio_bandpass(p)?;
        self.sideband = p.sideband;
        Ok(())
    }

    fn submit(&mut self, payload: TxPayload) -> Result<(), ChannelError> {
        let TxPayload::Audio(pcm) = payload else {
            return Err(ChannelError::InvalidPayload(
                "ssb carries audio, not frames".to_owned(),
            ));
        };
        self.queue.accept(pcm.len())?;
        // The whole exciter runs here rather than in `generate`: the hot path may not allocate,
        // and a FIR pair is the one part of this that has to see a block.
        self.audio_bp.process(&pcm, &mut self.shaped);
        self.hilbert.process(&self.shaped, &mut self.quadrature);
        self.delay.process(&self.shaped, &mut self.in_phase);
        let sign = match self.sideband {
            Sideband::Usb => 1.0,
            Sideband::Lsb => -1.0,
        };
        for (&i, &q) in self.in_phase.iter().zip(self.quadrature.iter()) {
            let sample = Complex::new(i, sign * q);
            // Limited in magnitude, not per component: scaling the pair keeps the instantaneous
            // phase — and with it the sideband — while a component clamp would fold energy into
            // the one this mode exists to suppress.
            let norm = sample.norm();
            self.queue
                .push(if norm > 1.0 { sample / norm } else { sample });
        }
        Ok(())
    }

    fn generate(&mut self, out: &mut [Complex<f32>]) -> usize {
        let mut written = 0;
        for slot in out {
            let Some(envelope) = self.burst.next(!self.queue.is_empty()) else {
                break;
            };
            *slot = self.queue.pop().unwrap_or_default() * envelope;
            written += 1;
        }
        written
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::NfmParams;

    use super::*;
    use crate::{
        testgen::{burst, tone_audio},
        testutil::{complex_tone, component, dominant_tone, rms, run_ragged, settings, tone_power},
    };

    const RATE: f64 = 48_000.0;

    /// Two tones at half scale each, so the exciter's analytic signal stays inside full scale
    /// and clipping cannot be what puts energy at a third frequency.
    fn two_tone(len: usize) -> Vec<f32> {
        tone_audio(700.0, 0.4, RATE, len)
            .iter()
            .zip(tone_audio(1_900.0, 0.4, RATE, len))
            .map(|(low, high)| low + high)
            .collect()
    }

    const fn ctx() -> ChannelCtx {
        ChannelCtx { input_rate: RATE }
    }

    fn voice_settings(sideband: Sideband) -> ChannelSettings {
        // AGC off: amplitude and rejection assertions need the raw filter output.
        settings(ChannelParams::Ssb(SsbParams {
            sideband,
            bandwidth_hz: 2_700.0,
            agc: false,
        }))
    }

    fn channel(sideband: Sideband) -> SsbChannel {
        SsbChannel::new(ctx(), voice_settings(sideband)).unwrap()
    }

    /// The whole burst the exciter makes of `audio`, ready to hand a receiver.
    fn excite(sideband: Sideband, audio: Vec<f32>) -> Vec<Complex<f32>> {
        let mut tx = SsbTx::new(ctx(), voice_settings(sideband)).unwrap();
        tx.submit(TxPayload::Audio(audio)).unwrap();
        burst(&mut tx)
    }

    /// Past the FIR pair's transient and the burst's ramps, where the exciter is in steady
    /// state and an amplitude is worth measuring.
    fn settled(iq: &[Complex<f32>]) -> &[Complex<f32>] {
        &iq[2 * AUDIO_TAPS..iq.len() - 2 * AUDIO_TAPS]
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

    /// Audio in, audio out through the exciter — the one-sided tones above pin the filter, this
    /// pins the round trip: both tones come back at their own frequencies with nothing else
    /// between them, and the opposite sideband hears neither.
    #[test]
    fn usb_recovers_a_two_tone_exciter_that_lsb_rejects() {
        let iq = excite(Sideband::Usb, two_tone(48_000));
        let mut usb = channel(Sideband::Usb);
        let audio = run_ragged(&mut usb, &iq);
        let window = &audio[2_000..14_000];
        let low = tone_power(window, 700.0, RATE);
        let high = tone_power(window, 1_900.0, RATE);
        assert!(low > 0.45, "700 Hz holds {low} of the audio power");
        assert!(high > 0.45, "1900 Hz holds {high} of the audio power");

        let mut lsb = channel(Sideband::Lsb);
        let rejected = rms(&run_ragged(&mut lsb, &iq)[2_000..14_000]);
        assert!(
            rejected < rms(window) / 10.0,
            "lsb rms {rejected} vs usb rms {}",
            rms(window)
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

    /// The whole point of an SSB exciter: the audio lands on one side of DC at the amplitude it
    /// went in at, and the mirror image is gone rather than merely damped.
    #[test]
    fn tx_puts_the_audio_on_one_side_of_dc_and_leaves_the_image_40_db_down() {
        for (sideband, sign) in [(Sideband::Usb, 1.0), (Sideband::Lsb, -1.0)] {
            for tone_hz in [700.0, 1_500.0, 2_200.0] {
                let iq = excite(sideband, tone_audio(tone_hz, 1.0, RATE, 48_000));
                let iq = settled(&iq);
                let wanted = component(iq, sign * tone_hz, RATE);
                let image = component(iq, -sign * tone_hz, RATE);
                assert!(
                    (wanted - 1.0).abs() < 0.03,
                    "{sideband:?} {tone_hz} Hz: wanted sideband {wanted}"
                );
                assert!(
                    image < 0.01,
                    "{sideband:?} {tone_hz} Hz: image {image} against {wanted}"
                );
            }
        }
    }

    /// Keeping the station inside the channel it claims is the exciter's job, not the
    /// operator's: the passband is flat, both edges sit at −6 dB where the receiver's filter
    /// puts them, and audio a kilohertz past the top does not reach the air at all.
    #[test]
    fn tx_transmits_the_passband_the_receiver_keeps() {
        let level = |tone_hz: f64| {
            let iq = excite(Sideband::Usb, tone_audio(tone_hz, 1.0, RATE, 48_000));
            component(settled(&iq), tone_hz, RATE)
        };
        for tone_hz in [700.0, 2_200.0] {
            let passed = level(tone_hz);
            assert!(
                (passed - 1.0).abs() < 0.03,
                "{tone_hz} Hz passed at {passed}"
            );
        }
        for tone_hz in [PASSBAND_LOW_HZ, 2_700.0] {
            let edge = level(tone_hz);
            assert!((0.4..0.6).contains(&edge), "{tone_hz} Hz edge at {edge}");
        }
        let stopband = level(4_000.0);
        assert!(stopband < 0.01, "4 kHz leaked at {stopband}");
    }

    /// Dense audio whose analytic envelope would pass full scale is limited, so nothing leaves
    /// the exciter out of range whatever was queued.
    #[test]
    fn tx_never_leaves_full_scale() {
        let loud: Vec<f32> = two_tone(48_000).iter().map(|s| s * 2.5).collect();
        for s in excite(Sideband::Usb, loud) {
            assert!(s.norm() <= 1.0 + 1e-6, "envelope {}", s.norm());
        }
    }

    /// A transmitter that ignored `apply` would be exciting the sideband the operator just
    /// moved off, and the receiver they moved with them would hear nothing.
    #[test]
    fn tx_apply_switches_the_sideband() {
        let mut tx = SsbTx::new(ctx(), voice_settings(Sideband::Usb)).unwrap();
        tx.apply(voice_settings(Sideband::Lsb)).unwrap();
        tx.submit(TxPayload::Audio(tone_audio(1_500.0, 1.0, RATE, 48_000)))
            .unwrap();
        let iq = burst(&mut tx);
        let iq = settled(&iq);
        assert!(component(iq, -1_500.0, RATE) > 0.9);
        assert!(component(iq, 1_500.0, RATE) < 0.01);
    }

    #[test]
    fn tx_radiates_nothing_until_audio_is_submitted() {
        let mut tx = SsbTx::new(ctx(), voice_settings(Sideband::Usb)).unwrap();
        let mut block = [Complex::new(9.0, 9.0); 64];
        assert_eq!(tx.generate(&mut block), 0);
        assert_eq!(block[0], Complex::new(9.0, 9.0));
    }

    #[test]
    fn tx_rejects_a_frame_payload_and_a_backlog_past_the_bound() {
        let mut tx = SsbTx::new(ctx(), voice_settings(Sideband::Usb)).unwrap();
        assert!(matches!(
            tx.submit(TxPayload::Frame(vec![0x7E])),
            Err(ChannelError::InvalidPayload(_))
        ));
        let over = (crate::tx::MAX_QUEUE_S * f64::from(AUDIO_RATE)) as usize + 1;
        assert!(matches!(
            tx.submit(TxPayload::Audio(vec![0.0; over])),
            Err(ChannelError::InvalidPayload(_))
        ));
        let mut block = [Complex::new(0.0, 0.0); 16];
        assert_eq!(tx.generate(&mut block), 0);
    }

    #[test]
    fn tx_rejects_mismatched_params_and_out_of_range_bandwidth() {
        let built = SsbTx::new(ctx(), settings(ChannelParams::Nfm(NfmParams::default())));
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
        let built = SsbTx::new(
            ctx(),
            settings(ChannelParams::Ssb(SsbParams {
                sideband: Sideband::Usb,
                bandwidth_hz: 50.0,
                agc: false,
            })),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
        let built = SsbTx::new(
            ChannelCtx {
                input_rate: 240_000.0,
            },
            voice_settings(Sideband::Usb),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }
}
