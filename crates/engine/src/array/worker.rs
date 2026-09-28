use std::{
    sync::{Arc, atomic::AtomicBool},
    thread::JoinHandle,
};

use rtrb::{Consumer, Producer};

use super::{CaptureBuffers, CaptureJob, CorrectionSet, Solution};
use crate::EngineError;

#[expect(dead_code)]
pub(crate) struct WorkerIo {
    pub(crate) lanes: usize,
    pub(crate) sample_rate: f64,
    pub(crate) jobs: Consumer<CaptureJob>,
    pub(crate) solutions: Producer<Box<Solution>>,
    pub(crate) buffers: Producer<Box<CaptureBuffers>>,
    pub(crate) sets: Consumer<Box<CorrectionSet>>,
    pub(crate) stop: Arc<AtomicBool>,
}

pub(crate) fn spawn_worker(_name: String, _io: WorkerIo) -> Result<JoinHandle<()>, EngineError> {
    Err(EngineError::Processor(
        "array sync is not built yet".to_owned(),
    ))
}
