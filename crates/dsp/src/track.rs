/// One thing the radar believes is out there, followed from one integration to the next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Track {
    pub id: u32,
    pub range: f32,
    pub doppler: f32,
    /// How fast the range is changing, in range units per update, which is what lets a track keep
    /// its gate over a target that is moving rather than sitting still.
    pub range_rate: f32,
    pub hits: u32,
    pub misses: u32,
    pub confirmed: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct TrackerParams {
    /// How far a detection may sit from where a track expected it and still be the same thing.
    pub gate_range: f32,
    pub gate_doppler: f32,
    /// How many updates in a row a candidate has to appear in before it is called a target.
    pub confirm_hits: u32,
    /// How many it may go missing for before it is dropped.
    pub drop_misses: u32,
    /// How much of each update's measurement is believed, against the track's own prediction.
    pub gain: f32,
}

impl Default for TrackerParams {
    fn default() -> Self {
        Self {
            gate_range: 2.0,
            gate_doppler: 8.0,
            confirm_hits: 3,
            drop_misses: 3,
            gain: 0.6,
        }
    }
}

/// Turns a list of detections per integration into targets with identity.
///
/// Nearest neighbour in range and Doppler, an alpha-beta smoother on what is associated, and a
/// count of hits and misses. Deliberately simple: a detection that appears once is noise until it
/// appears again, and that is the whole claim being made here.
#[derive(Default)]
pub struct Tracker {
    params: TrackerParams,
    tracks: Vec<Track>,
    next_id: u32,
    taken: Vec<bool>,
}

impl Tracker {
    #[must_use]
    pub fn new(params: TrackerParams) -> Self {
        Self {
            params,
            tracks: Vec::new(),
            next_id: 1,
            taken: Vec::new(),
        }
    }

    pub fn set_params(&mut self, params: TrackerParams) {
        self.params = params;
    }

    #[must_use]
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub fn reset(&mut self) {
        self.tracks.clear();
        self.taken.clear();
    }

    /// Folds one integration's detections in, and hands back which track each of them belongs to.
    ///
    /// A detection with no track is one the tracker has not decided about yet, which is exactly
    /// what a single-look false alarm looks like.
    pub fn update(&mut self, detections: &[(f32, f32)], out: &mut Vec<Option<u32>>) {
        out.clear();
        out.resize(detections.len(), None);
        self.taken.clear();
        self.taken.resize(detections.len(), false);
        for index in 0..self.tracks.len() {
            let predicted = self.tracks[index].range + self.tracks[index].range_rate;
            let mut best: Option<(usize, f32)> = None;
            for (slot, &(range, doppler)) in detections.iter().enumerate() {
                if self.taken[slot] {
                    continue;
                }
                let dr = (range - predicted).abs();
                let dd = (doppler - self.tracks[index].doppler).abs();
                if dr > self.params.gate_range || dd > self.params.gate_doppler {
                    continue;
                }
                let cost = dr / self.params.gate_range.max(f32::MIN_POSITIVE)
                    + dd / self.params.gate_doppler.max(f32::MIN_POSITIVE);
                if best.is_none_or(|(_, previous)| cost < previous) {
                    best = Some((slot, cost));
                }
            }
            match best {
                Some((slot, _)) => {
                    let (range, doppler) = detections[slot];
                    self.taken[slot] = true;
                    let track = &mut self.tracks[index];
                    let residual = range - predicted;
                    track.range = predicted + self.params.gain * residual;
                    track.range_rate += self.params.gain * residual * 0.5;
                    track.doppler += self.params.gain * (doppler - track.doppler);
                    track.hits = track.hits.saturating_add(1);
                    track.misses = 0;
                    track.confirmed |= track.hits >= self.params.confirm_hits;
                    if track.confirmed {
                        out[slot] = Some(track.id);
                    }
                }
                None => {
                    let track = &mut self.tracks[index];
                    track.misses = track.misses.saturating_add(1);
                    track.range = predicted;
                }
            }
        }
        for (slot, &(range, doppler)) in detections.iter().enumerate() {
            if self.taken[slot] {
                continue;
            }
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1).max(1);
            self.tracks.push(Track {
                id,
                range,
                doppler,
                range_rate: 0.0,
                hits: 1,
                misses: 0,
                confirmed: false,
            });
        }
        let drop_after = self.params.drop_misses;
        self.tracks.retain(|track| track.misses <= drop_after);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker() -> Tracker {
        Tracker::new(TrackerParams::default())
    }

    #[test]
    fn something_seen_once_is_not_a_target() {
        let mut tracker = tracker();
        let mut ids = Vec::new();
        tracker.update(&[(30.0, 12.0)], &mut ids);
        assert_eq!(ids, vec![None]);
        assert_eq!(tracker.tracks().len(), 1);
        assert!(!tracker.tracks()[0].confirmed);
    }

    #[test]
    fn something_seen_again_and_again_becomes_one() {
        let mut tracker = tracker();
        let mut ids = Vec::new();
        for _ in 0..3 {
            tracker.update(&[(30.0, 12.0)], &mut ids);
        }
        assert_eq!(ids, vec![Some(1)]);
        assert!(tracker.tracks()[0].confirmed);
    }

    #[test]
    fn a_target_that_is_moving_is_followed_rather_than_started_again() {
        let mut tracker = tracker();
        let mut ids = Vec::new();
        let mut range = 30.0;
        for _ in 0..8 {
            tracker.update(&[(range, 12.0)], &mut ids);
            range += 1.0;
        }
        assert_eq!(tracker.tracks().len(), 1, "one target, not eight");
        assert_eq!(ids, vec![Some(1)]);
        assert!(
            tracker.tracks()[0].range_rate > 0.2,
            "the tracker should know which way it is going: {:?}",
            tracker.tracks()[0]
        );
    }

    #[test]
    fn a_target_that_goes_quiet_for_long_enough_is_dropped() {
        let mut tracker = tracker();
        let mut ids = Vec::new();
        for _ in 0..3 {
            tracker.update(&[(30.0, 12.0)], &mut ids);
        }
        for _ in 0..3 {
            tracker.update(&[], &mut ids);
            assert_eq!(tracker.tracks().len(), 1, "it is missing, not gone");
        }
        tracker.update(&[], &mut ids);
        assert!(tracker.tracks().is_empty());
    }

    #[test]
    fn two_targets_keep_their_own_names() {
        let mut tracker = tracker();
        let mut ids = Vec::new();
        for _ in 0..3 {
            tracker.update(&[(30.0, 12.0), (90.0, -40.0)], &mut ids);
        }
        assert_eq!(ids, vec![Some(1), Some(2)]);
        for _ in 0..3 {
            tracker.update(&[(90.0, -40.0), (30.0, 12.0)], &mut ids);
        }
        assert_eq!(ids, vec![Some(2), Some(1)], "order is not identity");
    }
}
