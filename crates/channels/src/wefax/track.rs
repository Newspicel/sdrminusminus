pub(super) const TRACK_CAPACITY: usize = 1 << 17;

pub(super) struct Track {
    buf: Vec<f32>,
    head: u64,
}

impl Track {
    pub(super) fn new() -> Self {
        Self {
            buf: vec![0.0; TRACK_CAPACITY],
            head: 0,
        }
    }

    pub(super) fn head(&self) -> u64 {
        self.head
    }

    pub(super) fn push(&mut self, value: f32) {
        self.buf[(self.head as usize) & (TRACK_CAPACITY - 1)] = value;
        self.head += 1;
    }

    pub(super) fn oldest(&self) -> u64 {
        self.head.saturating_sub(TRACK_CAPACITY as u64)
    }

    pub(super) fn buffered(&self, from: u64, to: u64) -> bool {
        from >= self.oldest() && to <= self.head
    }

    pub(super) fn aged_out(&self, from: u64) -> bool {
        from < self.oldest()
    }

    fn get(&self, index: u64) -> f32 {
        if index >= self.head || index < self.oldest() {
            return 0.0;
        }
        self.buf[(index as usize) & (TRACK_CAPACITY - 1)]
    }

    pub(super) fn sum(&self, from: u64, to: u64) -> f64 {
        (from..to).map(|index| f64::from(self.get(index))).sum()
    }

    pub(super) fn mean(&self, from: u64, to: u64) -> f32 {
        let to = to.max(from + 1);
        (self.sum(from, to) / (to - from) as f64) as f32
    }

    pub(super) fn at(&self, index: u64) -> f64 {
        f64::from(self.get(index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_samples_age_out() {
        let mut track = Track::new();
        for index in 0..TRACK_CAPACITY + 10 {
            track.push(index as f32);
        }
        assert!(track.aged_out(5));
        assert!(track.buffered(10, track.head()));
        assert_eq!(track.mean(10, 12), 10.5);
    }
}
