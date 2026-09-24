use std::{
    f32::consts::{FRAC_1_SQRT_2, FRAC_PI_2, PI},
    f64::consts::TAU,
};

use num_complex::Complex;

use super::{
    acquisition::CoarseAcquisition,
    frame::{FrameDecoder, FrameHeader, HEADER_BITS, HIGH_RATE_BPS, UW},
    framer::DecodedFrame,
    taps::{Fir, rrc_taps},
};

pub(super) const BIT_RATE: u32 = HIGH_RATE_BPS;
pub(super) const CHANNEL_RATE_HR: f64 = 48_000.0;
pub(super) const HR_SKIP_BITS: usize = 16 + 178;
pub(super) const HR_CODED_BITS: usize = 64 * 78;
const STROBE_POINT: f64 = 0.65;
const LOCK_MSE: f32 = 0.5;
const LOCK_QUAD: f32 = 0.4;
const ACQ_FFT: usize = 16_384;
const ACQ_RANGE_HZ: f64 = 3_000.0;
const ACQ_MIN_SHIFT_HZ: f64 = 1.0;
const RRC_TAPS: usize = 55;
const AGC_CLIP: f32 = 2.84;
const BIAS_WINDOW: usize = 800;
const UW_TOLERANCE: u32 = 2;
const HIGH_RATE_RESONATOR: ([f32; 3], [f32; 2]) = (
    [0.000_327_142_2, 0.0, 0.000_327_142_2],
    [-0.390_053, 0.999_345_7],
);
const C_CHANNEL_RESONATOR: ([f32; 3], [f32; 2]) = (
    [0.001_284_585_8, 0.0, -0.001_284_585_8],
    [-0.906_814_63, 0.997_430_8],
);
const CARRIER_LOOP: ([f32; 3], [f32; 2]) = (
    [0.001_027_561, 0.002_055_122, 0.001_027_561],
    [-1.920_738_7, 0.925_092_46],
);

struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}

impl Biquad {
    fn new((b, a): ([f32; 3], [f32; 2])) -> Self {
        Self { b, a, z: [0.0; 2] }
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }

    fn reset(&mut self) {
        self.z = [0.0; 2];
    }
}

struct FracDelay {
    buffer: Vec<f32>,
    position: usize,
    delay: f64,
}

impl FracDelay {
    fn new(delay: f64) -> Self {
        Self {
            buffer: vec![0.0; delay.ceil() as usize + 2],
            position: 0,
            delay,
        }
    }

    fn run(&mut self, x: f32) -> f32 {
        let length = self.buffer.len();
        self.buffer[self.position] = x;
        let whole = self.delay.floor() as usize;
        let fraction = (self.delay - whole as f64) as f32;
        let a = self.buffer[(self.position + length - whole) % length];
        let b = self.buffer[(self.position + length - whole - 1) % length];
        self.position = (self.position + 1) % length;
        a * (1.0 - fraction) + b * fraction
    }
}

struct MovingAverage {
    buffer: Vec<f32>,
    position: usize,
    sum: f64,
}

impl MovingAverage {
    fn new(length: usize) -> Self {
        Self {
            buffer: vec![0.0; length],
            position: 0,
            sum: 0.0,
        }
    }

    fn run(&mut self, x: f32) -> f32 {
        self.sum += f64::from(x - self.buffer[self.position]);
        self.buffer[self.position] = x;
        self.position = (self.position + 1) % self.buffer.len();
        (self.sum / self.buffer.len() as f64) as f32
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.sum = 0.0;
    }
}

struct SymbolTiming {
    power_delay: FracDelay,
    quarter_first: FracDelay,
    quarter_second: FracDelay,
    eighth: FracDelay,
    resonator: Biquad,
    phase: f64,
    freq_hz: f64,
    bit_rate: f64,
}

impl SymbolTiming {
    fn new(samples_per_symbol: f64, bit_rate: f64, resonator: Biquad) -> Self {
        Self {
            power_delay: FracDelay::new(1.0),
            quarter_first: FracDelay::new(samples_per_symbol / 4.0),
            quarter_second: FracDelay::new(samples_per_symbol / 4.0),
            eighth: FracDelay::new(samples_per_symbol / 8.0),
            resonator,
            phase: 0.0,
            freq_hz: bit_rate,
            bit_rate,
        }
    }

