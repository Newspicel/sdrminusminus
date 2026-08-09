//! Scripted endpoints, so a transfer queue can be exercised without a radio (PLAN §14: no
//! hardware in CI, ever).
//!
//! Compiled for this crate's own tests, and behind the `test-util` feature for the backends
//! above it — the burst semantics a radio layers on top of [`TxQueue`](crate::TxQueue) need the
//! same mock this crate's tests use, and a second copy of it would be a second thing to keep
//! honest.

use std::{collections::VecDeque, sync::Mutex, time::Duration};

use nusb::transfer::TransferError;

use crate::{
    error::Result,
    tx::{BulkOut, OutCompletion},
};

/// What a [`ScriptedBulkOut`] has seen and what it will do next.
#[derive(Debug, Default)]
pub struct ScriptedState {
    /// Buffers still in flight, oldest first.
    queued: VecDeque<Vec<u8>>,
    /// Length of every buffer submitted so far, in order.
    pub submitted: Vec<usize>,
    /// Statuses the next completions report; an empty script means success.
    pub statuses: VecDeque<std::result::Result<(), TransferError>>,
    /// How often the endpoint was asked to cancel everything.
    pub cancel_calls: usize,
    /// How often the endpoint was un-stalled.
    pub clear_halt_calls: usize,
    /// While set, nothing completes — so a caller's deadline can be exercised.
    pub starve: bool,
}

/// A [`BulkOut`] that completes what it is given, in order, with scripted statuses.
#[derive(Debug, Default)]
pub struct ScriptedBulkOut {
    state: Mutex<ScriptedState>,
}

impl ScriptedBulkOut {
    /// Inspect or script the endpoint.
    pub fn state(&self) -> std::sync::MutexGuard<'_, ScriptedState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The next `count` completions fail with `error`.
    pub fn fail_next(&self, count: usize, error: TransferError) {
        let mut state = self.state();
        for _ in 0..count {
            state.statuses.push_back(Err(error));
        }
    }

    /// Buffer lengths submitted so far, in order.
    #[must_use]
    pub fn submitted(&self) -> Vec<usize> {
        self.state().submitted.clone()
    }
}

impl BulkOut for ScriptedBulkOut {
    fn clear_halt(&mut self) -> Result<()> {
        self.state().clear_halt_calls += 1;
        Ok(())
    }

    fn submit(&mut self, bytes: Vec<u8>) {
        let mut state = self.state();
        state.submitted.push(bytes.len());
        state.queued.push_back(bytes);
    }

    fn pending(&self) -> usize {
        self.state().queued.len()
    }

    fn wait_next_complete(&mut self, _timeout: Duration) -> Option<OutCompletion> {
        let mut state = self.state();
        if state.starve {
            return None;
        }
        let bytes = state.queued.pop_front()?;
        let status = state.statuses.pop_front().unwrap_or(Ok(()));
        Some(OutCompletion { bytes, status })
    }

    fn cancel_all(&mut self) {
        let mut state = self.state();
        state.cancel_calls += 1;
        // A real endpoint completes every cancelled transfer; the abort loop waits for them.
        let cancelled = state.queued.len();
        state.statuses.clear();
        for _ in 0..cancelled {
            state.statuses.push_back(Err(TransferError::Cancelled));
        }
    }
}
