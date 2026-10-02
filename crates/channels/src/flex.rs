use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass, hamming_distance, pocsag_bch_decode};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, DecoderFamily, FlexMessage,
    FlexParams, PagerPayload,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

mod tones;

use tones::{Afc, Bank, Powers, TONES};

const RATE: f64 = 48_000.0;
const CHANNEL_TAPS: usize = 129;
const SYNC_MARKER: u32 = 0xA6C6_AAAA;
const SEARCH_SPS: usize = 30;
const FAST_SPS: usize = 15;
const LATE: usize = 2;
const HISTORY: usize = 2 * LATE + 1;
const TIMING_GAIN: f32 = 0.15;
const SYNC_TOLERANCE: u32 = 3;
const WORDS: usize = 88;
const PHASE_BITS: usize = WORDS * 32;
const MAX_TEXT: usize = 256;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "flex".to_owned(),
    name: "FLEX pager".to_owned(),
    summary: "FLEX pager messages".to_owned(),
    family: DecoderFamily::Paging,
    bandwidth_hz: 12_500.0,
    input_rate_hz: RATE,
    has_audio: false,
    decoder_kind: Some("flex".to_owned()),
    ..ChannelDescriptor::default()
});

#[derive(Clone, Copy, PartialEq, Eq)]
struct Mode {
    symbol_rate: u16,
    levels: u8,
}

impl Mode {
    fn payload_baud(self) -> u16 {
        self.symbol_rate * if self.levels == 4 { 2 } else { 1 }
    }

    fn active_phases(self) -> &'static [usize] {
        match (self.symbol_rate, self.levels) {
            (1_600, 2) => &[0],
            (1_600, 4) => &[0, 1],
            (3_200, 2) => &[0, 2],
            (3_200, 4) => &[0, 1, 2, 3],
            _ => &[],
        }
    }
}

#[derive(Clone, Copy)]
struct Candidate {
    mode: Mode,
    polarity: bool,
    first: u64,
    last: u64,
}

#[derive(Clone, Copy, Default)]
struct SearchLane {
    register: u64,
}

enum State {
    Search,
    Fiw {
        mode: Mode,
        count: usize,
        word: u32,
    },
    Sync2 {
        mode: Mode,
        cycle: u8,
        frame: u8,
        errors: u32,
        remaining: usize,
    },
    Data {
        mode: Mode,
        cycle: u8,
        frame: u8,
        errors: u32,
        symbols: usize,
        toggle: bool,
        phases: [Vec<bool>; 4],
    },
}

pub struct FlexChannel {
    invert: bool,
    afc: Afc,
    slow: Bank,
    fast: Bank,
    history: [Powers; HISTORY],
    sample: u64,
    search_lanes: [SearchLane; SEARCH_SPS],
    candidate: Option<Candidate>,
    state: State,
    timing: f32,
    symbol_samples: usize,
    symbol_target: usize,
    polarity: bool,
}

fn params(settings: &ChannelSettings) -> Result<&FlexParams, ChannelError> {
    match &settings.params {
        ChannelParams::Flex(params) => Ok(params),
        other => Err(ChannelError::InvalidSettings(format!(
            "flex channel got {} params",
            other.type_id()
        ))),
    }
}

fn check_params(params: &FlexParams) -> Result<(), ChannelError> {
    if params.bandwidth_hz.is_finite()
        && params.bandwidth_hz >= 10_000.0
        && params.bandwidth_hz < RATE
    {
        Ok(())
    } else {
        Err(ChannelError::InvalidSettings(format!(
            "flex bandwidth must be in [10000, {RATE}) Hz, got {}",
            params.bandwidth_hz
        )))
    }
}

pub(crate) fn occupied_band(params: &FlexParams) -> (f64, f64) {
    let half = params.bandwidth_hz / 2.0;
    (-half, half)
}

pub(crate) fn channel_filter(params: &FlexParams) -> Result<ChannelFilter, ChannelError> {
    check_params(params)?;
    let half = params.bandwidth_hz / 2.0;
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(CHANNEL_TAPS, half / RATE),
        1,
    )))
}

