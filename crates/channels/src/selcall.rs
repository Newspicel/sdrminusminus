use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, ToneCorrelator, design_lowpass};
use sdrmm_modem::analog::{AngleDemod, AngleDetector, AngleKind, AngleParams, AngleRx};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, SelcallParams,
    SelcallSequence, SelcallSystem,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const INPUT_RATE_HZ: f64 = 48_000.0;
const BANDWIDTH_HZ: f64 = 12_500.0;
const DEVIATION_HZ: f64 = 2_500.0;
const CHANNEL_TAPS: usize = 129;
const WINDOW_MS: u32 = 25;
const DECISION_MS: u32 = 5;
const ACQUIRE_DECISIONS: u32 = 2;
const MIN_TONE_MS: u32 = 30;
const RESET_SILENCE_MS: u32 = 150;
const MIN_LEVEL: f32 = 0.06;
const WINNER_MARGIN: f32 = 1.8;
const CALL_SYMBOLS: usize = 5;

#[derive(Clone, Copy)]
struct Tone {
    symbol: char,
    hz: f64,
    repeat: bool,
}

const CCIR1: [Tone; 11] = [
    tone('0', 1_981.0),
    tone('1', 1_124.0),
    tone('2', 1_197.0),
    tone('3', 1_275.0),
    tone('4', 1_358.0),
    tone('5', 1_446.0),
    tone('6', 1_540.0),
    tone('7', 1_640.0),
    tone('8', 1_747.0),
    tone('9', 1_860.0),
    repeat(2_110.0),
];

const ZVEI1: [Tone; 15] = [
    tone('0', 2_400.0),
    tone('1', 1_060.0),
    tone('2', 1_160.0),
    tone('3', 1_270.0),
    tone('4', 1_400.0),
    tone('5', 1_530.0),
    tone('6', 1_670.0),
    tone('7', 1_830.0),
    tone('8', 2_000.0),
    tone('9', 2_200.0),
    tone('A', 2_800.0),
    tone('B', 810.0),
    tone('C', 970.0),
    tone('D', 885.0),
    repeat(2_600.0),
];

const fn tone(symbol: char, hz: f64) -> Tone {
    Tone {
        symbol,
        hz,
        repeat: false,
    }
}

const fn repeat(hz: f64) -> Tone {
    Tone {
        symbol: 'R',
        hz,
        repeat: true,
    }
}

fn tones(system: SelcallSystem) -> &'static [Tone] {
    match system {
        SelcallSystem::Ccir1 => &CCIR1,
        SelcallSystem::Zvei1 => &ZVEI1,
    }
}

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "selcall".to_owned(),
    name: "Selcall (CCIR/ZVEI)".to_owned(),
    bandwidth_hz: BANDWIDTH_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    decoder_kind: Some("selcall".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct SelcallChannel {
    demod: AngleDemod,
    audio: Vec<f32>,
    decoder: Decoder,
}

fn params(settings: &ChannelSettings) -> Result<&SelcallParams, ChannelError> {
    match &settings.params {
        ChannelParams::Selcall(params) => Ok(params),
        other => Err(ChannelError::InvalidSettings(format!(
            "selcall channel got {} params",
            other.type_id()
        ))),
    }
}

pub(crate) fn occupied_band() -> (f64, f64) {
    (-BANDWIDTH_HZ / 2.0, BANDWIDTH_HZ / 2.0)
}

pub(crate) fn channel_filter() -> ChannelFilter {
    ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(CHANNEL_TAPS, BANDWIDTH_HZ / 2.0 / INPUT_RATE_HZ),
        1,
    ))
}

fn demodulator() -> AngleDemod {
    AngleDemod::new(
        &AngleParams::new(
            AngleKind::Fm {
                deviation: DEVIATION_HZ / INPUT_RATE_HZ,
            },
            3_000.0 / INPUT_RATE_HZ,
        ),
        &AngleRx::detector_only(AngleDetector::Discriminator),
    )
}

impl ChannelRx for SelcallChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let params = params(&settings)?;
        Ok(Self {
            demod: demodulator(),
            audio: Vec::new(),
            decoder: Decoder::new(params.system),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let params = params(&settings)?;
        if params.system != self.decoder.system {
            self.decoder = Decoder::new(params.system);
        }
        Ok(())
    }

    fn retuned(&mut self) {
        self.demod = demodulator();
        self.decoder.reset();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.demod.process(iq, &mut self.audio);
        for &sample in &self.audio {
            self.decoder.push(sample, out);
        }
    }
}

struct Decoder {
    system: SelcallSystem,
    bank: Vec<ToneCorrelator>,
    levels: Vec<f32>,
    samples: u32,
    decision_samples: u32,
    candidate: Option<usize>,
    candidate_decisions: u32,
    active: Option<usize>,
    active_decisions: u32,
    silence_decisions: u32,
    completed: bool,
    symbols: Vec<char>,
    durations_ms: Vec<u32>,
}

impl Decoder {
    fn new(system: SelcallSystem) -> Self {
        let window = (INPUT_RATE_HZ * f64::from(WINDOW_MS) / 1_000.0) as usize;
        let plan = tones(system);
        Self {
            system,
            bank: plan
                .iter()
                .map(|tone| ToneCorrelator::new(INPUT_RATE_HZ, tone.hz, window))
                .collect(),
            levels: vec![0.0; plan.len()],
            samples: 0,
            decision_samples: (INPUT_RATE_HZ * f64::from(DECISION_MS) / 1_000.0) as u32,
            candidate: None,
            candidate_decisions: 0,
            active: None,
            active_decisions: 0,
            silence_decisions: 0,
            completed: false,
            symbols: Vec::with_capacity(CALL_SYMBOLS),
            durations_ms: Vec::with_capacity(CALL_SYMBOLS),
        }
    }

