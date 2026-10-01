use num_complex::Complex;
use sdrmm_dsp::ToneCorrelator;

pub const TONES: usize = 4;
const FREQUENCIES: [f64; TONES] = [-4_800.0, -1_600.0, 1_600.0, 4_800.0];
const AFC_ALPHA: f64 = 1.0 / 512.0;
const AFC_REFRESH: usize = 16;

pub type Powers = [f32; TONES];

pub struct Afc {
    upper: f64,
    lower: f64,
    last: Complex<f64>,
    rotor: Complex<f64>,
    step: Complex<f64>,
    since_step: usize,
}

impl Afc {
    pub fn new(rate: f64) -> Self {
        let deviation = std::f64::consts::TAU * FREQUENCIES[TONES - 1] / rate;
        Self {
            upper: deviation,
            lower: -deviation,
            last: Complex::new(0.0, 0.0),
            rotor: Complex::new(1.0, 0.0),
            step: Complex::new(1.0, 0.0),
            since_step: 0,
        }
    }

    pub fn offset(&self) -> f64 {
        (self.upper + self.lower) / 2.0
    }

    pub fn centre(&mut self, sample: Complex<f32>, track: bool) -> Complex<f32> {
        if !(sample.re.is_finite() && sample.im.is_finite()) {
            return Complex::new(0.0, 0.0);
        }
        let raw = Complex::new(f64::from(sample.re), f64::from(sample.im));
        let lag = raw * self.last.conj();
        if track && lag.norm_sqr() > 0.0 {
            let frequency = lag.arg();
            let centroid = if frequency > self.offset() {
                &mut self.upper
            } else {
                &mut self.lower
            };
            *centroid += AFC_ALPHA * (frequency - *centroid);
        }
        self.last = raw;
        let turned = raw * self.rotor;
        self.rotor *= self.step;
        self.since_step += 1;
        if self.since_step == AFC_REFRESH {
            self.since_step = 0;
            self.step = Complex::from_polar(1.0, -self.offset());
            self.rotor /= self.rotor.norm();
        }
        Complex::new(turned.re as f32, turned.im as f32)
    }
}

pub struct Bank {
    tones: [ToneCorrelator<Complex<f32>>; TONES],
}

impl Bank {
    pub fn new(rate: f64, window: usize) -> Self {
        Self {
            tones: FREQUENCIES.map(|frequency| ToneCorrelator::complex(rate, frequency, window)),
        }
    }

    pub fn push(&mut self, sample: Complex<f32>) -> Powers {
        let mut powers = [0.0; TONES];
        for (power, tone) in powers.iter_mut().zip(&mut self.tones) {
            *power = tone.push(sample);
        }
        powers
    }

    pub fn reset(&mut self) {
        for tone in &mut self.tones {
            tone.reset();
        }
    }
}

pub fn decide(powers: Powers, levels: u8) -> usize {
    if levels == 2 {
        return if powers[0] > powers[TONES - 1] {
            0
        } else {
            TONES - 1
        };
    }
    (0..TONES)
        .max_by(|&a, &b| powers[a].total_cmp(&powers[b]))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth;

    #[test]
    fn each_tone_wins_its_own_bank_slot() {
        let rate = 48_000.0;
        for (index, frequency) in FREQUENCIES.into_iter().enumerate() {
            let mut iq = vec![Complex::new(1.0, 0.0); 60];
            synth::shift(&mut iq, frequency, rate);
            let mut bank = Bank::new(rate, 15);
            let powers = iq.iter().map(|&s| bank.push(s)).last().unwrap();
            assert_eq!(decide(powers, 4), index);
        }
    }

    #[test]
    fn the_afc_removes_a_carrier_offset_on_a_balanced_signal() {
        let rate = 48_000.0;
        let mut phase = 0.0f64;
        let mut iq: Vec<Complex<f32>> = (0..20_000)
            .map(|k| {
                let tone = if k / 30 % 2 == 0 { 4_800.0 } else { -4_800.0 };
                phase += std::f64::consts::TAU * tone / rate;
                Complex::from_polar(1.0, phase as f32)
            })
            .collect();
        synth::shift(&mut iq, 1_500.0, rate);
        let mut afc = Afc::new(rate);
        for &sample in &iq {
            afc.centre(sample, true);
        }
        let offset = afc.offset() * rate / std::f64::consts::TAU;
        assert!((offset - 1_500.0).abs() < 50.0, "offset {offset}");
    }
}
