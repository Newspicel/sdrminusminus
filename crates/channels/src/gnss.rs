use std::{
    f32::consts::TAU,
    sync::{Arc, LazyLock},
};

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, GnssFrame, GnssParams,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const RATE: f64 = 2_048_000.0;
const SAMPLES_PER_MS: usize = 2_048;
const CHIPS: usize = 1_023;
const DOPPLER_STEP_HZ: i32 = 500;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "gnss".to_owned(),
    name: "GNSS lab (GPS L1 C/A)".to_owned(),
    bandwidth_hz: 2_046_000.0,
    input_rate_hz: RATE,
    native_rate_max_hz: Some(RATE),
    has_audio: false,
    decoder_kind: Some("gnss".to_owned()),
    ..ChannelDescriptor::default()
});

#[derive(Clone, Copy)]
struct Lock {
    doppler_hz: f32,
    code_phase: usize,
    cn0_db_hz: f32,
    carrier_phase: f32,
}

pub struct GnssChannel {
    params: GnssParams,
    code: Vec<f32>,
    code_fft: Vec<Complex<f32>>,
    samples: Vec<Complex<f32>>,
    fft_buf: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    lock: Option<Lock>,
    acquisition_wait_ms: u8,
    prompt_ms: Vec<f32>,
    bit_phase: Option<usize>,
    nav: NavDecoder,
}

fn params(settings: &ChannelSettings) -> Result<&GnssParams, ChannelError> {
    match &settings.params {
        ChannelParams::Gnss(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "gnss channel got {} params",
            other.type_id()
        ))),
    }
}

fn check_params(p: &GnssParams) -> Result<(), ChannelError> {
    if !(1..=32).contains(&p.prn) {
        return Err(ChannelError::InvalidSettings(format!(
            "GPS L1 C/A PRN must be 1–32, got {}",
            p.prn
        )));
    }
    if p.doppler_hz > 20_000 || p.doppler_hz < 500 {
        return Err(ChannelError::InvalidSettings(format!(
            "GNSS Doppler search must be 500–20000 Hz, got {}",
            p.doppler_hz
        )));
    }
    if !(p.threshold.is_finite() && (1.5..=100.0).contains(&p.threshold)) {
        return Err(ChannelError::InvalidSettings(format!(
            "GNSS acquisition threshold must be 1.5–100, got {}",
            p.threshold
        )));
    }
    Ok(())
}

pub(crate) fn occupied_band() -> (f64, f64) {
    (-1_023_000.0, 1_023_000.0)
}

pub(crate) fn channel_filter() -> ChannelFilter {
    ChannelFilter::Passthrough
}

impl ChannelRx for GnssChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = *params(&settings)?;
        check_params(&p)?;
        Ok(Self::build(p))
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = *params(&settings)?;
        check_params(&p)?;
        if p != self.params {
            *self = Self::build(p);
        }
        Ok(())
    }

    fn retuned(&mut self) {
        self.lock = None;
        self.acquisition_wait_ms = 0;
        self.samples.clear();
        self.prompt_ms.clear();
        self.bit_phase = None;
        self.nav.clear();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        let mut rest = iq;
        while !rest.is_empty() {
            let take = (SAMPLES_PER_MS - self.samples.len()).min(rest.len());
            self.samples.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.samples.len() == SAMPLES_PER_MS {
                self.process_millisecond(out);
                self.samples.clear();
            }
        }
    }
}

impl GnssChannel {
    fn build(params: GnssParams) -> Self {
        let code = sampled_code(params.prn);
        let mut code_fft: Vec<_> = code.iter().map(|&v| Complex::new(v, 0.0)).collect();
        let mut planner = FftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(SAMPLES_PER_MS);
        let inverse = planner.plan_fft_inverse(SAMPLES_PER_MS);
        let mut setup_scratch = vec![Complex::default(); forward.get_inplace_scratch_len()];
        forward.process_with_scratch(&mut code_fft, &mut setup_scratch);
        let scratch_len = forward
            .get_inplace_scratch_len()
            .max(inverse.get_inplace_scratch_len());
        Self {
            params,
            code,
            code_fft,
            samples: Vec::with_capacity(SAMPLES_PER_MS),
            fft_buf: vec![Complex::default(); SAMPLES_PER_MS],
            scratch: vec![Complex::default(); scratch_len],
            forward,
            inverse,
            lock: None,
            acquisition_wait_ms: 0,
            prompt_ms: Vec::with_capacity(6_400),
            bit_phase: None,
            nav: NavDecoder::default(),
        }
    }