fn mode(sync: u16) -> Option<Mode> {
    [
        (
            0x870Cu16,
            Mode {
                symbol_rate: 1_600,
                levels: 2,
            },
        ),
        (
            0xB068u16,
            Mode {
                symbol_rate: 1_600,
                levels: 4,
            },
        ),
        (
            0x7B18u16,
            Mode {
                symbol_rate: 3_200,
                levels: 2,
            },
        ),
        (
            0xDEA0u16,
            Mode {
                symbol_rate: 3_200,
                levels: 4,
            },
        ),
        (
            0x4C7Cu16,
            Mode {
                symbol_rate: 3_200,
                levels: 4,
            },
        ),
    ]
    .into_iter()
    .find(|(word, _)| hamming_distance(u64::from(*word), u64::from(sync)) <= SYNC_TOLERANCE)
    .map(|(_, mode)| mode)
}

fn sync_match(register: u64) -> Option<(Mode, bool)> {
    for polarity in [false, true] {
        let candidate = if polarity { !register } else { register };
        let marker = (candidate >> 16) as u32;
        if hamming_distance(u64::from(marker), u64::from(SYNC_MARKER)) > SYNC_TOLERANCE {
            continue;
        }
        let high = (candidate >> 48) as u16;
        let low = candidate as u16;
        if hamming_distance(u64::from(high), u64::from(!low)) > SYNC_TOLERANCE {
            continue;
        }
        if let Some(mode) = mode(high) {
            return Some((mode, polarity));
        }
    }
    None
}

fn reverse31(word: u32) -> u32 {
    word.reverse_bits() >> 1
}

fn reverse21(word: u32) -> u32 {
    word.reverse_bits() >> 11
}

fn decode_word(word: u32) -> Option<(u32, u32)> {
    let parity = word >> 31;
    let pocsag = reverse31(word) << 1 | parity;
    let (corrected, errors) = pocsag_bch_decode(pocsag)?;
    Some((reverse21(corrected >> 11), errors))
}

fn checksum(word: u32) -> bool {
    let sum = (0..5).map(|index| word >> (index * 4) & 0xF).sum::<u32>() + (word >> 20);
    sum & 0xF == 0xF
}

impl FlexChannel {
    fn reset(&mut self) {
        self.state = State::Search;
        self.candidate = None;
        self.timing = 0.0;
        self.symbol_samples = 0;
        self.symbol_target = SEARCH_SPS;
    }

    fn search(&mut self, powers: Powers) {
        let position = self.sample as usize % SEARCH_SPS;
        if self.sample < SEARCH_SPS as u64 {
            return;
        }
        let bit = powers[0] > powers[TONES - 1];
        let lane = &mut self.search_lanes[position];
        lane.register = lane.register << 1 | u64::from(bit);
        let found = sync_match(lane.register);
        match (&mut self.candidate, found) {
            (None, Some((mode, polarity))) => {
                self.candidate = Some(Candidate {
                    mode,
                    polarity,
                    first: self.sample,
                    last: self.sample,
                });
            }
            (Some(candidate), Some(found)) if found == (candidate.mode, candidate.polarity) => {
                candidate.last = self.sample;
            }
            _ => {}
        }
        if let Some(candidate) = self.candidate
            && self.sample - candidate.first + 1 >= SEARCH_SPS as u64
        {
            self.lock(candidate);
        }
    }

    fn lock(&mut self, candidate: Candidate) {
        let centre = candidate.first + (candidate.last - candidate.first) / 2;
        self.candidate = None;
        self.polarity = candidate.polarity ^ self.invert;
        self.state = State::Fiw {
            mode: candidate.mode,
            count: 0,
            word: 0,
        };
        self.timing = 0.0;
        self.symbol_samples = (self.sample - centre) as usize;
        self.symbol_target = SEARCH_SPS;
    }

    fn push_sample(&mut self, sample: Complex<f32>, out: &mut ChannelOutputs) {
        let searching = matches!(self.state, State::Search);
        let centred = self.afc.centre(sample, searching);
        let slow = self.slow.push(centred);
        if searching {
            self.search(slow);
            return;
        }
        let fast = self.fast.push(centred);
        self.history.rotate_left(1);
        self.history[HISTORY - 1] = if self.symbol_target == FAST_SPS {
            fast
        } else {
            slow
        };
        self.symbol_samples += 1;
        if self.symbol_samples < self.symbol_target + LATE {
            return;
        }
        let levels = self.levels();
        let tone = tones::decide(self.history[LATE], levels);
        self.symbol_samples = LATE.saturating_add_signed(-self.retime(tone));
        let tone = if self.polarity {
            TONES - 1 - tone
        } else {
            tone
        };
        self.symbol(tone, out);
    }

