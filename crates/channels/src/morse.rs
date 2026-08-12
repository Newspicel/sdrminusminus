//! Morse/CW decoder (PLAN §13 P2): envelope detection with adaptive element timing.
//!
//! The host has already applied the CW filter at `MorseParams::bandwidth_hz`, so the tone
//! arrives near DC and its magnitude is the key line. Smoothing it and slicing it against a
//! tracked noise floor turns the stream into mark/gap runs; everything above that is timing.

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, Envelope, KeyingSlicer, design_lowpass};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, MorseParams, MorseText,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const CHANNEL_TAPS: usize = 257;

/// **Why this decoder does not run the library's envelope tier** (MODEM-PLAN §7 phase 4). Hand-sent
/// CW has no symbol clock at all: an element's length is the operator's, the ratio of dot to dash
/// is only nominally 1:3, and the decoder's whole job is to infer the timing from what it hears.
/// `sdrmm_modem::linear::EnvelopeDemod` is symbol-synchronous by construction — it needs an
/// oversampling and emits one amplitude per symbol period — so what the library's OOK row and this
/// front end share is the modulation and the adaptive threshold, not the chain. The committed OOK
/// bundle characterises magnitude detection of a *clocked* keyed carrier; asynchronous keying is a
/// different receiver, and this is it.
///
/// Envelope smoothing: long enough to average the noise inside the CW filter, far shorter than
/// the ~15 ms dot of the fastest speed this decoder tracks.
const ENV_ATTACK_S: f64 = 2e-3;
const ENV_RELEASE_S: f64 = 2e-3;
/// Runs shorter than this are slicer chatter, not elements, and are merged into their
/// neighbours — under a third of a dot even at [`WPM_MAX`].
const MIN_RUN_S: f64 = 5e-3;
/// Peak-to-floor ratio a mark must hold throughout to be treated as sent rather than as a
/// noise crest. The slicer refuses to key below its own floor; re-checking here means a
/// marginal signal that keys on one crest still produces no characters.
const MIN_SNR: f32 = 6.0;
/// Element and gap boundaries in dot units. Nominal Morse is 1/3 for marks and 1/3/7 for
/// gaps; the boundaries sit off-centre so ±30% sloppy sending still lands on the right side.
const DASH_MIN_DOTS: f32 = 2.0;
const LETTER_GAP_DOTS: f32 = 2.0;
const WORD_GAP_DOTS: f32 = 4.5;
/// Speed-tracker step. One element moves the estimate 20% of the way to the observation, so a
/// speed change settles within a few characters without a single mistimed element derailing it.
const TRACK_ALPHA: f32 = 0.2;
/// Marks collected before the tracker commits to a dot length. The first mark of a
/// transmission is measured against a cold peak tracker and reads short, so it is held out.
const CALIB_MARKS: usize = 9;
/// Speeds the tracker will settle on. Outside this range the "signal" is not hand-sent CW.
const WPM_MIN: f32 = 3.0;
const WPM_MAX: f32 = 80.0;
/// Key-up time that ends a transmission: the buffered text is emitted even though no word gap
/// closed it. Longer than a word gap at [`WPM_MIN`] would be, so it never splits a sentence.
const IDLE_FLUSH_S: f64 = 3.0;
/// Longest text an event carries. Continuous sending with no word gaps still reports at a
/// bounded latency instead of growing a buffer forever.
const MAX_CHUNK_CHARS: usize = 64;
/// Longest element run the table holds; anything longer cannot be a character.
const MAX_ELEMENTS: u8 = 8;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "morse".to_owned(),
    name: "Morse (CW)".to_owned(),
    bandwidth_hz: 400.0,
    input_rate_hz: 8_000.0,
    has_audio: false,
    decoder_kind: Some("morse".to_owned()),
    ..ChannelDescriptor::default()
});

/// Pack an element run into its lookup key: a leading 1 marks the start, then one bit per
/// element (dash = 1). Unique per run, so the table is a flat `u16` search.
const fn code(pattern: &str) -> u16 {
    let bytes = pattern.as_bytes();
    let mut key = 1;
    let mut i = 0;
    while i < bytes.len() {
        key = key * 2 + (bytes[i] == b'-') as u16;
        i += 1;
    }
    key
}

