pub(crate) const TRACK_CAPACITY: usize = 1 << 16;

pub(crate) struct Track {
    buf: Vec<f32>,
    head: u64,
}

impl Track {
    pub(crate) fn new() -> Self {
        Self {
            buf: vec![0.0; TRACK_CAPACITY],
            head: 0,
        }
    }

    pub(crate) fn push(&mut self, value: f32) {
        self.buf[(self.head as usize) & (TRACK_CAPACITY - 1)] = value;
        self.head += 1;
    }

    pub(crate) fn head(&self) -> u64 {
        self.head
    }

    pub(crate) fn oldest(&self) -> u64 {
        self.head.saturating_sub(TRACK_CAPACITY as u64)
    }

    pub(crate) fn buffered(&self, from: u64, to: u64) -> bool {
        from >= self.oldest() && to <= self.head
    }

    pub(crate) fn aged_out(&self, from: u64) -> bool {
        from < self.oldest()
    }

    pub(crate) fn get(&self, index: u64) -> f32 {
        if index >= self.head || index < self.oldest() {
            return 0.0;
        }
        self.buf[(index as usize) & (TRACK_CAPACITY - 1)]
    }

    pub(crate) fn at(&self, position: f64) -> f32 {
        let position = position.max(0.0);
        let index = position.floor();
        let frac = (position - index) as f32;
        let index = index as u64;
        let before = self.get(index);
        let after = self.get(index + 1);
        before + (after - before) * frac
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_between_samples_interpolate() {
        let mut track = Track::new();
        track.push(0.0);
        track.push(2.0);
        assert!((track.at(0.25) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn old_samples_age_out() {
        let mut track = Track::new();
        for _ in 0..TRACK_CAPACITY + 10 {
            track.push(1.0);
        }
        assert!(track.aged_out(5));
        assert_eq!(track.get(5), 0.0);
        assert!(track.buffered(10, track.head()));
    }
}