    fn levels(&self) -> u8 {
        match &self.state {
            State::Data { mode, .. } => mode.levels,
            _ => 2,
        }
    }

    fn retime(&mut self, tone: usize) -> isize {
        let early = self.history[0][tone];
        let late = self.history[HISTORY - 1][tone];
        let total = early + late;

        if total > 0.0 {
            self.timing += TIMING_GAIN * (late - early) / total;
        }
        if self.timing >= 1.0 {
            self.timing -= 1.0;
            return 1;
        }
        if self.timing <= -1.0 {
            self.timing += 1.0;
            return -1;
        }
        0
    }

    fn symbol(&mut self, tone: usize, out: &mut ChannelOutputs) {
        let bit = tone < TONES / 2;
        let mut next = None;
        match &mut self.state {
            State::Search => {}
            State::Fiw { mode, count, word } => {
                if *count >= 16 {
                    *word |= u32::from(bit) << (*count - 16);
                }
                *count += 1;
                if *count == 48 {
                    let mode = *mode;
                    next = decode_word(*word).and_then(|(data, errors)| {
                        checksum(data).then_some(State::Sync2 {
                            mode,
                            cycle: (data >> 4 & 0xF) as u8,
                            frame: (data >> 8 & 0x7F) as u8,
                            errors,
                            remaining: usize::from(mode.symbol_rate) / 40,
                        })
                    });
                    self.symbol_target = RATE as usize / usize::from(mode.symbol_rate);
                }
            }
            State::Sync2 {
                mode,
                cycle,
                frame,
                errors,
                remaining,
            } => {
                *remaining -= 1;
                if *remaining == 0 {
                    next = Some(State::Data {
                        mode: *mode,
                        cycle: *cycle,
                        frame: *frame,
                        errors: *errors,
                        symbols: 0,
                        toggle: false,
                        phases: std::array::from_fn(|_| Vec::with_capacity(PHASE_BITS)),
                    });
                }
            }
            State::Data {
                mode,
                cycle,
                frame,
                errors,
                symbols,
                toggle,
                phases,
            } => {
                let pair = flex_bits(tone, mode.levels);
                let group = if mode.symbol_rate == 3_200 && *toggle {
                    2
                } else {
                    0
                };
                phases[group].push(pair[0]);
                if mode.levels == 4 {
                    phases[group + 1].push(pair[1]);
                }
                if mode.symbol_rate == 3_200 {
                    *toggle = !*toggle;
                }
                *symbols += 1;
                let total = usize::from(mode.symbol_rate) * 176 / 100;
                if *symbols == total {
                    for &phase in mode.active_phases() {
                        decode_phase(&phases[phase], *mode, *cycle, *frame, *errors, phase, out);
                    }
                    next = Some(State::Search);
                    self.symbol_target = SEARCH_SPS;
                }
            }
        }
        if let Some(state) = next {
            self.state = state;
        } else if matches!(self.state, State::Fiw { count: 48, .. }) {
            self.reset();
        }
    }
}

fn flex_bits(tone: usize, levels: u8) -> [bool; 2] {
    let negative = tone < TONES / 2;
    if levels == 2 {
        return [negative, false];
    }
    [negative, tone == 1 || tone == 2]
}

fn deinterleave(bits: &[bool]) -> Option<[u32; WORDS]> {
    if bits.len() != PHASE_BITS {
        return None;
    }
    let mut words = [0u32; WORDS];
    let mut at = 0;
    for block in 0..11 {
        for bit in 0..32 {
            for word in 0..8 {
                words[block * 8 + word] |= u32::from(bits[at]) << bit;
                at += 1;
            }
        }
    }
    Some(words)
}

