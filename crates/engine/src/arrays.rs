use std::collections::BTreeMap;

use sdrmm_device::{DeviceError, SdrDevice};
use sdrmm_device_array::{ArrayIngress, StreamArray};
use sdrmm_wire::{
    ArrayDefinition, ChannelInfo, DeviceInfo, DeviceSetStatus, DeviceSettings, StreamSettings,
    Tuning,
};

use crate::{DeviceSetState, Engine, EngineError, runtime::DspCommand};

pub(crate) fn array_of(sets: &BTreeMap<u32, DeviceSetState>, member: u32) -> Option<(u32, u32)> {
    sets.iter().find_map(|(id, state)| {
        let binding = state.array.as_ref()?;
        let at = binding.members.iter().position(|(ds, _)| *ds == member)?;
        Some((
            *id,
            binding.members[..at].iter().map(|(_, lanes)| lanes).sum(),
        ))
    })
}

pub(crate) fn array_channels(
    sets: &BTreeMap<u32, DeviceSetState>,
    array: u32,
    moved: Option<(u32, u32, &ChannelInfo)>,
) -> Vec<ChannelInfo> {
    let Some(state) = sets.get(&array) else {
        return Vec::new();
    };
    let placed = |ds: u32, offset: u32, channel: &ChannelInfo| {
        let chosen = match moved {
            Some((owner, ch, replaced)) if owner == ds && ch == channel.id => replaced,
            _ => channel,
        };
        ChannelInfo {
            stream: chosen.stream + offset,
            ..chosen.clone()
        }
    };
    let mut channels: Vec<ChannelInfo> = state
        .channels
        .iter()
        .map(|channel| placed(array, 0, channel))
        .collect();
    let Some(binding) = state.array.as_ref() else {
        return channels;
    };
    let mut offset = 0;
    for (member, lanes) in &binding.members {
        if let Some(member_state) = sets.get(member) {
            channels.extend(
                member_state
                    .channels
                    .iter()
                    .map(|channel| placed(*member, offset, channel)),
            );
        }
        offset += lanes;
    }
    channels
}

#[derive(Clone)]
pub(crate) struct ArrayBinding {
    definition: ArrayDefinition,
    ingress: ArrayIngress,
    members: Vec<(u32, u32)>,
}

impl ArrayBinding {
    pub(crate) fn member_sets(&self) -> impl Iterator<Item = u32> + '_ {
        self.members.iter().map(|(member, _)| *member)
    }
}

impl Engine {
    pub fn create_array_set(&self, key: &str) -> Result<u32, EngineError> {
        let _edit = sdrmm_device::lock(&self.array_edits);
        let definition = self
            .arrays
            .get(key)
            .filter(ArrayDefinition::valid)
            .ok_or_else(|| DeviceError::NotFound(format!("array:{key}")))?;
        self.refuse_reopen(&definition.id())?;
        let (members, device, ingress, tuning) = {
            let inner = self.lock();
            let mut states = Vec::new();
            let mut members = Vec::new();
            for member in &definition.members {
                let (id, state) = inner
                    .device_sets
                    .iter()
                    .find(|(_, state)| state.info.id() == *member && state.array.is_none())
                    .ok_or_else(|| {
                        DeviceError::Unsupported(format!(
                            "array member {member} must be opened by its Device node first"
                        ))
                    })?;
                if state.status != DeviceSetStatus::Running {
                    return Err(DeviceError::Unsupported(format!(
                        "array member {member} is not continuously receiving"
                    ))
                    .into());
                }
                if inner.device_sets.values().any(|other| {
                    other
                        .array
                        .as_ref()
                        .is_some_and(|array| array.members.iter().any(|(ds, _)| ds == id))
                }) {
                    return Err(
                        DeviceError::InUse(format!("{member} already feeds an array")).into(),
                    );
                }
                members.push((*id, state.capabilities.rx_streams));
                states.push((&state.capabilities, &state.settings));
            }
            let tuning = states.first().and_then(|(_, settings)| settings.tuning);
            let (device, ingress) = StreamArray::new(&definition, &states)?;
            (members, device, ingress, tuning)
        };
        let info = DeviceInfo {
            driver: "array".into(),
            key: definition.key.clone(),
            label: definition.label.clone(),
            serial: None,
            profile: None,
        };
        let binding = ArrayBinding {
            definition,
            members,
            ingress: ingress.clone(),
        };
        let id = self.create_opened_set(info, Box::new(device), Some(binding.clone()))?;
        if let Some(state) = self.lock().device_sets.get_mut(&id) {
            state.settings.tuning = tuning;
        }
        let result = self.connect_array_inputs(id, &binding);
        if let Err(error) = result {
            self.remove_set(id)?;
            return Err(error);
        }
        Ok(id)
    }