    fn process_millisecond(&mut self, out: &mut ChannelOutputs) {
        if self.lock.is_none() {
            if self.acquisition_wait_ms > 0 {
                self.acquisition_wait_ms -= 1;
                return;
            }
            self.acquisition_wait_ms = 19;
            if let Some(found) = self.acquire() {
                self.lock = Some(found);
                out.events.push(DecoderEvent::Gnss(self.event(found, None)));
            }
            return;
        }
        let mut lock = self.lock.unwrap_or(Lock {
            doppler_hz: 0.0,
            code_phase: 0,
            cn0_db_hz: 0.0,
            carrier_phase: 0.0,
        });
        let prompt = prompt(
            &self.samples,
            &self.code,
            lock.code_phase,
            lock.doppler_hz,
            lock.carrier_phase,
        );
        let error = prompt.im.atan2(prompt.re.abs().max(1e-9));
        lock.doppler_hz += error * 20.0;
        lock.carrier_phase =
            wrap_phase(lock.carrier_phase - TAU * lock.doppler_hz / 1_000.0 + error * 0.1);
        self.prompt_ms.push(prompt.re);
        self.lock = Some(lock);
        self.extract_nav(lock, out);
    }

    fn acquire(&mut self) -> Option<Lock> {
        let mut best_power = 0.0_f32;
        let mut best_phase = 0;
        let mut best_doppler = 0;
        let mut best_carrier_phase = 0.0;
        let mut floor_sum = 0.0_f64;
        let mut floor_count = 0_u64;
        let span = self.params.doppler_hz as i32;
        for doppler in (-span..=span).step_by(DOPPLER_STEP_HZ as usize) {
            wipe(&self.samples, doppler as f32, &mut self.fft_buf);
            self.forward
                .process_with_scratch(&mut self.fft_buf, &mut self.scratch);
            for (bin, code) in self.fft_buf.iter_mut().zip(&self.code_fft) {
                *bin *= code.conj();
            }
            self.inverse
                .process_with_scratch(&mut self.fft_buf, &mut self.scratch);
            for (phase, value) in self.fft_buf.iter().enumerate() {
                let power = value.norm_sqr();
                floor_sum += f64::from(power);
                floor_count += 1;
                if power > best_power {
                    best_power = power;
                    best_phase = (SAMPLES_PER_MS - phase) % SAMPLES_PER_MS;
                    best_doppler = doppler;
                    best_carrier_phase = -value.arg();
                }
            }
        }
        let floor = (floor_sum / floor_count.max(1) as f64) as f32;
        let ratio = best_power / floor.max(f32::MIN_POSITIVE);
        (ratio >= self.params.threshold).then(|| Lock {
            doppler_hz: best_doppler as f32,
            code_phase: best_phase,
            cn0_db_hz: (10.0 * ((ratio - 1.0).max(1e-6) * 1_000.0).log10()).clamp(0.0, 65.0),
            carrier_phase: best_carrier_phase,
        })
    }

    fn extract_nav(&mut self, lock: Lock, out: &mut ChannelOutputs) {
        if self.bit_phase.is_none() && self.prompt_ms.len() >= 6_160 {
            self.bit_phase = find_bit_phase(&self.prompt_ms);
            if let Some(phase) = self.bit_phase {
                let chunks = self.prompt_ms[phase..].as_chunks::<20>().0;
                for chunk in chunks {
                    if let Some(nav) = self.nav.feed(sum(chunk) >= 0.0) {
                        out.events
                            .push(DecoderEvent::Gnss(self.event(lock, Some(nav))));
                    }
                }
                let consumed = phase + ((self.prompt_ms.len() - phase) / 20) * 20;
                self.prompt_ms.drain(..consumed);
            } else {
                self.prompt_ms.drain(..3_000);
            }
            return;
        }
        if self.bit_phase.is_some() {
            while self.prompt_ms.len() >= 20 {
                let bit = sum(&self.prompt_ms[..20]) >= 0.0;
                self.prompt_ms.drain(..20);
                if let Some(nav) = self.nav.feed(bit) {
                    out.events
                        .push(DecoderEvent::Gnss(self.event(lock, Some(nav))));
                }
            }
        }
    }

