use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass};
use sdrmm_modem::{
    constellation::tables,
    linear::{
        CarrierLoop, DifferentialDetector, LinearDemod, LinearParams, LinearTiming, PhaseDetector,
    },
    pulse::{self, Norm},
};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, PskParams, PskText,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const INPUT_RATE_HZ: f64 = 8_000.0;
const FILTER_TAPS: usize = 257;
const TEXT_FLUSH_CHARS: usize = 64;

static PSK31_DESCRIPTOR: LazyLock<ChannelDescriptor> =
    LazyLock::new(|| descriptor("psk31", "PSK31", 80.0));
static PSK63_DESCRIPTOR: LazyLock<ChannelDescriptor> =
    LazyLock::new(|| descriptor("psk63", "PSK63", 160.0));

fn descriptor(type_id: &str, name: &str, bandwidth_hz: f64) -> ChannelDescriptor {
    ChannelDescriptor {
        type_id: type_id.to_owned(),
        name: name.to_owned(),
        bandwidth_hz,
        input_rate_hz: INPUT_RATE_HZ,
        has_audio: false,
        decoder_kind: Some(type_id.to_owned()),
        ..ChannelDescriptor::default()
    }
}

pub(crate) fn baud(params: &ChannelParams) -> Result<f64, ChannelError> {
    match params {
        ChannelParams::Psk31(_) => Ok(31.25),
        ChannelParams::Psk63(_) => Ok(62.5),
        other => Err(ChannelError::InvalidSettings(format!(
            "PSK channel got {} params",
            other.type_id()
        ))),
    }
}

fn settings(params: &ChannelParams) -> Result<&PskParams, ChannelError> {
    match params {
        ChannelParams::Psk31(p) | ChannelParams::Psk63(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "PSK channel got {} params",
            other.type_id()
        ))),
    }
}

fn demodulator(baud: f64) -> Result<LinearDemod, ChannelError> {
    let sps = (INPUT_RATE_HZ / baud).round() as usize;
    let constellation = tables::psk(2)
        .map_err(|error| ChannelError::InvalidSettings(format!("BPSK table: {error}")))?;
    let pulse = pulse::root_raised_cosine(sps as f64, 1.0, 4, Norm::Energy);
    let params = LinearParams::new(constellation, pulse.clone(), sps)
        .map_err(|error| ChannelError::InvalidSettings(format!("PSK waveform: {error}")))?;
    let carrier = CarrierLoop::new(PhaseDetector::MthPower { m: 2 }, 0.01).with_frequency_aid(0.01);
    Ok(LinearDemod::new(
        &params,
        &pulse,
        LinearTiming::CONTINUOUS,
        Some(carrier),
    ))
}

pub(crate) fn occupied_band(params: &ChannelParams) -> (f64, f64) {
    let half = baud(params).unwrap_or(62.5) * 1.3;
    (-half, half)
}

pub(crate) fn channel_filter(params: &ChannelParams) -> Result<ChannelFilter, ChannelError> {
    let half = baud(params)? * 1.3;
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(FILTER_TAPS, half / INPUT_RATE_HZ),
        1,
    )))
}

struct PskChannel {
    demod: LinearDemod,
    differential: DifferentialDetector,
    decoder: VaricodeDecoder,
    invert: bool,
    symbols: Vec<Complex<f32>>,
    products: Vec<Complex<f32>>,
}

impl PskChannel {
    fn new(params: &ChannelParams) -> Result<Self, ChannelError> {
        Ok(Self {
            demod: demodulator(baud(params)?)?,
            differential: DifferentialDetector::new(),
            decoder: VaricodeDecoder::default(),
            invert: settings(params)?.invert,
            symbols: Vec::new(),
            products: Vec::new(),
        })
    }

    fn apply(&mut self, params: &ChannelParams) -> Result<(), ChannelError> {
        self.demod = demodulator(baud(params)?)?;
        self.invert = settings(params)?.invert;
        self.reset();
        Ok(())
    }

    fn reset(&mut self) {
        self.demod.reset();
        self.differential.reset();
        self.decoder = VaricodeDecoder::default();
        self.symbols.clear();
        self.products.clear();
    }

