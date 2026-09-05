mod sensors;
mod slicer;

#[cfg(test)]
mod tests;

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{
    Decimator, Envelope, FmDemod, KeyingSlicer, KeyingTiming, design_lowpass, flat_bandwidth_hz,
    one_pole_coeff,
};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, SubghzEncoding, SubghzFrame,
    SubghzModulation, SubghzParams,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const CHANNEL_TAPS: usize = 63;

const ENVELOPE_TAU_S: f64 = 20e-6;

const FSK_DEVIATION_HZ: f64 = 25_000.0;

const FSK_LEVEL_TAU_S: f64 = 50e-3;

const MAX_EDGES: usize = 2_048;

const MAX_REPORTED_TIMINGS: usize = 128;

const COLLAPSE_S: f64 = 0.5;

const MIN_BITS: usize = 8;

const QUANTIZE_TOLERANCE: f64 = 0.3;
const MAX_MULTIPLE: u32 = 4;

const TRI_STATE_BITS: usize = 24;
const EV1527_ADDRESS_BITS: usize = 20;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "subghz".to_owned(),
    name: "Sub-GHz".to_owned(),
    bandwidth_hz: 150_000.0,
    input_rate_hz: 250_000.0,
    has_audio: false,
    decoder_kind: Some("subghz".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct SubghzChannel {
    detector: Detector,
    timing: Timing,
    collapse: Collapse,
    rate: f64,
}

fn params(settings: &ChannelSettings) -> Result<&SubghzParams, ChannelError> {
    match &settings.params {
        ChannelParams::Subghz(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "subghz channel got {} params",
            other.type_id()
        ))),
    }
}

fn check_params(p: &SubghzParams) -> Result<(), ChannelError> {
    let rate = DESCRIPTOR.input_rate_hz;
    let widest = flat_bandwidth_hz(rate);
    if !(p.bandwidth_hz.is_finite() && p.bandwidth_hz > 0.0 && p.bandwidth_hz < widest) {
        return Err(ChannelError::InvalidSettings(format!(
            "subghz bandwidth must be in (0, {widest}) Hz, got {}",
            p.bandwidth_hz
        )));
    }
    if p.min_pulse_us == 0 || p.frame_gap_us <= p.min_pulse_us {
        return Err(ChannelError::InvalidSettings(format!(
            "subghz frame gap ({} µs) must exceed the minimum pulse ({} µs), and neither may be zero",
            p.frame_gap_us, p.min_pulse_us
        )));
    }
    Ok(())
}

pub(crate) fn occupied_band(p: &SubghzParams) -> (f64, f64) {
    let half = p.bandwidth_hz / 2.0;
    (-half, half)
}

pub(crate) fn channel_filter(p: &SubghzParams) -> Result<ChannelFilter, ChannelError> {
    check_params(p)?;
    let (_, half) = occupied_band(p);
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(CHANNEL_TAPS, half / DESCRIPTOR.input_rate_hz),
        1,
    )))
}

enum Detector {
    Ook {
        envelope: Envelope,
        slicer: KeyingSlicer,
    },
    Fsk {
        envelope: Envelope,
        slicer: KeyingSlicer,
        demod: FmDemod,
        demod_buf: Vec<f32>,
        level: f32,
        level_coeff: f32,
    },
}

impl Detector {
    fn new(modulation: SubghzModulation, rate: f64) -> Self {
        let envelope = Envelope::new(rate, ENVELOPE_TAU_S, ENVELOPE_TAU_S);
        let slicer = KeyingSlicer::with_timing(rate, KeyingTiming::BURST);
        match modulation {
            SubghzModulation::Ook => Self::Ook { envelope, slicer },
            SubghzModulation::Fsk => Self::Fsk {
                envelope,
                slicer,
                demod: FmDemod::new(rate, FSK_DEVIATION_HZ),
                demod_buf: Vec::new(),
                level: 0.0,
                level_coeff: one_pole_coeff(rate, FSK_LEVEL_TAU_S),
            },
        }
    }

    fn process(&mut self, iq: &[Complex<f32>], keyed: &mut Vec<bool>) {
        keyed.clear();
        match self {
            Self::Ook { envelope, slicer } => {
                for s in iq {
                    keyed.push(slicer.push(envelope.push(s.norm())));
                }
            }
            Self::Fsk {
                envelope,
                slicer,
                demod,
                demod_buf,
                level,
                level_coeff,
            } => {
                demod.process(iq, demod_buf);
                if !level.is_finite() {
                    *level = 0.0;
                }
                for (s, &tone) in iq.iter().zip(demod_buf.iter()) {
                    let carrier = slicer.push(envelope.push(s.norm()));
                    if !tone.is_finite() {
                        keyed.push(false);
                        continue;
                    }
                    if carrier {
                        *level += *level_coeff * (tone - *level);
                    }
                    keyed.push(carrier && tone > *level);
                }
            }
        }
    }
}

struct Timing {
    key: bool,
    run: u32,
    candidate: u32,
    min_pulse: u32,
    frame_gap: u32,
    edges: Vec<u32>,
    overflowed: bool,
}