    fn event(&self, lock: Lock, nav: Option<NavFrame>) -> GnssFrame {
        let (subframe, tow_seconds, week, words) = nav
            .map_or((None, None, None, Vec::new()), |n| {
                (Some(n.subframe), Some(n.tow_seconds), n.week, n.words)
            });
        GnssFrame {
            prn: self.params.prn,
            doppler_hz: lock.doppler_hz,
            code_phase_chips: lock.code_phase as f32 * CHIPS as f32 / SAMPLES_PER_MS as f32,
            cn0_db_hz: lock.cn0_db_hz,
            subframe,
            tow_seconds,
            week,
            words,
        }
    }
}

fn wipe(input: &[Complex<f32>], doppler_hz: f32, out: &mut [Complex<f32>]) {
    let step = Complex::from_polar(1.0, -TAU * doppler_hz / RATE as f32);
    let mut carrier = Complex::new(1.0, 0.0);
    for (slot, &sample) in out.iter_mut().zip(input) {
        *slot = sample * carrier;
        carrier *= step;
    }
}

fn prompt(
    input: &[Complex<f32>],
    code: &[f32],
    phase: usize,
    doppler_hz: f32,
    carrier_phase: f32,
) -> Complex<f32> {
    let step = Complex::from_polar(1.0, -TAU * doppler_hz / RATE as f32);
    let mut carrier = Complex::from_polar(1.0, carrier_phase);
    let mut value = Complex::default();
    for (n, &sample) in input.iter().enumerate() {
        value += sample * carrier * code[(n + phase) % SAMPLES_PER_MS];
        carrier *= step;
    }
    value
}

fn wrap_phase(phase: f32) -> f32 {
    (phase + std::f32::consts::PI).rem_euclid(TAU) - std::f32::consts::PI
}

pub(crate) fn sampled_code(prn: u8) -> Vec<f32> {
    let chips = ca_code(prn);
    (0..SAMPLES_PER_MS)
        .map(|n| chips[n * CHIPS / SAMPLES_PER_MS])
        .collect()
}

fn ca_code(prn: u8) -> [f32; CHIPS] {
    const TAPS: [(usize, usize); 32] = [
        (2, 6),
        (3, 7),
        (4, 8),
        (5, 9),
        (1, 9),
        (2, 10),
        (1, 8),
        (2, 9),
        (3, 10),
        (2, 3),
        (3, 4),
        (5, 6),
        (6, 7),
        (7, 8),
        (8, 9),
        (9, 10),
        (1, 4),
        (2, 5),
        (3, 6),
        (4, 7),
        (5, 8),
        (6, 9),
        (1, 3),
        (4, 6),
        (5, 7),
        (6, 8),
        (7, 9),
        (8, 10),
        (1, 6),
        (2, 7),
        (3, 8),
        (4, 9),
    ];
    let (tap1, tap2) = TAPS[usize::from(prn.saturating_sub(1).min(31))];
    let mut g1 = [true; 10];
    let mut g2 = [true; 10];
    let mut out = [0.0; CHIPS];
    for chip in &mut out {
        let bit = g1[9] ^ g2[tap1 - 1] ^ g2[tap2 - 1];
        *chip = if bit { -1.0 } else { 1.0 };
        let g1_feedback = g1[2] ^ g1[9];
        let g2_feedback = g2[1] ^ g2[2] ^ g2[5] ^ g2[7] ^ g2[8] ^ g2[9];
        g1.copy_within(0..9, 1);
        g2.copy_within(0..9, 1);
        g1[0] = g1_feedback;
        g2[0] = g2_feedback;
    }
    out
}