/// International Morse (ITU-R M.1677-1 §1 and §2). The prosigns AR and BT share their element
/// runs with `+` and `=` and are reported as those glyphs, the conventional rendering; SK has
/// no punctuation twin, so it is reported as its letter run.
static TABLE: &[(u16, &str)] = &[
    (code(".-"), "A"),
    (code("-..."), "B"),
    (code("-.-."), "C"),
    (code("-.."), "D"),
    (code("."), "E"),
    (code("..-."), "F"),
    (code("--."), "G"),
    (code("...."), "H"),
    (code(".."), "I"),
    (code(".---"), "J"),
    (code("-.-"), "K"),
    (code(".-.."), "L"),
    (code("--"), "M"),
    (code("-."), "N"),
    (code("---"), "O"),
    (code(".--."), "P"),
    (code("--.-"), "Q"),
    (code(".-."), "R"),
    (code("..."), "S"),
    (code("-"), "T"),
    (code("..-"), "U"),
    (code("...-"), "V"),
    (code(".--"), "W"),
    (code("-..-"), "X"),
    (code("-.--"), "Y"),
    (code("--.."), "Z"),
    (code("-----"), "0"),
    (code(".----"), "1"),
    (code("..---"), "2"),
    (code("...--"), "3"),
    (code("....-"), "4"),
    (code("....."), "5"),
    (code("-...."), "6"),
    (code("--..."), "7"),
    (code("---.."), "8"),
    (code("----."), "9"),
    (code(".-.-.-"), "."),
    (code("--..--"), ","),
    (code("..--.."), "?"),
    (code(".----."), "'"),
    (code("-.-.--"), "!"),
    (code("-..-."), "/"),
    (code("-.--."), "("),
    (code("-.--.-"), ")"),
    (code(".-..."), "&"),
    (code("---..."), ":"),
    (code("-.-.-."), ";"),
    (code("-...-"), "="),
    (code(".-.-."), "+"),
    (code("-....-"), "-"),
    (code("..--.-"), "_"),
    (code(".-..-."), "\""),
    (code("...-..-"), "$"),
    (code(".--.-."), "@"),
    (code("...-.-"), "SK"),
];

/// Element runs with no entry in the table are reported rather than dropped, so a garbled
/// character is visible in the log instead of silently shortening the line.
const UNKNOWN: &str = "*";

/// One keyed or unkeyed interval, in samples. `clean` is false when the slicer's SNR dipped
/// below [`MIN_SNR`] anywhere inside a mark.
#[derive(Clone, Copy)]
struct Run {
    on: bool,
    len: u32,
    clean: bool,
}

pub struct MorseChannel {
    rate: f64,
    /// `Some` pins the element grid to the operator's stated speed; `None` tracks it.
    fixed_wpm: Option<f32>,
    env: Envelope,
    slicer: KeyingSlicer,
    min_run: u32,
    idle_flush: u32,
    dot_bounds: (f32, f32),

    key: bool,
    run: u32,
    mark_clean: bool,
    idle: u32,
    /// Held one run back so a chatter run can be merged into its neighbours before it is
    /// classified.
    pending: Option<Run>,

    /// Dot length in samples; `None` until the tracker has calibrated.
    dot: Option<f32>,
    /// Runs seen before calibration, replayed once the dot length is known.
    hold: Vec<Run>,
    /// Swap partner for [`Self::hold`], so replaying it allocates nothing.
    replay: Vec<Run>,
    marks_held: usize,

    /// Current character's element run, packed by [`code`].
    pattern: u16,
    elements: u8,
    overflow: bool,
    started: bool,
    pending_space: bool,
    text: String,
}

fn params(settings: &ChannelSettings) -> Result<&MorseParams, ChannelError> {
    match &settings.params {
        ChannelParams::Morse(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "morse channel got {} params",
            other.type_id()
        ))),
    }
}

