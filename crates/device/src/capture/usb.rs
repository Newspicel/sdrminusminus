//! The shared USB transport, seen as a [`CaptureStream`].
//!
//! Behind the `usb` feature so a Soapy-only or virtual-only build never compiles a USB stack it
//! cannot use (PLAN §3: every backend stays optional). The two native backends turn it on; that
//! is the whole of what either has to write to reach the supervisor above.

use std::{sync::mpsc::RecvTimeoutError, time::Duration};

use sdrmm_usb_stream::{Block, RxStream, Stopper};

use super::{CaptureStream, Next, StopHandle, StreamFailure};

impl StopHandle for Stopper {
    fn stop(&self) {
        Self::stop(self);
    }
}

impl CaptureStream for RxStream {
    type Block = Block;
    type Stop = Stopper;

    fn stop_handle(&self) -> Stopper {
        self.stopper()
    }

    fn next_block(&self, timeout: Duration) -> Next<Block> {
        match self.recv_timeout(timeout) {
            Ok(block) => Next::Block(block),
            Err(RecvTimeoutError::Timeout) => Next::Idle,
            Err(RecvTimeoutError::Disconnected) => Next::Ended,
        }
    }

    fn dropped(&self) -> u64 {
        self.stats().dropped
    }

    fn failure(&self) -> StreamFailure {
        // A pump that ended without recording an error was stopped by its consumer going away,
        // which the supervisor still has to treat as a stream that ended on its own.
        self.error().map_or_else(
            || StreamFailure {
                reason: "usb stream ended".to_string(),
                fatal: false,
            },
            |error| StreamFailure {
                reason: error.to_string(),
                fatal: error.is_disconnected(),
            },
        )
    }
}
