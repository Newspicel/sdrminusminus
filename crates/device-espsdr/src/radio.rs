use std::{
    ops::Deref,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use sdrmm_device::{
    Block, BlockGap, BlockPool, CaptureRadio, CaptureStream, DeviceError, Next, Sample,
    SampleConverter, StreamFailure, lock,
};

use crate::{
    caps::{Profile, Remote},
    link::Stop,
    proto::{FULL_SCALE, unpack},
    session::{BurstError, Session},
};

const BYTES_PER_SAMPLE: usize = 4;

pub(crate) struct Inner {
    pub(crate) profile: Profile,
    pub(crate) desired: Mutex<Remote>,
    session: Mutex<Session>,
    stop: Stop,
    pool: BlockPool,
}

pub(crate) struct EspRadio(pub(crate) Arc<Inner>);

impl EspRadio {
    pub(crate) fn new(profile: Profile, session: Session) -> Self {
        let stop = session.stop().clone();
        Self(Arc::new(Inner {
            desired: Mutex::new(profile.defaults()),
            profile,
            session: Mutex::new(session),
            stop,
            pool: BlockPool::default(),
        }))
    }
}

impl CaptureRadio for EspRadio {
    type Stream = EspStream;

    fn arm(&self) -> Result<EspStream, DeviceError> {
        let inner = &self.0;
        inner.stop.clear();
        let mut session = lock(&inner.session);
        session.resync()?;
        let desired = *lock(&inner.desired);
        session.sync(&inner.profile, &desired)?;
        drop(session);
        Ok(EspStream {
            inner: inner.clone(),
            previous: Mutex::new(None),
            failure: Mutex::new(None),
            damaged: Mutex::new(0),
        })
    }
}

#[derive(Clone, Copy)]
struct Previous {
    at: Instant,
    samples: u32,
}

pub(crate) struct EspStream {
    inner: Arc<Inner>,
    previous: Mutex<Option<Previous>>,
    failure: Mutex<Option<StreamFailure>>,
    damaged: Mutex<u64>,
}

pub(crate) struct Burst {
    block: Block,
    gap: u64,
}

impl Deref for Burst {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.block
    }
}

impl EspStream {
    fn fail(&self, error: &DeviceError) -> Next<Burst> {
        *lock(&self.failure) = Some(StreamFailure {
            reason: error.to_string(),
            gone: matches!(error, DeviceError::Disconnected(_)),
        });
        Next::Ended
    }

    fn gap(&self, at: Instant, samples: u32, rate: u32) -> u64 {
        let previous = lock(&self.previous).replace(Previous { at, samples });
        previous.map_or(0, |previous| {
            let span = at.duration_since(previous.at).as_secs_f64() * f64::from(rate);
            (span - f64::from(previous.samples)).max(0.0).round() as u64
        })
    }

    fn damaged(&self, session: &mut Session, reason: &str) -> Next<Burst> {
        let total = {
            let mut damaged = lock(&self.damaged);
            *damaged += 1;
            *damaged
        };
        tracing::warn!(reason, total, "ESP-SDR burst lost");
        match session.resync() {
            Ok(()) => Next::Idle,
            Err(error) => self.fail(&error),
        }
    }

    fn deliver(&self, session: &Session, desired: &Remote, at: Instant) -> Next<Burst> {
        let mut block = self
            .inner
            .pool
            .take(desired.burst as usize * BYTES_PER_SAMPLE);
        let count = unpack(desired.bits, session.payload(), block.bytes_mut());
        block.truncate(count.min(desired.burst as usize) * BYTES_PER_SAMPLE);
        let gap = self.gap(at, desired.burst, desired.rate);
        Next::Block(Burst { block, gap })
    }
}

impl CaptureStream for EspStream {
    type Block = Burst;
    type Stop = Stop;

    fn stop_handle(&self) -> Stop {
        self.inner.stop.clone()
    }

    fn next_block(&self, _timeout: Duration) -> Next<Burst> {
        let mut session = lock(&self.inner.session);
        let desired = *lock(&self.inner.desired);
        if let Err(error) = session.sync(&self.inner.profile, &desired) {
            return self.fail(&error);
        }
        let at = Instant::now();
        match session.burst(&desired) {
            Ok(_) => self.deliver(&session, &desired, at),
            Err(BurstError::Damaged(reason)) => self.damaged(&mut session, &reason),
            Err(BurstError::Stopped) => self.fail(&DeviceError::Io("stopped".to_string())),
            Err(BurstError::Fatal(error)) => self.fail(&error),
        }
    }

    fn dropped(&self) -> u64 {
        0
    }

    fn block_gap(&self, block: &Burst, _bytes_per_sample: u64) -> Option<BlockGap> {
        Some(BlockGap {
            exact: 0,
            estimated: block.gap,
        })
    }

    fn failure(&self) -> StreamFailure {
        lock(&self.failure).clone().unwrap_or(StreamFailure {
            reason: "ESP-SDR stream ended".to_string(),
            gone: false,
        })
    }
}

pub(crate) struct Converter {
    out: Vec<Sample>,
}

impl Converter {
    pub(crate) fn new(max_samples: u32) -> Self {
        Self {
            out: Vec::with_capacity(max_samples as usize),
        }
    }
}

impl SampleConverter for Converter {
    fn convert(&mut self, bytes: &[u8]) -> &[Sample] {
        self.out.clear();
        self.out
            .extend(bytes.as_chunks::<BYTES_PER_SAMPLE>().0.iter().map(|word| {
                let i = i16::from_le_bytes([word[0], word[1]]);
                let q = i16::from_le_bytes([word[2], word[3]]);
                Sample::new(f32::from(i) / FULL_SCALE, f32::from(q) / FULL_SCALE)
            }));
        &self.out
    }

    fn reset(&mut self) {}

    fn bytes_per_sample(&self) -> u64 {
        BYTES_PER_SAMPLE as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_scale_to_unit_full_scale() {
        let mut converter = Converter::new(2);
        let bytes = [0x00, 0x02, 0x00, 0xFE, 0xFF, 0xFF, 0x01, 0x00];
        let samples = converter.convert(&bytes).to_vec();
        assert_eq!(
            samples,
            [
                Sample::new(1.0, -1.0),
                Sample::new(-1.0 / 512.0, 1.0 / 512.0)
            ]
        );
    }

    #[test]
    fn the_converter_never_reallocates_for_a_full_burst() {
        let mut converter = Converter::new(16380);
        let block = vec![0u8; 16380 * BYTES_PER_SAMPLE];
        let first = converter.convert(&block).as_ptr();
        assert_eq!(converter.convert(&block).as_ptr(), first);
    }
}