fn check_params(p: &MorseParams) -> Result<(), ChannelError> {
    let rate = DESCRIPTOR.input_rate_hz;
    if !(p.bandwidth_hz.is_finite() && p.bandwidth_hz > 0.0 && p.bandwidth_hz < rate / 2.0) {
        return Err(ChannelError::InvalidSettings(format!(
            "morse bandwidth must be in (0, {}) Hz, got {}",
            rate / 2.0,
            p.bandwidth_hz
        )));
    }
    if let Some(wpm) = p.wpm
        && !(wpm.is_finite() && wpm > 0.0)
    {
        return Err(ChannelError::InvalidSettings(format!(
            "morse wpm must be positive, got {wpm}"
        )));
    }
    Ok(())
}

/// Occupied RF band relative to the channel offset, in Hz.
pub(crate) fn occupied_band(p: &MorseParams) -> (f64, f64) {
    let half = p.bandwidth_hz / 2.0;
    (-half, half)
}

pub(crate) fn channel_filter(p: &MorseParams) -> Result<ChannelFilter, ChannelError> {
    check_params(p)?;
    let (_, half) = occupied_band(p);
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(CHANNEL_TAPS, half / DESCRIPTOR.input_rate_hz),
        1,
    )))
}

/// PARIS: one dot is 1.2 s / wpm.
fn dot_samples(wpm: f32, rate: f64) -> f32 {
    (1.2 * rate / f64::from(wpm)) as f32
}

impl MorseChannel {
    /// Feed one run to the deglitcher: chatter shorter than [`MIN_RUN_S`] is absorbed into the
    /// run before it, which then also swallows the same-polarity run that follows.
    fn push_run(&mut self, run: Run, out: &mut ChannelOutputs) {
        match self.pending.take() {
            None => self.pending = Some(run),
            Some(prev) if prev.on == run.on || run.len < self.min_run => {
                self.pending = Some(Run {
                    on: prev.on,
                    len: prev.len.saturating_add(run.len),
                    clean: prev.clean && run.clean,
                });
            }
            Some(prev) => {
                self.consume(prev, out);
                self.pending = Some(run);
            }
        }
    }

    fn consume(&mut self, run: Run, out: &mut ChannelOutputs) {
        if self.dot.is_none() {
            self.hold.push(run);
            if run.on {
                self.marks_held += 1;
                if self.marks_held >= CALIB_MARKS {
                    self.calibrate(out);
                }
            }
            return;
        }
        self.classify(run, out);
    }

    /// Seed the dot length from the shortest held mark — over [`CALIB_MARKS`] marks of real
    /// sending at least one is a dot — then replay everything that was waiting on it.
    fn calibrate(&mut self, out: &mut ChannelOutputs) {
        let marks = self.hold.iter().filter(|r| r.on).count();
        let seed = self
            .hold
            .iter()
            .filter(|r| r.on)
            .skip(usize::from(marks > 1))
            .map(|r| r.len)
            .min();
        if let Some(seed) = seed {
            let (lo, hi) = self.dot_bounds;
            self.dot = Some((seed as f32).clamp(lo, hi));
            std::mem::swap(&mut self.hold, &mut self.replay);
            let mut i = 0;
            while let Some(&run) = self.replay.get(i) {
                self.classify(run, out);
                i += 1;
            }
            self.replay.clear();
            std::mem::swap(&mut self.hold, &mut self.replay);
        }
        self.hold.clear();
        self.marks_held = 0;
    }

    fn classify(&mut self, run: Run, out: &mut ChannelOutputs) {
        let Some(dot) = self.dot else { return };
        let len = run.len as f32;
        if run.on {
            if !run.clean {
                self.reset_character();
                return;
            }
            self.started = true;
            let dash = len >= DASH_MIN_DOTS * dot;
            if self.elements < MAX_ELEMENTS {
                self.pattern = self.pattern * 2 + u16::from(dash);
                self.elements += 1;
            } else {
                self.overflow = true;
            }
            self.track(if dash { len / 3.0 } else { len });
        } else if len >= WORD_GAP_DOTS * dot {
            self.finish_character(out);
            self.pending_space |= self.started;
            self.flush(out);
        } else if len >= LETTER_GAP_DOTS * dot {
            self.finish_character(out);
            self.track(len / 3.0);
        } else {
            self.track(len);
        }
    }

