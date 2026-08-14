//! WFM: 240 kHz IQ → quadrature discriminator → 5:1 decimate to 48 kHz → de-emphasis.
//! With `stereo` set, the composite is also demultiplexed against the 19 kHz pilot into the
//! L−R difference signal, and the channel's audio leaves as interleaved L/R.
//! The composite is tapped off into [`RdsDecoder`] as well: RDS is the same signal's 57 kHz
//! subcarrier, so there is nothing to switch on — a station without it simply decodes nothing.
use std::{f64::consts::FRAC_1_SQRT_2, sync::LazyLock};

use num_complex::Complex;
use sdrmm_dsp::{
    ComplexOnePole, Decimator, Deemphasis, Nco, Pll, RealDecimator, design_lowpass, one_pole_coeff,
};
use sdrmm_modem::analog::{AngleDemod, AngleDetector, AngleKind, AngleParams, AngleRx};
use sdrmm_wire::{ChannelDescriptor, ChannelParams, ChannelSettings, WfmParams};

use crate::{
    AUDIO_RATE, ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx,
    check_input_rate, clamp_full_scale, rds::RdsDecoder,
};

const DEVIATION_HZ: f64 = 75_000.0;
/// Audio ends at 15 kHz; everything above (pilot, stereo subcarrier, RDS) is cut.
const AUDIO_CUTOFF_HZ: f64 = 15_000.0;
const DECIM_FACTOR: usize = 5;
/// A Blackman lowpass's transition half-width is 2.75/taps (see `dsp::fir`), so 199 taps put
/// the stopband edge at 18.3 kHz — below the pilot. Fewer taps leave 19 kHz in the transition
/// band, and with de-emphasis no longer damping the composite ahead of this filter, that is
/// audible pilot whine in the mono sum.
const DECIM_TAPS: usize = 199;
/// The ±100 kHz channel edge sits at 0.417 of the 240 kHz rate; 65 taps put the stopband
/// just inside Nyquist while keeping the per-sample cost sane at this rate.
const CHANNEL_TAPS: usize = 65;

/// Stereo pilot; the difference subcarrier is its second harmonic (ITU-R BS.450).
const PILOT_HZ: f64 = 19_000.0;
/// Per-stage corner of the cascade that isolates the mixed-down pilot. Its nearest composite
/// neighbours are 4 kHz away (audio ends at 15 kHz, the difference subcarrier starts at
/// 23 kHz), which three stages at this corner put ~55 dB down.
const PILOT_CUTOFF_HZ: f64 = 400.0;
const PILOT_STAGES: usize = 3;
/// Pilot loop bandwidth and pull-in range, in Hz. The pilot is transmitter-locked to the
/// subcarrier, so the loop only has to track the receiver's own clock error — narrow enough to
/// ignore what leaks past the pilot filter, wide enough to acquire in a few tens of ms.
const PILOT_LOOP_BW_HZ: f64 = 30.0;
const PILOT_RANGE_HZ: f64 = 120.0;
/// Lock quality that turns the difference signal on, and the lower one that turns it off
/// again. Hysteresis, so a pilot flickering at the threshold cannot toggle the matrix.
const LOCK_ON: f32 = 0.6;
const LOCK_OFF: f32 = 0.4;
/// Time constant of the ramp between mono and stereo, in seconds. Long enough that neither
/// transition clicks, short enough that a station change does not leave the difference signal
/// misapplied for audibly long.
const BLEND_TAU_S: f64 = 0.05;