    fn reset(&mut self) {
        for tone in &mut self.bank {
            tone.reset();
        }
        self.samples = 0;
        self.candidate = None;
        self.candidate_decisions = 0;
        self.active = None;
        self.active_decisions = 0;
        self.silence_decisions = 0;
        self.completed = false;
        self.symbols.clear();
        self.durations_ms.clear();
    }

    fn push(&mut self, sample: f32, out: &mut ChannelOutputs) {
        for (correlator, level) in self.bank.iter_mut().zip(&mut self.levels) {
            *level = correlator.push(sample);
        }
        self.samples += 1;
        if self.samples < self.decision_samples {
            return;
        }
        self.samples = 0;
        self.decide(out);
    }

    fn winner(&self) -> Option<usize> {
        let mut best = (0, 0.0f32);
        let mut runner_up = 0.0f32;
        for (index, &level) in self.levels.iter().enumerate() {
            if level > best.1 {
                runner_up = best.1;
                best = (index, level);
            } else if level > runner_up {
                runner_up = level;
            }
        }
        (best.1 >= MIN_LEVEL && best.1 >= runner_up * WINNER_MARGIN).then_some(best.0)
    }

    fn decide(&mut self, out: &mut ChannelOutputs) {
        let winner = self.winner();
        if winner == self.active {
            self.active_decisions += 1;
            self.candidate = winner;
            self.candidate_decisions = 0;
        } else if winner == self.candidate {
            self.candidate_decisions += 1;
        } else {
            self.candidate = winner;
            self.candidate_decisions = 1;
        }

        if winner.is_none() {
            self.silence_decisions += 1;
        } else {
            self.silence_decisions = 0;
        }

        if winner != self.active && self.candidate_decisions >= ACQUIRE_DECISIONS {
            self.finish_active(out);
            self.active = winner;
            self.active_decisions = self.candidate_decisions;
            self.candidate_decisions = 0;
        }

        if self.silence_decisions * DECISION_MS >= RESET_SILENCE_MS {
            self.active = None;
            self.active_decisions = 0;
            self.candidate = None;
            self.candidate_decisions = 0;
            self.completed = false;
            self.symbols.clear();
            self.durations_ms.clear();
        }
    }

    fn finish_active(&mut self, out: &mut ChannelOutputs) {
        let Some(index) = self.active else { return };
        let duration_ms = self.active_decisions * DECISION_MS;
        if duration_ms < MIN_TONE_MS || self.completed {
            return;
        }
        let tone = tones(self.system)[index];
        let symbol = if tone.repeat {
            let Some(&previous) = self.symbols.last() else {
                self.symbols.clear();
                self.durations_ms.clear();
                return;
            };
            previous
        } else {
            tone.symbol
        };
        self.symbols.push(symbol);
        self.durations_ms.push(duration_ms);
        if self.symbols.len() == CALL_SYMBOLS {
            let mut durations = [0; CALL_SYMBOLS];
            durations.copy_from_slice(&self.durations_ms);
            durations.sort_unstable();
            out.events.push(DecoderEvent::Selcall(SelcallSequence {
                system: self.system,
                code: self.symbols.iter().collect(),
                tone_ms: durations[CALL_SYMBOLS / 2],
            }));
            self.completed = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{testgen, testutil::settings};

    fn decode(system: SelcallSystem, code: &str) -> Vec<DecoderEvent> {
        let params = SelcallParams { system };
        let iq = testgen::selcall::transmission(system, code, INPUT_RATE_HZ).unwrap();
        let mut channel = SelcallChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(ChannelParams::Selcall(params)),
        )
        .unwrap();
        let mut out = ChannelOutputs::default();
        for block in iq.chunks(317) {
            channel.process(block, &mut out);
        }
        out.events
    }

    #[test]
    fn ccir_repeat_marker_decodes_to_the_repeated_digit() {
        assert!(matches!(
            decode(SelcallSystem::Ccir1, "12234").as_slice(),
            [DecoderEvent::Selcall(call)] if call.code == "12234" && call.system == SelcallSystem::Ccir1
        ));
    }

    #[test]
    fn zvei_group_symbols_and_repeat_marker_decode() {
        assert!(matches!(
            decode(SelcallSystem::Zvei1, "A11D0").as_slice(),
            [DecoderEvent::Selcall(call)] if call.code == "A11D0" && call.system == SelcallSystem::Zvei1
        ));
    }

    #[test]
    fn a_three_digit_run_alternates_the_repeat_marker() {
        assert!(matches!(
            decode(SelcallSystem::Ccir1, "11123").as_slice(),
            [DecoderEvent::Selcall(call)] if call.code == "11123"
        ));
    }

    #[test]
    fn noise_and_short_tones_do_not_form_a_call() {
        let mut decoder = Decoder::new(SelcallSystem::Ccir1);
        let mut out = ChannelOutputs::default();
        for sample in testgen::tone_audio(1_124.0, 0.8, INPUT_RATE_HZ, 800) {
            decoder.push(sample, &mut out);
        }
        assert!(out.events.is_empty());
    }
}