    pub(crate) fn connect_array_inputs(
        &self,
        id: u32,
        binding: &ArrayBinding,
    ) -> Result<(), EngineError> {
        let inner = self.lock();
        let mut sinks = binding.ingress.take().into_iter();
        for (member, lanes) in &binding.members {
            let state = inner
                .device_sets
                .get(member)
                .filter(|state| state.status == DeviceSetStatus::Running)
                .ok_or(EngineError::DeviceSetNotFound(*member))?;
            for stream in 0..*lanes {
                let sink = sinks
                    .next()
                    .ok_or_else(|| DeviceError::Io("array ingress is incomplete".into()))?;
                state.send_dsp(stream, DspCommand::ConnectArray { id, sink });
            }
        }
        Ok(())
    }

    pub(crate) fn reopen_array(
        &self,
        binding: &ArrayBinding,
    ) -> Result<(Box<dyn SdrDevice>, ArrayBinding), DeviceError> {
        let inner = self.lock();
        let mut states = Vec::new();
        for (member, lanes) in &binding.members {
            let state = inner
                .device_sets
                .get(member)
                .filter(|state| {
                    state.status == DeviceSetStatus::Running
                        && state.capabilities.rx_streams == *lanes
                })
                .ok_or_else(|| {
                    DeviceError::NotFound(format!("array member {member} is not receiving"))
                })?;
            states.push((&state.capabilities, &state.settings));
        }
        let (device, ingress) = StreamArray::new(&binding.definition, &states)?;
        Ok((
            Box::new(device),
            ArrayBinding {
                ingress,
                ..binding.clone()
            },
        ))
    }

    pub(crate) fn recover_arrays(&self) {
        let arrays: Vec<u32> = {
            let inner = self.lock();
            inner
                .device_sets
                .iter()
                .filter(|(_, state)| {
                    state.status == DeviceSetStatus::Error
                        && state.array.as_ref().is_some_and(|array| {
                            array.members.iter().all(|(member, _)| {
                                inner
                                    .device_sets
                                    .get(member)
                                    .is_some_and(|state| state.status == DeviceSetStatus::Running)
                            })
                        })
                })
                .map(|(id, _)| *id)
                .collect()
        };
        for id in arrays {
            self.reconnect(id);
        }
    }

    pub(crate) fn arrays_using(&self, ds: u32) -> Vec<u32> {
        self.lock()
            .device_sets
            .iter()
            .filter_map(|(id, state)| {
                state
                    .array
                    .as_ref()
                    .filter(|array| array.members.iter().any(|(member, _)| *member == ds))
                    .map(|_| *id)
            })
            .collect()
    }

    pub(crate) fn detach_array(&self, id: u32, binding: Option<&ArrayBinding>) {
        let Some(binding) = binding else { return };
        let inner = self.lock();
        for (member, lanes) in &binding.members {
            if let Some(state) = inner.device_sets.get(member) {
                for stream in 0..*lanes {
                    state.send_dsp(stream, DspCommand::DisconnectArray { id });
                }
            }
        }
    }