fn sum(values: &[f32]) -> f32 {
    values.iter().sum()
}

fn find_bit_phase(prompt: &[f32]) -> Option<usize> {
    for phase in 0..20 {
        let mut bits = [false; 310];
        let chunks = prompt[phase..].as_chunks::<20>().0;
        let count = chunks.len().min(bits.len());
        for (slot, chunk) in bits[..count].iter_mut().zip(chunks) {
            *slot = sum(chunk) >= 0.0;
        }
        for start in 0..=count.saturating_sub(300) {
            if decode_subframe(&bits[start..start + 300]).is_some() {
                return Some(phase);
            }
        }
    }
    None
}

#[derive(Default)]
struct NavDecoder {
    bits: Vec<bool>,
}

struct NavFrame {
    subframe: u8,
    tow_seconds: u32,
    week: Option<u16>,
    words: Vec<String>,
}

impl NavDecoder {
    fn clear(&mut self) {
        self.bits.clear();
    }

    fn feed(&mut self, bit: bool) -> Option<NavFrame> {
        self.bits.push(bit);
        while self.bits.len() >= 300 {
            if let Some(frame) = decode_subframe(&self.bits[..300]) {
                self.bits.drain(..300);
                return Some(frame);
            }
            self.bits.remove(0);
        }
        None
    }
}

fn decode_subframe(bits: &[bool]) -> Option<NavFrame> {
    let transmitted_preamble = byte(bits.get(..8)?)?;
    if transmitted_preamble != 0x8B && transmitted_preamble != !0x8B {
        return None;
    }
    let previous_d30 = transmitted_preamble != 0x8B;
    for previous_d29 in [false, true] {
        let mut d29 = previous_d29;
        let mut d30 = previous_d30;
        let mut data = [0_u32; 10];
        let mut raw = [0_u32; 10];
        let mut valid = true;
        for word_index in 0..10 {
            let word = word(bits.get(word_index * 30..(word_index + 1) * 30)?)?;
            raw[word_index] = word;
            let Some(decoded) = check_word(word, d29, d30) else {
                valid = false;
                break;
            };
            data[word_index] = decoded;
            d29 = word & 2 != 0;
            d30 = word & 1 != 0;
        }
        if valid {
            let how = data[1];
            let subframe = ((how >> 2) & 0x7) as u8;
            if !(1..=5).contains(&subframe) {
                continue;
            }
            let tow_seconds = ((how >> 7) & 0x1_FFFF) * 6;
            let week = (subframe == 1).then_some(((data[2] >> 14) & 0x3FF) as u16);
            return Some(NavFrame {
                subframe,
                tow_seconds,
                week,
                words: raw.iter().map(|value| format!("{value:08X}")).collect(),
            });
        }
    }
    None
}

fn byte(bits: &[bool]) -> Option<u8> {
    (bits.len() == 8).then(|| bits.iter().fold(0, |v, &bit| (v << 1) | u8::from(bit)))
}

fn word(bits: &[bool]) -> Option<u32> {
    (bits.len() == 30).then(|| bits.iter().fold(0, |v, &bit| (v << 1) | u32::from(bit)))
}

