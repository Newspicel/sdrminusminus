use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, Envelope, KeyingSlicer, design_lowpass};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, MorseParams, MorseText,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const CHANNEL_TAPS: usize = 257;

const ENV_ATTACK_S: f64 = 2e-3;
const ENV_RELEASE_S: f64 = 2e-3;
const MIN_RUN_S: f64 = 5e-3;
const MIN_SNR: f32 = 6.0;
const DASH_MIN_DOTS: f32 = 2.0;
const LETTER_GAP_DOTS: f32 = 2.0;
const WORD_GAP_DOTS: f32 = 4.5;
const TRACK_ALPHA: f32 = 0.2;
const CALIB_MARKS: usize = 9;
const WPM_MIN: f32 = 3.0;
const WPM_MAX: f32 = 80.0;
const IDLE_FLUSH_S: f64 = 3.0;
const MAX_CHUNK_CHARS: usize = 64;
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

const UNKNOWN: &str = "*";

#[derive(Clone, Copy)]
struct Run {
    on: bool,
    len: u32,
    clean: bool,
}

#[derive(Clone)]
pub struct MorseChannel {
    rate: f64,
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
    pending: Option<Run>,

    dot: Option<f32>,
    hold: Vec<Run>,
    replay: Vec<Run>,
    marks_held: usize,

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

fn dot_samples(wpm: f32, rate: f64) -> f32 {
    (1.2 * rate / f64::from(wpm)) as f32
}

impl MorseChannel {
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

    fn burst(text: &str, wpm: f32, tone_hz: f64) -> Vec<Complex<f32>> {
        let mut iq = testgen::silence((0.5 * RATE) as usize);
        iq.extend(testgen::morse::transmission(text, wpm, tone_hz, RATE));
        iq.extend(testgen::silence(((IDLE_FLUSH_S + 0.5) * RATE) as usize));
        iq
    }

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
        for actual in [16.0, 24.0] {
            let mut chan = channel(Some(20.0));
            let (text, wpm) = decode_ragged(&mut chan, &burst(CALL, actual, 0.0));
            assert_eq!(text, CALL, "stated 20 wpm, sent {actual}");
            assert!((wpm - 20.0).abs() < 0.5, "fixed speed reported as {wpm}");
        }
    }

    #[test]
    fn continuous_sending_is_chunked_at_the_buffer_size() {
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
        assert_eq!(
            decode(&mut channel(None), &burst("EE E", 18.0, 0.0)),
            "EE E"
        );
    }

    #[test]
    fn unknown_element_runs_are_reported_not_dropped() {
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
