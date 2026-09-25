const FULL_SPAN: f64 = 0.1;
const MAX_DRIFT: f64 = 0.004;

pub struct JitterBuffer {
    buf: Vec<f32>,
    channels: usize,
    capacity: usize,
    floor: usize,
    ceiling: usize,
    step: usize,
    trim_slack: usize,
    trim_hold: usize,
    relax_after: usize,
    smooth_over: f64,
    deadband: f64,
    target: usize,
    read_pos: usize,
    write_pos: usize,
    length: usize,
    buffering: bool,
    frac: f64,
    avg_depth: f64,
    clean_frames: usize,
    trim_streak: usize,
    underruns: u32,
    trimmed: u64,
}

impl JitterBuffer {
    #[must_use]
    pub fn new(target_frames: usize, max_frames: usize, channels: usize) -> Self {
        let capacity = max_frames.max(1);
        let channels = channels.max(1);
        let target = target_frames.min(capacity).max(1);
        Self {
            buf: vec![0.0; capacity * channels],
            channels,
            capacity,
            floor: target,
            ceiling: target.max((3 * target).min(capacity / 2)),
            step: ((target as f64 / 2.0).round() as usize).max(1),
            trim_slack: 2 * target,
            trim_hold: 20 * target,
            relax_after: 300 * target,
            smooth_over: (10 * target) as f64,
            deadband: 0.05f64.max(1.0 / target as f64),
            target,
            read_pos: 0,
            write_pos: 0,
            length: 0,
            buffering: true,
            frac: 0.0,
            avg_depth: target as f64,
            clean_frames: 0,
            trim_streak: 0,
            underruns: 0,
            trimmed: 0,
        }
    }

    #[must_use]
    pub const fn buffered(&self) -> usize {
        self.length
    }

    #[cfg(test)]
    #[must_use]
    pub const fn target_depth(&self) -> usize {
        self.target
    }

    #[must_use]
    pub const fn underruns(&self) -> u32 {
        self.underruns
    }

    #[must_use]
    pub const fn trimmed(&self) -> u64 {
        self.trimmed
    }

    pub fn push(&mut self, chunk: &[f32]) {
        let channels = self.channels;
        let frames = chunk.len() / channels;
        let start = frames.saturating_sub(self.capacity);
        let fresh = frames - start;
        self.trimmed += start as u64;
        if self.length + fresh > self.capacity {
            self.drop_oldest((self.length + fresh).saturating_sub(self.target));
            self.trim_streak = 0;
        }
        for frame in chunk[start * channels..frames * channels].chunks_exact(channels) {
            let at = self.write_pos * channels;
            self.buf[at..at + channels].copy_from_slice(frame);
            self.write_pos = (self.write_pos + 1) % self.capacity;
        }
        self.length += fresh;
        if self.length >= self.target {
            self.buffering = false;
        }
        self.trim_backlog(fresh);
    }

    fn trim_backlog(&mut self, fresh: usize) {
        if self.avg_depth <= (self.target + self.trim_slack) as f64 {
            self.trim_streak = 0;
            return;
        }
        self.trim_streak += fresh;
        if self.trim_streak >= self.trim_hold {
            self.drop_oldest(self.length.saturating_sub(self.target));
            self.avg_depth = self.target as f64;
            self.trim_streak = 0;
        }
    }

    #[cfg(test)]
    pub fn read(&mut self, outputs: &mut [&mut [f32]]) -> bool {
        self.read_at(outputs, 1.0)
    }

    pub fn read_at(&mut self, outputs: &mut [&mut [f32]], base_rate: f64) -> bool {
        let wanted = outputs.first().map_or(0, |out| out.len());
        if wanted == 0 {
            return false;
        }
        if self.buffering {
            for out in outputs.iter_mut() {
                out.fill(0.0);
            }
            self.smooth(wanted);
            return false;
        }
        let rate = self.drift_rate() * base_rate;
        let produced = self.resample(outputs, wanted, rate);
        if produced < wanted {
            for out in outputs.iter_mut() {
                if let Some(rest) = out.get_mut(produced..) {
                    rest.fill(0.0);
                }
            }
            self.underrun();
        } else {
            self.clean_frames += produced;
            if self.clean_frames >= self.relax_after {
                self.clean_frames = 0;
                self.target = self.floor.max(self.target.saturating_sub(self.step));
            }
        }
        self.smooth(wanted);
        produced > 0
    }

