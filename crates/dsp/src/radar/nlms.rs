use num_complex::Complex;

use super::RadarDspError;
use crate::fft::FftPair;

type C32 = Complex<f32>;

pub const HEALTH_WINDOW_S: f64 = 0.5;
pub const MAX_CANCELLER_TAPS: usize = 4096;

const REGRESSOR_FLOOR: f32 = 1e-6;
const POWER_FLOOR: f32 = 1e-9;
const POWER_MEMORY: f32 = 0.9;
const POWER_LOADING: f32 = 1e-2;
const RATIO_FLOOR: f64 = 1e-12;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WindowRatio {
    window: usize,
    count: usize,
    numerator: f64,
    denominator: f64,
    last_db: Option<f32>,
    last_denominator_mean: Option<f64>,
}

impl WindowRatio {
    pub(crate) fn new(sample_rate: f64) -> Result<Self, RadarDspError> {
        if !(sample_rate.is_finite() && sample_rate > 0.0) {
            return Err(RadarDspError::Setting);
        }
        Ok(Self {
            window: ((HEALTH_WINDOW_S * sample_rate).round() as usize).max(1),
            count: 0,
            numerator: 0.0,
            denominator: 0.0,
            last_db: None,
            last_denominator_mean: None,
        })
    }

    pub(crate) fn add(&mut self, numerator: f32, denominator: f32, samples: usize) {
        self.numerator += f64::from(numerator);
        self.denominator += f64::from(denominator);
        self.count += samples;
        if self.count >= self.window {
            self.last_db = Some(ratio_db(self.numerator, self.denominator));
            self.last_denominator_mean = Some(self.denominator / self.count as f64);
            self.count = 0;
            self.numerator = 0.0;
            self.denominator = 0.0;
        }
    }

    pub(crate) fn value_db(&self) -> f32 {
        self.last_db
            .unwrap_or_else(|| ratio_db(self.numerator, self.denominator))
    }

    pub(crate) fn denominator_mean(&self) -> Option<f64> {
        self.last_denominator_mean
    }

    pub(crate) fn clear(&mut self) {
        self.count = 0;
        self.numerator = 0.0;
        self.denominator = 0.0;
        self.last_db = None;
        self.last_denominator_mean = None;
    }
}

fn ratio_db(numerator: f64, denominator: f64) -> f32 {
    if !(numerator > 0.0 && numerator.is_finite()) {
        return 0.0;
    }
    (10.0 * (numerator / denominator.max(RATIO_FLOOR * numerator)).log10()) as f32
}

fn check_canceller(taps: usize, lead: usize, step: f32) -> Result<usize, RadarDspError> {
    let order = taps + lead;
    if order == 0 || order > MAX_CANCELLER_TAPS || !(step > 0.0 && step <= 1.0) {
        return Err(RadarDspError::Setting);
    }
    Ok(order)
}

#[derive(Clone, Debug)]
struct DelayLine {
    samples: Vec<C32>,
    head: usize,
}

impl DelayLine {
    fn new(len: usize) -> Self {
        Self {
            samples: vec![C32::default(); len],
            head: 0,
        }
    }

    fn push(&mut self, value: C32) -> C32 {
        if self.samples.is_empty() {
            return value;
        }
        let out = std::mem::replace(&mut self.samples[self.head], value);
        self.head = (self.head + 1) % self.samples.len();
        out
    }

    fn clear(&mut self) {
        self.samples.fill(C32::default());
        self.head = 0;
    }
}

pub struct SurveillanceNlms {
    order: usize,
    lead: usize,
    step: f32,
    weights: Vec<C32>,
    history: Vec<C32>,
    head: usize,
    delay: DelayLine,
    meter: WindowRatio,
    resets: u64,
}

impl SurveillanceNlms {
    pub fn new(
        taps: usize,
        lead: usize,
        step: f32,
        sample_rate: f64,
    ) -> Result<Self, RadarDspError> {
        let order = check_canceller(taps, lead, step)?;
        Ok(Self {
            order,
            lead,
            step,
            weights: vec![C32::default(); order],
            history: vec![C32::default(); 2 * order],
            head: 0,
            delay: DelayLine::new(lead),
            meter: WindowRatio::new(sample_rate)?,
            resets: 0,
        })
    }