fn decode_phase(
    bits: &[bool],
    mode: Mode,
    cycle: u8,
    frame: u8,
    fiw_errors: u32,
    phase: usize,
    out: &mut ChannelOutputs,
) {
    let Some(encoded) = deinterleave(bits) else {
        return;
    };
    let mut words = [0u32; WORDS];
    let mut damaged = [false; WORDS];
    let mut errors = fiw_errors;
    for ((destination, broken), encoded) in words.iter_mut().zip(&mut damaged).zip(encoded) {
        match decode_word(encoded) {
            Some((data, repaired)) => {
                *destination = data;
                errors += repaired;
            }
            None => *broken = true,
        }
    }
    if damaged[0] {
        return;
    }
    let biw = words[0];
    let address_start = usize::from(((biw >> 8) & 3) as u8) + 1;
    let vector_start = usize::from(((biw >> 10) & 0x3F) as u8);
    if address_start >= vector_start || vector_start >= WORDS {
        return;
    }
    for address_index in address_start..vector_start {
        let vector_index = vector_start + address_index - address_start;
        let Some(&vector) = words.get(vector_index) else {
            break;
        };
        let address_word = words[address_index];
        if damaged[address_index]
            || damaged[vector_index]
            || matches!(address_word, 0 | 0x1F_FFFF)
            || !checksum(vector)
        {
            continue;
        }
        let address = u64::from(address_word.wrapping_sub(0x8000));
        let kind = (vector >> 4) & 7;
        let start = usize::from(((vector >> 7) & 0x7F) as u8);
        let len = usize::from(((vector >> 14) & 0x7F) as u8);
        let (payload, text) = match kind {
            2 => (PagerPayload::Tone, String::new()),
            3 | 4 | 7 => (PagerPayload::Numeric, numeric(&words, vector_index, kind)),
            5 => (PagerPayload::Alpha, alpha(&words, start, len)),
            6 => (PagerPayload::Binary, binary(&words, start, len)),
            _ => continue,
        };
        if content(vector, kind).any(|index| damaged.get(index).is_none_or(|&broken| broken)) {
            continue;
        }
        out.events.push(DecoderEvent::Flex(FlexMessage {
            address,
            payload,
            text,
            baud: mode.payload_baud(),
            levels: mode.levels,
            cycle,
            frame,
            phase: char::from(b'A' + phase as u8),
            errors_corrected: errors,
        }));
    }
}

fn content(vector: u32, kind: u32) -> std::ops::Range<usize> {
    let start = ((vector >> 7) & 0x7F) as usize;
    match kind {
        3 | 4 | 7 => start..start + ((vector >> 14) & 7) as usize + 1,
        5 | 6 => start..start + ((vector >> 14) & 0x7F) as usize,
        _ => 0..0,
    }
}

fn alpha(words: &[u32], start: usize, len: usize) -> String {
    if len < 2 || start >= words.len() {
        return String::new();
    }
    let fragment = words[start] >> 11 & 3;
    let mut text = String::new();
    for (index, &word) in words.iter().skip(start + 1).take(len - 1).enumerate() {
        for slot in 0..3 {
            if index == 0 && slot == 0 && fragment == 3 {
                continue;
            }
            let character = (word >> (slot * 7) & 0x7F) as u8;
            if character == 3 {
                return text;
            }
            if character.is_ascii() && !character.is_ascii_control() && text.len() < MAX_TEXT {
                text.push(char::from(character));
            }
        }
    }
    text.trim_end().to_owned()
}

fn numeric(words: &[u32], vector_index: usize, kind: u32) -> String {
    const ALPHABET: &[u8; 16] = b"0123456789 U -][";
    let vector = words[vector_index];
    let start = usize::from(((vector >> 7) & 0x7F) as u8);
    let end = start + usize::from(((vector >> 14) & 7) as u8);
    let mut stream = Vec::new();
    for &word in words.get(start..=end).unwrap_or_default() {
        stream.extend((0..21).map(|bit| word >> bit & 1 == 1));
    }
    let skip = if kind == 7 { 14 } else { 6 };
    stream
        .get(skip..)
        .unwrap_or_default()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| {
            chunk
                .iter()
                .rev()
                .fold(0usize, |value, bit| value << 1 | usize::from(*bit))
        })
        .filter(|&digit| digit != 12)
        .take(MAX_TEXT)
        .map(|digit| char::from(ALPHABET[digit]))
        .collect::<String>()
        .trim_end()
        .to_owned()
}

