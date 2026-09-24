use num_complex::Complex;

const BLOCK: usize = 256;
const DETECT_RATIO: f32 = 1.6;
const FLOOR_RATE: f32 = 0.05;
const PRE_SAMPLES: u64 = 1_000;
const POST_SAMPLES: u64 = 1_500;
const MAX_BURST_SAMPLES: u64 = 25_000;
const ONSET_LEAD: u64 = 64;

#[derive(Clone, Copy)]
struct Active {
    start: u64,
    last_hot: u64,
}

pub struct Window {
    pub start: u64,
    pub end: u64,
    pub onset: u64,
}

pub struct BurstDetector {
    sum: f32,
    count: usize,
    position: u64,
    floor: Option<f32>,
    active: Option<Active>,
}

impl BurstDetector {
    pub fn new() -> Self {
        Self {
            sum: 0.0,
            count: 0,
            position: 0,
            floor: None,
            active: None,
        }
    }

    pub fn earliest_needed(&self) -> u64 {
        let from = self.active.map_or(self.position, |a| a.start);
        from.saturating_sub(PRE_SAMPLES + BLOCK as u64)
    }

    pub fn push(&mut self, input: &[Complex<f32>], out: &mut Vec<Window>) {
        for sample in input {
            self.sum += sample.norm_sqr();
            self.count += 1;
            self.position += 1;
            if self.count == BLOCK {
                let mean = self.sum / BLOCK as f32;
                self.sum = 0.0;
                self.count = 0;
                self.block(mean, out);
            }
        }
    }

    fn block(&mut self, mean: f32, out: &mut Vec<Window>) {
        let floor = *self.floor.get_or_insert(mean);
        let end = self.position;
        let hot = mean > floor * DETECT_RATIO;
        match self.active {
            None if hot => {
                self.active = Some(Active {
                    start: end - BLOCK as u64,
                    last_hot: end,
                });
            }
            None => {
                self.floor = Some(floor + FLOOR_RATE * (mean - floor));
            }
            Some(mut active) => {
                if hot {
                    active.last_hot = end;
                }
                let quiet = end - active.last_hot >= POST_SAMPLES;
                let long = end - active.start >= MAX_BURST_SAMPLES;
                if quiet || long {
                    out.push(Window {
                        start: active.start.saturating_sub(PRE_SAMPLES),
                        end: active.last_hot + POST_SAMPLES,
                        onset: active.start.saturating_sub(ONSET_LEAD),
                    });
                    self.active = None;
                } else {
                    self.active = Some(active);
                }
            }
        }
    }
}
