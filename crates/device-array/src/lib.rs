use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use sdrmm_device::{DeviceError, RxSink, SdrDevice, lock};
use sdrmm_wire::{ArrayDefinition, Capabilities, DeviceSettings, StreamSettings};

mod caps;
pub use caps::{composite, composite_profile, intersect};

#[derive(Clone, Default)]
pub struct ArrayCatalog(Arc<Mutex<Vec<ArrayDefinition>>>);

impl ArrayCatalog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn all(&self) -> Vec<ArrayDefinition> {
        lock(&self.0).clone()
    }

    pub fn replace(&self, definitions: Vec<ArrayDefinition>) {
        *lock(&self.0) = definitions;
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<ArrayDefinition> {
        lock(&self.0)
            .iter()
            .find(|definition| definition.key == key)
            .cloned()
    }
}

#[derive(Clone, Default)]
pub struct ArrayIngress {
    sinks: Arc<Mutex<Vec<RxSink>>>,
    enabled: Arc<AtomicBool>,
}

impl ArrayIngress {
    pub fn pause(&self) {
        self.enabled.store(false, Ordering::Release);
    }
    pub fn resume(&self) {
        self.enabled.store(true, Ordering::Release);
    }

    pub fn take(&self) -> Vec<RxSink> {
        std::mem::take(&mut *lock(&self.sinks))
    }
}

pub struct StreamArray {
    capabilities: Capabilities,
    settings: DeviceSettings,
    ingress: ArrayIngress,
    active: Arc<AtomicBool>,
}

impl StreamArray {
    pub fn new(
        definition: &ArrayDefinition,
        members: &[(&Capabilities, &DeviceSettings)],
    ) -> Result<(Self, ArrayIngress), DeviceError> {
        if !definition.valid() || definition.members.len() != members.len() {
            return Err(DeviceError::Unsupported(
                "an array needs all its distinct member streams".into(),
            ));
        }
        let first = members[0].1;
        if first.sample_rate.is_none()
            || members
                .iter()
                .any(|(_, settings)| settings.sample_rate != first.sample_rate)
        {
            return Err(DeviceError::Unsupported(
                "array members must use the same sample rate".into(),
            ));
        }
        let capabilities = composite(
            &members.iter().map(|(caps, _)| *caps).collect::<Vec<_>>(),
            definition,
        );
        if capabilities.rx_streams > sdrmm_wire::MAX_STREAMS {
            return Err(DeviceError::Unsupported(
                "array has too many receive streams".into(),
            ));
        }
        let mut settings = DeviceSettings {
            sample_rate: first.sample_rate,
            center_hz: first.center_hz,
            gains: first.gains.clone(),
            antenna: first.antenna.clone(),
            bandwidth: first.bandwidth,
            ppm: first.ppm,
            ..Default::default()
        };
        let mut lane = 0;
        for (caps, member) in members {
            for stream in 0..caps.rx_streams {
                let source = member.for_stream(stream, &caps.per_stream);
                if definition.shared_tuning && source.center_hz != first.center_hz {
                    return Err(DeviceError::Unsupported(
                        "array members must be tuned to the same frequency".into(),
                    ));
                }
                if capabilities.per_stream != Default::default() {
                    settings.streams.push(StreamSettings {
                        stream: lane,
                        center_hz: (!definition.shared_tuning)
                            .then_some(source.center_hz)
                            .flatten(),
                        gains: if capabilities.per_stream.gain {
                            source.gains
                        } else {
                            Vec::new()
                        },
                        antenna: if capabilities.per_stream.antenna {
                            source.antenna
                        } else {
                            None
                        },
                    });
                }
                lane += 1;
            }
        }
        let ingress = ArrayIngress::default();
        Ok((
            Self {
                capabilities,
                settings,
                ingress: ingress.clone(),
                active: Arc::new(AtomicBool::new(false)),
            },
            ingress,
        ))
    }
}

impl SdrDevice for StreamArray {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }
    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        sdrmm_device::check_stream_settings(settings, &self.capabilities)?;
        self.settings.merge_from(settings);
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        if self.active.load(Ordering::Acquire) {
            return Err(DeviceError::AlreadyStreaming);
        }
        let count = sinks.len();
        if count != self.capabilities.rx_streams as usize {
            return Err(DeviceError::Unsupported(format!(
                "array expects {} streams, got {count}",
                self.capabilities.rx_streams
            )));
        }
        self.active = Arc::new(AtomicBool::new(true));
        self.ingress.resume();
        let arrived = Arc::new(AtomicUsize::new(0));
        let inputs = sinks
            .into_iter()
            .map(|mut sink| {
                let active = self.active.clone();
                let enabled = self.ingress.enabled.clone();
                let arrived = arrived.clone();
                let mut first = true;
                let mut next = None;
                let failure = sink.share_failure();
                RxSink::with_fatal_handler(
                    move |samples, index| {
                        if !active.load(Ordering::Acquire) || !enabled.load(Ordering::Acquire) {
                            return;
                        }
                        if first {
                            first = false;
                            arrived.fetch_add(1, Ordering::AcqRel);
                        }
                        if arrived.load(Ordering::Acquire) != count {
                            return;
                        }
                        if let Some(expected) = next {
                            sink.dropped(index.saturating_sub(expected));
                        }
                        next = Some(index + samples.len() as u64);
                        sink.push(samples);
                    },
                    move |error| failure.fail(error),
                )
            })
            .collect();
        *lock(&self.ingress.sinks) = inputs;
        Ok(())
    }

    fn rx_stop(&mut self) {
        self.active.store(false, Ordering::Release);
        lock(&self.ingress.sinks).clear();
    }
}

impl Drop for StreamArray {
    fn drop(&mut self) {
        self.rx_stop();
    }
}

#[cfg(test)]
mod tests;
