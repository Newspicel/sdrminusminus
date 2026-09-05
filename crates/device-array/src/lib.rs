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
mod tests {
    use std::sync::{Arc, Mutex};

    use num_complex::Complex;
    use sdrmm_device::{DeviceRegistry, RxSink, lock};
    use sdrmm_device_virtual::VirtualDriver;
    use sdrmm_wire::{Coherence, Range};

    use super::*;

    fn definition(members: &[&str], coherence: Coherence) -> ArrayDefinition {
        ArrayDefinition {
            key: "bench".into(),
            label: "Bench".into(),
            members: members.iter().map(|id| (*id).into()).collect(),
            coherence,
            shared_tuning: true,
        }
    }

    fn pair() -> (StreamArray, ArrayIngress) {
        let mut registry = DeviceRegistry::new();
        registry.register(10, Box::new(VirtualDriver::new()));
        let (_, one) = registry.open("virtual:siggen").expect("first source");
        let (_, two) = registry.open("virtual:halfduplex").expect("second source");
        StreamArray::new(
            &definition(
                &["virtual:siggen", "virtual:halfduplex"],
                Coherence::TimeSync,
            ),
            &[
                (one.capabilities(), one.settings()),
                (two.capabilities(), two.settings()),
            ],
        )
        .expect("compose")
    }

    #[test]
    fn a_definition_needs_at_least_two_named_radios_and_a_shared_clock() {
        let mut good = definition(
            &["virtual:siggen", "virtual:halfduplex"],
            Coherence::TimeSync,
        );
        assert!(good.valid());
        good.members.pop();
        assert!(!good.valid(), "one radio is not an array");
        let duplicated = definition(&["virtual:siggen", "virtual:siggen"], Coherence::TimeSync);
        assert!(!duplicated.valid(), "the same radio cannot be two lanes");
        let unlocked = definition(&["virtual:siggen", "virtual:halfduplex"], Coherence::None);
        assert!(
            !unlocked.valid(),
            "a bank with no shared clock is not an array"
        );
        let named = ArrayDefinition {
            key: "a key with spaces".to_owned(),
            ..definition(
                &["virtual:siggen", "virtual:halfduplex"],
                Coherence::TimeSync,
            )
        };
        assert!(!named.valid());
    }

    #[test]
    fn composing_exposes_the_member_lanes_without_opening_any_radio() {
        let (device, _) = pair();
        assert_eq!(device.capabilities().rx_streams, 2);
        assert_eq!(device.capabilities().tx_streams, 0);
        assert_eq!(device.capabilities().coherence, Coherence::TimeSync);
    }

    #[test]
    fn stream_gaps_survive_composition_and_stopping_detaches_the_inputs() {
        let (mut device, ingress) = pair();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sinks = (0..2)
            .map(|lane| {
                let seen = seen.clone();
                RxSink::new(move |samples, index| lock(&seen).push((lane, index, samples[0].re)))
            })
            .collect();
        device.rx_start(sinks).expect("start");
        let mut inputs = ingress.take();
        inputs[0].push(&[Complex::new(1.0, 0.0); 4]);
        inputs[1].push(&[Complex::new(2.0, 0.0); 4]);
        inputs[0].push(&[Complex::new(3.0, 0.0); 4]);
        inputs[0].dropped(7);
        inputs[0].push(&[Complex::new(4.0, 0.0); 4]);
        assert_eq!(*lock(&seen), [(1, 0, 2.0), (0, 0, 3.0), (0, 11, 4.0)]);
        device.rx_stop();
        inputs[0].push(&[Complex::new(5.0, 0.0); 4]);
        assert_eq!(lock(&seen).len(), 3);
        device
            .rx_start(vec![RxSink::new(|_, _| {}), RxSink::new(|_, _| {})])
            .expect("restart");
        inputs[0].push(&[Complex::new(6.0, 0.0); 4]);
        assert_eq!(lock(&seen).len(), 3, "old inputs must stay detached");
    }

    #[test]
    fn a_member_failure_reaches_the_array() {
        let (mut device, ingress) = pair();
        let failures = Arc::new(Mutex::new(Vec::new()));
        let sinks = (0..2)
            .map(|lane| {
                let failures = failures.clone();
                RxSink::with_fatal_handler(
                    |_, _| {},
                    move |error| lock(&failures).push((lane, error.to_string())),
                )
            })
            .collect();
        device.rx_start(sinks).expect("start");
        ingress.take()[1].fail(DeviceError::Io("lost member".into()));
        assert_eq!(lock(&failures)[0].0, 1);
        assert!(lock(&failures)[0].1.contains("lost member"));
    }

    #[test]
    fn the_composite_reaches_only_what_every_member_reaches() {
        let narrow = [Range {
            min: 100e6,
            max: 200e6,
            step: None,
        }];
        let wide = [Range {
            min: 50e6,
            max: 150e6,
            step: None,
        }];
        let both = intersect(&[&narrow, &wide]);
        assert_eq!(both.len(), 1);
        assert!((both[0].min - 100e6).abs() < 1.0);
        assert!((both[0].max - 150e6).abs() < 1.0);

        let apart = [Range {
            min: 400e6,
            max: 500e6,
            step: None,
        }];
        assert!(intersect(&[&narrow, &apart]).is_empty());
    }

    #[test]
    fn composed_settings_preserve_each_members_gain_and_antenna() {
        let (device, _) = pair();
        let mut caps = device.capabilities().clone();
        caps.rx_streams = 1;
        caps.gains = vec![sdrmm_wire::GainStage {
            name: "RF".into(),
            range: Range {
                min: 0.0,
                max: 40.0,
                step: None,
            },
            values: Vec::new(),
        }];
        caps.antennas = vec!["A".into(), "B".into()];
        caps.per_stream = Default::default();
        let first = DeviceSettings {
            gains: vec![sdrmm_wire::GainValue {
                stage: "RF".into(),
                value_db: 10.0,
            }],
            antenna: Some("A".into()),
            ..device.settings().clone()
        };
        let second = DeviceSettings {
            gains: vec![sdrmm_wire::GainValue {
                stage: "RF".into(),
                value_db: 20.0,
            }],
            antenna: Some("B".into()),
            ..first.clone()
        };
        let (array, _) = StreamArray::new(
            &definition(&["virtual:one", "virtual:two"], Coherence::TimeSync),
            &[(&caps, &first), (&caps, &second)],
        )
        .expect("compose");
        for (lane, original) in [first, second].into_iter().enumerate() {
            let settings = array
                .settings()
                .for_stream(lane as u32, &array.capabilities().per_stream);
            assert_eq!(settings.gains, original.gains);
            assert_eq!(settings.antenna, original.antenna);
        }
    }
}