impl Timing {
    fn new(p: &SubghzParams, rate: f64) -> Self {
        let samples = |us: u32| ((f64::from(us) * 1e-6 * rate).round() as u32).max(1);
        Self {
            key: false,
            run: 0,
            candidate: 0,
            min_pulse: samples(p.min_pulse_us),
            frame_gap: samples(p.frame_gap_us),
            edges: Vec::with_capacity(MAX_EDGES),
            overflowed: false,
        }
    }

    fn edges(&self) -> &[u32] {
        &self.edges
    }

    fn push(&mut self, key: bool) -> bool {
        self.run = self.run.saturating_add(1);
        if key == self.key {
            self.candidate = 0;
        } else {
            self.candidate += 1;
            if self.candidate >= self.min_pulse {
                let held = self.run - self.candidate;
                if self.key || !self.edges.is_empty() {
                    if self.edges.len() >= MAX_EDGES {
                        self.overflowed = true;
                    } else {
                        self.edges.push(held);
                    }
                }
                self.key = key;
                self.run = self.candidate;
                self.candidate = 0;
            }
            return false;
        }

        !self.key && self.run >= self.frame_gap && !self.edges.is_empty()
    }

    fn clear_frame(&mut self) {
        self.edges.clear();
        self.overflowed = false;
    }

    fn reset(&mut self) {
        self.key = false;
        self.run = 0;
        self.candidate = 0;
        self.edges.clear();
        self.overflowed = false;
    }
}

struct Collapse {
    pending: Option<SubghzFrame>,
    since: u32,
    window: u32,
}

impl Collapse {
    fn new(rate: f64) -> Self {
        Self {
            pending: None,
            since: 0,
            window: (COLLAPSE_S * rate) as u32,
        }
    }

    fn offer(&mut self, frame: SubghzFrame, out: &mut ChannelOutputs) {
        self.since = 0;
        match &mut self.pending {
            Some(held) if held.data == frame.data && held.encoding == frame.encoding => {
                held.repeats = held.repeats.saturating_add(frame.repeats);
            }
            Some(held) if rank(&frame) > rank(held) && held.repeats == 1 => {
                self.pending = Some(frame);
            }
            Some(held) if rank(&frame) <= rank(held) && held.repeats > 1 => {}
            Some(_) => {
                self.flush(out);
                self.pending = Some(frame);
            }
            None => self.pending = Some(frame),
        }
    }

    fn tick(&mut self, samples: u32, out: &mut ChannelOutputs) {
        if self.pending.is_none() {
            return;
        }
        self.since = self.since.saturating_add(samples);
        if self.since >= self.window {
            self.flush(out);
        }
    }

    fn flush(&mut self, out: &mut ChannelOutputs) {
        if let Some(frame) = self.pending.take() {
            out.events.push(DecoderEvent::Subghz(frame));
        }
        self.since = 0;
    }
}

fn rank(frame: &SubghzFrame) -> (u8, u8, u32) {
    let read = u8::from(frame.reading.is_some());
    let named = u8::from(frame.encoding != SubghzEncoding::Raw);
    (read, named, frame.bits)
}

impl ChannelRx for SubghzChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        check_params(p)?;
        Ok(Self {
            detector: Detector::new(p.modulation, ctx.input_rate),
            timing: Timing::new(p, ctx.input_rate),
            collapse: Collapse::new(ctx.input_rate),
            rate: ctx.input_rate,
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        check_params(p)?;
        self.detector = Detector::new(p.modulation, self.rate);
        self.timing = Timing::new(p, self.rate);
        self.timing.reset();
        Ok(())
    }

    fn retuned(&mut self) {
        self.timing.reset();
        self.collapse.pending = None;
        self.collapse.since = 0;
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        let mut keyed = Vec::new();
        self.detector.process(iq, &mut keyed);
        let modulation = modulation_of(&self.detector);
        for &key in &keyed {
            if !self.timing.push(key) {
                continue;
            }
            let frame = (!self.timing.overflowed)
                .then(|| classify(self.timing.edges(), self.rate, modulation));
            self.timing.clear_frame();
            if let Some(frame) = frame {
                self.collapse.offer(frame, out);
            }
        }
        self.collapse.tick(keyed.len() as u32, out);
    }
}

fn modulation_of(detector: &Detector) -> SubghzModulation {
    match detector {
        Detector::Ook { .. } => SubghzModulation::Ook,
        Detector::Fsk { .. } => SubghzModulation::Fsk,
    }
}

fn base_period(edges: &[u32]) -> Option<u32> {
    let min = edges.iter().copied().min()?;
    let cluster: Vec<u32> = edges
        .iter()
        .copied()
        .filter(|&d| u64::from(d) * 2 < u64::from(min) * 3)
        .collect();
    let sum: u64 = cluster.iter().map(|&d| u64::from(d)).sum();
    u32::try_from(sum / cluster.len().max(1) as u64)
        .ok()
        .filter(|&d| d > 0)
}

