//! WFM mono: 240 kHz IQ → quadrature discriminator → de-emphasis → 5:1 decimate to 48 kHz.
//! With `rds` set, the discriminator's composite is also tapped off — ahead of de-emphasis and
//! the audio decimation, both of which destroy the 57 kHz subcarrier — into [`RdsDecoder`].

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, Deemphasis, FmDemod, RealDecimator, design_lowpass};
use sdrmm_wire::{ChannelDescriptor, ChannelParams, ChannelSettings, WfmParams};

use crate::{
    AUDIO_RATE, ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx,
    check_input_rate, clamp_full_scale, rds::RdsDecoder,
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
    has_audio: true,
    // WFM is the only channel that is both: audio out, and RDS frames when `rds` is set.
    decoder_kind: Some("rds".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct WfmChannel {
    demod: FmDemod,
    deemphasis: Deemphasis,
    decim: RealDecimator,
    demod_buf: Vec<f32>,
    /// Present only while `rds` is set — a disabled decoder costs neither state nor cycles.
    rds: Option<RdsDecoder>,
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
        let p = params(&settings)?;
        let deemphasis = deemphasis(p)?;
        Ok(Self {
            demod: FmDemod::new(ctx.input_rate, DEVIATION_HZ),
            deemphasis,
            decim: RealDecimator::new(
                &design_lowpass(DECIM_TAPS, AUDIO_CUTOFF_HZ / ctx.input_rate),
                DECIM_FACTOR,
            ),
            demod_buf: Vec::new(),
            rds: p.rds.then(|| RdsDecoder::new(ctx.input_rate)),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        self.deemphasis = deemphasis(p)?;
        if !p.rds {
            self.rds = None;
        } else if self.rds.is_none() {
            self.rds = Some(RdsDecoder::new(DESCRIPTOR.input_rate_hz));
        }
        Ok(())
    }

    fn retuned(&mut self) {
        if let Some(rds) = &mut self.rds {
            rds.reset();
        }
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.demod.process(iq, &mut self.demod_buf);
        if let Some(rds) = &mut self.rds {
            rds.process(&self.demod_buf, &mut out.events);
        }
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
    use sdrmm_wire::{DecoderEvent, RdsUpdate, SsbParams};

    use super::*;
    use crate::{
        testgen::rds::{Station, transmission},
        testutil::{dominant_tone, fm_iq, rms, run_ragged, settings},
    };

    const RATE: f64 = 240_000.0;

    fn channel(deemphasis_us: f32) -> WfmChannel {
        WfmChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Wfm(WfmParams {
                deemphasis_us,
                rds: false,
            })),
        )
        .unwrap()
    }

    fn rds_settings(rds: bool, offset_hz: f64) -> ChannelSettings {
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            params: ChannelParams::Wfm(WfmParams {
                deemphasis_us: 50.0,
                rds,
            }),
        }
    }

    fn station() -> Station {
        Station {
            pi: 0x1234,
            ps: "WFM+RDS".to_owned(),
            radiotext: "broadcast fm with data".to_owned(),
            pty: 4,
            tp: false,
            ta: false,
            music: true,
            alt_freqs_hz: vec![98_500_000.0],
        }
    }

    /// Like `run_ragged`, but keeping the decoder events the blocks produced too.
    fn run_collecting(chan: &mut WfmChannel, iq: &[Complex<f32>]) -> (Vec<f32>, Vec<DecoderEvent>) {
        let mut out = ChannelOutputs::default();
        let (mut audio, mut events) = (Vec::new(), Vec::new());
        let mut pos = 0;
        for len in [997usize, 1, 4_096, 65, 2_048, 7, 1_024].iter().cycle() {
            if pos >= iq.len() {
                break;
            }
            let end = (pos + len).min(iq.len());
            out.reset();
            chan.process(&iq[pos..end], &mut out);
            audio.extend_from_slice(&out.audio_pcm);
            events.append(&mut out.events);
            pos = end;
        }
        (audio, events)
    }

    fn last_update(events: &[DecoderEvent]) -> RdsUpdate {
        match events.last() {
            Some(DecoderEvent::Rds(update)) => update.clone(),
            other => panic!("expected an rds update, got {other:?}"),
        }
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
            rds: false,
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

    #[test]
    fn a_station_with_rds_disabled_produces_no_events() {
        let mut chan = channel(50.0);
        let iq = transmission(&station(), 2.0, Some(1_000.0), RATE);
        let (audio, events) = run_collecting(&mut chan, &iq);
        assert!(events.is_empty(), "{} events with rds off", events.len());
        assert_eq!(audio.len(), iq.len() / DECIM_FACTOR);
    }

    #[test]
    fn rds_decodes_the_station_while_the_audio_still_demodulates() {
        let mut chan =
            WfmChannel::new(ChannelCtx { input_rate: RATE }, rds_settings(true, 0.0)).unwrap();
        let (audio, events) = run_collecting(
            &mut chan,
            &transmission(&station(), 3.5, Some(1_000.0), RATE),
        );

        let update = last_update(&events);
        assert_eq!(update.pi.as_deref(), Some("1234"));
        assert_eq!(update.ps.as_deref(), Some("WFM+RDS"));
        assert_eq!(update.radiotext.as_deref(), Some("broadcast fm with data"));
        assert_eq!(update.pty_name.as_deref(), Some("Sport"));
        assert_eq!(update.alt_freqs_hz, vec![98_500_000.0]);
        assert_eq!(update.block_errors, 0);

        // The audio path is untouched by the tap: the 1 kHz tone is still the only thing in
        // it, at the level 45 % deviation through 50 µs de-emphasis gives.
        let window = &audio[20_000..140_000];
        let (freq, ratio) = dominant_tone(window, f64::from(AUDIO_RATE));
        assert!((995.0..1_005.0).contains(&freq), "dominant {freq} Hz");
        assert!(ratio > 10.0, "tone-to-rest ratio {ratio}");
        let amplitude = rms(window);
        assert!((0.26..0.34).contains(&amplitude), "rms {amplitude}");
    }

    #[test]
    fn apply_starts_and_stops_the_rds_decoder() {
        let mut chan = channel(50.0);
        let iq = transmission(&station(), 3.5, None, RATE);

        chan.apply(rds_settings(true, 0.0)).unwrap();
        let (_, events) = run_collecting(&mut chan, &iq);
        assert_eq!(last_update(&events).ps.as_deref(), Some("WFM+RDS"));

        chan.apply(rds_settings(false, 0.0)).unwrap();
        let (_, events) = run_collecting(&mut chan, &iq);
        assert!(
            events.is_empty(),
            "{} events after rds was turned off",
            events.len()
        );
    }

    /// A retune reaches the channel through `ChannelRx::retuned`, not `apply` — the engine
    /// sends no settings command for an offset-only patch, so testing this through `apply`
    /// would prove nothing about the path production takes (see `DspCommand::Retune`).
    #[test]
    fn retuning_drops_the_previous_station() {
        let mut chan =
            WfmChannel::new(ChannelCtx { input_rate: RATE }, rds_settings(true, 0.0)).unwrap();
        let (_, events) = run_collecting(&mut chan, &transmission(&station(), 3.5, None, RATE));
        let before = last_update(&events);
        assert_eq!(before.ps.as_deref(), Some("WFM+RDS"));
        assert!(before.groups >= 5, "groups accreted: {}", before.groups);

        chan.retuned();

        // Long enough for the new station's picture to complete, so the assertion is about
        // what carried over rather than about an empty event list.
        let (_, events) = run_collecting(&mut chan, &transmission(&station(), 3.5, None, RATE));
        let after = last_update(&events);
        assert_eq!(after.ps.as_deref(), Some("WFM+RDS"));
        assert!(
            after.groups <= before.groups,
            "the group counter survived the retune: {} then {}",
            before.groups,
            after.groups
        );
        // The first event after a retune must describe the new station from scratch: a PS
        // that survived the reset would be reported before a single group had been read.
        let first = events
            .iter()
            .find_map(|e| match e {
                DecoderEvent::Rds(u) => Some(u),
                _ => None,
            })
            .expect("the new station reports");
        assert!(
            first.groups >= 1,
            "an update was emitted before any group was decoded"
        );
    }
}
