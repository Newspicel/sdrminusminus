use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, SyncDetector, design_lowpass, ermes_bch_decode};
use sdrmm_modem::{
    cpm::{CpmDemod, CpmParams, Mapping, TIMING_BW_CONTINUOUS},
    pulse::{self, Norm},
};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, ErmesMessage, ErmesParams,
    PagerPayload,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const RATE: f64 = 48_000.0;
const BAUD: f64 = 3_125.0;
const DEVIATION_HZ: f64 = 4_687.5;
const CHANNEL_TAPS: usize = 129;
const SYNC: u32 = 0b10_00_10_10_00_10_00_00_10_10_00_00_10_10_10;
const APT: u32 = 0b10_01_00_11_10_00_01_10_00_10_00_11_10_01_00;
const DELIMITER: u32 = 0b11_01_01_01_11_10_01_11_11_10_11_10_11_10_11;
const MAX_MESSAGE_WORDS: usize = 1_024;
const MAX_TEXT: usize = 4_096;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "ermes".to_owned(),
    name: "ERMES pager".to_owned(),
    bandwidth_hz: 12_500.0,
    input_rate_hz: RATE,
    has_audio: false,
    decoder_kind: Some("ermes".to_owned()),
    ..ChannelDescriptor::default()
});

#[derive(Debug)]
enum State {
    Search,
    System {
        bits: usize,
        word: u32,
        words: usize,
    },
    Address {
        bits: usize,
        word: u32,
        terminated: bool,
    },
    Message {
        bits: Vec<bool>,
    },
}

pub struct ErmesChannel {
    invert: bool,
    mapping: Mapping,
    demod: CpmDemod,
    soft: Vec<f32>,
    sync: SyncDetector,
    state: State,
    message: Vec<u32>,
    message_errors: u32,
}

fn mapping() -> Mapping {
    Mapping::new(vec![-3.0, -1.0, 3.0, 1.0])
}

fn params(settings: &ChannelSettings) -> Result<&ErmesParams, ChannelError> {
    match &settings.params {
        ChannelParams::Ermes(params) => Ok(params),
        other => Err(ChannelError::InvalidSettings(format!(
            "ermes channel got {} params",
            other.type_id()
        ))),
    }
}

fn check_params(params: &ErmesParams) -> Result<(), ChannelError> {
    if params.bandwidth_hz.is_finite()
        && params.bandwidth_hz >= 10_000.0
        && params.bandwidth_hz < RATE
    {
        Ok(())
    } else {
        Err(ChannelError::InvalidSettings(format!(
            "ermes bandwidth must be in [10000, {RATE}) Hz, got {}",
            params.bandwidth_hz
        )))
    }
}

pub(crate) fn occupied_band(params: &ErmesParams) -> (f64, f64) {
    let half = params.bandwidth_hz / 2.0;
    (-half, half)
}

pub(crate) fn channel_filter(params: &ErmesParams) -> Result<ChannelFilter, ChannelError> {
    check_params(params)?;
    let half = params.bandwidth_hz / 2.0;
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(CHANNEL_TAPS, half / RATE),
        1,
    )))
}

impl ErmesChannel {
    fn reset(&mut self) {
        self.state = State::Search;
        self.message.clear();
        self.message_errors = 0;
    }

    fn bit(&mut self, bit: bool, out: &mut ChannelOutputs) {
        let bit = bit ^ self.invert;
        if self.sync.push(bit) {
            self.state = State::System {
                bits: 0,
                word: 0,
                words: 0,
            };
            self.message.clear();
            self.message_errors = 0;
            return;
        }
        match &mut self.state {
            State::Search => {}
            State::System { bits, word, words } => {
                *word = *word << 1 | u32::from(bit);
                *bits += 1;
                if *bits == 30 {
                    if ermes_bch_decode(*word).is_none() {
                        self.reset();
                    } else {
                        *words += 1;
                        *bits = 0;
                        *word = 0;
                        if *words == 3 {
                            self.state = State::Address {
                                bits: 0,
                                word: 0,
                                terminated: false,
                            };
                        }
                    }
                }
            }
            State::Address {
                bits,
                word,
                terminated,
            } => {
                *word = *word << 1 | u32::from(bit);
                *bits += 1;
                if *bits == 30 {
                    if *word == APT {
                        *terminated = true;
                        *bits = 0;
                        *word = 0;
                    } else if !*terminated && ermes_bch_decode(*word).is_some() {
                        *bits = 0;
                        *word = 0;
                    } else {
                        let mut block = Vec::with_capacity(270);
                        block.extend((0..30).rev().map(|position| *word >> position & 1 == 1));
                        self.state = State::Message { bits: block };
                    }
                }
            }
            State::Message { bits } => {
                if bits.len() < 270 {
                    bits.push(bit);
                }
                if bits.len() == 270 {
                    let block = std::mem::take(bits);
                    self.block(&block, out);
                    if let State::Message { bits } = &mut self.state {
                        bits.reserve(270);
                    }
                }
            }
        }
    }

    fn block(&mut self, bits: &[bool], out: &mut ChannelOutputs) {
        let mut words = [0u32; 9];
        let mut at = 0;
        for _ in 0..30 {
            for word in &mut words {
                *word = *word << 1 | u32::from(bits[at]);
                at += 1;
            }
        }
        for word in words {
            if word == DELIMITER {
                self.flush(out);
                continue;
            }
            let Some((word, errors)) = ermes_bch_decode(word) else {
                self.message.clear();
                self.message_errors = 0;
                continue;
            };
            if self.message.len() == MAX_MESSAGE_WORDS {
                self.message.clear();
                self.message_errors = 0;
                continue;
            }
            self.message.push(word >> 12);
            self.message_errors += errors;
        }
    }

