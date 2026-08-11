//! NFM voice: 48 kHz IQ → quadrature discriminator → voice-band lowpass. RF selectivity at
//! `bandwidth_hz` is the host's channel filter (see [`crate::channel_filter`]); here the
//! bandwidth only sets the deviation the discriminator is scaled to.
//!
//! The channel also reads the subaudible signalling under the voice — CTCSS or DCS — and will
//! gate the audio on it (see [`crate::tone_squelch`]). That path only exists when it is asked
//! for: with [`NfmToneMode::Off`] the audio is what it always was, flat to DC.
//!
//! [`NfmTx`] is the modulator that pairs with it. Neither carries pre- or de-emphasis: the two
//! have to agree, and a flat pair is the one that round-trips.

use std::{f64::consts::TAU, sync::LazyLock};

use num_complex::Complex;
use sdrmm_dsp::{Decimator, FmDemod, Highpass, RealDecimator, design_lowpass};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, NfmParams, NfmToneMode,
    ToneSquelchStatus,
};

use crate::{
    AUDIO_RATE, ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, ChannelTx,
    TxPayload, check_input_rate, clamp_full_scale,
    tone_squelch::{AUDIO_CORNER_HZ, ToneSquelch, is_standard_ctcss, is_standard_dcs},
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
    decoder_kind: Some("tone".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct NfmChannel {
    demod: FmDemod,
    audio_lp: RealDecimator,
    demod_buf: Vec<f32>,
    /// `None` while [`NfmToneMode::Off`] — the subaudible path costs a decimation chain and
    /// fifty correlators, and a channel that was not asked for it pays neither.
    tone: Option<Tone>,
}

/// Everything the tone modes add to the channel: the detector, the highpass that keeps what it
/// detects out of the audio, and the last status reported so that only changes are.
struct Tone {
    mode: NfmToneMode,
    ctcss_hz: Option<f64>,
    dcs_code: Option<u16>,
    detector: ToneSquelch,
    highpass: Highpass,
    reported: ToneSquelchStatus,
}

impl Tone {
    fn new(p: &NfmParams) -> Self {
        Self {
            mode: p.tone_mode,
            ctcss_hz: p.ctcss_hz,
            dcs_code: p.dcs_code,
            detector: ToneSquelch::new(),
            highpass: Highpass::new(DESCRIPTOR.input_rate_hz, AUDIO_CORNER_HZ),
            // What a channel with nothing under it computes on its first block, so a silent
            // start says nothing rather than announcing silence.
            reported: ToneSquelchStatus {
                ctcss_hz: None,
                dcs_code: None,
                open: p.tone_mode == NfmToneMode::Detect,
            },
        }
    }

    fn configure(&mut self, p: &NfmParams) {
        self.mode = p.tone_mode;
        self.ctcss_hz = p.ctcss_hz;
        self.dcs_code = p.dcs_code;
    }

    fn reset(&mut self) {
        self.detector.reset();
        self.highpass.reset();
    }

    /// Read the subaudible band, take it out of `demodulated`, and report the gate.
    fn step(&mut self, demodulated: &mut [f32], out: &mut ChannelOutputs) -> bool {
        let heard = self.detector.process(demodulated);
        self.highpass.process(demodulated);
        let open = match self.mode {
            // Detect never gates: it answers "what does this repeater use?", and muting the
            // audio until the answer arrived would make that question unaskable.
            NfmToneMode::Off | NfmToneMode::Detect => true,
            NfmToneMode::Ctcss => match (heard.ctcss_hz, self.ctcss_hz) {
                (Some(heard), Some(want)) => (heard - want).abs() < 0.05,
                _ => false,
            },
            NfmToneMode::Dcs => heard.dcs_code.is_some() && heard.dcs_code == self.dcs_code,
        };
        let status = ToneSquelchStatus {
            ctcss_hz: heard.ctcss_hz,
            dcs_code: heard.dcs_code,
            open,
        };
        if status != self.reported {
            self.reported = status.clone();
            out.events.push(DecoderEvent::Tone(status));
        }
        open
    }
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

/// A tone the detector does not search for could never open the gate, so it is refused where
/// the operator set it rather than left to look like a dead channel.
fn check_params(p: &NfmParams) -> Result<(), ChannelError> {
    check_bandwidth(p)?;
    match p.tone_mode {
        NfmToneMode::Ctcss if !p.ctcss_hz.is_some_and(is_standard_ctcss) => {
            Err(ChannelError::InvalidSettings(format!(
                "nfm ctcss squelch needs one of the 50 standard tones, got {:?}",
                p.ctcss_hz
            )))
        }
        NfmToneMode::Dcs if !p.dcs_code.is_some_and(is_standard_dcs) => {
            Err(ChannelError::InvalidSettings(format!(
                "nfm dcs squelch needs one of the 83 standard codes, got {:?}",
                p.dcs_code
            )))
        }
        _ => Ok(()),
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
        check_params(p)?;
        Ok(Self {
            demod: FmDemod::new(ctx.input_rate, deviation_hz(p)),
            audio_lp: audio_lowpass(),
            demod_buf: Vec::new(),
            tone: (p.tone_mode != NfmToneMode::Off).then(|| Tone::new(p)),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        check_params(p)?;
        self.demod = FmDemod::new(DESCRIPTOR.input_rate_hz, deviation_hz(p));
        // Which tone is required is a comparison, not state, so retuning the gate keeps
        // whatever the detector has already heard on this frequency.
        match (&mut self.tone, p.tone_mode) {
            (_, NfmToneMode::Off) => self.tone = None,
            (Some(tone), _) => tone.configure(p),
            (none, _) => *none = Some(Tone::new(p)),
        }
        Ok(())
    }

    /// A subaudible tone belongs to the station that was sending it. The channel has left.
    fn retuned(&mut self) {
        if let Some(tone) = &mut self.tone {
            tone.reset();
        }
    }

    fn needs_gated_input(&self) -> bool {
        // With a tone mode on, the gated span is what lets the detector *lose* a tone: the
        // correlators decay over their window and the "no tone, muted" event follows. Off,
        // there is nothing to measure and the host may skip the channel entirely.
        self.tone.is_some()
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.demod.process(iq, &mut self.demod_buf);
        let mut open = true;
        if let Some(tone) = &mut self.tone {
            open = tone.step(&mut self.demod_buf, out);
        }
        self.audio_lp.process(&self.demod_buf, &mut out.audio_pcm);
        clamp_full_scale(&mut out.audio_pcm);
        // Muted, not skipped: a closed tone gate still owes the client audio of the right
        // duration, exactly as a closed squelch does.
        if !open {
            out.audio_pcm.fill(0.0);
        }
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
        testgen::{
            burst, fm_modulate,
            nfm::{ctcss_audio, dcs_audio, mix},
            tone_audio,
        },
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
            ..NfmParams::default()
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
                settings(ChannelParams::Nfm(NfmParams {
                    bandwidth_hz: bad,
                    ..NfmParams::default()
                })),
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

    // ── subaudible signalling ──────────────────────────────────────────────────────────────

    /// A repeater keys its subaudible signalling at 10–15 % of deviation, under speech at
    /// most of the rest. Both are fractions of full deviation, which is what the
    /// discriminator hands back.
    const SUBAUDIBLE: f32 = 0.15;
    const VOICE: f32 = 0.6;
    /// Long enough for the correlator bank to fill its half-second window twice over.
    const TONE_LEN: usize = 48_000 * 2;

    fn tone_params(mode: NfmToneMode, ctcss_hz: Option<f64>, dcs_code: Option<u16>) -> NfmParams {
        NfmParams {
            tone_mode: mode,
            ctcss_hz,
            dcs_code,
            ..NfmParams::default()
        }
    }

    fn tone_channel(p: NfmParams) -> NfmChannel {
        NfmChannel::new(ctx(), settings(ChannelParams::Nfm(p))).unwrap()
    }

    /// A carrier with `subaudible` under a 1 kHz voice tone.
    fn signalling_iq(subaudible: &[f32]) -> Vec<Complex<f32>> {
        let voice = tone_audio(1_000.0, VOICE, RATE, subaudible.len());
        fm_modulate(&mix(subaudible, &voice), DEVIATION_HZ, RATE)
    }

    /// Feed the channel in ragged blocks and return the last status it reported plus the
    /// audio, which is what a listener would actually get.
    fn run_tone(chan: &mut NfmChannel, iq: &[Complex<f32>]) -> (Vec<ToneSquelchStatus>, Vec<f32>) {
        let mut out = ChannelOutputs::default();
        let (mut statuses, mut audio) = (Vec::new(), Vec::new());
        let mut pos = 0;
        for len in [997usize, 4_096, 65, 2_048].iter().cycle() {
            if pos >= iq.len() {
                break;
            }
            let end = (pos + len).min(iq.len());
            out.reset();
            chan.process(&iq[pos..end], &mut out);
            for event in &out.events {
                let DecoderEvent::Tone(status) = event else {
                    panic!("nfm emitted {}", event.kind())
                };
                statuses.push(status.clone());
            }
            audio.extend_from_slice(&out.audio_pcm);
            pos = end;
        }
        (statuses, audio)
    }

    #[test]
    fn detect_names_the_ctcss_tone_under_the_voice() {
        for tone_hz in [67.0, 88.5, 162.2, 165.5, 254.1] {
            let iq = signalling_iq(&ctcss_audio(tone_hz, SUBAUDIBLE, RATE, TONE_LEN));
            let mut chan = tone_channel(tone_params(NfmToneMode::Detect, None, None));
            let (statuses, _) = run_tone(&mut chan, &iq);
            let last = statuses.last().expect("a tone must be reported");
            assert_eq!(last.ctcss_hz, Some(tone_hz), "{tone_hz} Hz: {statuses:?}");
            assert_eq!(last.dcs_code, None, "{tone_hz} Hz");
            // Detect answers the question without acting on it.
            assert!(last.open, "{tone_hz} Hz");
        }
    }

    /// 162.2 and 165.5 are 3.3 Hz apart and the pair at 67.0/69.3 only 2.3 — the whole reason
    /// the correlator window is half a second. A bank that named the neighbour would look
    /// exactly like one that worked.
    #[test]
    fn a_neighbouring_standard_tone_is_not_named_instead() {
        for (sent, neighbour) in [(69.3, 67.0), (162.2, 165.5), (206.5, 203.5)] {
            let iq = signalling_iq(&ctcss_audio(sent, SUBAUDIBLE, RATE, TONE_LEN));
            let mut chan = tone_channel(tone_params(NfmToneMode::Detect, None, None));
            let (statuses, _) = run_tone(&mut chan, &iq);
            let named = statuses.last().and_then(|s| s.ctcss_hz);
            assert_eq!(named, Some(sent), "{sent} Hz was named {named:?}");
            assert_ne!(named, Some(neighbour));
        }
    }

    #[test]
    fn detect_reads_the_dcs_code_under_the_voice() {
        for code in [23u16, 114, 754, 172] {
            let iq = signalling_iq(&dcs_audio(code, SUBAUDIBLE, RATE, TONE_LEN));
            let mut chan = tone_channel(tone_params(NfmToneMode::Detect, None, None));
            let (statuses, _) = run_tone(&mut chan, &iq);
            let last = statuses.last().expect("a code must be reported");
            assert_eq!(last.dcs_code, Some(code), "{code:03}: {statuses:?}");
        }
    }

    /// A DCS transmission through an inverted discriminator is its inverse-pair partner, not
    /// nothing and not the same code — which is why there is no polarity switch to get wrong.
    #[test]
    fn an_inverted_dcs_transmission_reads_as_the_partner_code() {
        let inverted: Vec<f32> = dcs_audio(23, SUBAUDIBLE, RATE, TONE_LEN)
            .iter()
            .map(|s| -s)
            .collect();
        let mut chan = tone_channel(tone_params(NfmToneMode::Detect, None, None));
        let (statuses, _) = run_tone(&mut chan, &signalling_iq(&inverted));
        assert_eq!(statuses.last().and_then(|s| s.dcs_code), Some(47));
    }

    /// The carrier is never exactly on frequency, and the tuning error arrives as DC on the
    /// discriminator — right where a DCS slicer's decision threshold is.
    #[test]
    fn dcs_decodes_through_a_carrier_offset() {
        for offset_hz in [-400.0f64, 250.0] {
            let word = dcs_audio(23, SUBAUDIBLE, RATE, TONE_LEN);
            // A constant frequency error is a constant term in the modulating waveform.
            let bias = (offset_hz / DEVIATION_HZ) as f32;
            let biased: Vec<f32> = word.iter().map(|s| s + bias).collect();
            let mut chan = tone_channel(tone_params(NfmToneMode::Detect, None, None));
            let (statuses, _) = run_tone(&mut chan, &signalling_iq(&biased));
            assert_eq!(
                statuses.last().and_then(|s| s.dcs_code),
                Some(23),
                "{offset_hz} Hz offset"
            );
        }
    }

    /// The gate, both ways round: the tone it was told to open on, and the one it was not.
    #[test]
    fn ctcss_squelch_passes_only_its_own_tone() {
        for (sent, open) in [(88.5, true), (91.5, false)] {
            let iq = signalling_iq(&ctcss_audio(sent, SUBAUDIBLE, RATE, TONE_LEN));
            let mut chan = tone_channel(tone_params(NfmToneMode::Ctcss, Some(88.5), None));
            let (statuses, audio) = run_tone(&mut chan, &iq);
            assert_eq!(statuses.last().map(|s| s.open), Some(open), "{sent} Hz");
            // Muted, not skipped: the client's jitter buffer still gets its samples.
            assert_eq!(audio.len(), iq.len(), "{sent} Hz");
            let settled = rms(&audio[audio.len() / 2..]);
            if open {
                assert!(settled > 0.3, "{sent} Hz: passed audio rms {settled}");
            } else {
                assert_eq!(settled, 0.0, "{sent} Hz: muted audio rms {settled}");
            }
        }
    }

    #[test]
    fn dcs_squelch_passes_only_its_own_code() {
        for (sent, open) in [(23u16, true), (114, false)] {
            let iq = signalling_iq(&dcs_audio(sent, SUBAUDIBLE, RATE, TONE_LEN));
            let mut chan = tone_channel(tone_params(NfmToneMode::Dcs, None, Some(23)));
            let (statuses, audio) = run_tone(&mut chan, &iq);
            assert_eq!(statuses.last().map(|s| s.open), Some(open), "{sent:03}");
            let settled = rms(&audio[audio.len() / 2..]);
            assert_eq!(settled > 0.3, open, "{sent:03}: audio rms {settled}");
        }
    }

    /// The tone must not be audible in what it lets through — that is the difference between
    /// tone squelch and a 67 Hz hum on every transmission.
    #[test]
    fn the_subaudible_tone_is_taken_out_of_the_audio_it_opens() {
        let iq = signalling_iq(&ctcss_audio(88.5, SUBAUDIBLE, RATE, TONE_LEN));
        let mut chan = tone_channel(tone_params(NfmToneMode::Ctcss, Some(88.5), None));
        let (_, audio) = run_tone(&mut chan, &iq);
        let settled = &audio[audio.len() / 2..];
        let (freq, ratio) = dominant_tone(settled, RATE);
        assert!((995.0..1_005.0).contains(&freq), "dominant {freq} Hz");
        assert!(ratio > 10.0, "voice-to-rest ratio {ratio}");

        // The same channel with the tone path off keeps the 88.5 Hz component it was carrying.
        let mut plain = channel();
        let (_, plain_audio) = run_tone(&mut plain, &iq);
        let plain_settled = &plain_audio[plain_audio.len() / 2..];
        assert!(
            subaudible_rms(plain_settled) > 4.0 * subaudible_rms(settled),
            "highpass removed {:.4} -> {:.4}",
            subaudible_rms(plain_settled),
            subaudible_rms(settled)
        );
    }

    /// Energy below 300 Hz, measured with the same correlator the detector uses.
    fn subaudible_rms(audio: &[f32]) -> f32 {
        let mut correlator = sdrmm_dsp::ToneCorrelator::new(RATE, 88.5, (RATE * 0.1) as usize);
        audio
            .iter()
            .map(|&s| correlator.push(s))
            .fold(0.0, f32::max)
    }

    #[test]
    fn a_tone_that_stops_closes_the_gate_again() {
        let mut with_tone = ctcss_audio(88.5, SUBAUDIBLE, RATE, TONE_LEN);
        with_tone.extend(std::iter::repeat_n(0.0, TONE_LEN));
        let mut chan = tone_channel(tone_params(NfmToneMode::Ctcss, Some(88.5), None));
        let (statuses, _) = run_tone(&mut chan, &signalling_iq(&with_tone));
        assert_eq!(
            statuses.first().map(|s| s.open),
            Some(true),
            "{statuses:?}: the gate never opened"
        );
        assert_eq!(statuses.last().map(|s| s.open), Some(false), "{statuses:?}");
        assert_eq!(statuses.last().and_then(|s| s.ctcss_hz), None);
    }

    /// Continuous signalling would otherwise be a continuous event: one per block, forty a
    /// second, forever.
    #[test]
    fn only_changes_are_reported() {
        let iq = signalling_iq(&ctcss_audio(88.5, SUBAUDIBLE, RATE, TONE_LEN));
        let mut chan = tone_channel(tone_params(NfmToneMode::Detect, None, None));
        let (statuses, _) = run_tone(&mut chan, &iq);
        assert_eq!(statuses.len(), 1, "{statuses:?}");
    }

    /// Nothing under the carrier is nothing to say, and a channel that opens on silence in
    /// tone-squelch mode would be a channel with no tone squelch.
    #[test]
    fn a_carrier_with_no_signalling_says_nothing_and_opens_nothing() {
        let iq = signalling_iq(&vec![0.0; TONE_LEN]);
        let mut detect = tone_channel(tone_params(NfmToneMode::Detect, None, None));
        assert!(run_tone(&mut detect, &iq).0.is_empty());

        let mut gated = tone_channel(tone_params(NfmToneMode::Ctcss, Some(88.5), None));
        let (statuses, audio) = run_tone(&mut gated, &iq);
        assert!(statuses.is_empty(), "{statuses:?}");
        assert_eq!(rms(&audio[audio.len() / 2..]), 0.0);
    }

    /// Noise is not a tone: a bank that named the strongest of fifty bins whatever it read
    /// would open a tone squelch on hiss.
    #[test]
    fn noise_names_no_tone_and_opens_no_gate() {
        let mut chan = tone_channel(tone_params(NfmToneMode::Detect, None, None));
        let (statuses, _) = run_tone(&mut chan, &complex_noise(0x7013_5511, 0.5, TONE_LEN));
        for status in &statuses {
            assert_eq!(status.ctcss_hz, None, "{statuses:?}");
            assert_eq!(status.dcs_code, None, "{statuses:?}");
        }
    }

    /// What the channel accreted belongs to the station it was pointed at.
    #[test]
    fn retuning_forgets_the_tone() {
        let iq = signalling_iq(&ctcss_audio(88.5, SUBAUDIBLE, RATE, TONE_LEN));
        let mut chan = tone_channel(tone_params(NfmToneMode::Detect, None, None));
        assert_eq!(
            run_tone(&mut chan, &iq).0.last().unwrap().ctcss_hz,
            Some(88.5)
        );
        chan.retuned();
        // A silent carrier on the new frequency must retract the tone, not keep reporting it.
        let quiet = signalling_iq(&vec![0.0; TONE_LEN]);
        let (statuses, _) = run_tone(&mut chan, &quiet);
        assert_eq!(
            statuses.last().and_then(|s| s.ctcss_hz),
            None,
            "{statuses:?}"
        );
    }

    /// The channel only asks for gated input once it has something to measure under a carrier.
    #[test]
    fn only_a_tone_mode_needs_the_gated_span() {
        assert!(!channel().needs_gated_input());
        for mode in [NfmToneMode::Detect, NfmToneMode::Ctcss] {
            let ctcss_hz = (mode == NfmToneMode::Ctcss).then_some(88.5);
            assert!(tone_channel(tone_params(mode, ctcss_hz, None)).needs_gated_input());
        }
        // Turned off again, the channel goes back to being skippable.
        let mut chan = tone_channel(tone_params(NfmToneMode::Detect, None, None));
        chan.apply(settings(ChannelParams::Nfm(NfmParams::default())))
            .unwrap();
        assert!(!chan.needs_gated_input());
    }

    #[test]
    fn a_tone_the_detector_does_not_search_for_is_refused() {
        for p in [
            tone_params(NfmToneMode::Ctcss, None, None),
            tone_params(NfmToneMode::Ctcss, Some(88.0), None),
            tone_params(NfmToneMode::Ctcss, Some(f64::NAN), None),
            tone_params(NfmToneMode::Dcs, None, None),
            tone_params(NfmToneMode::Dcs, None, Some(24)),
            tone_params(NfmToneMode::Dcs, None, Some(999)),
        ] {
            let built = NfmChannel::new(ctx(), settings(ChannelParams::Nfm(p.clone())));
            assert!(
                matches!(built, Err(ChannelError::InvalidSettings(_))),
                "{p:?} must be refused"
            );
            // And refused on the way in through `apply`, not only at construction.
            let mut chan = channel();
            assert!(matches!(
                chan.apply(settings(ChannelParams::Nfm(p))),
                Err(ChannelError::InvalidSettings(_))
            ));
        }
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
            let params = ChannelParams::Nfm(NfmParams {
                bandwidth_hz,
                ..NfmParams::default()
            });
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
            ..NfmParams::default()
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