    fn resample(&mut self, outputs: &mut [&mut [f32]], wanted: usize, rate: f64) -> usize {
        let capacity = self.capacity;
        let channels = self.channels;
        let mut pos = self.read_pos;
        let mut frac = self.frac;
        let mut avail = self.length;
        let mut produced = 0;
        while produced < wanted {
            let whole = frac.floor() as usize;
            if whole > 0 {
                if avail < whole {
                    break;
                }
                pos = (pos + whole) % capacity;
                avail -= whole;
                frac -= whole as f64;
            }
            if avail < if frac > 0.0 { 2 } else { 1 } {
                break;
            }
            let next = (pos + 1) % capacity;
            for (lane_index, out) in outputs.iter_mut().enumerate() {
                let lane = lane_index.min(channels - 1);
                let a = f64::from(self.buf[pos * channels + lane]);
                let b = f64::from(self.buf[next * channels + lane]);
                if let Some(slot) = out.get_mut(produced) {
                    *slot = if frac > 0.0 { a + (b - a) * frac } else { a } as f32;
                }
            }
            produced += 1;
            frac += rate;
        }
        let played = frac.floor() as usize;
        if played > 0 && avail >= played {
            pos = (pos + played) % capacity;
            avail -= played;
            frac -= played as f64;
        }
        self.read_pos = pos;
        self.frac = frac;
        self.length = avail;
        produced
    }

    pub fn clear(&mut self) {
        self.read_pos = 0;
        self.write_pos = 0;
        self.length = 0;
        self.frac = 0.0;
        self.buffering = true;
        self.trim_streak = 0;
        self.clean_frames = 0;
        self.avg_depth = self.target as f64;
    }

    fn drift_rate(&self) -> f64 {
        let error = (self.avg_depth - self.target as f64) / self.target as f64;
        let excess = error.abs() - self.deadband;
        if excess <= 0.0 {
            return 1.0;
        }
        let correction = (excess / FULL_SPAN).min(1.0) * MAX_DRIFT;
        if error > 0.0 {
            1.0 + correction
        } else {
            1.0 - correction
        }
    }

    fn smooth(&mut self, frames: usize) {
        let alpha = (frames as f64 / self.smooth_over).min(1.0);
        self.avg_depth += (self.length as f64 - self.avg_depth) * alpha;
    }

    fn underrun(&mut self) {
        self.buffering = true;
        self.underruns += 1;
        self.clean_frames = 0;
        self.trim_streak = 0;
        self.target = self.ceiling.min(self.target + self.step);
    }