    fn process(
        &mut self,
        iq: &[Complex<f32>],
        out: &mut ChannelOutputs,
        event: fn(PskText) -> DecoderEvent,
    ) {
        self.symbols.clear();
        self.demod.process(iq, &mut self.symbols);
        self.products.clear();
        self.differential.process(&self.symbols, &mut self.products);
        for product in &self.products {
            let mut bit = product.re >= 0.0;
            if self.invert {
                bit = !bit;
            }
            if let Some(text) = self.decoder.feed(bit) {
                out.events.push(event(PskText { text }));
            }
        }
    }
}

#[derive(Default)]
struct VaricodeDecoder {
    code: u16,
    bits: u8,
    zero_run: u8,
    text: String,
}

impl VaricodeDecoder {
    fn feed(&mut self, bit: bool) -> Option<String> {
        if bit {
            if self.zero_run == 1 {
                self.push_bit(false);
            }
            self.zero_run = 0;
            self.push_bit(true);
        } else {
            self.zero_run = self.zero_run.saturating_add(1);
            if self.zero_run == 2 {
                self.finish_character();
            }
        }

        let line_end = self.text.ends_with('\n') || self.text.ends_with('\r');
        if !self.text.is_empty() && (line_end || self.text.chars().count() >= TEXT_FLUSH_CHARS) {
            Some(std::mem::take(&mut self.text))
        } else {
            None
        }
    }

    fn push_bit(&mut self, bit: bool) {
        if self.bits >= 10 {
            self.bits = 11;
            return;
        }
        self.code = (self.code << 1) | u16::from(bit);
        self.bits += 1;
    }

    fn finish_character(&mut self) {
        if self.bits <= 10
            && let Some(index) = VARICODE
                .iter()
                .position(|candidate| code_matches(candidate, self.code, self.bits))
            && let Some(ch) = char::from_u32(index as u32)
            && (ch == '\n' || ch == '\r' || !ch.is_control())
        {
            self.text.push(ch);
        }
        self.code = 0;
        self.bits = 0;
    }
}

fn code_matches(candidate: &str, code: u16, bits: u8) -> bool {
    candidate.len() == usize::from(bits)
        && candidate
            .bytes()
            .fold(0u16, |value, bit| (value << 1) | u16::from(bit == b'1'))
            == code
}

macro_rules! channel {
    ($name:ident, $variant:ident, $descriptor:ident, $event:ident) => {
        pub struct $name(PskChannel);

        impl ChannelRx for $name {
            fn descriptor() -> &'static ChannelDescriptor {
                &$descriptor
            }

            fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
                check_input_rate(ctx, &$descriptor)?;
                if !matches!(settings.params, ChannelParams::$variant(_)) {
                    return Err(ChannelError::InvalidSettings(format!(
                        "{} channel got {} params",
                        $descriptor.type_id,
                        settings.params.type_id()
                    )));
                }
                Ok(Self(PskChannel::new(&settings.params)?))
            }

            fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
                if !matches!(settings.params, ChannelParams::$variant(_)) {
                    return Err(ChannelError::InvalidSettings(format!(
                        "{} channel got {} params",
                        $descriptor.type_id,
                        settings.params.type_id()
                    )));
                }
                self.0.apply(&settings.params)
            }

            fn retuned(&mut self) {
                self.0.reset();
            }

            fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
                self.0.process(iq, out, DecoderEvent::$event);
            }
        }
    };
}

channel!(Psk31Channel, Psk31, PSK31_DESCRIPTOR, Psk31);
channel!(Psk63Channel, Psk63, PSK63_DESCRIPTOR, Psk63);