fn check_word(raw: u32, previous_d29: bool, previous_d30: bool) -> Option<u32> {
    let transmitted = raw >> 6;
    let data = if previous_d30 {
        transmitted ^ 0xFF_FFFF
    } else {
        transmitted
    };
    let d = |n: u8| data >> (24 - n) & 1 != 0;
    let x = |values: &[u8]| values.iter().fold(false, |v, &n| v ^ d(n));
    let expected = [
        previous_d29 ^ x(&[1, 2, 3, 5, 6, 10, 11, 12, 13, 14, 17, 18, 20, 23]),
        previous_d30 ^ x(&[2, 3, 4, 6, 7, 11, 12, 13, 14, 15, 18, 19, 21, 24]),
        previous_d29 ^ x(&[1, 3, 4, 5, 7, 8, 12, 13, 14, 15, 16, 19, 20, 22]),
        previous_d30 ^ x(&[2, 4, 5, 6, 8, 9, 13, 14, 15, 16, 17, 20, 21, 23]),
        previous_d30 ^ x(&[1, 3, 5, 6, 7, 9, 10, 14, 15, 16, 17, 18, 21, 22, 24]),
        previous_d29 ^ x(&[3, 5, 6, 8, 9, 10, 11, 13, 15, 19, 22, 23, 24]),
    ];
    let parity = expected
        .iter()
        .fold(0_u32, |v, &bit| (v << 1) | u32::from(bit));
    (parity == raw & 0x3F).then_some(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ca_codes_have_the_gold_code_balance_and_distinct_prns() {
        let one = ca_code(1);
        let two = ca_code(2);
        assert_eq!(one.iter().filter(|&&v| v > 0.0).count(), 511);
        assert_eq!(two.iter().filter(|&&v| v > 0.0).count(), 511);
        assert_ne!(one, two);
        let sidelobe: f32 = one
            .iter()
            .zip(one.iter().cycle().skip(1))
            .map(|(a, b)| a * b)
            .sum();
        assert_eq!(sidelobe, -1.0);
    }

    #[test]
    fn clean_prn_fixture_acquires_doppler_and_code_phase() {
        let params = GnssParams {
            prn: 7,
            doppler_hz: 2_000,
            threshold: 2.5,
        };
        let mut channel = GnssChannel::build(params);
        let code = sampled_code(7);
        let shift = 317;
        let doppler = 1_000.0;
        channel.samples.extend((0..SAMPLES_PER_MS).map(|n| {
            let carrier = Complex::from_polar(1.0, TAU * doppler * n as f32 / RATE as f32);
            carrier * code[(n + shift) % SAMPLES_PER_MS]
        }));
        let lock = channel.acquire().expect("clean fixture acquires");
        assert_eq!(lock.doppler_hz, doppler);
        assert_eq!(lock.code_phase, shift);
        assert!(lock.cn0_db_hz > 45.0);
    }

    #[test]
    fn acquisition_refuses_noise_floor() {
        let mut channel = GnssChannel::build(GnssParams::default());
        channel
            .samples
            .resize(SAMPLES_PER_MS, Complex::new(0.0, 0.0));
        assert!(channel.acquire().is_none());
    }

    #[test]
    fn parity_checked_nav_fixture_reports_subframe_time_and_week() {
        let mut data = [0_u32; 10];
        data[0] = 0x8B << 16;
        data[1] = (100 << 7) | (1 << 2);
        data[2] = 512 << 14;
        let bits = encode_subframe(data);
        let frame = decode_subframe(&bits).expect("valid GPS NAV fixture");
        assert_eq!(frame.subframe, 1);
        assert_eq!(frame.tow_seconds, 600);
        assert_eq!(frame.week, Some(512));
        let mut corrupt = bits;
        corrupt[35] = !corrupt[35];
        assert!(decode_subframe(&corrupt).is_none());
    }

    #[test]
    fn acquisition_search_stays_ahead_of_realtime() {
        let iq = vec![Complex::default(); RATE as usize];
        let mut channel = GnssChannel::build(GnssParams::default());
        let mut output = ChannelOutputs::default();
        channel.process(&iq[..SAMPLES_PER_MS * 20], &mut output);
        let started = std::time::Instant::now();
        channel.process(&iq, &mut output);
        let elapsed = started.elapsed().as_secs_f64();
        assert!(
            elapsed < 1.0,
            "one second of acquisition search took {elapsed:.3} s"
        );
    }

    fn encode_subframe(data: [u32; 10]) -> Vec<bool> {
        let mut d29 = false;
        let mut d30 = false;
        let mut bits = Vec::with_capacity(300);
        for value in data {
            let transmitted = if d30 { value ^ 0xFF_FFFF } else { value };
            let raw = (0..64)
                .map(|parity| transmitted << 6 | parity)
                .find(|&candidate| check_word(candidate, d29, d30) == Some(value))
                .expect("one parity pattern matches");
            bits.extend((0..30).rev().map(|shift| raw >> shift & 1 != 0));
            d29 = raw & 2 != 0;
            d30 = raw & 1 != 0;
        }
        bits
    }
}