fn binary(words: &[u32], start: usize, len: usize) -> String {
    words
        .iter()
        .skip(start)
        .take(len)
        .map(|word| format!("{word:06X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

impl ChannelRx for FlexChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let params = params(&settings)?;
        check_params(params)?;
        Ok(Self {
            invert: params.invert,
            afc: Afc::new(RATE),
            slow: Bank::new(RATE, SEARCH_SPS),
            fast: Bank::new(RATE, FAST_SPS),
            history: [[0.0; TONES]; HISTORY],
            sample: 0,
            search_lanes: [SearchLane::default(); SEARCH_SPS],
            candidate: None,
            state: State::Search,
            timing: 0.0,
            symbol_samples: 0,
            symbol_target: SEARCH_SPS,
            polarity: false,
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let params = params(&settings)?;
        check_params(params)?;
        self.invert = params.invert;
        Ok(())
    }

    fn retuned(&mut self) {
        self.reset();
        self.afc = Afc::new(RATE);
        self.slow.reset();
        self.fast.reset();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        for &sample in iq {
            self.push_sample(sample, out);
            self.sample = self.sample.wrapping_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{synth, testutil::settings};

    #[test]
    fn decodes_all_flex_modes_from_recorded_iq_in_ragged_blocks() {
        let page = synth::flex::Page {
            address: 123_456,
            text: "FLEX ALPHA PAGE".to_owned(),
        };
        for (mode, baud, levels) in [
            (synth::flex::Mode::Flex1600_2, 1_600, 2),
            (synth::flex::Mode::Flex1600_4, 3_200, 4),
            (synth::flex::Mode::Flex3200_2, 3_200, 2),
            (synth::flex::Mode::Flex3200_4, 6_400, 4),
        ] {
            let iq = synth::flex::transmission_mode(&page, 7, 83, RATE, mode);
            let mut channel = FlexChannel::new(
                ChannelCtx { input_rate: RATE },
                settings(ChannelParams::Flex(FlexParams::default())),
            )
            .unwrap();
            let mut out = ChannelOutputs::default();
            for chunk in iq.chunks(997) {
                channel.process(chunk, &mut out);
            }
            let messages = out
                .events
                .iter()
                .filter_map(|event| match event {
                    DecoderEvent::Flex(message) => Some(message),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(messages.len(), 1, "{mode:?}");
            assert_eq!(messages[0].address, 123_456);
            assert_eq!(messages[0].text, "FLEX ALPHA PAGE");
            assert_eq!(messages[0].baud, baud);
            assert_eq!(messages[0].levels, levels);
            assert_eq!(messages[0].cycle, 7);
            assert_eq!(messages[0].frame, 83);
        }
    }

    fn page() -> synth::flex::Page {
        synth::flex::Page {
            address: 123_456,
            text: "FLEX ALPHA PAGE".to_owned(),
        }
    }

    fn received(iq: &[Complex<f32>]) -> Vec<FlexMessage> {
        let Ok(ChannelFilter::Symmetric(mut filter)) = channel_filter(&FlexParams::default())
        else {
            panic!("flex filters symmetrically");
        };
        let mut filtered = Vec::new();
        filter.process(iq, &mut filtered);
        let mut channel = FlexChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Flex(FlexParams::default())),
        )
        .unwrap();
        let mut out = ChannelOutputs::default();
        for chunk in filtered.chunks(997) {
            channel.process(chunk, &mut out);
        }
        out.events
            .into_iter()
            .filter_map(|event| match event {
                DecoderEvent::Flex(message) => Some(message),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_detuned_pager_with_a_drifting_clock_decodes_in_noise() {
        for mode in [
            synth::flex::Mode::Flex1600_2,
            synth::flex::Mode::Flex1600_4,
            synth::flex::Mode::Flex3200_2,
            synth::flex::Mode::Flex3200_4,
        ] {
            for seed in [0x1357_9bdf, 0x2468_ace1] {
                let mut iq = synth::silence(3_000);
                iq.extend(synth::flex::transmission_mode(&page(), 7, 83, RATE, mode));
                iq.extend(synth::silence(3_000));
                let mut iq = synth::resample(&iq, RATE, RATE * 1.000_15);
                synth::shift(&mut iq, 800.0, RATE);
                synth::add_noise(&mut iq, seed, 1.5);
                let messages = received(&iq);
                assert!(
                    messages.iter().any(|m| m.text == "FLEX ALPHA PAGE"),
                    "{mode:?} seed {seed:#x}: {messages:?}"
                );
            }
        }
    }

    #[test]
    fn an_uncorrectable_word_drops_only_the_messages_that_use_it() {
        let mut iq = synth::flex::transmission(&page(), 7, 83, RATE);
        let data_start = (1_072 + 40) * 30;
        let last_word_bits = (10 * 256 + 7..10 * 256 + 7 + 3 * 8).step_by(8);
        for symbol in last_word_bits {
            let start = data_start + symbol * 30;
            for sample in &mut iq[start..start + 30] {
                *sample = sample.conj();
            }
        }
        let messages = received(&iq);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert_eq!(messages[0].text, "FLEX ALPHA PAGE");
    }
}
