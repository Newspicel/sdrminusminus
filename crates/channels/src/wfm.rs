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
const AUDIO_CUTOFF_HZ: f64 = 15_000.0;
const DECIM_FACTOR: usize = 5;
const DECIM_TAPS: usize = 199;
const CHANNEL_TAPS: usize = 65;

const PILOT_HZ: f64 = 19_000.0;
const PILOT_CUTOFF_HZ: f64 = 400.0;
const PILOT_STAGES: usize = 3;
const PILOT_LOOP_BW_HZ: f64 = 30.0;
const PILOT_RANGE_HZ: f64 = 120.0;
const LOCK_ON: f32 = 0.6;
const LOCK_OFF: f32 = 0.4;
const BLEND_TAU_S: f64 = 0.05;

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
    decoder_kind: Some("rds".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct WfmChannel {
    demod: AngleDemod,
    deemphasis: Deemphasis,
    decim: RealDecimator,
    demod_buf: Vec<f32>,
    sum: Vec<f32>,
    stereo: Option<StereoDemux>,
    rds: RdsDecoder,
}

struct StereoDemux {
    pilot: Nco,
    filter: ComplexOnePole,
    pll: Pll,
    difference: Vec<f32>,
    side: Vec<f32>,
    decim: RealDecimator,
    deemphasis: Deemphasis,
    blend: f32,
    blend_coeff: f32,
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

    fn two_tone_station(pilot: bool) -> Vec<Complex<f32>> {
        let left = tone_audio(LEFT_HZ, 1.0, RATE, RUN_SAMPLES);
        let right = tone_audio(RIGHT_HZ, 1.0, RATE, RUN_SAMPLES);
        stereo_transmission(&left, &right, pilot, RATE)
    }

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

        let window = &audio[20_000..140_000];
        let (freq, ratio) = dominant_tone(window, f64::from(AUDIO_RATE));
        assert!((995.0..1_005.0).contains(&freq), "dominant {freq} Hz");
        assert!(ratio > 10.0, "tone-to-rest ratio {ratio}");
        let amplitude = rms(window);
        assert!((0.26..0.34).contains(&amplitude), "rms {amplitude}");
    }

    #[test]
    fn apply_leaves_the_rds_picture_standing() {
        let mut chan = channel(50.0);
        let (_, events) = run_collecting(&mut chan, &transmission(&station(), 3.5, None, RATE));
        let before = last_update(&events);
        assert_eq!(before.ps.as_deref(), Some("WFM+RDS"));

        chan.apply(settings(wfm_params(75.0, false))).unwrap();

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

    #[test]
    fn retuning_drops_the_previous_station() {
        let mut chan = channel(50.0);
        let (_, events) = run_collecting(&mut chan, &transmission(&station(), 3.5, None, RATE));
        let before = last_update(&events);
        assert_eq!(before.ps.as_deref(), Some("WFM+RDS"));
        assert!(before.groups >= 5, "groups accreted: {}", before.groups);

        chan.retuned();

        let (_, events) = run_collecting(&mut chan, &transmission(&station(), 3.5, None, RATE));
        let after = last_update(&events);
        assert_eq!(after.ps.as_deref(), Some("WFM+RDS"));
        assert!(
            after.groups <= before.groups,
            "the group counter survived the retune: {} then {}",
            before.groups,
            after.groups
        );
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

    #[test]
    fn the_pilot_does_not_reach_the_audio() {
        let mut chan = channel(50.0);
        let audio = run_ragged(&mut chan, &two_tone_station(true));
        let settled = &audio[audio.len() / 2..];
        let pilot = tone_power(settled, PILOT_HZ, f64::from(AUDIO_RATE));
        assert!(pilot < 1e-6, "pilot share of the mono audio {pilot}");
    }

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
        let imbalance = (rms(&left) - rms(&right)).abs() / rms(&left);
        assert!(imbalance < 0.05, "channel imbalance {imbalance}");
    }
}