    fn flush(&mut self, out: &mut ChannelOutputs) {
        if self.message.len() < 2 {
            self.message.clear();
            self.message_errors = 0;
            return;
        }
        let header = u64::from(self.message[0]) << 18 | u64::from(self.message[1]);
        let local_address = (header >> 14) as u32;
        let message_number = (header >> 9 & 0x1F) as u8;
        let additional = header >> 7 & 1 == 1;
        let vif = (header & 0x7F) as u8;
        if additional {
            self.message.clear();
            self.message_errors = 0;
            return;
        }
        let payload = match vif >> 4 & 3 {
            0 => PagerPayload::Tone,
            1 => PagerPayload::Numeric,
            2 => PagerPayload::Alpha,
            _ => PagerPayload::Binary,
        };
        let text = match payload {
            PagerPayload::Tone => String::new(),
            PagerPayload::Numeric => decode_numeric(&self.message[2..]),
            PagerPayload::Alpha => decode_alpha(&self.message[2..]),
            PagerPayload::Binary => decode_binary(&self.message[2..]),
        };
        out.events.push(DecoderEvent::Ermes(ErmesMessage {
            local_address,
            message_number,
            payload,
            text,
            urgent: vif >> 3 & 1 == 1,
            alert: vif & 7,
            errors_corrected: self.message_errors,
        }));
        self.message.clear();
        self.message_errors = 0;
    }
}

fn data_bits(words: &[u32]) -> impl Iterator<Item = bool> + '_ {
    words
        .iter()
        .flat_map(|word| (0..18).rev().map(move |bit| word >> bit & 1 == 1))
}

fn decode_alpha(words: &[u32]) -> String {
    let bits = data_bits(words).collect::<Vec<_>>();
    let mut text = String::new();
    for character in bits.as_chunks::<7>().0 {
        let value = character
            .iter()
            .fold(0u8, |value, bit| value << 1 | u8::from(*bit));
        if value == 0x11 || text.len() == MAX_TEXT {
            break;
        }
        if value.is_ascii() && !value.is_ascii_control() {
            text.push(char::from(value));
        }
    }
    text.trim_end().to_owned()
}

fn decode_numeric(words: &[u32]) -> String {
    const ALPHABET: &[u8; 16] = b"0123456789*U -)(";
    data_bits(words)
        .collect::<Vec<_>>()
        .as_chunks::<4>()
        .0
        .iter()
        .take(MAX_TEXT)
        .map(|digit| {
            digit
                .iter()
                .fold(0usize, |value, bit| value << 1 | usize::from(*bit))
        })
        .map(|digit| char::from(ALPHABET[digit]))
        .collect::<String>()
        .trim_end()
        .to_owned()
}

fn decode_binary(words: &[u32]) -> String {
    words
        .iter()
        .map(|word| format!("{word:05X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

impl ChannelRx for ErmesChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let configured = params(&settings)?;
        check_params(configured)?;
        let mapping = mapping();
        let sps = RATE / BAUD;
        let params = CpmParams::from_deviation(
            mapping.clone(),
            DEVIATION_HZ,
            BAUD,
            pulse::rect(sps, Norm::Area),
            sps,
        );
        Ok(Self {
            invert: configured.invert,
            mapping,
            demod: CpmDemod::new(&params, &pulse::rect(sps, Norm::Area), TIMING_BW_CONTINUOUS),
            soft: Vec::new(),
            sync: SyncDetector::new(u64::from(SYNC), 30, 2),
            state: State::Search,
            message: Vec::new(),
            message_errors: 0,
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
        self.sync.reset();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        let mut soft = std::mem::take(&mut self.soft);
        self.demod.process(iq, &mut soft);
        for symbol in soft.drain(..) {
            let symbol = self.mapping.slice(symbol);
            self.bit(symbol >> 1 & 1 == 1, out);
            self.bit(symbol & 1 == 1, out);
        }
        self.soft = soft;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{testgen, testutil::settings};

    #[test]
    fn decodes_recorded_ermes_iq_in_ragged_blocks() {
        let page = testgen::ermes::Page {
            local_address: 234_567,
            message_number: 3,
            text: "ERMES ALPHA PAGE".to_owned(),
            urgent: true,
            alert: 5,
        };
        let iq = testgen::ermes::transmission(&page, RATE);
        let mut exact = ErmesChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Ermes(ErmesParams::default())),
        )
        .unwrap();
        let mut exact_out = ChannelOutputs::default();
        for bit in testgen::ermes::bits(&page) {
            exact.bit(bit, &mut exact_out);
        }
        assert_eq!(
            exact_out.events.len(),
            1,
            "exact bitstream state {:?} message {:?}",
            exact.state,
            exact.message
        );
        let mut channel = ErmesChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Ermes(ErmesParams::default())),
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
                DecoderEvent::Ermes(message) => Some(message),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].local_address, 234_567);
        assert_eq!(messages[0].message_number, 3);
        assert_eq!(messages[0].text, "ERMES ALPHA PAGE");
        assert!(messages[0].urgent);
        assert_eq!(messages[0].alert, 5);
    }
}