    #[must_use]
    pub const fn latency(&self) -> usize {
        self.lead
    }

    #[must_use]
    pub const fn order(&self) -> usize {
        self.order
    }

    pub fn process(&mut self, reference: &[C32], surveillance: &[C32], out: &mut [C32]) -> usize {
        let len = reference.len().min(surveillance.len()).min(out.len());
        for ((&r, &s), e) in reference
            .iter()
            .zip(surveillance)
            .zip(out.iter_mut())
            .take(len)
        {
            *e = self.step_sample(r, s);
        }
        len
    }

    #[must_use]
    pub fn suppression_db(&self) -> f32 {
        self.meter.value_db()
    }

    #[must_use]
    pub const fn resets(&self) -> u64 {
        self.resets
    }

    #[must_use]
    pub fn weights(&self) -> &[C32] {
        &self.weights
    }

    pub fn reset(&mut self) {
        self.weights.fill(C32::default());
        self.history.fill(C32::default());
        self.head = 0;
        self.delay.clear();
        self.meter.clear();
    }

    fn step_sample(&mut self, reference: C32, surveillance: C32) -> C32 {
        let order = self.order;
        self.head = (self.head + order - 1) % order;
        self.history[self.head] = reference;
        self.history[self.head + order] = reference;
        let desired = self.delay.push(surveillance);
        let regressor = &self.history[self.head..self.head + order];
        let (estimate, norm) = self
            .weights
            .iter()
            .zip(regressor)
            .fold((C32::default(), 0.0f32), |(sum, norm), (w, x)| {
                (sum + w.conj() * x, norm + x.norm_sqr())
            });
        let error = desired - estimate;
        let gain = error.conj() * (self.step / (REGRESSOR_FLOOR + norm));
        let mut energy = 0.0f32;
        for (w, x) in self.weights.iter_mut().zip(regressor) {
            *w += gain * x;
            energy += w.norm_sqr();
        }
        if !(energy.is_finite() && error.is_finite()) {
            self.weights.fill(C32::default());
            self.history.fill(C32::default());
            self.meter.clear();
            self.resets += 1;
            return if desired.is_finite() {
                desired
            } else {
                C32::default()
            };
        }
        self.meter.add(desired.norm_sqr(), error.norm_sqr(), 1);
        error
    }
}

pub struct BlockNlms {
    lead: usize,
    block: usize,
    step: f32,
    fft: FftPair,
    weights: Vec<C32>,
    power: Vec<f32>,
    previous: Vec<C32>,
    reference: Vec<C32>,
    desired: Vec<C32>,
    output: Vec<C32>,
    spectrum: Vec<C32>,
    work: Vec<C32>,
    fill: usize,
    delay: DelayLine,
    meter: WindowRatio,
    resets: u64,
}

impl BlockNlms {
    pub fn new(
        taps: usize,
        lead: usize,
        step: f32,
        sample_rate: f64,
    ) -> Result<Self, RadarDspError> {
        let order = check_canceller(taps, lead, step)?;
        let block = order.next_power_of_two();
        let full = 2 * block;
        Ok(Self {
            lead,
            block,
            step,
            fft: FftPair::new(full),
            weights: vec![C32::default(); full],
            power: vec![0.0; full],
            previous: vec![C32::default(); block],
            reference: vec![C32::default(); block],
            desired: vec![C32::default(); block],
            output: vec![C32::default(); block],
            spectrum: vec![C32::default(); full],
            work: vec![C32::default(); full],
            fill: 0,
            delay: DelayLine::new(lead),
            meter: WindowRatio::new(sample_rate)?,
            resets: 0,
        })
    }

    #[must_use]
    pub const fn latency(&self) -> usize {
        self.lead + self.block
    }

    #[must_use]
    pub const fn block(&self) -> usize {
        self.block
    }

    pub fn process(&mut self, reference: &[C32], surveillance: &[C32], out: &mut [C32]) -> usize {
        let len = reference.len().min(surveillance.len()).min(out.len());
        for ((&r, &s), e) in reference
            .iter()
            .zip(surveillance)
            .zip(out.iter_mut())
            .take(len)
        {
            *e = self.output[self.fill];
            self.reference[self.fill] = r;
            self.desired[self.fill] = self.delay.push(s);
            self.fill += 1;
            if self.fill == self.block {
                self.run_block();
                self.fill = 0;
            }
        }
        len
    }

