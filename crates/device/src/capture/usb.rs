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
        self.error().map_or_else(
            || StreamFailure {
                reason: "usb stream ended".to_string(),
                gone: false,
            },
            |error| StreamFailure {
                reason: error.to_string(),
                gone: error.is_disconnected(),
            },
        )
    }
}