    fn strobe_fraction(&mut self, sample: Complex<f32>, fs: f64) -> Option<f32> {
        let power = sample.norm_sqr();
        let difference = self.power_delay.run(power) - power;
        let first = self.quarter_first.run(difference);
        let second = self.quarter_second.run(first);
        let eta = self.resonator.run((second - difference) * first);
        let detector = Complex::new(eta, -self.eighth.run(eta));
        let rotation = Complex::from_polar(1.0, (TAU * self.phase) as f32);
        let error = f64::from((rotation * detector).arg());
        self.freq_hz =
            (self.freq_hz - error * 1e-8).clamp(self.bit_rate - 0.1, self.bit_rate + 0.1);
        let previous = self.phase;
        self.phase += self.freq_hz / fs - error * 0.01 / 360.0;
        let from = previous.rem_euclid(1.0);
        let to = self.phase.rem_euclid(1.0);
        let passed = if from <= to {
            from < STROBE_POINT && STROBE_POINT <= to
        } else {
            from < STROBE_POINT || STROBE_POINT <= to
        };
        self.phase = to;
        if !passed {
            return None;
        }
        let step = self.freq_hz / fs;
        Some((((self.phase - STROBE_POINT).rem_euclid(1.0)) / step).clamp(0.0, 1.0) as f32)
    }
}

pub(super) struct OqpskDemod {
    fs: f64,
    rrc: Fir,
    nco_freq_hz: f64,
    nco_phase: f64,
    phase_trim: f32,
    agc: f32,
    timing: SymbolTiming,
    last_sample: Complex<f32>,
    pair_toggle: bool,
    held: Complex<f32>,
    carrier_loop: Biquad,
    bias: MovingAverage,
    mse: f32,
    quad: Complex<f32>,
    acquisition: CoarseAcquisition,
}

impl OqpskDemod {
    pub(super) fn new(channel_rate: f64) -> Self {
        Self::with_rate(channel_rate, f64::from(BIT_RATE), HIGH_RATE_RESONATOR, 1.0)
    }

    pub(super) fn new_c_channel(channel_rate: f64) -> Self {
        Self::with_rate(channel_rate, 8_400.0, C_CHANNEL_RESONATOR, 0.6)
    }

    fn with_rate(
        channel_rate: f64,
        bit_rate: f64,
        resonator: ([f32; 3], [f32; 2]),
        rrc_beta: f64,
    ) -> Self {
        let samples_per_symbol = channel_rate / (bit_rate / 2.0);
        Self {
            fs: channel_rate,
            rrc: Fir::new(rrc_taps(samples_per_symbol, RRC_TAPS, rrc_beta), 1),
            nco_freq_hz: 0.0,
            nco_phase: 0.0,
            phase_trim: 0.0,
            agc: 1.0,
            timing: SymbolTiming::new(samples_per_symbol, bit_rate, Biquad::new(resonator)),
            last_sample: Complex::new(0.0, 0.0),
            pair_toggle: false,
            held: Complex::new(0.0, 0.0),
            carrier_loop: Biquad::new(CARRIER_LOOP),
            bias: MovingAverage::new(BIAS_WINDOW),
            mse: 100.0,
            quad: Complex::new(0.0, 0.0),
            acquisition: CoarseAcquisition::new(
                ACQ_FFT,
                channel_rate,
                bit_rate / 2.0,
                ACQ_RANGE_HZ,
                Some(ACQ_MIN_SHIFT_HZ),
            ),
        }
    }

    pub(super) fn locked(&self) -> bool {
        self.mse < LOCK_MSE && self.quad.norm() > LOCK_QUAD
    }