pub(crate) const VARICODE: [&str; 128] = [
    "1010101011",
    "1011011011",
    "1011101101",
    "1101110111",
    "1011101011",
    "1101011111",
    "1011101111",
    "1011111101",
    "1011111111",
    "11101111",
    "11101",
    "1101101111",
    "1011011101",
    "11111",
    "1101110101",
    "1110101011",
    "1011110111",
    "1011110101",
    "1110101101",
    "1110101111",
    "1101011011",
    "1101101011",
    "1101101101",
    "1101010111",
    "1101111011",
    "1101111101",
    "1110110111",
    "1101010101",
    "1101011101",
    "1110111011",
    "1011111011",
    "1101111111",
    "1",
    "111111111",
    "101011111",
    "111110101",
    "111011011",
    "1011010101",
    "1010111011",
    "101111111",
    "11111011",
    "11110111",
    "101101111",
    "111011111",
    "1110101",
    "110101",
    "1010111",
    "110101111",
    "10110111",
    "10111101",
    "11101101",
    "11111111",
    "101110111",
    "101011011",
    "101101011",
    "110101101",
    "110101011",
    "110110111",
    "11110101",
    "110111101",
    "111101101",
    "1010101",
    "111010111",
    "1010101111",
    "1010111101",
    "1111101",
    "11101011",
    "10101101",
    "10110101",
    "1110111",
    "11011011",
    "11111101",
    "101010101",
    "1111111",
    "111111101",
    "101111101",
    "11010111",
    "10111011",
    "11011101",
    "10101011",
    "11010101",
    "111011101",
    "10101111",
    "1101111",
    "1101101",
    "101010111",
    "110110101",
    "101011101",
    "101110101",
    "101111011",
    "1010101101",
    "111110111",
    "111101111",
    "111111011",
    "1010111111",
    "101101101",
    "1011011111",
    "1011",
    "1011111",
    "101111",
    "101101",
    "11",
    "111101",
    "1011011",
    "101011",
    "1101",
    "111101011",
    "10111111",
    "11011",
    "111011",
    "1111",
    "111",
    "111111",
    "110111111",
    "10101",
    "10111",
    "101",
    "110111",
    "1111011",
    "1101011",
    "11011111",
    "1011101",
    "111010101",
    "1010110111",
    "110111011",
    "1010110101",
    "1011010111",
    "1110110101",
];

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::{testgen, testutil::settings};

    #[test]
    fn varicode_is_complete_prefix_safe_and_unique() {
        assert_eq!(VARICODE.len(), 128);
        assert_eq!(VARICODE[usize::from(b' ')], "1");
        assert_eq!(VARICODE[usize::from(b'e')], "11");
        assert_eq!(VARICODE[usize::from(b'Z')], "1010101101");
        let unique: HashSet<&str> = VARICODE.into_iter().collect();
        assert_eq!(unique.len(), VARICODE.len());
        for code in VARICODE {
            assert!(code.starts_with('1') && code.ends_with('1'), "{code}");
            assert!(!code.contains("00"), "{code}");
            assert!(code.len() <= 10, "{code}");
        }
    }

    #[test]
    fn decoder_recovers_varicode_and_flushes_a_line() {
        let mut decoder = VaricodeDecoder::default();
        let mut output = None;
        for ch in "CQ de DL1ABC\n".bytes() {
            for bit in VARICODE[usize::from(ch)].bytes().chain(*b"00") {
                output = decoder.feed(bit == b'1').or(output);
            }
        }
        assert_eq!(output.as_deref(), Some("CQ de DL1ABC\n"));
    }

    fn decoded_text(params: ChannelParams, iq: &[Complex<f32>]) -> String {
        let mut channel = match params {
            ChannelParams::Psk31(p) => Box::new(
                Psk31Channel::new(
                    ChannelCtx {
                        input_rate: INPUT_RATE_HZ,
                    },
                    settings(ChannelParams::Psk31(p)),
                )
                .unwrap(),
            ) as Box<dyn ChannelRx>,
            ChannelParams::Psk63(p) => Box::new(
                Psk63Channel::new(
                    ChannelCtx {
                        input_rate: INPUT_RATE_HZ,
                    },
                    settings(ChannelParams::Psk63(p)),
                )
                .unwrap(),
            ),
            _ => unreachable!(),
        };
        let mut out = ChannelOutputs::default();
        for block in iq.chunks(997) {
            channel.process(block, &mut out);
        }
        out.events
            .iter()
            .filter_map(|event| match event {
                DecoderEvent::Psk31(text) | DecoderEvent::Psk63(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn psk31_and_psk63_fixtures_decode_across_ragged_blocks() {
        for (params, baud) in [
            (ChannelParams::Psk31(PskParams::default()), 31.25),
            (ChannelParams::Psk63(PskParams::default()), 62.5),
        ] {
            let iq = testgen::psk::transmission("CQ de DL1ABC\n", baud);
            let text = decoded_text(params, &iq);
            assert!(text.contains("DL1ABC"), "{baud} baud decoded {text:?}");
        }
    }
}
