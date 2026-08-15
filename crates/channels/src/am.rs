use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, RealDecimator, design_lowpass};
use sdrmm_modem::analog::{AmDemod, AmDetector, AmMode, AmParams as AmWaveform, AmRx};
use sdrmm_wire::{AmParams, ChannelDescriptor, ChannelParams, ChannelSettings};

use crate::{
    AUDIO_RATE, ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, ChannelTx,
    TxPayload, check_input_rate, clamp_full_scale,
    tx::{Burst, TxQueue},
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
    demod: AmDemod,
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

fn audio_lowpass(p: &AmParams) -> Result<RealDecimator, ChannelError> {
    check_bandwidth(p)?;
    let cutoff = p.bandwidth_hz / 2.0 / DESCRIPTOR.input_rate_hz;
    Ok(RealDecimator::new(&design_lowpass(AUDIO_TAPS, cutoff), 1))
}

fn waveform(p: &AmParams) -> Result<AmWaveform, ChannelError> {
    check_bandwidth(p)?;
    let mut waveform = AmWaveform::new(
        AmMode::FullCarrier {
            depth: f64::from(MODULATION_DEPTH),
        },
        p.bandwidth_hz / 2.0 / DESCRIPTOR.input_rate_hz,
    );
    waveform.audio_taps = AUDIO_TAPS;
    Ok(waveform)
}

fn demodulator(p: &AmParams) -> Result<AmDemod, ChannelError> {
    Ok(AmDemod::new(
        &waveform(p)?,
        &AmRx {
            predetection: false,
            ..AmRx::new(AmDetector::Envelope)
        },
    ))
}

impl ChannelRx for AmChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        Ok(Self {
            demod: demodulator(p)?,
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        self.demod = demodulator(p)?;
        Ok(())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.demod.process(iq, &mut out.audio_pcm);
        clamp_full_scale(&mut out.audio_pcm);
        if !out.audio_pcm.is_empty() {
            out.audio_rate = AUDIO_RATE;
        }
    }
}

const MODULATION_DEPTH: f32 = 0.8;

pub struct AmTx {
    queue: TxQueue<f32>,
    audio_lp: RealDecimator,
    filtered: Vec<f32>,
    burst: Burst,
}

impl ChannelTx for AmTx {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        Ok(Self {
            queue: TxQueue::new(DESCRIPTOR.type_id.as_str(), f64::from(AUDIO_RATE)),
            audio_lp: audio_lowpass(p)?,
            filtered: Vec::new(),
            burst: Burst::new(ctx.input_rate),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        self.audio_lp = audio_lowpass(params(&settings)?)?;
        Ok(())
    }

    fn submit(&mut self, payload: TxPayload) -> Result<(), ChannelError> {
        let TxPayload::Audio(pcm) = payload else {
            return Err(ChannelError::InvalidPayload(
                "am carries audio, not frames".to_owned(),
            ));
        };
        self.queue.accept(pcm.len())?;
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
            let modulated = (1.0 + MODULATION_DEPTH * audio) / (1.0 + MODULATION_DEPTH);
            *slot = Complex::new(envelope * modulated, 0.0);
            written += 1;
        }
        written
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{NfmParams, WfmParams};

    use super::*;
    use crate::{
        testgen::{burst, tone_audio},
        testutil::{am_iq, dominant_tone, rms, run_ragged, settings},
    };

    const RATE: f64 = 48_000.0;

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
        });
        let audio = run_ragged(&mut chan, &am_iq(RATE, 1_000.0, 0.5, 48_000));
        let window = &audio[4_000..16_000];
        let (freq, ratio) = dominant_tone(window, RATE);
        assert!((995.0..1_005.0).contains(&freq), "dominant {freq} Hz");
        assert!(ratio > 10.0, "tone-to-rest ratio {ratio}");
        let amplitude = rms(window);
        assert!((0.32..0.39).contains(&amplitude), "rms {amplitude}");
    }

    #[test]
    fn apply_reconfigures_bandwidth() {
        let mut chan = channel(AmParams {
            bandwidth_hz: 10_000.0,
        });
        chan.apply(settings(ChannelParams::Am(AmParams {
            bandwidth_hz: 6_000.0,
        })))
        .unwrap();
        let audio = run_ragged(&mut chan, &am_iq(RATE, 1_000.0, 0.5, 48_000));
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

    const fn ctx() -> ChannelCtx {
        ChannelCtx { input_rate: RATE }
    }

    fn tx_params() -> ChannelSettings {
        settings(ChannelParams::Am(AmParams {
            bandwidth_hz: 10_000.0,
        }))
    }

    fn transmitter() -> AmTx {
        AmTx::new(ctx(), tx_params()).unwrap()
    }

    #[test]
    fn tx_round_trips_a_tone_through_the_detector() {
        let mut tx = transmitter();
        tx.submit(TxPayload::Audio(tone_audio(1_000.0, 1.0, RATE, 24_000)))
            .unwrap();
        let iq = burst(&mut tx);
        assert_eq!(iq.len(), 24_000 + Burst::new(RATE).ramp_len());

        let mut rx = AmChannel::new(ctx(), tx_params()).unwrap();
        let audio = run_ragged(&mut rx, &iq);
        let window = &audio[4_000..20_000];
        let (freq, ratio) = dominant_tone(window, RATE);
        assert!((995.0..1_005.0).contains(&freq), "dominant {freq} Hz");
        assert!(ratio > 10.0, "tone-to-rest ratio {ratio}");
        let amplitude = rms(window);
        assert!((0.28..0.35).contains(&amplitude), "rms {amplitude}");
    }

    #[test]
    fn tx_keeps_the_envelope_between_the_trough_and_full_scale() {
        let mut tx = transmitter();
        tx.submit(TxPayload::Audio(tone_audio(1_000.0, 4.0, RATE, 24_000)))
            .unwrap();
        let iq = burst(&mut tx);
        let ramp = Burst::new(RATE).ramp_len();
        let keyed = &iq[ramp..iq.len() - ramp];
        let peak = keyed.iter().map(|s| s.norm()).fold(f32::MIN, f32::max);
        let trough = keyed.iter().map(|s| s.norm()).fold(f32::MAX, f32::min);
        assert!((peak - 1.0).abs() < 0.02, "peak envelope {peak}");
        let floor = (1.0 - MODULATION_DEPTH) / (1.0 + MODULATION_DEPTH);
        assert!(trough > floor - 0.02, "trough envelope {trough}");
    }

    #[test]
    fn tx_ramps_the_burst_edges() {
        let mut tx = transmitter();
        tx.submit(TxPayload::Audio(vec![0.0; 4_800])).unwrap();
        let iq = burst(&mut tx);
        let ramp = Burst::new(RATE).ramp_len();
        assert!(iq[0].norm() < 0.01, "first sample {}", iq[0].norm());
        assert!(
            iq[iq.len() - 1].norm() < 0.01,
            "last sample {}",
            iq[iq.len() - 1].norm()
        );
        for (k, s) in iq[ramp..iq.len() - ramp].iter().enumerate() {
            let unmodulated = 1.0 / (1.0 + MODULATION_DEPTH);
            assert!(
                (s.norm() - unmodulated).abs() < 1e-5,
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
        assert_eq!(block[0], Complex::new(9.0, 9.0));
    }

    #[test]
    fn tx_rejects_a_frame_payload_and_a_backlog_past_the_bound() {
        let mut tx = transmitter();
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
    fn tx_rejects_mismatched_params_and_input_rate() {
        let built = AmTx::new(ctx(), settings(ChannelParams::Nfm(NfmParams::default())));
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
        let built = AmTx::new(
            ChannelCtx {
                input_rate: 240_000.0,
            },
            tx_params(),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }
}