    pub(super) fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<(f32, u8)>) {
        for &raw in input {
            self.step(raw, out);
        }
    }

    fn step(&mut self, raw: Complex<f32>, out: &mut Vec<(f32, u8)>) {
        let rotation = Complex::from_polar(1.0, -self.nco_phase as f32 + self.phase_trim);
        self.nco_phase += TAU * self.nco_freq_hz / self.fs;
        if self.nco_phase > TAU {
            self.nco_phase -= TAU;
        }
        let filtered = self.rrc.filter(raw * rotation);
        let mixed = raw * Complex::from_polar(1.0, -self.nco_phase as f32);
        let locked = self.locked();
        if let Some(shift) = self.acquisition.push(mixed, locked) {
            self.retune(shift);
        }
        self.agc += 0.001 * (filtered.norm() - self.agc);
        let mut sample = filtered / self.agc.max(1e-9);
        let magnitude = sample.norm();
        if magnitude > AGC_CLIP {
            sample *= AGC_CLIP / magnitude;
        }
        if let Some(fraction) = self.timing.strobe_fraction(sample, self.fs) {
            let point = sample * (1.0 - fraction) + self.last_sample * fraction;
            self.strobe(point, out);
        }
        self.last_sample = sample;
    }

    fn retune(&mut self, shift_hz: f64) {
        self.nco_freq_hz += shift_hz;
        self.carrier_loop.reset();
        self.bias.reset();
        self.phase_trim = 0.0;
    }

    fn strobe(&mut self, point: Complex<f32>, out: &mut Vec<(f32, u8)>) {
        self.pair_toggle = !self.pair_toggle;
        if self.pair_toggle {
            self.held = point;
            return;
        }
        let mut pair = Complex::new(point.re, self.held.im);
        let cross =
            (self.held.re.tanh() * self.held.im - point.im.tanh() * point.re).clamp(-PI, PI);
        let error = self.carrier_loop.run(cross).clamp(-FRAC_PI_2, FRAC_PI_2);
        self.phase_trim += error.to_radians();
        self.nco_freq_hz -= 0.01 * f64::from(error);
        pair *= Complex::from_polar(1.0, self.bias.run(error));
        let ideal = Complex::new(
            FRAC_1_SQRT_2.copysign(pair.re),
            FRAC_1_SQRT_2.copysign(pair.im),
        );
        self.mse += 0.0025 * ((pair - ideal).norm_sqr() - self.mse);
        let unit = pair / pair.norm().max(1e-9);
        self.quad += 0.0025 * ((unit * unit) * (unit * unit) - self.quad);
        out.push((pair.im, u8::from(pair.im > 0.0)));
        out.push((pair.re, u8::from(pair.re > 0.0)));
    }
}

fn check_uw(window: u64) -> Option<[f32; 2]> {
    let mut even = 0u32;
    let mut odd = 0u32;
    for k in 0..32 {
        even |= (((window >> (63 - 2 * k)) & 1) as u32) << (31 - k);
        odd |= (((window >> (63 - (2 * k + 1))) & 1) as u32) << (31 - k);
    }
    let sign = |rail: u32| {
        if (rail ^ UW).count_ones() <= UW_TOLERANCE {
            Some(1.0)
        } else if (rail ^ !UW).count_ones() <= UW_TOLERANCE {
            Some(-1.0)
        } else {
            None
        }
    };
    Some([sign(even)?, sign(odd)?])
}

pub(super) struct HrFramer {
    decoder: FrameDecoder,
    shift: u64,
    buffer: Vec<f32>,
    inversion: Option<[f32; 2]>,
}

impl HrFramer {
    pub(super) fn new() -> Self {
        Self {
            decoder: FrameDecoder::new(BIT_RATE),
            shift: 0,
            buffer: Vec::with_capacity(HR_SKIP_BITS + HR_CODED_BITS),
            inversion: None,
        }
    }

    pub(super) fn push(&mut self, soft: f32, hard: u8) -> Option<DecodedFrame> {
        let mut frame = None;
        if let Some(inversion) = self.inversion {
            let index = self.buffer.len();
            self.buffer.push(soft * inversion[index % 2]);
            if self.buffer.len() == HR_SKIP_BITS + HR_CODED_BITS {
                let header = FrameHeader::from_soft_bits(&self.buffer[..HEADER_BITS]);
                frame = Some(DecodedFrame::decode(
                    &mut self.decoder,
                    header,
                    &self.buffer[HR_SKIP_BITS..],
                ));
                self.buffer.clear();
                self.inversion = None;
            }
        }
        self.shift = (self.shift << 1) | u64::from(hard);
        if self.inversion.is_none() {
            self.inversion = check_uw(self.shift);
        }
        frame
    }
}

#[cfg(test)]
pub(super) use modulator::{hr_frame_bits, modulate_oqpsk, modulate_oqpsk_rate};

#[cfg(test)]
mod modulator {
    use super::*;
    use crate::inmarsat_aero::frame::FrameEncoder;

