use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use sdrmm_wire::{PlaybackAction, PlaybackRequest, PlaybackStatus};

const NO_SEEK: u64 = u64::MAX;

#[derive(Debug)]
pub struct PlaybackShared {
    position: AtomicU64,
    requested_position: AtomicU64,
    position_generation: AtomicU64,
    total: AtomicU64,
    paused: AtomicBool,
    seek: AtomicU64,
}

impl PlaybackShared {
    #[must_use]
    pub fn new(total_samples: u64) -> Self {
        Self {
            position: AtomicU64::new(0),
            requested_position: AtomicU64::new(0),
            position_generation: AtomicU64::new(0),
            total: AtomicU64::new(total_samples),
            paused: AtomicBool::new(false),
            seek: AtomicU64::new(NO_SEEK),
        }
    }

    #[must_use]
    pub fn status(&self) -> PlaybackStatus {
        PlaybackStatus {
            position_samples: self.position.load(Ordering::Relaxed),
            total_samples: self.total.load(Ordering::Relaxed),
            paused: self.paused.load(Ordering::Relaxed),
        }
    }

    pub fn control(&self, request: &PlaybackRequest) {
        match request.action {
            PlaybackAction::Play => self.paused.store(false, Ordering::Relaxed),
            PlaybackAction::Pause => {
                self.paused.store(true, Ordering::Relaxed);
                self.request_seek(self.position.load(Ordering::SeqCst));
            }
            PlaybackAction::Stop => {
                self.paused.store(true, Ordering::Relaxed);
                self.request_seek(0);
            }
            PlaybackAction::Seek => self.request_seek(request.position_samples.unwrap_or(0)),
        }
    }

    #[must_use]
    pub fn position_generation(&self) -> u64 {
        self.position_generation.load(Ordering::SeqCst)
    }

    pub fn set_position(&self, samples: u64, generation: u64) {
        if self.position_generation.load(Ordering::SeqCst) != generation {
            return;
        }
        self.position.store(samples, Ordering::SeqCst);
        if self.position_generation.load(Ordering::SeqCst) == generation {
            return;
        }

        loop {
            let current = self.position_generation.load(Ordering::SeqCst);
            let requested = self.requested_position.load(Ordering::SeqCst);
            self.position.store(requested, Ordering::SeqCst);
            if self.position_generation.load(Ordering::SeqCst) == current {
                break;
            }
        }
    }

    fn request_seek(&self, samples: u64) {
        let requested = samples.min(self.total());
        self.requested_position.store(requested, Ordering::SeqCst);
        self.seek.store(samples, Ordering::SeqCst);
        self.position_generation.fetch_add(1, Ordering::SeqCst);
        self.position.store(requested, Ordering::SeqCst);
    }

    #[must_use]
    pub fn take_seek(&self) -> Option<u64> {
        match self.seek.swap(NO_SEEK, Ordering::Relaxed) {
            NO_SEEK => None,
            target => Some(target),
        }
    }

    #[must_use]
    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn seek_pending(&self) -> bool {
        self.seek.load(Ordering::Relaxed) != NO_SEEK
    }

    #[must_use]
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(action: PlaybackAction, position_samples: Option<u64>) -> PlaybackRequest {
        PlaybackRequest {
            action,
            position_samples,
        }
    }

    #[test]
    fn play_and_pause_flip_the_transport() {
        let shared = PlaybackShared::new(1_000);
        assert!(!shared.paused());

        shared.control(&request(PlaybackAction::Pause, None));
        assert!(shared.status().paused);

        shared.control(&request(PlaybackAction::Play, None));
        assert!(!shared.status().paused);
    }

    #[test]
    fn stop_pauses_and_rewinds_in_one_step() {
        let shared = PlaybackShared::new(1_000);
        shared.set_position(400, shared.position_generation());

        shared.control(&request(PlaybackAction::Stop, None));
        let status = shared.status();
        assert!(status.paused);
        assert_eq!(status.position_samples, 0);
        assert_eq!(shared.take_seek(), Some(0));
    }

    #[test]
    fn stale_worker_progress_cannot_overwrite_a_stop() {
        let shared = PlaybackShared::new(1_000);
        let in_flight = shared.position_generation();

        shared.control(&request(PlaybackAction::Stop, None));
        shared.set_position(1_000, in_flight);

        assert_eq!(
            shared.status(),
            PlaybackStatus {
                position_samples: 0,
                total_samples: 1_000,
                paused: true,
            }
        );
    }

    #[test]
    fn stale_worker_progress_cannot_overwrite_a_pause() {
        let shared = PlaybackShared::new(1_000);
        let in_flight = shared.position_generation();

        shared.control(&request(PlaybackAction::Pause, None));
        let paused = shared.status();
        shared.set_position(1_000, in_flight);

        assert_eq!(shared.status(), paused);
        assert_eq!(shared.take_seek(), Some(paused.position_samples));
    }

    #[test]
    fn a_seek_moves_the_reported_position_before_the_worker_sees_it() {
        let shared = PlaybackShared::new(1_000);
        shared.control(&request(PlaybackAction::Pause, None));

        shared.control(&request(PlaybackAction::Seek, Some(750)));
        assert_eq!(shared.status().position_samples, 750);
        assert_eq!(shared.take_seek(), Some(750));
    }

    #[test]
    fn a_seek_past_the_end_reports_the_end() {
        let shared = PlaybackShared::new(1_000);
        shared.control(&request(PlaybackAction::Seek, Some(9_999)));
        assert_eq!(shared.status().position_samples, 1_000);
    }

    #[test]
    fn a_seek_is_taken_once() {
        let shared = PlaybackShared::new(1_000);
        assert_eq!(shared.take_seek(), None);

        shared.control(&request(PlaybackAction::Seek, Some(12)));
        assert_eq!(shared.take_seek(), Some(12));
        assert_eq!(shared.take_seek(), None, "a taken seek must not repeat");
    }

    #[test]
    fn a_seek_without_a_position_is_a_seek_to_the_start() {
        let shared = PlaybackShared::new(1_000);
        shared.set_position(500, shared.position_generation());
        shared.control(&request(PlaybackAction::Seek, None));
        assert_eq!(shared.take_seek(), Some(0));
        assert_eq!(shared.status().position_samples, 0);
    }
}