/// The library detector this channel is an attachment to: `sdrmm_modem::analog`'s quadrature
/// discriminator alone, at broadcast FM's ±75 kHz. The engine's own predetection and audio
/// filters are off — the host runtime supplies the first, and what follows the discriminator is
/// the composite, which this channel demultiplexes itself.
fn discriminator(rate: f64) -> AngleDemod {
    let params = AngleParams::new(
        AngleKind::Fm {
            deviation: DEVIATION_HZ / rate,
        },
        AUDIO_CUTOFF_HZ / rate,
    );
    AngleDemod::new(
        &params,
        &AngleRx::detector_only(AngleDetector::Discriminator),
    )
}

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "wfm".to_owned(),
    name: "WFM (broadcast)".to_owned(),
    bandwidth_hz: 200_000.0,
    input_rate_hz: 240_000.0,
    has_audio: true,
    // WFM is the only channel that is both: audio out, and RDS frames.
    decoder_kind: Some("rds".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct WfmChannel {
    demod: AngleDemod,
    deemphasis: Deemphasis,
    decim: RealDecimator,
    demod_buf: Vec<f32>,
    /// Sum signal at 48 kHz; only the stereo path needs it out of line, because the mono path
    /// decimates straight into the channel's output.
    sum: Vec<f32>,
    /// Present only while `stereo` is set. Holds the pilot loop and the difference path.
    stereo: Option<StereoDemux>,
    rds: RdsDecoder,
}

/// Pilot recovery and L−R demodulation (ITU-R BS.450 pilot-tone system).
///
/// The pilot is mixed to DC and isolated there — a 400 Hz-wide filter at 19 kHz would need
/// thousands of FIR taps, while at DC three complex one-pole sections do it — and a PLL tracks
/// what is left. Rebuilding the analytic pilot as `conj(mix)·reference` puts its phase back at
/// 19 kHz sample-accurately, and squaring it lands on the subcarrier: the multiplex's pilot is
/// `cos(θ)`, so the difference subcarrier the standard's zero-crossing rule prescribes is
/// `−sin(2θ) = −Im(P²)`. A quadrature slip there collapses the separation; a sign slip swaps
/// the channels.
struct StereoDemux {
    pilot: Nco,
    filter: ComplexOnePole,
    pll: Pll,
    /// L−R at the composite rate, before the audio decimation, and at 48 kHz after it.
    difference: Vec<f32>,
    side: Vec<f32>,
    decim: RealDecimator,
    /// Same de-emphasis as the sum path: applying it to sum and difference separately is
    /// identical to applying it to L and R, and keeps both filters on contiguous buffers.
    deemphasis: Deemphasis,
    /// How much of the difference signal reaches the matrix, ramped rather than switched.
    blend: f32,
    blend_coeff: f32,
    /// What `blend` is ramping towards: the hysteretic verdict on the pilot lock.
    target: f32,
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
    Ok(Deemphasis::new(f64::from(AUDIO_RATE), p.deemphasis_us))
}

/// The 240 kHz → 48 kHz audio decimation, built the same way for the sum and the difference
/// signal: identical filters fed identical block lengths stay sample-aligned, which is what
/// lets the matrix pair them.
fn audio_decimator(rate: f64) -> RealDecimator {
    RealDecimator::new(
        &design_lowpass(DECIM_TAPS, AUDIO_CUTOFF_HZ / rate),
        DECIM_FACTOR,
    )
}

impl StereoDemux {
    fn new(rate: f64, deemphasis: Deemphasis) -> Self {
        Self {
            pilot: Nco::new(PILOT_HZ as f32, rate as f32),
            filter: ComplexOnePole::new(rate, PILOT_CUTOFF_HZ, PILOT_STAGES),
            pll: Pll::new(
                PILOT_LOOP_BW_HZ / rate,
                FRAC_1_SQRT_2,
                0.0,
                PILOT_RANGE_HZ / rate,
            ),
            difference: Vec::new(),
            side: Vec::new(),
            decim: audio_decimator(rate),
            deemphasis,
            blend: 0.0,
            blend_coeff: one_pole_coeff(f64::from(AUDIO_RATE), BLEND_TAU_S),
            target: 0.0,
        }
    }

    /// Interleave L/R into `out` from this block's composite and the sum path's 48 kHz output.
    fn process(&mut self, composite: &[f32], sum: &[f32], out: &mut Vec<f32>) {
        self.demodulate(composite);
        self.decim.process(&self.difference, &mut self.side);
        self.deemphasis.process(&mut self.side);

        let lock = self.pll.lock();
        if lock > LOCK_ON {
            self.target = 1.0;
        } else if lock < LOCK_OFF {
            self.target = 0.0;
        }
        // Both decimators are the same filter fed the same block, so they emit the same count.
        debug_assert_eq!(sum.len(), self.side.len(), "stereo paths drifted apart");
        out.clear();
        out.reserve(2 * sum.len());
        for (&mid, &side) in sum.iter().zip(&self.side) {
            self.blend += self.blend_coeff * (self.target - self.blend);
            let side = side * self.blend;
            out.push(mid + side);
            out.push(mid - side);
        }
    }

    /// L−R at the composite rate: the multiplex times the recovered 38 kHz subcarrier.
    fn demodulate(&mut self, composite: &[f32]) {
        self.difference.clear();
        self.difference.reserve(composite.len());
        for &sample in composite {
            let carrier = self.pilot.next_sample();
            let baseband = self
                .filter
                .process(Complex::new(sample, 0.0) * carrier.conj());
            let reference = self.pll.process(baseband);
            let analytic = carrier * reference;
            let subcarrier = analytic * analytic;
            // ×2 because the product of two unit sinusoids halves the amplitude.
            self.difference.push(-2.0 * subcarrier.im * sample);
        }
    }
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
            demod: discriminator(ctx.input_rate),
            decim: audio_decimator(ctx.input_rate),
            demod_buf: Vec::new(),
            sum: Vec::new(),
            stereo: p
                .stereo
                .then(|| StereoDemux::new(ctx.input_rate, deemphasis.clone())),
            deemphasis,
            rds: RdsDecoder::new(ctx.input_rate),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        let deemphasis = deemphasis(p)?;
        self.deemphasis = deemphasis.clone();
        let rate = DESCRIPTOR.input_rate_hz;
        if p.stereo != self.stereo.is_some() {
            // A freshly built decimator has no history, so it would emit fewer samples for the
            // first block than the running one: restart both together, or the matrix pairs
            // sum and difference samples from different instants for the rest of the stream.
            self.decim = audio_decimator(rate);
            self.stereo = p.stereo.then(|| StereoDemux::new(rate, deemphasis));
        } else if let Some(stereo) = &mut self.stereo {
            stereo.deemphasis = deemphasis;
        }
        Ok(())
    }

    fn retuned(&mut self) {
        self.rds.reset();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.demod.process(iq, &mut self.demod_buf);
        self.rds.process(&self.demod_buf, &mut out.events);
        match &mut self.stereo {
            None => {
                self.decim.process(&self.demod_buf, &mut out.audio_pcm);
                self.deemphasis.process(&mut out.audio_pcm);
            }
            Some(stereo) => {
                self.decim.process(&self.demod_buf, &mut self.sum);
                self.deemphasis.process(&mut self.sum);
                stereo.process(&self.demod_buf, &self.sum, &mut out.audio_pcm);
            }
        }
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
        testgen::{
            rds::{Station, transmission},
            tone_audio,
            wfm::transmission as stereo_transmission,
        },
        testutil::{dominant_tone, fm_iq, rms, run_ragged, settings, split_stereo, tone_power},
    };

    const RATE: f64 = 240_000.0;
    /// One second of programme: long enough for the pilot loop to lock and the stereo blend to
    /// ramp in, with half a second of settled audio left to measure.
    const RUN_SAMPLES: usize = 240_000;
    const LEFT_HZ: f64 = 1_000.0;
    const RIGHT_HZ: f64 = 3_000.0;

    fn wfm_params(deemphasis_us: f32, stereo: bool) -> ChannelParams {
        ChannelParams::Wfm(WfmParams {
            deemphasis_us,
            stereo,
        })
    }

    fn channel(deemphasis_us: f32) -> WfmChannel {
        WfmChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(wfm_params(deemphasis_us, false)),
        )
        .unwrap()
    }

    fn stereo_channel() -> WfmChannel {
        WfmChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(wfm_params(50.0, true)),
        )
        .unwrap()
    }

    /// A station carrying a different tone on each channel, `pilot` false making it mono.
    fn two_tone_station(pilot: bool) -> Vec<Complex<f32>> {
        let left = tone_audio(LEFT_HZ, 1.0, RATE, RUN_SAMPLES);
        let right = tone_audio(RIGHT_HZ, 1.0, RATE, RUN_SAMPLES);
        stereo_transmission(&left, &right, pilot, RATE)
    }

    /// The settled half of each channel of an interleaved run.
    fn settled_channels(audio: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let (left, right) = split_stereo(audio);
        let from = left.len() / 2;
        (left[from..].to_vec(), right[from..].to_vec())
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
        chan.apply(settings(wfm_params(75.0, false))).unwrap();
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

    /// The decoder always runs, so a station carrying no subcarrier must stay silent on the
    /// event path rather than report a picture assembled out of noise.
    #[test]
    fn a_station_without_rds_produces_no_events() {
        let mut chan = channel(50.0);
        let iq = two_tone_station(true);
        let (audio, events) = run_collecting(&mut chan, &iq);
        assert!(events.is_empty(), "{} events without rds", events.len());
        assert_eq!(audio.len(), iq.len() / DECIM_FACTOR);
    }

    #[test]
    fn rds_decodes_the_station_while_the_audio_still_demodulates() {
        let mut chan = channel(50.0);
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

    /// A settings change is not a station change: what the decoder has accreted must survive
    /// one, or every touch of the de-emphasis knob would blank the panel. Told apart from a
    /// reset by the group counter, which only `retuned` may zero.
    #[test]
    fn apply_leaves_the_rds_picture_standing() {
        let mut chan = channel(50.0);
        let (_, events) = run_collecting(&mut chan, &transmission(&station(), 3.5, None, RATE));
        let before = last_update(&events);
        assert_eq!(before.ps.as_deref(), Some("WFM+RDS"));

        chan.apply(settings(wfm_params(75.0, false))).unwrap();

        // A different PS, so the decoder has something to report on the far side of `apply`;
        // an unchanged station emits nothing, having nothing to say.
        let renamed = Station {
            ps: "RENAMED".to_owned(),
            ..station()
        };
        let (_, events) = run_collecting(&mut chan, &transmission(&renamed, 3.5, None, RATE));
        let after = last_update(&events);
        assert_eq!(after.ps.as_deref(), Some("RENAMED"));
        assert!(
            after.groups > before.groups,
            "the group counter restarted across apply: {} then {}",
            before.groups,
            after.groups
        );
    }

    /// A retune reaches the channel through `ChannelRx::retuned`, not `apply` — the engine
    /// sends no settings command for an offset-only patch, so testing this through `apply`
    /// would prove nothing about the path production takes (see `DspCommand::Retune`).
    #[test]
    fn retuning_drops_the_previous_station() {
        let mut chan = channel(50.0);
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

    #[test]
    fn stereo_output_is_two_interleaved_channels() {
        let mut chan = stereo_channel();
        let iq = two_tone_station(true);
        let audio = run_ragged(&mut chan, &iq);
        assert_eq!(audio.len(), 2 * (iq.len() / DECIM_FACTOR));
    }

    /// The separation test: each channel must carry its own tone and almost none of the
    /// other's. A quadrature slip in the recovered subcarrier collapses this to ~0 dB; a sign
    /// slip swaps which tone lands where, which is why both are named.
    #[test]
    fn stereo_separates_the_two_programme_channels() {
        let mut chan = stereo_channel();
        let audio = run_ragged(&mut chan, &two_tone_station(true));
        let (left, right) = settled_channels(&audio);

        for (channel, own, other) in [(&left, LEFT_HZ, RIGHT_HZ), (&right, RIGHT_HZ, LEFT_HZ)] {
            let (freq, ratio) = dominant_tone(channel, f64::from(AUDIO_RATE));
            assert!((own - 5.0..own + 5.0).contains(&freq), "dominant {freq} Hz");
            assert!(ratio > 10.0, "tone-to-rest ratio {ratio}");
            let separation = tone_power(channel, own, f64::from(AUDIO_RATE))
                / tone_power(channel, other, f64::from(AUDIO_RATE));
            assert!(
                separation > 100.0,
                "{own} Hz channel carries {other} Hz only {} dB down",
                10.0 * separation.log10()
            );
        }
        assert!(
            (0.27..0.34).contains(&rms(&left)),
            "left rms {}",
            rms(&left)
        );
        assert!(
            (0.20..0.26).contains(&rms(&right)),
            "right rms {}",
            rms(&right)
        );
    }

    /// A station with no pilot still plays: the difference signal is gated off, so both
    /// channels carry the mono sum — bit for bit, not merely close.
    #[test]
    fn a_mono_station_plays_the_same_audio_on_both_channels() {
        let mut chan = stereo_channel();
        let audio = run_ragged(&mut chan, &two_tone_station(false));
        let (left, right) = split_stereo(&audio);
        assert_eq!(left, right, "unlocked pilot leaked into the matrix");
        let settled = &left[left.len() / 2..];
        let shares: Vec<f64> = [LEFT_HZ, RIGHT_HZ]
            .iter()
            .map(|&freq| tone_power(settled, freq, f64::from(AUDIO_RATE)))
            .collect();
        assert!(
            shares.iter().all(|&s| s > 0.3) && shares.iter().sum::<f64>() > 0.9,
            "tone shares of the sum signal {shares:?}"
        );
    }

    /// The pilot sits 4 kHz above the audio band and must not survive the decimation filter:
    /// nothing de-emphasizes the composite ahead of it any more.
    #[test]
    fn the_pilot_does_not_reach_the_audio() {
        let mut chan = channel(50.0);
        let audio = run_ragged(&mut chan, &two_tone_station(true));
        let settled = &audio[audio.len() / 2..];
        let pilot = tone_power(settled, PILOT_HZ, f64::from(AUDIO_RATE));
        assert!(pilot < 1e-6, "pilot share of the mono audio {pilot}");
    }

    /// Toggling stereo is a params patch, not a pipeline rebuild, so the switch happens inside
    /// a running channel — and both layouts must come out intact on either side of it.
    #[test]
    fn stereo_can_be_switched_on_and_off_while_running() {
        let iq = two_tone_station(true);
        let mut chan = channel(50.0);
        let mono = run_ragged(&mut chan, &iq);
        assert_eq!(mono.len(), iq.len() / DECIM_FACTOR);

        chan.apply(settings(wfm_params(50.0, true))).unwrap();
        let audio = run_ragged(&mut chan, &iq);
        assert_eq!(audio.len(), 2 * (iq.len() / DECIM_FACTOR));
        let (left, _) = settled_channels(&audio);
        let separation = tone_power(&left, LEFT_HZ, f64::from(AUDIO_RATE))
            / tone_power(&left, RIGHT_HZ, f64::from(AUDIO_RATE));
        assert!(separation > 100.0, "separation after switching on");

        chan.apply(settings(wfm_params(50.0, false))).unwrap();
        let audio = run_ragged(&mut chan, &iq);
        assert_eq!(audio.len(), iq.len() / DECIM_FACTOR);
        let (freq, _) = dominant_tone(&audio[audio.len() / 2..], f64::from(AUDIO_RATE));
        assert!((995.0..1_005.0).contains(&freq), "mono dominant {freq} Hz");
    }

    /// RDS rides on the third harmonic of the same pilot the stereo matrix locks to. Running
    /// both at once must leave each intact — and an RDS station's composite carries a pilot
    /// but no difference signal, so its two channels are the same programme.
    #[test]
    fn rds_and_stereo_run_on_the_same_pilot() {
        let mut chan = stereo_channel();
        let (audio, events) = run_collecting(
            &mut chan,
            &transmission(&station(), 3.5, Some(1_000.0), RATE),
        );

        let update = last_update(&events);
        assert_eq!(update.ps.as_deref(), Some("WFM+RDS"));
        assert_eq!(update.block_errors, 0);

        let (left, right) = settled_channels(&audio);
        let (freq, ratio) = dominant_tone(&left, f64::from(AUDIO_RATE));
        assert!((995.0..1_005.0).contains(&freq), "dominant {freq} Hz");
        assert!(ratio > 10.0, "tone-to-rest ratio {ratio}");
        // No difference signal on the air, so what the matrix adds and subtracts is only what
        // leaked into it: the two channels must stay within a fraction of a dB of each other.
        let imbalance = (rms(&left) - rms(&right)).abs() / rms(&left);
        assert!(imbalance < 0.05, "channel imbalance {imbalance}");
    }
}