    #[must_use]
    pub fn suppression_db(&self) -> f32 {
        self.meter.value_db()
    }

    #[must_use]
    pub const fn resets(&self) -> u64 {
        self.resets
    }

    pub fn reset(&mut self) {
        for buffer in [
            &mut self.weights,
            &mut self.previous,
            &mut self.reference,
            &mut self.desired,
            &mut self.output,
        ] {
            buffer.fill(C32::default());
        }
        self.power.fill(0.0);
        self.fill = 0;
        self.delay.clear();
        self.meter.clear();
    }

    fn run_block(&mut self) {
        let block = self.block;
        self.spectrum[..block].copy_from_slice(&self.previous);
        self.spectrum[block..].copy_from_slice(&self.reference);
        self.fft.forward(&mut self.spectrum);
        for ((out, x), w) in self.work.iter_mut().zip(&self.spectrum).zip(&self.weights) {
            *out = x * w;
        }
        self.fft.inverse_scaled(&mut self.work);
        let mut desired_energy = 0.0f32;
        let mut error_energy = 0.0f32;
        for ((e, d), y) in self
            .output
            .iter_mut()
            .zip(&self.desired)
            .zip(&self.work[block..])
        {
            *e = d - y;
            desired_energy += d.norm_sqr();
            error_energy += e.norm_sqr();
        }
        self.work[..block].fill(C32::default());
        self.work[block..].copy_from_slice(&self.output);
        self.fft.forward(&mut self.work);
        let mut total = 0.0f32;
        for (power, x) in self.power.iter_mut().zip(&self.spectrum) {
            *power = POWER_MEMORY * *power + (1.0 - POWER_MEMORY) * x.norm_sqr();
            total += *power;
        }
        let floor = POWER_FLOOR + POWER_LOADING * total / self.power.len() as f32;
        for ((value, x), power) in self.work.iter_mut().zip(&self.spectrum).zip(&self.power) {
            *value = x.conj() * *value / (*power + floor);
        }
        self.fft.inverse_scaled(&mut self.work);
        self.work[block..].fill(C32::default());
        self.fft.forward(&mut self.work);
        let mut energy = 0.0f32;
        for (w, gradient) in self.weights.iter_mut().zip(&self.work) {
            *w += gradient * self.step;
            energy += w.norm_sqr();
        }
        std::mem::swap(&mut self.previous, &mut self.reference);
        if energy.is_finite() && error_energy.is_finite() {
            self.meter.add(desired_energy, error_energy, self.block);
        } else {
            self.weights.fill(C32::default());
            self.power.fill(0.0);
            self.previous.fill(C32::default());
            self.output.fill(C32::default());
            self.meter.clear();
            self.resets += 1;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::f64::consts::TAU;

    use super::*;

    const FS: f64 = 266_666.67;

    pub(crate) struct Noise(pub(crate) u64);

    impl Noise {
        pub(crate) fn uniform(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }

        pub(crate) fn complex(&mut self) -> C32 {
            let radius = (-(1.0 - self.uniform()).ln()).sqrt();
            let angle = TAU * self.uniform();
            C32::new((radius * angle.cos()) as f32, (radius * angle.sin()) as f32)
        }
    }

    pub(crate) fn fm_reference(len: usize, seed: u64) -> Vec<C32> {
        let mut noise = Noise(seed);
        let mut phase = 0.0f64;
        let mut drive = 0.0f64;
        (0..len)
            .map(|_| {
                drive = 0.995 * drive + 0.1 * (noise.uniform() - 0.5);
                phase += drive;
                C32::new(phase.cos() as f32, phase.sin() as f32)
            })
            .collect()
    }

    fn direct_path(reference: &[C32], thermal: f32, seed: u64) -> Vec<C32> {
        let paths = [
            (0usize, C32::new(3.0, 1.0)),
            (1, C32::new(-0.8, 0.5)),
            (4, C32::new(0.3, -0.6)),
            (9, C32::new(0.1, 0.2)),
        ];
        let mut noise = Noise(seed);
        (0..reference.len())
            .map(|n| {
                let clutter: C32 = paths
                    .iter()
                    .filter(|(delay, _)| n >= *delay)
                    .map(|(delay, gain)| reference[n - delay] * gain)
                    .sum();
                clutter + noise.complex() * thermal
            })
            .collect()
    }

    fn direct_path_recording() -> (Vec<C32>, Vec<C32>) {
        let reference = fm_reference((1.25 * FS) as usize, 3);
        let surveillance = direct_path(&reference, 3e-3, 5);
        (reference, surveillance)
    }

    #[test]
    fn nlms_converges_on_a_static_direct_path() {
        let (reference, surveillance) = direct_path_recording();
        let mut nlms = SurveillanceNlms::new(14, 2, 0.05, FS).unwrap();
        let mut out = vec![C32::default(); reference.len()];
        assert_eq!(
            nlms.process(&reference, &surveillance, &mut out),
            reference.len()
        );
        let supp = nlms.suppression_db();
        assert!(supp >= 30.0, "{supp}");
        assert_eq!(nlms.latency(), 2);
        assert_eq!(nlms.resets(), 0);
    }

    #[test]
    fn nlms_output_is_the_surveillance_delayed_by_the_lead() {
        let reference = vec![C32::default(); 64];
        let surveillance: Vec<C32> = (0..64).map(|n| C32::new(n as f32, 0.0)).collect();
        let mut nlms = SurveillanceNlms::new(4, 3, 0.05, FS).unwrap();
        let mut out = vec![C32::default(); 64];
        nlms.process(&reference, &surveillance, &mut out);
        for n in 3..64 {
            assert_eq!(out[n], surveillance[n - 3]);
        }
    }

    #[test]
    fn block_nlms_reaches_nlms_within_3_db() {
        let (reference, surveillance) = direct_path_recording();
        let mut nlms = SurveillanceNlms::new(14, 2, 0.05, FS).unwrap();
        let mut block = BlockNlms::new(14, 2, 0.05, FS).unwrap();
        let mut out = vec![C32::default(); reference.len()];
        nlms.process(&reference, &surveillance, &mut out);
        for (start, end) in [(0, 1000), (1000, 1001), (1001, reference.len())] {
            block.process(
                &reference[start..end],
                &surveillance[start..end],
                &mut out[start..end],
            );
        }
        let (single, batched) = (nlms.suppression_db(), block.suppression_db());
        assert!(batched >= single - 3.0, "{batched} vs {single}");
        assert_eq!(block.latency(), 2 + 16);
        let tail = &out[out.len() - 4096..];
        let residual: f32 = tail.iter().map(C32::norm_sqr).sum::<f32>() / 4096.0;
        let input: f32 =
            surveillance.iter().map(C32::norm_sqr).sum::<f32>() / surveillance.len() as f32;
        assert!(10.0 * (input / residual).log10() > 25.0);
    }

    #[test]
    fn invalid_cancellers_are_refused() {
        assert!(SurveillanceNlms::new(0, 0, 0.05, FS).is_err());
        assert!(SurveillanceNlms::new(4, 0, 0.0, FS).is_err());
        assert!(BlockNlms::new(4, 0, f32::NAN, FS).is_err());
        assert!(BlockNlms::new(4, 0, 0.1, 0.0).is_err());
    }

    #[test]
    fn a_non_finite_input_resets_and_counts() {
        let (reference, mut surveillance) = direct_path_recording();
        surveillance[5000] = C32::new(f32::NAN, 0.0);
        let mut nlms = SurveillanceNlms::new(14, 2, 0.05, FS).unwrap();
        let mut block = BlockNlms::new(14, 2, 0.05, FS).unwrap();
        let mut out = vec![C32::default(); reference.len()];
        nlms.process(&reference, &surveillance, &mut out);
        assert!(nlms.resets() >= 1);
        assert!(out.iter().all(|value| value.is_finite()));
        block.process(&reference, &surveillance, &mut out);
        assert!(block.resets() >= 1);
        assert!(out[20_000..].iter().all(|value| value.is_finite()));
        assert!(block.suppression_db() > 25.0);
    }
}
