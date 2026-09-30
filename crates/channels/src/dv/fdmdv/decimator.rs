use super::{MAX_FRAME, Sample, fir, tables::DECIMATION_LOWPASS};

const TAPS: usize = DECIMATION_LOWPASS.len();
const HISTORY: usize = TAPS + MAX_FRAME;

pub(super) struct Decimator {
    history: [Sample; HISTORY],
}

impl Decimator {
    pub(super) fn new() -> Self {
        Self {
            history: [Sample::ZERO; HISTORY],
        }
    }

    pub(super) fn filter(&mut self, samples: &[Sample], output: &mut [Sample]) {
        let length = samples.len();
        self.history.copy_within(length.., 0);
        self.history[HISTORY - length..].copy_from_slice(samples);
        fir(
            &DECIMATION_LOWPASS,
            &self.history[HISTORY - length - (TAPS - 1)..],
            output,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_impulse_comes_out_as_the_filter_taps() {
        let mut decimator = Decimator::new();
        let mut impulse = [Sample::ZERO; 160];
        impulse[0] = Sample::ONE;
        let mut output = [Sample::ZERO; 160];
        decimator.filter(&impulse, &mut output);
        let response: Vec<f32> = output[..TAPS].iter().map(|value| value.re).collect();
        assert_eq!(response, DECIMATION_LOWPASS);
    }

    #[test]
    fn history_carries_across_frames() {
        let mut decimator = Decimator::new();
        let mut output = [Sample::ZERO; 120];
        let mut impulse = [Sample::ZERO; 120];
        impulse[119] = Sample::new(0.0, 1.0);
        decimator.filter(&impulse, &mut output);
        decimator.filter(&[Sample::ZERO; 120], &mut output);
        assert_eq!(output[TAPS - 2].im, DECIMATION_LOWPASS[0]);
    }
}