    /// Adaptation rule: every element that is not a word gap contributes its own estimate of
    /// the dot length — the interval itself for a dot or an element gap, a third of it for a
    /// dash or a letter gap — and the estimate moves [`TRACK_ALPHA`] of the way there. Word
    /// gaps are excluded because operators stretch them at will.
    fn track(&mut self, observed_dot: f32) {
        if self.fixed_wpm.is_some() || !observed_dot.is_finite() {
            return;
        }
        if let Some(dot) = self.dot {
            let (lo, hi) = self.dot_bounds;
            self.dot = Some((dot + TRACK_ALPHA * (observed_dot - dot)).clamp(lo, hi));
        }
    }

    fn reset_character(&mut self) {
        self.pattern = 1;
        self.elements = 0;
        self.overflow = false;
    }

    fn finish_character(&mut self, out: &mut ChannelOutputs) {
        if self.elements == 0 && !self.overflow {
            return;
        }
        let decoded = if self.overflow {
            UNKNOWN
        } else {
            TABLE
                .iter()
                .find(|(c, _)| *c == self.pattern)
                .map_or(UNKNOWN, |(_, s)| *s)
        };
        if self.pending_space {
            self.text.push(' ');
            self.pending_space = false;
        }
        self.text.push_str(decoded);
        self.reset_character();
        // Every string the table yields is ASCII, so byte length is the character count.
        if self.text.len() >= MAX_CHUNK_CHARS {
            self.flush(out);
        }
    }

    fn flush(&mut self, out: &mut ChannelOutputs) {
        if self.text.is_empty() {
            return;
        }
        out.events.push(DecoderEvent::Morse(MorseText {
            text: std::mem::take(&mut self.text),
            wpm: self.wpm(),
        }));
    }

    fn wpm(&self) -> f32 {
        self.dot
            .filter(|d| *d > 0.0)
            .map_or(0.0, |dot| (1.2 * self.rate) as f32 / dot)
    }

    /// The carrier has been down long enough to call the transmission over: settle the run
    /// still in the deglitcher, calibrate on whatever was held, and emit what was decoded.
    fn on_idle(&mut self, out: &mut ChannelOutputs) {
        if let Some(run) = self.pending.take() {
            self.consume(run, out);
        }
        if self.dot.is_none() {
            self.calibrate(out);
        }
        self.finish_character(out);
        self.flush(out);
    }
}

