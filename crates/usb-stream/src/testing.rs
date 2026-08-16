use std::{collections::VecDeque, sync::Mutex, time::Duration};

use nusb::transfer::TransferError;

use crate::{
    error::Result,
    tx::{BulkOut, OutCompletion},
};

#[derive(Debug, Default)]
pub struct ScriptedState {
    queued: VecDeque<Vec<u8>>,
    pub submitted: Vec<usize>,
    pub statuses: VecDeque<std::result::Result<(), TransferError>>,
    pub cancel_calls: usize,
    pub clear_halt_calls: usize,
    pub starve: bool,
}

#[derive(Debug, Default)]
pub struct ScriptedBulkOut {
    state: Mutex<ScriptedState>,
}

impl ScriptedBulkOut {
    pub fn state(&self) -> std::sync::MutexGuard<'_, ScriptedState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn fail_next(&self, count: usize, error: TransferError) {
        let mut state = self.state();
        for _ in 0..count {
            state.statuses.push_back(Err(error));
        }
    }

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
        let cancelled = state.queued.len();
        state.statuses.clear();
        for _ in 0..cancelled {
            state.statuses.push_back(Err(TransferError::Cancelled));
        }
    }
}