fn multiple(duration: u32, base: u32) -> Option<u32> {
    let ratio = f64::from(duration) / f64::from(base);
    let n = ratio.round();
    ((1.0..=f64::from(MAX_MULTIPLE)).contains(&n) && (ratio - n).abs() <= QUANTIZE_TOLERANCE)
        .then_some(n as u32)
}

fn classify(edges: &[u32], rate: f64, modulation: SubghzModulation) -> SubghzFrame {
    let to_us = |samples: u32| (f64::from(samples) * 1e6 / rate).round() as u32;
    let edges_us: Vec<u32> = edges.iter().map(|&d| to_us(d)).collect();
    let timings_us: Vec<u32> = edges_us
        .iter()
        .copied()
        .take(MAX_REPORTED_TIMINGS)
        .collect();
    let base = base_period(edges);
    let raw = SubghzFrame {
        modulation,
        encoding: SubghzEncoding::Raw,
        bits: 0,
        data: String::new(),
        address: None,
        button: None,
        tri_state: None,
        reading: None,
        short_us: base.map_or(0, to_us),
        repeats: 1,
        timings_us,
    };
    if let Some(found) = sensors::identify(&edges_us) {
        return SubghzFrame {
            encoding: found.encoding,
            bits: found.bits.len() as u32,
            data: hex_of(&found.bits),
            reading: Some(found.reading),
            short_us: found.short_us,
            repeats: found.repeats,
            ..raw
        };
    }
    let Some(base) = base else { return raw };
    let Some(steps) = edges
        .iter()
        .map(|&d| multiple(d, base))
        .collect::<Option<Vec<u32>>>()
    else {
        return raw;
    };

    let (encoding, bits) = match pwm_bits(&steps) {
        Some(bits) => (SubghzEncoding::Pwm, bits),
        None => match manchester_bits(&steps) {
            Some(bits) => (SubghzEncoding::Manchester, bits),
            None => return raw,
        },
    };

    let ev1527 = (bits.len() == TRI_STATE_BITS).then(|| {
        let address = bits[..EV1527_ADDRESS_BITS]
            .iter()
            .fold(0u32, |acc, &b| (acc << 1) | u32::from(b));
        let button = bits[EV1527_ADDRESS_BITS..]
            .iter()
            .fold(0u8, |acc, &b| (acc << 1) | u8::from(b));
        (address, button)
    });

    SubghzFrame {
        encoding,
        bits: bits.len() as u32,
        data: hex_of(&bits),
        address: ev1527.map(|(address, _)| address),
        button: ev1527.map(|(_, button)| button),
        tri_state: tri_state(&bits),
        ..raw
    }
}

fn pwm_bits(steps: &[u32]) -> Option<Vec<bool>> {
    let pairs = steps.len() / 2;
    if pairs < MIN_BITS {
        return None;
    }
    let mut bits = Vec::with_capacity(pairs);
    let (cells, _) = steps[..pairs * 2].as_chunks::<2>();
    for &[pulse, gap] in cells {
        if pulse == gap {
            return None;
        }
        bits.push(pulse > gap);
    }
    Some(bits)
}

fn manchester_bits(steps: &[u32]) -> Option<Vec<bool>> {
    if steps.iter().any(|&n| n > 2) {
        return None;
    }
    let mut cells = Vec::with_capacity(steps.len() * 2);
    let mut level = true;
    for &n in steps {
        for _ in 0..n {
            cells.push(level);
        }
        level = !level;
    }
    (0..2)
        .filter_map(|offset| decode_cells(&cells[offset..]))
        .max_by_key(Vec::len)
        .filter(|bits| bits.len() >= MIN_BITS)
}

fn decode_cells(cells: &[bool]) -> Option<Vec<bool>> {
    let mut bits = Vec::with_capacity(cells.len() / 2);
    let (pairs, _) = cells.as_chunks::<2>();
    for &[first, second] in pairs {
        bits.push(sdrmm_dsp::manchester_decode(first, second)?);
    }
    Some(bits)
}

fn tri_state(bits: &[bool]) -> Option<String> {
    if bits.len() != TRI_STATE_BITS {
        return None;
    }
    let (pairs, _) = bits.as_chunks::<2>();
    pairs
        .iter()
        .map(|pair| match pair {
            [false, false] => Some('0'),
            [true, true] => Some('1'),
            [false, true] => Some('F'),
            [true, false] => None,
        })
        .collect()
}

fn hex_of(bits: &[bool]) -> String {
    let pad = (4 - bits.len() % 4) % 4;
    let mut out = String::with_capacity((bits.len() + pad) / 4);
    let mut nibble = 0u32;
    let mut filled = 0;
    for bit in std::iter::repeat_n(&false, pad).chain(bits) {
        nibble = (nibble << 1) | u32::from(*bit);
        filled += 1;
        if filled == 4 {
            out.push(
                char::from_digit(nibble, 16)
                    .unwrap_or('0')
                    .to_ascii_uppercase(),
            );
            nibble = 0;
            filled = 0;
        }
    }
    out
}
