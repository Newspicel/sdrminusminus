//! Transport shared between a file-playback worker and the control plane.
//!
//! Atomics rather than a lock, because of who touches this: the capture thread writes the
//! position once per block (CLAUDE.md — the hot path takes no locks), and the control plane
//! reads it on every state emit. A mutex here would put the snapshot behind a device thread
//! and the device thread behind whoever last asked for state.
//!
//! Only a device replaying a recording has one; see [`crate::SdrDevice::playback`].

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use sdrmm_wire::{PlaybackAction, PlaybackRequest, PlaybackStatus};

/// No seek pending. A real target can never collide with it: it would be an eight-exabyte
/// recording.
const NO_SEEK: u64 = u64::MAX;

#[derive(Debug)]
pub struct PlaybackShared {
    position: AtomicU64,
    /// Last position fixed by pause, seek or stop. A worker whose in-flight block loses a race
    /// with that request restores this value instead of publishing stale progress.
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

    /// What the client's transport draws.
    #[must_use]
    pub fn status(&self) -> PlaybackStatus {
        PlaybackStatus {
            position_samples: self.position.load(Ordering::Relaxed),
            total_samples: self.total.load(Ordering::Relaxed),
            paused: self.paused.load(Ordering::Relaxed),
        }
    }

    /// Apply a transport request. Stop is pause-and-rewind in one step: two requests would let
    /// a block slip out between them, so a stopped recording could sit one block off zero.
    pub fn control(&self, request: &PlaybackRequest) {
        match request.action {
            PlaybackAction::Play => self.paused.store(false, Ordering::Relaxed),
            PlaybackAction::Pause => {
                self.paused.store(true, Ordering::Relaxed);
                // Freeze at the last published boundary. The seek both invalidates a block
                // already in flight and makes resume continue from the position returned to
                // the client, rather than from beyond an unseen block.
                self.request_seek(self.position.load(Ordering::SeqCst));
            }
            PlaybackAction::Stop => {
                self.paused.store(true, Ordering::Relaxed);
                self.request_seek(0);
            }
            // A seek with no position is a seek to the start, which is what a client that
            // omits the field can only have meant.
            PlaybackAction::Seek => self.request_seek(request.position_samples.unwrap_or(0)),
        }
    }

    /// Snapshot the seek generation before the worker starts reading a block.
    #[must_use]
    pub fn position_generation(&self) -> u64 {
        self.position_generation.load(Ordering::SeqCst)
    }

    /// Publish where the worker has got to, unless pause, seek or stop landed while its block
    /// was in flight. The second generation check closes the race where the request arrives
    /// between the first check and the position store.
    pub fn set_position(&self, samples: u64, generation: u64) {
        if self.position_generation.load(Ordering::SeqCst) != generation {
            return;
        }
        self.position.store(samples, Ordering::SeqCst);
        if self.position_generation.load(Ordering::SeqCst) == generation {
            return;
        }

        // A request crossed the store above. Restore its position, retrying if another request
        // arrives while the restoration itself is in progress.
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
        // The position moves with the request rather than when the worker gets there: a paused
        // transport never reaches the worker, and a scrub that snapped back until playback
        // resumed would read as a dropped input.
        // Publish the requested value before advancing the generation. That gives an in-flight
        // worker a stable value to restore if it crosses this request.
        self.requested_position.store(requested, Ordering::SeqCst);
        self.seek.store(samples, Ordering::SeqCst);
        self.position_generation.fetch_add(1, Ordering::SeqCst);
        self.position.store(requested, Ordering::SeqCst);
    }

    /// The pending seek target, cleared as it is taken — a seek is an event, and replaying it
    /// every block would peg the transport wherever it last landed.
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

    /// Whether a seek is waiting, without taking it. A worker parked at the end of a recording
    /// polls this to know it has somewhere to go: consuming the seek to find out would throw
    /// away the very thing that should wake it.
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

    /// The worker may have read a block before stop arrived. Completing that old block must not
    /// move the stopped transport away from zero.
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

    /// Pause returns a status synchronously. A block that was already being read must not make
    /// the immediately following state snapshot disagree with that response.
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

    /// The readout has to follow the scrub immediately: while paused the worker never runs, so
    /// waiting for it to consume the seek would leave the bar sitting at the old position.
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