    pub fn hr_frame_bits(encoder: &mut FrameEncoder, su_bytes: &[u8], counter: u8) -> Vec<u8> {
        let low = encoder.encode(su_bytes, counter);
        let mut bits = Vec::with_capacity(64 + HR_SKIP_BITS + HR_CODED_BITS);
        for k in 0..32 {
            let bit = ((UW >> (31 - k)) & 1) as u8;
            bits.push(bit);
            bits.push(bit);
        }
        bits.extend_from_slice(&low[32..48]);
        bits.extend(std::iter::repeat_n(0, HR_SKIP_BITS - 16));
        bits.extend_from_slice(&low[48..]);
        bits
    }

    pub fn modulate_oqpsk(
        bits: &[u8],
        sample_rate: f64,
        freq_offset_hz: f64,
        amplitude: f32,
    ) -> Vec<Complex<f32>> {
        modulate_oqpsk_rate(
            bits,
            f64::from(BIT_RATE),
            1.0,
            sample_rate,
            freq_offset_hz,
            amplitude,
        )
    }

    pub fn modulate_oqpsk_rate(
        bits: &[u8],
        bit_rate: f64,
        rrc_beta: f64,
        sample_rate: f64,
        freq_offset_hz: f64,
        amplitude: f32,
    ) -> Vec<Complex<f32>> {
        let symbol_rate = bit_rate / 2.0;
        let half = sample_rate / symbol_rate / 2.0;
        let total = ((bits.len() + 4) as f64 * half) as usize;
        let mut in_phase = vec![Complex::new(0.0f32, 0.0); total];
        let mut quadrature = vec![Complex::new(0.0f32, 0.0); total];
        for (k, &bit) in bits.iter().enumerate() {
            let value = if bit == 1 { 1.0 } else { -1.0 };
            let at = ((k + 1) as f64 * half) as usize;
            if at < total {
                if k % 2 == 0 {
                    in_phase[at].re = value;
                } else {
                    quadrature[at].re = value;
                }
            }
        }
        let taps = rrc_taps(sample_rate / symbol_rate, 129, rrc_beta);
        let mut shaped_i = Vec::new();
        let mut shaped_q = Vec::new();
        Fir::new(taps.clone(), 1).process(&in_phase, &mut shaped_i);
        Fir::new(taps, 1).process(&quadrature, &mut shaped_q);
        (0..total)
            .map(|n| {
                let phase = TAU * freq_offset_hz * n as f64 / sample_rate;
                Complex::new(shaped_i[n].re, shaped_q[n].re)
                    * Complex::from_polar(amplitude, phase as f32)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lcg(u64);

    impl Lcg {
        fn bit(&mut self) -> u8 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 33) & 1) as u8
        }
    }

    fn rail_ber(sent: &[u8], received: &[f32]) -> f64 {
        let mut best = 1.0f64;
        for lag in 0..256.min(received.len()) {
            let n = sent.len().min(received.len() - lag);
            if n < 2000 {
                break;
            }
            let mut errors = 0usize;
            let mut total = 0usize;
            for rail in 0..2 {
                let pairs: Vec<(f64, bool)> = (n / 2..n)
                    .filter(|k| k % 2 == rail)
                    .map(|k| (f64::from(received[lag + k]), sent[k] == 1))
                    .collect();
                let correlation: f64 = pairs
                    .iter()
                    .map(|&(soft, bit)| if bit { soft } else { -soft })
                    .sum();
                let sign = correlation.signum();
                errors += pairs
                    .iter()
                    .filter(|&&(soft, bit)| (soft * sign > 0.0) != bit)
                    .count();
                total += pairs.len();
            }
            best = best.min(errors as f64 / total as f64);
        }
        best
    }

    #[test]
    fn locks_and_demodulates_with_cfo() {
        for cfo in [0.0_f64, 120.0, -250.0] {
            let mut rng = Lcg(42);
            let bits: Vec<u8> = (0..40_000).map(|_| rng.bit()).collect();
            let iq = modulate_oqpsk(&bits, CHANNEL_RATE_HR, cfo, 0.5);
            let mut demod = OqpskDemod::new(CHANNEL_RATE_HR);
            let mut out = Vec::new();
            demod.process(&iq, &mut out);
            assert!(demod.locked(), "cfo={cfo}: mse {}", demod.mse);
            let soft: Vec<f32> = out.iter().map(|&(soft, _)| soft).collect();
            let ber = rail_ber(&bits, &soft);
            assert!(ber < 0.001, "cfo={cfo}: BER {ber}");
        }
    }
}
