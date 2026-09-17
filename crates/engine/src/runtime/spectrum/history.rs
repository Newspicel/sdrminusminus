use num_complex::Complex;

pub(in crate::runtime) struct SpectrumHistory {
    samples: Vec<Complex<f32>>,
    window: Vec<Complex<f32>>,
    write: usize,
    since_last: usize,
}

impl SpectrumHistory {
    pub(in crate::runtime) fn new(size: usize) -> Self {
        assert!(size > 0);
        Self {
            samples: vec![Complex::new(0.0, 0.0); size],
            window: vec![Complex::new(0.0, 0.0); size],
            write: 0,
            since_last: 0,
        }
    }

    pub(in crate::runtime) fn reset(&mut self) {
        self.samples.fill(Complex::new(0.0, 0.0));
        self.write = 0;
        self.since_last = 0;
    }

    pub(in crate::runtime) fn push(
        &mut self,
        mut input: &[Complex<f32>],
        mut index: u64,
        hop: usize,
        mut emit: impl FnMut(&[Complex<f32>], u64),
    ) {
        while !input.is_empty() {
            let len = input.len().min(hop.saturating_sub(self.since_last).max(1));
            self.append(&input[..len]);
            input = &input[len..];
            index += len as u64;
            self.since_last += len;
            if self.since_last >= hop {
                self.since_last = 0;
                let (head, tail) = self.samples.split_at(self.write);
                self.window[..tail.len()].copy_from_slice(tail);
                self.window[tail.len()..].copy_from_slice(head);
                emit(&self.window, index);
            }
        }
    }

    fn append(&mut self, input: &[Complex<f32>]) {
        let size = self.samples.len();
        if input.len() >= size {
            self.samples.copy_from_slice(&input[input.len() - size..]);
            self.write = 0;
            return;
        }
        let first = input.len().min(size - self.write);
        self.samples[self.write..self.write + first].copy_from_slice(&input[..first]);
        self.samples[..input.len() - first].copy_from_slice(&input[first..]);
        self.write = (self.write + input.len()) % size;
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_test_support::{assert_no_alloc, measure_throughput};

    use super::*;

    struct Reference {
        samples: Vec<Complex<f32>>,
        window: Vec<Complex<f32>>,
        write: usize,
        since_last: usize,
    }

    impl Reference {
        fn new(size: usize) -> Self {
            Self {
                samples: vec![Complex::new(0.0, 0.0); size],
                window: vec![Complex::new(0.0, 0.0); size],
                write: 0,
                since_last: 0,
            }
        }

        fn push(
            &mut self,
            input: &[Complex<f32>],
            mut index: u64,
            hop: usize,
            mut emit: impl FnMut(&[Complex<f32>], u64),
        ) {
            for &sample in input {
                self.samples[self.write] = sample;
                self.write += 1;
                if self.write == self.samples.len() {
                    self.write = 0;
                }
                index += 1;
                self.since_last += 1;
                if self.since_last >= hop {
                    self.since_last = 0;
                    for (offset, value) in self.window.iter_mut().enumerate() {
                        *value = self.samples[(self.write + offset) % self.samples.len()];
                    }
                    emit(&self.window, index);
                }
            }
        }
    }

    #[test]
    fn batching_preserves_windows_timestamps_and_rate_changes() {
        let samples: Vec<_> = (0..10_003)
            .map(|value| Complex::new(value as f32, -(value as f32)))
            .collect();
        for size in [1, 7, 64, 4096] {
            for block in [1, 3, 127, 2048, 10_003] {
                let mut actual = SpectrumHistory::new(size);
                let mut reference = Reference::new(size);
                let mut index = 71;
                for (part, input) in samples.chunks(block).enumerate() {
                    let hop = [1, 19, 6000, 31][part % 4];
                    let mut expected = Vec::new();
                    reference.push(input, index, hop, |window, timestamp| {
                        expected.push((window.to_vec(), timestamp));
                    });
                    let mut expected = expected.into_iter();
                    actual.push(input, index, hop, |window, timestamp| {
                        let (samples, stamp) = expected.next().expect("reference frame");
                        assert_eq!(window, samples);
                        assert_eq!(timestamp, stamp);
                    });
                    assert!(expected.next().is_none());
                    index += input.len() as u64;
                }
            }
        }
    }

    #[test]
    fn a_gap_clears_history_and_restarts_the_cadence() {
        let mut history = SpectrumHistory::new(4);
        history.push(&[Complex::new(9.0, 0.0); 3], 0, 4, |_, _| {
            panic!("early frame");
        });
        history.reset();
        history.push(&[Complex::new(2.0, 0.0); 2], 100, 2, |window, index| {
            assert_eq!(index, 102);
            assert_eq!(window, [0.0, 0.0, 2.0, 2.0].map(|v| Complex::new(v, 0.0)));
        });
    }

    #[test]
    fn spectrum_history_reuses_storage_at_radio_rates() {
        let input = vec![Complex::new(0.5, -0.5); 2048];
        let mut history = SpectrumHistory::new(4096);
        let mut reference = Reference::new(4096);
        let emit = |samples: &[Complex<f32>], index| {
            std::hint::black_box((samples, index));
        };
        assert_no_alloc("spectrum history", || {
            history.push(&input, 0, 666_666, emit)
        });
        let scalar = measure_throughput(2000, input.len() as u64, || {
            reference.push(std::hint::black_box(&input), 0, 666_666, emit);
        });
        let batched = measure_throughput(2000, input.len() as u64, || {
            history.push(std::hint::black_box(&input), 0, 666_666, emit);
        });
        eprintln!("spectrum history: scalar={scalar:.1} MS/s batched={batched:.1} MS/s");
        assert!(batched > 20.0, "spectrum history: {batched:.1} MS/s");
    }
}