    pub(crate) fn split_member_patch(
        &self,
        member: u32,
        delta: &mut DeviceSettings,
    ) -> Option<(u32, DeviceSettings)> {
        let inner = self.lock();
        let member_scope = inner.device_sets.get(&member)?.capabilities.per_stream;
        let (array, offset, lanes, shared) = inner.device_sets.iter().find_map(|(id, state)| {
            let binding = state.array.as_ref()?;
            let at = binding.members.iter().position(|(ds, _)| *ds == member)?;
            let offset: u32 = binding.members[..at].iter().map(|(_, lanes)| lanes).sum();
            Some((
                *id,
                offset,
                binding.members[at].1,
                !state.capabilities.per_stream.tuning,
            ))
        })?;
        drop(inner);
        let mut forward = DeviceSettings {
            sample_rate: delta.sample_rate.take(),
            ..DeviceSettings::default()
        };
        let mut aim = |lane: Option<u32>, center_hz: Option<f64>, tuning: Option<Tuning>| {
            if center_hz.is_none() && tuning.is_none() {
                return;
            }
            match lane {
                Some(lane) if !shared => forward.streams.push(StreamSettings {
                    stream: offset + lane,
                    center_hz,
                    tuning,
                    ..StreamSettings::default()
                }),
                None if !shared => forward
                    .streams
                    .extend((0..lanes).map(|lane| StreamSettings {
                        stream: offset + lane,
                        center_hz,
                        tuning,
                        ..StreamSettings::default()
                    })),
                _ => {
                    forward.center_hz = center_hz.or(forward.center_hz);
                    forward.tuning = tuning.or(forward.tuning);
                }
            }
        };
        aim(None, delta.center_hz.take(), delta.tuning.take());
        if member_scope.tuning {
            for stream in &mut delta.streams {
                aim(
                    Some(stream.stream),
                    stream.center_hz.take(),
                    stream.tuning.take(),
                );
            }
        }
        delta.streams.retain(|stream| {
            *stream
                != StreamSettings {
                    stream: stream.stream,
                    ..StreamSettings::default()
                }
        });
        (forward != DeviceSettings::default()).then_some((array, forward))
    }

    pub(crate) fn array_of(&self, member: u32) -> Option<(u32, u32)> {
        array_of(&self.lock().device_sets, member)
    }

