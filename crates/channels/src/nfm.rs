//! NFM voice: 48 kHz IQ → quadrature discriminator → voice-band lowpass. RF selectivity at
//! `bandwidth_hz` is the host's channel filter (see [`crate::channel_filter`]); here the
//! bandwidth only sets the deviation the discriminator is scaled to.
//!
//! [`NfmTx`] is the modulator that pairs with it. Neither carries pre- or de-emphasis: the two
//! have to agree, and a flat pair is the one that round-trips.

use std::{f64::consts::TAU, sync::LazyLock};

use num_complex::Complex;
use sdrmm_dsp::{Decimator, FmDemod, RealDecimator, design_lowpass};
use sdrmm_wire::{ChannelDescriptor, ChannelParams, ChannelSettings, NfmParams};

use crate::{
    AUDIO_RATE, ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, ChannelTx,
    TxPayload, check_input_rate, clamp_full_scale,
    tx::{Burst, TxQueue},
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
    has_audio: true,
    decoder_kind: None,
    ..ChannelDescriptor::default()
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

/// NFM modulator: queued voice → phase-accumulated FM on a constant-envelope carrier.
///
/// Written against the deviation and audio bandwidth [`NfmChannel`] demodulates, and sharing
/// those constants with it, but no code — an error in the discriminator cannot cancel against
/// one here.
pub struct NfmTx {
    rate: f64,
    deviation_hz: f64,
    /// Band-limited by [`audio_lowpass`] on the way in, so `generate` only accumulates phase.
    queue: TxQueue<f32>,
    audio_lp: RealDecimator,
    filtered: Vec<f32>,
    burst: Burst,
    phase: f64,
}

impl ChannelTx for NfmTx {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        check_bandwidth(p)?;
        Ok(Self {
            rate: ctx.input_rate,
            deviation_hz: deviation_hz(p),
            queue: TxQueue::new(DESCRIPTOR.type_id.as_str(), f64::from(AUDIO_RATE)),
            audio_lp: audio_lowpass(),
            filtered: Vec::new(),
            burst: Burst::new(ctx.input_rate),
            phase: 0.0,
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        check_bandwidth(p)?;
        self.deviation_hz = deviation_hz(p);
        Ok(())
    }

    fn submit(&mut self, payload: TxPayload) -> Result<(), ChannelError> {
        let TxPayload::Audio(pcm) = payload else {
            return Err(ChannelError::InvalidPayload(
                "nfm carries audio, not frames".to_owned(),
            ));
        };
        self.queue.accept(pcm.len())?;
        // Band-limited here rather than in `generate` — the hot path may not allocate — and
        // clamped after the filter rather than before it: what may not pass ±1 is what reaches
        // the phase accumulator, and a filter's overshoot is as capable of over-deviating the
        // carrier as a caller's over-range audio is.
        self.audio_lp.process(&pcm, &mut self.filtered);
        clamp_full_scale(&mut self.filtered);
        self.queue.extend(self.filtered.iter().copied());
        Ok(())
    }

    fn generate(&mut self, out: &mut [Complex<f32>]) -> usize {
        let mut written = 0;
        for slot in out {
            let Some(envelope) = self.burst.next(!self.queue.is_empty()) else {
                break;
            };
            let audio = self.queue.pop().unwrap_or(0.0);
            self.phase += TAU * self.deviation_hz * f64::from(audio) / self.rate;
            if self.phase > TAU {
                self.phase -= TAU;
            } else if self.phase < -TAU {
                self.phase += TAU;
            }
            *slot = Complex::from_polar(envelope, self.phase as f32);
            written += 1;
        }
        written
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::AmParams;

    use super::*;
    use crate::{
        testgen::{burst, tone_audio},
        testutil::{complex_noise, dominant_tone, fm_iq, rms, run_ragged, settings},
        tx::MAX_QUEUE_S,
    };

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

    const fn ctx() -> ChannelCtx {
        ChannelCtx { input_rate: RATE }
    }

    fn ramp_len() -> usize {
        Burst::new(RATE).ramp_len()
    }

    fn transmitter() -> NfmTx {
        NfmTx::new(ctx(), settings(ChannelParams::Nfm(NfmParams::default()))).unwrap()
    }

    /// The pair's whole reason for existing: what the modulator sends, the demodulator hears.
    #[test]
    fn tx_round_trips_a_tone_through_the_demodulator() {
        for bandwidth_hz in [12_500.0, 25_000.0] {
            let params = ChannelParams::Nfm(NfmParams { bandwidth_hz });
            let mut tx = NfmTx::new(ctx(), settings(params.clone())).unwrap();
            tx.submit(TxPayload::Audio(tone_audio(1_000.0, 1.0, RATE, 24_000)))
                .unwrap();
            let iq = burst(&mut tx);
            assert_eq!(
                iq.len(),
                24_000 + ramp_len(),
                "{bandwidth_hz} Hz burst length"
            );

            let mut rx = NfmChannel::new(ctx(), settings(params)).unwrap();
            let audio = run_ragged(&mut rx, &iq);
            let window = &audio[2_000..20_000];
            let (freq, ratio) = dominant_tone(window, RATE);
            assert!(
                (995.0..1_005.0).contains(&freq),
                "{bandwidth_hz} Hz: {freq} Hz"
            );
            assert!(ratio > 10.0, "{bandwidth_hz} Hz: tone-to-rest {ratio}");
            // Deviation tracks the channel plan on both sides, so a full-scale tone comes back
            // full-scale at either spacing — not doubled at the wide one.
            let amplitude = rms(window);
            assert!(
                (0.6..0.8).contains(&amplitude),
                "{bandwidth_hz} Hz: rms {amplitude}"
            );
        }
    }

    /// Both ends moved to 25 kHz spacing still round-trip at full scale. A transmitter that
    /// ignored `apply` would still be deviating ±2.5 kHz into a receiver now scaled to ±5 kHz,
    /// and the tone would come back at half amplitude.
    #[test]
    fn tx_apply_rescales_deviation() {
        let wide = settings(ChannelParams::Nfm(NfmParams {
            bandwidth_hz: 25_000.0,
        }));
        let mut tx = transmitter();
        tx.apply(wide.clone()).unwrap();
        tx.submit(TxPayload::Audio(tone_audio(1_000.0, 1.0, RATE, 24_000)))
            .unwrap();
        let iq = burst(&mut tx);

        let mut rx = channel();
        rx.apply(wide).unwrap();
        let amplitude = rms(&run_ragged(&mut rx, &iq)[2_000..20_000]);
        assert!((0.6..0.8).contains(&amplitude), "rms {amplitude}");
    }

    #[test]
    fn tx_ramps_the_burst_edges_and_holds_a_constant_envelope_between() {
        let ramp = ramp_len();
        let mut tx = transmitter();
        tx.submit(TxPayload::Audio(vec![0.0; 4_800])).unwrap();
        let iq = burst(&mut tx);

        assert!(iq[0].norm() < 0.01, "first sample {}", iq[0].norm());
        for (k, pair) in iq[..ramp].windows(2).enumerate() {
            assert!(pair[1].norm() > pair[0].norm(), "rise not monotonic at {k}");
        }
        let tail = &iq[iq.len() - ramp..];
        for (k, pair) in tail.windows(2).enumerate() {
            assert!(pair[1].norm() < pair[0].norm(), "fall not monotonic at {k}");
        }
        assert!(
            tail[ramp - 1].norm() < 0.01,
            "last sample {}",
            tail[ramp - 1].norm()
        );
        for (k, s) in iq[ramp..iq.len() - ramp].iter().enumerate() {
            assert!(
                (s.norm() - 1.0).abs() < 1e-5,
                "envelope {} at {k}",
                s.norm()
            );
        }
    }

    #[test]
    fn tx_radiates_nothing_until_audio_is_submitted() {
        let mut tx = transmitter();
        let mut block = [Complex::new(9.0, 9.0); 64];
        assert_eq!(tx.generate(&mut block), 0);
        // `generate` writes to the head of the caller's buffer and reports how far it got; the
        // rest is the caller's, not something to zero-fill.
        assert_eq!(block[0], Complex::new(9.0, 9.0));
    }

    #[test]
    fn tx_rejects_a_frame_payload() {
        let mut tx = transmitter();
        let err = tx.submit(TxPayload::Frame(vec![0x7E]));
        assert!(matches!(err, Err(ChannelError::InvalidPayload(_))));
    }

    #[test]
    fn tx_refuses_a_backlog_past_the_queue_bound() {
        let mut tx = transmitter();
        let over = (MAX_QUEUE_S * f64::from(AUDIO_RATE)) as usize + 1;
        assert!(matches!(
            tx.submit(TxPayload::Audio(vec![0.0; over])),
            Err(ChannelError::InvalidPayload(_))
        ));
        // A refused payload is not a half-queued one.
        let mut block = [Complex::new(0.0, 0.0); 16];
        assert_eq!(tx.generate(&mut block), 0);
    }

    #[test]
    fn tx_rejects_mismatched_params_and_input_rate() {
        let built = NfmTx::new(ctx(), settings(ChannelParams::Am(AmParams::default())));
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
        let built = NfmTx::new(
            ChannelCtx {
                input_rate: 240_000.0,
            },
            settings(ChannelParams::Nfm(NfmParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }
}
