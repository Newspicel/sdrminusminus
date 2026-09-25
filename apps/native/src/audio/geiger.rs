use std::{
    f32::consts::TAU,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

pub const SLOWEST_HZ: f32 = 1.5;
pub const FASTEST_HZ: f32 = 40.0;
const CLICK_SECONDS: f32 = 0.004;
const TONE_HZ: f32 = 1_800.0;
const PEAK_GAIN: f32 = 0.28;
const TAIL_GAIN: f32 = 0.0001;
const FIRST_CLICK_SECONDS: f32 = 0.05;

#[must_use]
pub fn click_rate_hz(strength: f32) -> f32 {
    let clamped = if strength.is_finite() {
        strength.clamp(0.0, 1.0)
    } else {
        0.0
    };
    SLOWEST_HZ + (FASTEST_HZ - SLOWEST_HZ) * clamped * clamped
}

#[derive(Default)]
pub struct ClickState {
    on: AtomicBool,
    strength: AtomicU32,
}

impl ClickState {
    pub fn set(&self, on: bool, strength: f32) {
        self.strength.store(strength.to_bits(), Ordering::Relaxed);
        self.on.store(on, Ordering::Relaxed);
    }

    #[must_use]
    pub fn read(&self) -> Option<f32> {
        self.on
            .load(Ordering::Relaxed)
            .then(|| f32::from_bits(self.strength.load(Ordering::Relaxed)))
    }
}

pub struct ClickSynth {
    rate: f32,
    until_next: f32,
    elapsed: u32,
    length: u32,
    decay: f32,
    playing: bool,
}

impl ClickSynth {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let length = (CLICK_SECONDS * sample_rate).round().max(1.0) as u32;
        Self {
            rate: sample_rate,
            until_next: FIRST_CLICK_SECONDS * sample_rate,
            elapsed: 0,
            length,
            decay: (TAIL_GAIN / PEAK_GAIN).powf(1.0 / length as f32),
            playing: false,
        }
    }

    pub fn add(&mut self, strength: Option<f32>, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let sample = self.next_sample(strength);
            *l += sample;
            *r += sample;
        }
    }

    fn next_sample(&mut self, strength: Option<f32>) -> f32 {
        let Some(strength) = strength else {
            self.until_next = FIRST_CLICK_SECONDS * self.rate;
            return self.ring();
        };
        self.until_next -= 1.0;
        if self.until_next <= 0.0 {
            self.until_next += self.rate / click_rate_hz(strength);
            self.playing = true;
            self.elapsed = 0;
        }
        self.ring()
    }

    fn ring(&mut self) -> f32 {
        if !self.playing {
            return 0.0;
        }
        let t = self.elapsed as f32;
        self.elapsed += 1;
        if self.elapsed >= self.length {
            self.playing = false;
        }
        PEAK_GAIN * self.decay.powf(t) * (TAU * TONE_HZ * t / self.rate).sin()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clicks_in_one_second(strength: f32) -> usize {
        let mut synth = ClickSynth::new(48_000.0);
        let mut left = vec![0.0f32; 48_000];
        let mut right = vec![0.0f32; 48_000];
        synth.add(Some(strength), &mut left, &mut right);
        left.windows(2)
            .filter(|pair| pair[0] == 0.0 && pair[1] != 0.0)
            .count()
            + usize::from(left[0] != 0.0)
    }

    #[test]
    fn spans_the_whole_rate_range_across_the_whole_strength_range() {
        assert!((click_rate_hz(0.0) - SLOWEST_HZ).abs() < f32::EPSILON);
        assert!((click_rate_hz(1.0) - FASTEST_HZ).abs() < f32::EPSILON);
    }

    #[test]
    fn climbs_so_closing_in_always_sounds_faster() {
        let steps: Vec<f32> = [0.0, 0.2, 0.4, 0.6, 0.8, 1.0]
            .into_iter()
            .map(click_rate_hz)
            .collect();
        assert!(steps.windows(2).all(|pair| pair[1] > pair[0]));
    }

    #[test]
    fn spends_its_resolution_near_the_transmitter() {
        assert!(click_rate_hz(0.9) - click_rate_hz(0.8) > click_rate_hz(0.2) - click_rate_hz(0.1));
    }

    #[test]
    fn refuses_to_be_driven_out_of_range_by_a_nonsense_reading() {
        for bad in [f32::NAN, f32::INFINITY, -5.0, 12.0] {
            let rate = click_rate_hz(bad);
            assert!((SLOWEST_HZ..=FASTEST_HZ).contains(&rate));
        }
    }

    #[test]
    fn clicks_at_the_rate_the_strength_asks_for() {
        assert!((1..=2).contains(&clicks_in_one_second(0.0)));
        assert!((37..=41).contains(&clicks_in_one_second(1.0)));
    }

    #[test]
    fn stays_silent_while_off_and_writes_both_lanes_alike() {
        let mut synth = ClickSynth::new(48_000.0);
        let mut left = vec![0.0f32; 4_800];
        let mut right = vec![0.0f32; 4_800];
        synth.add(None, &mut left, &mut right);
        assert!(left.iter().all(|sample| *sample == 0.0));
        synth.add(Some(1.0), &mut left, &mut right);
        assert!(left.iter().any(|sample| *sample != 0.0));
        assert_eq!(left, right);
    }

    #[test]
    fn a_state_reads_nothing_until_switched_on() {
        let state = ClickState::default();
        assert_eq!(state.read(), None);
        state.set(true, 0.5);
        assert_eq!(state.read(), Some(0.5));
    }
}
