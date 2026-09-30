use super::{
    CARRIERS, FILTER_TAPS, FrameLength, MAX_FRAME, OVERSAMPLED_STEP, OVERSAMPLING, Sample,
    carrier_radians, cis, tables::ROOT_RAISED_COSINE, unit,
};

pub(super) type Filtered = [[Sample; OVERSAMPLING + 1]; CARRIERS];

const HISTORY: usize = FILTER_TAPS + MAX_FRAME;
const DECIMATION: usize = 4;
const TAPS: usize = FILTER_TAPS / DECIMATION;
const GROUP: usize = 5;
const OUTPUT_STRIDE: usize = OVERSAMPLED_STEP / DECIMATION;

static RX_TAPS: [f32; TAPS] = {
    let mut taps = [0.0; TAPS];
    let mut n = 0;
    while n < TAPS {
        taps[n] = ROOT_RAISED_COSINE[n * DECIMATION];
        n += 1;
    }
    taps
};

type Plane = [f32; CARRIERS];

#[derive(Clone, Copy)]
struct Planes {
    re: Plane,
    im: Plane,
}

impl Planes {
    const ZERO: Self = Self {
        re: [0.0; CARRIERS],
        im: [0.0; CARRIERS],
    };

    fn from_fn(value: impl Fn(usize) -> Sample) -> Self {
        let values: [Sample; CARRIERS] = std::array::from_fn(value);
        Self {
            re: values.map(|value| value.re),
            im: values.map(|value| value.im),
        }
    }

    fn get(&self, carrier: usize) -> Sample {
        Sample::new(self.re[carrier], self.im[carrier])
    }

    fn rotate(&mut self, by: &Self) {
        for c in 0..CARRIERS {
            let re = self.re[c] * by.re[c] - self.im[c] * by.im[c];
            let im = self.re[c] * by.im[c] + self.im[c] * by.re[c];
            self.re[c] = re;
            self.im[c] = im;
        }
    }

    fn mix_down(&self, sample: Sample, out: &mut Self) {
        for c in 0..CARRIERS {
            let (re, im) = (self.re[c], -self.im[c]);
            out.re[c] = sample.re * re - sample.im * im;
            out.im[c] = sample.re * im + sample.im * re;
        }
    }

    fn normalize(&mut self) {
        for c in 0..CARRIERS {
            let unit = unit(self.get(c));
            self.re[c] = unit.re;
            self.im[c] = unit.im;
        }
    }
}

pub(super) struct Downconverter {
    history: [Sample; HISTORY],
    phases: Planes,
    windback: Planes,
    steps: Planes,
    baseband: Box<[Planes; HISTORY / DECIMATION]>,
}

impl Downconverter {
    pub(super) fn new() -> Self {
        let rotations = Planes::from_fn(|c| cis(carrier_radians(c)));
        let mut steps = rotations;
        for _ in 1..DECIMATION {
            steps.rotate(&rotations);
        }
        Self {
            history: [Sample::ZERO; HISTORY],
            phases: Planes::from_fn(|_| Sample::ONE),
            windback: Planes::from_fn(|c| cis(-carrier_radians(c) * FILTER_TAPS as f32)),
            steps,
            baseband: Box::new([Planes::ZERO; HISTORY / DECIMATION]),
        }
    }

    pub(super) fn admit(&mut self, length: usize) -> &mut [Sample] {
        self.history.copy_within(length.., 0);
        &mut self.history[HISTORY - length..]
    }

    pub(super) fn filter(&mut self, length: FrameLength, filtered: &mut Filtered) {
        let history = &self.history[HISTORY - length.samples() - FILTER_TAPS..];
        self.phases.rotate(&self.windback);
        for (mixed, &sample) in self
            .baseband
            .iter_mut()
            .zip(history.iter().step_by(DECIMATION))
        {
            self.phases.rotate(&self.steps);
            self.phases.mix_down(sample, mixed);
        }
        for (step, window) in self
            .baseband
            .windows(TAPS)
            .step_by(OUTPUT_STRIDE)
            .take(length.oversampled())
            .enumerate()
        {
            let output = matched(window);
            for (carrier, outputs) in filtered.iter_mut().enumerate() {
                outputs[step] = output.get(carrier);
            }
        }
        self.phases.normalize();
    }
}

fn matched(window: &[Planes]) -> Planes {
    let (groups, _) = window[..TAPS].as_chunks::<GROUP>();
    let (taps, _) = RX_TAPS.as_chunks::<GROUP>();
    let mut sum = Planes::ZERO;
    for (rows, taps) in groups.iter().zip(taps) {
        accumulate_group(&mut sum.re, rows.each_ref().map(|row| &row.re), taps);
        accumulate_group(&mut sum.im, rows.each_ref().map(|row| &row.im), taps);
    }
    for value in sum.re.iter_mut().chain(&mut sum.im) {
        *value *= DECIMATION as f32;
    }
    sum
}

fn accumulate_group(sum: &mut Plane, rows: [&Plane; GROUP], taps: &[f32; GROUP]) {
    for c in 0..CARRIERS {
        sum[c] += rows[0][c] * taps[0]
            + rows[1][c] * taps[1]
            + rows[2][c] * taps[2]
            + rows[3][c] * taps[3]
            + rows[4][c] * taps[4];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_carrier_tone_lands_at_dc_on_its_own_branch() {
        let mut downconverter = Downconverter::new();
        let target = 3;
        let rotation = cis(carrier_radians(target));
        let mut phase = Sample::ONE;
        let mut filtered: Filtered = [[Sample::ZERO; OVERSAMPLING + 1]; CARRIERS];
        for _ in 0..10 {
            for slot in downconverter.admit(160) {
                phase *= rotation;
                *slot = phase;
            }
            downconverter.filter(FrameLength::Nominal, &mut filtered);
        }
        let power = |c: usize| filtered[c][..4].iter().map(|v| v.norm_sqr()).sum::<f32>();
        assert!(power(target) > 0.5);
        for other in (0..CARRIERS).filter(|&c| c != target) {
            assert!(power(other) < power(target) * 1e-3, "carrier {other}");
        }
        let angles: Vec<f32> = filtered[target][..4].iter().map(|v| v.arg()).collect();
        assert!(
            angles
                .windows(2)
                .all(|pair| (pair[0] - pair[1]).abs() < 1e-3)
        );
    }

    #[test]
    fn oscillators_stay_on_the_unit_circle() {
        let mut downconverter = Downconverter::new();
        let mut filtered: Filtered = [[Sample::ZERO; OVERSAMPLING + 1]; CARRIERS];
        for _ in 0..5_000 {
            downconverter.admit(200).fill(Sample::ONE);
            downconverter.filter(FrameLength::Long, &mut filtered);
        }
        assert!((0..CARRIERS).all(|c| (downconverter.phases.get(c).norm() - 1.0).abs() < 1e-6));
    }
}