    fn drop_oldest(&mut self, count: usize) {
        let drop = count.min(self.length);
        if drop == 0 {
            return;
        }
        self.trimmed += drop as u64;
        self.read_pos = (self.read_pos + drop) % self.capacity;
        self.length -= drop;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(start: f32, length: usize) -> Vec<f32> {
        (0..length).map(|i| start + i as f32).collect()
    }

    fn read(jitter: &mut JitterBuffer, frames: usize, channels: usize) -> (bool, Vec<Vec<f32>>) {
        let mut lanes = vec![vec![0.0f32; frames]; channels];
        let ok = {
            let mut outputs: Vec<&mut [f32]> = lanes.iter_mut().map(Vec::as_mut_slice).collect();
            jitter.read(&mut outputs)
        };
        (ok, lanes)
    }

    fn mono(jitter: &mut JitterBuffer, frames: usize) -> (bool, Vec<f32>) {
        let (ok, mut lanes) = read(jitter, frames, 1);
        (ok, lanes.remove(0))
    }

    fn stream(jitter: &mut JitterBuffer, frames: usize, iterations: usize, from: f32) -> f32 {
        let mut next = from;
        let mut last = f32::NAN;
        let mut max_step = 0.0f32;
        for _ in 0..iterations {
            for value in mono(jitter, frames).1 {
                if !last.is_nan() {
                    max_step = max_step.max(value - last);
                }
                last = value;
            }
            jitter.push(&ramp(next, frames));
            next += frames as f32;
        }
        max_step
    }

    fn drift(jitter: &mut JitterBuffer, ppm: f64, seconds: f64) -> f32 {
        let quantum = 128;
        let packet: u32 = 960;
        let mut out = vec![0.0f32; quantum];
        let mut owed = 0.0f64;
        let mut next = 0u64;
        let mut last = f32::NAN;
        let mut max_step = 0.0f32;
        for _ in 0..((seconds * 48_000.0) / quantum as f64) as usize {
            owed += quantum as f64 * (1.0 + ppm / 1e6);
            while owed >= f64::from(packet) {
                let chunk: Vec<f32> = (0..packet)
                    .map(|i| ((next + u64::from(i)) % 100_000) as f32)
                    .collect();
                jitter.push(&chunk);
                next += u64::from(packet);
                owed -= f64::from(packet);
            }
            if jitter.read(&mut [out.as_mut_slice()]) {
                for &value in &out {
                    if !last.is_nan() {
                        max_step = max_step.max(value - last);
                    }
                    last = value;
                }
            }
        }
        max_step
    }

    #[test]
    fn outputs_silence_until_the_target_fill_is_reached() {
        let mut jitter = JitterBuffer::new(4, 16, 1);
        jitter.push(&ramp(1.0, 3));
        assert_eq!(mono(&mut jitter, 4), (false, vec![0.0; 4]));
        assert_eq!(jitter.buffered(), 3);
        jitter.push(&ramp(4.0, 2));
        assert_eq!(mono(&mut jitter, 4), (true, vec![1.0, 2.0, 3.0, 4.0]));
    }

    #[test]
    fn underruns_to_silence_grows_the_target_and_rebuffers_to_the_grown_one() {
        let mut jitter = JitterBuffer::new(4, 16, 1);
        jitter.push(&ramp(1.0, 5));
        assert_eq!(mono(&mut jitter, 4).1, vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(mono(&mut jitter, 4), (true, vec![5.0, 0.0, 0.0, 0.0]));
        assert_eq!(mono(&mut jitter, 4), (false, vec![0.0; 4]));
        assert_eq!(jitter.target_depth(), 6);
        jitter.push(&ramp(6.0, 5));
        assert_eq!(mono(&mut jitter, 4), (false, vec![0.0; 4]));
        jitter.push(&ramp(11.0, 1));
        let resumed = mono(&mut jitter, 4).1;
        assert!((resumed[0] - 6.0).abs() < f32::EPSILON);
        assert!((resumed[3] - 9.0).abs() < 0.05);
    }

    #[test]
    fn stops_growing_the_target_at_its_ceiling() {
        let mut jitter = JitterBuffer::new(4, 16, 1);
        for _ in 0..10 {
            let depth = jitter.target_depth();
            jitter.push(&ramp(1.0, depth));
            mono(&mut jitter, depth + 1);
        }
        assert_eq!(jitter.target_depth(), 8);
    }

    #[test]
    fn relaxes_the_grown_target_back_down_after_a_long_clean_run() {
        let mut jitter = JitterBuffer::new(4, 16, 1);
        jitter.push(&ramp(1.0, 4));
        mono(&mut jitter, 5);
        assert_eq!(jitter.target_depth(), 6);
        let mut next = 1.0;
        for _ in 0..320 {
            jitter.push(&ramp(next, 4));
            next += 4.0;
            mono(&mut jitter, 4);
        }
        assert_eq!(jitter.target_depth(), 4);
    }

    #[test]
    fn re_enters_buffering_after_an_exact_drain() {
        let mut jitter = JitterBuffer::new(4, 16, 1);
        jitter.push(&ramp(1.0, 4));
        assert!(mono(&mut jitter, 4).0);
        assert_eq!(jitter.buffered(), 0);
        assert_eq!(mono(&mut jitter, 4), (false, vec![0.0; 4]));
        jitter.push(&ramp(5.0, 3));
        assert!(!mono(&mut jitter, 4).0);
    }

    #[test]
    fn a_push_past_capacity_sheds_the_oldest_back_toward_target() {
        let mut jitter = JitterBuffer::new(2, 8, 1);
        jitter.push(&ramp(1.0, 6));
        jitter.push(&ramp(7.0, 6));
        assert_eq!(jitter.buffered(), 6);
        assert_eq!(mono(&mut jitter, 6).1, ramp(7.0, 6));
    }

    #[test]
    fn a_burst_past_capacity_resumes_at_target_depth() {
        let mut jitter = JitterBuffer::new(4, 16, 1);
        jitter.push(&ramp(1.0, 14));
        jitter.push(&ramp(15.0, 4));
        assert_eq!(jitter.buffered(), 4);
        assert_eq!(mono(&mut jitter, 4).1, ramp(15.0, 4));
    }

    #[test]
    fn sheds_sustained_backlog_by_playing_imperceptibly_fast() {
        let mut jitter = JitterBuffer::new(100, 1000, 1);
        jitter.push(&ramp(1.0, 250));
        let max_step = stream(&mut jitter, 50, 1_000, 251.0);
        assert!(jitter.buffered() < 200);
        assert!(jitter.buffered() > 50);
        assert!(max_step < 1.01);
    }

    #[test]
    fn rebuilds_headroom_by_playing_imperceptibly_slow() {
        let mut jitter = JitterBuffer::new(100, 1000, 1);
        jitter.push(&ramp(1.0, 100));
        mono(&mut jitter, 50);
        let max_step = stream(&mut jitter, 50, 1_000, 101.0);
        assert!(jitter.buffered() > 80);
        assert!(max_step < 1.01);
    }

    #[test]
    fn hard_sheds_a_backlog_far_too_large_for_drift_correction() {
        let mut jitter = JitterBuffer::new(100, 1000, 1);
        jitter.push(&ramp(1.0, 600));
        stream(&mut jitter, 50, 60, 601.0);
        assert!(jitter.buffered() < 150);
    }

    #[test]
    fn rides_out_a_producer_clock_a_tenth_percent_fast() {
        let mut jitter = JitterBuffer::new(4_800, 48_000, 1);
        let max_step = drift(&mut jitter, 1_000.0, 180.0);
        assert!(jitter.buffered() < 2 * 4_800);
        assert!(max_step < 1.01);
        assert_eq!(jitter.target_depth(), 4_800);
    }

    #[test]
    fn rides_out_a_producer_clock_a_tenth_percent_slow() {
        let mut jitter = JitterBuffer::new(4_800, 48_000, 1);
        let max_step = drift(&mut jitter, -1_000.0, 180.0);
        assert!(jitter.buffered() > 2_400);
        assert!(max_step < 1.01);
        assert_eq!(jitter.target_depth(), 4_800);
    }

    #[test]
    fn holds_its_depth_against_a_producer_clock_a_fifth_percent_slow() {
        let mut jitter = JitterBuffer::new(4_800, 48_000, 1);
        let max_step = drift(&mut jitter, -2_000.0, 60.0);
        assert!(jitter.buffered() as f64 > 0.85 * 4_800.0);
        assert!(max_step < 1.01);
        assert_eq!(jitter.underruns(), 0);
    }

    #[test]
    fn keeps_only_the_newest_samples_of_a_chunk_larger_than_capacity() {
        let mut jitter = JitterBuffer::new(2, 4, 1);
        jitter.push(&ramp(1.0, 10));
        assert_eq!(jitter.buffered(), 4);
        assert_eq!(mono(&mut jitter, 4).1, ramp(7.0, 4));
    }

    #[test]
    fn stays_correct_across_ring_wraparound() {
        let mut jitter = JitterBuffer::new(2, 5, 1);
        jitter.push(&ramp(1.0, 4));
        assert_eq!(mono(&mut jitter, 3).1, ramp(1.0, 3));
        jitter.push(&ramp(5.0, 4));
        assert_eq!(mono(&mut jitter, 5).1, ramp(4.0, 5));
    }

    #[test]
    fn clear_resets_to_an_empty_buffering_state() {
        let mut jitter = JitterBuffer::new(2, 8, 1);
        jitter.push(&ramp(1.0, 4));
        jitter.clear();
        assert_eq!(jitter.buffered(), 0);
        assert_eq!(mono(&mut jitter, 4), (false, vec![0.0; 4]));
        jitter.push(&ramp(1.0, 2));
        let (ok, resumed) = mono(&mut jitter, 2);
        assert!(ok);
        assert!((resumed[0] - 1.0).abs() < f32::EPSILON);
        assert!((resumed[1] - 2.0).abs() < 0.01);
    }

    #[test]
    fn deinterleaves_stereo_frames_into_one_output_per_channel() {
        let mut jitter = JitterBuffer::new(2, 8, 2);
        jitter.push(&[1.0, -1.0, 2.0, -2.0, 3.0, -3.0]);
        assert_eq!(jitter.buffered(), 3);
        assert_eq!(
            read(&mut jitter, 3, 2).1,
            vec![vec![1.0, 2.0, 3.0], vec![-1.0, -2.0, -3.0]]
        );
    }

    #[test]
    fn counts_depth_target_and_capacity_in_frames() {
        let mut jitter = JitterBuffer::new(4, 8, 2);
        jitter.push(&[1.0, -1.0, 2.0, -2.0, 3.0, -3.0]);
        assert_eq!(jitter.buffered(), 3);
        assert!(!read(&mut jitter, 4, 2).0);
        jitter.push(&[4.0, -4.0]);
        assert_eq!(
            read(&mut jitter, 4, 2),
            (
                true,
                vec![vec![1.0, 2.0, 3.0, 4.0], vec![-1.0, -2.0, -3.0, -4.0]]
            )
        );
    }

    #[test]
    fn stays_correct_across_ring_wraparound_in_stereo() {
        let mut jitter = JitterBuffer::new(2, 3, 2);
        jitter.push(&[1.0, -1.0, 2.0, -2.0]);
        assert_eq!(read(&mut jitter, 1, 2).1, vec![vec![1.0], vec![-1.0]]);
        jitter.push(&[3.0, -3.0, 4.0, -4.0]);
        assert_eq!(
            read(&mut jitter, 3, 2).1,
            vec![vec![2.0, 3.0, 4.0], vec![-2.0, -3.0, -4.0]]
        );
    }

    #[test]
    fn feeds_every_output_channel_from_a_stream_with_fewer() {
        let mut jitter = JitterBuffer::new(2, 8, 1);
        jitter.push(&ramp(1.0, 3));
        assert_eq!(
            read(&mut jitter, 3, 2).1,
            vec![vec![1.0, 2.0, 3.0], vec![1.0, 2.0, 3.0]]
        );
    }

    #[test]
    fn counts_trimmed_audio_when_the_backlog_exceeds_its_budget() {
        let mut jitter = JitterBuffer::new(4, 12, 1);
        jitter.push(&[0.0; 10]);
        jitter.push(&[0.0; 10]);
        assert!(jitter.buffered() <= 12);
        assert!(jitter.trimmed() > 0);
    }

    #[test]
    fn a_slower_base_rate_stretches_the_same_audio_over_more_output() {
        let mut jitter = JitterBuffer::new(4, 64, 1);
        jitter.push(&ramp(0.0, 32));
        let mut out = vec![0.0f32; 8];
        assert!(jitter.read_at(&mut [out.as_mut_slice()], 0.5));
        assert!((out[1] - 0.5).abs() < 0.01);
        assert!((out[7] - 3.5).abs() < 0.01);
    }
}