    pub(crate) fn patch_array(&self, ds: u32, delta: DeviceSettings) -> Result<(), EngineError> {
        if delta.offset_hz.is_some() {
            return Err(DeviceError::Unsupported(
                "set the converter offset on each radio of the array".into(),
            )
            .into());
        }
        let (binding, before) = {
            let inner = self.lock();
            let state = inner
                .device_sets
                .get(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let binding = state
                .array
                .clone()
                .ok_or_else(|| DeviceError::Unsupported("not an array".into()))?;
            let before = binding
                .members
                .iter()
                .map(|(member, _)| {
                    inner
                        .device_sets
                        .get(member)
                        .map(|state| state.settings.clone())
                        .ok_or(EngineError::DeviceSetNotFound(*member))
                })
                .collect::<Result<Vec<_>, _>>()?;
            (binding, before)
        };
        let mut offset = 0;
        let mut changes = Vec::with_capacity(binding.members.len());
        for (member, lanes) in &binding.members {
            let caps = self
                .capabilities(*member)
                .ok_or(EngineError::DeviceSetNotFound(*member))?;
            let mut wanted = delta.clone();
            wanted.streams.clear();
            for stream in delta
                .streams
                .iter()
                .filter(|stream| (offset..offset + lanes).contains(&stream.stream))
            {
                let mut local = StreamSettings {
                    stream: stream.stream - offset,
                    ..stream.clone()
                };
                if local.stream == 0 {
                    if !caps.per_stream.tuning {
                        wanted.center_hz = local.center_hz.or(wanted.center_hz);
                        local.center_hz = None;
                    }
                    if !caps.per_stream.gain && !local.gains.is_empty() {
                        wanted.gains = std::mem::take(&mut local.gains);
                    }
                    if !caps.per_stream.antenna {
                        wanted.antenna = local.antenna.take().or(wanted.antenna);
                    }
                    if !caps.per_stream.agc {
                        wanted.agc = local.agc.take().or(wanted.agc);
                    }
                }
                if local.center_hz.is_some()
                    || !local.gains.is_empty()
                    || local.antenna.is_some()
                    || local.agc.is_some()
                {
                    wanted.streams.push(local);
                }
            }
            self.validate_configuration(*member, &wanted, &[])?;
            changes.push(wanted);
            offset += lanes;
        }
        self.validate_configuration(ds, &delta, &[])?;
        let _rate_guards = self.guard_device_patches(
            std::iter::once((ds, &delta)).chain(
                binding
                    .members
                    .iter()
                    .zip(&changes)
                    .map(|((member, _), change)| (*member, change)),
            ),
        )?;
        binding.ingress.pause();
        let result = (|| {
            for ((member, _), wanted) in binding.members.iter().zip(&changes) {
                self.patch_device_from(*member, wanted.clone())?;
            }
            let snapshot = self.snapshot();
            let states = binding
                .members
                .iter()
                .map(|(member, _)| {
                    snapshot
                        .device_sets
                        .iter()
                        .find(|set| set.id == *member)
                        .map(|set| (&set.capabilities, &set.settings))
                        .ok_or(EngineError::DeviceSetNotFound(*member))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let (readback, _) = StreamArray::new(&binding.definition, &states)?;
            let mut actual = delta.clone();
            actual.sample_rate = readback.settings().sample_rate;
            actual.center_hz = readback.settings().center_hz;
            actual.streams = readback.settings().streams.clone();
            actual.gains = readback.settings().gains.clone();
            actual.antenna = readback.settings().antenna.clone();
            actual.bandwidth = readback.settings().bandwidth;
            actual.ppm = readback.settings().ppm;
            self.patch_device_from(ds, actual)
        })();
        if let Err(error) = result {
            let mut restored = true;
            for ((changed, _), original) in binding.members.iter().zip(&before) {
                if let Err(rollback) = self.patch_device_from(*changed, original.clone()) {
                    restored = false;
                    self.mark_device_fault(ds, DeviceError::Io(format!("array tuning failed: {error}; restoring member {changed} failed: {rollback}")));
                }
            }
            if restored {
                binding.ingress.resume();
            }
            return Err(error);
        }
        binding.ingress.resume();
        Ok(())
    }

    pub fn reconcile_arrays(&self) -> Result<(), EngineError> {
        let definitions = self.arrays.all();
        let stale: Vec<u32> = self
            .lock()
            .device_sets
            .iter()
            .filter_map(|(id, state)| {
                state
                    .array
                    .as_ref()
                    .filter(|array| !definitions.contains(&array.definition))
                    .map(|_| *id)
            })
            .collect();
        for id in stale {
            self.remove_device_set(id)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn member_recovery_reconnects_array_streams_without_reopening_other_radios() {
        let mut registry = sdrmm_device::DeviceRegistry::new();
        registry.register(10, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
        let engine = Engine::with_registry(registry, None);
        engine.arrays().replace(vec![ArrayDefinition {
            key: "recovery".into(),
            label: "Recovery".into(),
            members: vec!["virtual:siggen".into(), "virtual:halfduplex".into()],
            coherence: sdrmm_wire::Coherence::TimeSync,
            shared_tuning: true,
        }]);
        let source = engine.create_device_set("virtual:siggen").expect("source");
        let other = engine
            .create_device_set("virtual:halfduplex")
            .expect("other source");
        let array = engine.create_array_set("recovery").expect("array");
        let mut known = None;
        let mut missing = std::collections::HashSet::new();
        for _ in 0..3 {
            engine.hotplug_tick_for_test(&mut known, &mut missing);
        }
        assert!(
            engine
                .snapshot()
                .device_sets
                .iter()
                .all(|set| set.status == DeviceSetStatus::Running)
        );
        engine.mark_device_fault(source, DeviceError::Disconnected("test unplug".into()));
        assert_eq!(
            engine
                .snapshot()
                .device_sets
                .iter()
                .find(|set| set.id == array)
                .expect("array remains bound")
                .status,
            DeviceSetStatus::Error
        );
        engine.reconnect(source);
        engine.recover_arrays();
        let live = engine.snapshot();
        assert_eq!(live.device_sets.len(), 3);
        for id in [source, other, array] {
            assert_eq!(
                live.device_sets
                    .iter()
                    .find(|set| set.id == id)
                    .expect("identity preserved")
                    .status,
                DeviceSetStatus::Running
            );
        }
        let mut spectrum = engine
            .subscribe_spectrum(array, 0)
            .expect("array subscription");
        tokio::time::timeout(std::time::Duration::from_secs(5), spectrum.recv())
            .await
            .expect("array receiving again")
            .expect("spectrum");
        engine.shutdown();
    }
}