impl ChannelRx for MorseChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        check_params(p)?;
        let rate = ctx.input_rate;
        Ok(Self {
            rate,
            fixed_wpm: p.wpm,
            env: Envelope::new(rate, ENV_ATTACK_S, ENV_RELEASE_S),
            slicer: KeyingSlicer::new(rate),
            min_run: (MIN_RUN_S * rate).round().max(1.0) as u32,
            idle_flush: (IDLE_FLUSH_S * rate).round().max(1.0) as u32,
            dot_bounds: (dot_samples(WPM_MAX, rate), dot_samples(WPM_MIN, rate)),
            key: false,
            run: 0,
            mark_clean: true,
            idle: 0,
            pending: None,
            dot: p.wpm.map(|wpm| dot_samples(wpm, rate)),
            hold: Vec::new(),
            replay: Vec::new(),
            marks_held: 0,
            pattern: 1,
            elements: 0,
            overflow: false,
            started: false,
            pending_space: false,
            text: String::new(),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        check_params(p)?;
        self.fixed_wpm = p.wpm;
        // Switching to a stated speed overrides the tracker; switching back to tracking keeps
        // the current estimate as its starting point rather than recalibrating from scratch.
        if let Some(wpm) = p.wpm {
            self.dot = Some(dot_samples(wpm, self.rate));
        }
        Ok(())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        for sample in iq {
            let key = self.slicer.push(self.env.push(sample.norm()));
            if key {
                self.mark_clean &= self.slicer.snr() >= MIN_SNR;
                self.idle = 0;
            } else {
                self.idle = self.idle.saturating_add(1);
            }
            if key != self.key {
                let run = Run {
                    on: self.key,
                    len: self.run,
                    clean: self.mark_clean,
                };
                self.push_run(run, out);
                self.key = key;
                self.run = 0;
                self.mark_clean = true;
            }
            self.run = self.run.saturating_add(1);
            if self.idle == self.idle_flush {
                self.on_idle(out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::RttyParams;

    use super::*;
    use crate::{testgen, testutil::settings};

    const RATE: f64 = 8_000.0;
    const CALL: &str = "CQ CQ DE DL1ABC K";

    fn channel(wpm: Option<f32>) -> MorseChannel {
        MorseChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Morse(MorseParams {
                bandwidth_hz: 400.0,
                wpm,
            })),
        )
        .unwrap()
    }

    /// A transmission with the lead-in the slicer needs to seed its floor and the lead-out
    /// that ends it, so a test sees exactly what the host would deliver.
    fn burst(text: &str, wpm: f32, tone_hz: f64) -> Vec<Complex<f32>> {
        let mut iq = testgen::silence((0.5 * RATE) as usize);
        iq.extend(testgen::morse::transmission(text, wpm, tone_hz, RATE));
        iq.extend(testgen::silence(((IDLE_FLUSH_S + 0.5) * RATE) as usize));
        iq
    }

    /// Feed `iq` in deliberately ragged blocks; returns the concatenated text and the last
    /// reported speed.
    fn decode_ragged(chan: &mut MorseChannel, iq: &[Complex<f32>]) -> (String, f32) {
        let mut out = ChannelOutputs::default();
        let (mut text, mut wpm) = (String::new(), 0.0);
        let mut pos = 0;
        for len in [997usize, 1, 4_096, 65, 2_048, 7, 1_024].iter().cycle() {
            if pos >= iq.len() {
                break;
            }
            let end = (pos + len).min(iq.len());
            out.reset();
            chan.process(&iq[pos..end], &mut out);
            for event in &out.events {
                let DecoderEvent::Morse(m) = event else {
                    panic!("morse channel emitted {}", event.kind())
                };
                assert!(!m.text.is_empty(), "empty chunk");
                text.push_str(&m.text);
                wpm = m.wpm;
            }
            assert!(out.audio_pcm.is_empty(), "morse must not produce audio");
            pos = end;
        }
        (text, wpm)
    }

    fn decode(chan: &mut MorseChannel, iq: &[Complex<f32>]) -> String {
        decode_ragged(chan, iq).0
    }

    #[test]
    fn decodes_a_call_at_dc_and_at_an_offset() {
        for tone_hz in [0.0, 120.0] {
            let iq = burst(CALL, 20.0, tone_hz);
            let mut filter =
                channel_filter(&MorseParams::default()).expect("default params are valid");
            let mut filtered = Vec::new();
            filter.process(&iq, &mut filtered);
            let text = decode(&mut channel(None), &filtered);
            assert_eq!(text, CALL, "tone at {tone_hz} Hz");
        }
    }

    #[test]
    fn tracks_speed_from_the_signal() {
        for truth in [12.0, 35.0] {
            let mut chan = channel(None);
            let (text, wpm) = decode_ragged(&mut chan, &burst(CALL, truth, 0.0));
            assert_eq!(text, CALL, "at {truth} wpm");
            let error = (wpm - truth).abs() / truth;
            assert!(error < 0.2, "estimated {wpm} wpm for {truth} wpm sending");
        }
    }

    #[test]
    fn fixed_speed_tolerates_sloppy_sending() {
        // The operator states 20 wpm and sends 20% off it in both directions.
        for actual in [16.0, 24.0] {
            let mut chan = channel(Some(20.0));
            let (text, wpm) = decode_ragged(&mut chan, &burst(CALL, actual, 0.0));
            assert_eq!(text, CALL, "stated 20 wpm, sent {actual}");
            assert!((wpm - 20.0).abs() < 0.5, "fixed speed reported as {wpm}");
        }
    }

    #[test]
    fn continuous_sending_is_chunked_at_the_buffer_size() {
        // No word gap anywhere, so only the buffer bound can split this.
        let sent = "E".repeat(MAX_CHUNK_CHARS + 4);
        let mut chan = channel(None);
        let mut out = ChannelOutputs::default();
        chan.process(&burst(&sent, 30.0, 0.0), &mut out);
        assert!(out.events.len() > 1, "not chunked: {:?}", out.events);
        let joined: String = out
            .events
            .iter()
            .map(|e| match e {
                DecoderEvent::Morse(m) => m.text.as_str(),
                other => panic!("morse channel emitted {}", other.kind()),
            })
            .collect();
        assert_eq!(joined, sent);
    }

    #[test]
    fn punctuation_and_digits_round_trip() {
        let text = "73 DE OM! QRV 14.060 =+/?,";
        assert_eq!(decode(&mut channel(None), &burst(text, 22.0, 0.0)), text);
    }

    #[test]
    fn word_gaps_become_single_spaces_and_letter_gaps_do_not() {
        // "IT" and "EE" are the pairs a mistimed letter gap would fuse or split.
        assert_eq!(
            decode(&mut channel(None), &burst("EE E", 18.0, 0.0)),
            "EE E"
        );
    }

    #[test]
    fn unknown_element_runs_are_reported_not_dropped() {
        // Nine dots at single-dot spacing: one element run, longer than any table entry.
        let dot = (1.2 * RATE / 20.0) as usize;
        let mut env = Vec::new();
        for i in 0..9 {
            if i > 0 {
                env.resize(env.len() + dot, 0.0);
            }
            env.resize(env.len() + dot, 1.0);
        }
        let mut iq = testgen::silence((0.5 * RATE) as usize);
        iq.extend(testgen::ook(&env, 0.0, RATE));
        iq.extend(testgen::silence(((IDLE_FLUSH_S + 0.5) * RATE) as usize));
        assert_eq!(decode(&mut channel(None), &iq), UNKNOWN);
    }

    #[test]
    fn pure_noise_decodes_to_nothing() {
        let mut iq = testgen::silence((6.0 * RATE) as usize);
        testgen::add_noise(&mut iq, 0x0c0f_fee1, 0.3);
        let mut out = ChannelOutputs::default();
        channel(None).process(&iq, &mut out);
        assert!(out.events.is_empty(), "noise decoded to {:?}", out.events);
    }

    #[test]
    fn ragged_block_splits_give_identical_results() {
        let iq = burst(CALL, 20.0, 0.0);
        let mut whole = ChannelOutputs::default();
        channel(None).process(&iq, &mut whole);
        let one_shot: Vec<&DecoderEvent> = whole.events.iter().collect();
        let mut chan = channel(None);
        let (ragged, _) = decode_ragged(&mut chan, &iq);
        let joined: String = one_shot
            .iter()
            .map(|e| match e {
                DecoderEvent::Morse(m) => m.text.as_str(),
                other => panic!("morse channel emitted {}", other.kind()),
            })
            .collect();
        assert_eq!(joined, ragged);
        assert_eq!(ragged, CALL);
    }

    #[test]
    fn mismatched_params_variant_is_rejected() {
        let mut chan = channel(None);
        let err = chan.apply(settings(ChannelParams::Rtty(RttyParams::default())));
        assert!(matches!(err, Err(ChannelError::InvalidSettings(_))));
        let built = MorseChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Rtty(RttyParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }

    #[test]
    fn out_of_range_params_are_rejected() {
        for p in [
            MorseParams {
                bandwidth_hz: 0.0,
                wpm: None,
            },
            MorseParams {
                bandwidth_hz: f64::NAN,
                wpm: None,
            },
            MorseParams {
                bandwidth_hz: 400.0,
                wpm: Some(0.0),
            },
        ] {
            assert!(
                matches!(check_params(&p), Err(ChannelError::InvalidSettings(_))),
                "{p:?} must be rejected"
            );
        }
    }

    #[test]
    fn wrong_input_rate_is_rejected() {
        let built = MorseChannel::new(
            ChannelCtx {
                input_rate: 48_000.0,
            },
            settings(ChannelParams::Morse(MorseParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }
}
