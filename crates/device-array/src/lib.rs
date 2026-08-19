use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use sdrmm_device::{
    DeviceDriver, DeviceError, DeviceRegistry, FatalHandle, RxSink, SdrDevice, lock,
};
use sdrmm_wire::{
    ARRAY_DRIVER_ID, ArrayDefinition, Capabilities, DcArtifact, DeviceInfo, DeviceProfile,
    DeviceSettings, Duplex, Range, StreamSettings,
};

mod caps;

pub use caps::intersect;

/// The array definitions the operator has written down, shared between whoever edits them and the
/// driver that opens them.
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

/// Presents a bank of separate radios as one multi-lane radio.
///
/// Framing and counting only: no sample ever changes value here. Everything above the device
/// layer — calibration, direction finding, passive radar — sees exactly what it sees for a radio
/// that came with several lanes of its own.
pub struct ArrayDriver {
    catalog: ArrayCatalog,
    members: Arc<DeviceRegistry>,
}

impl ArrayDriver {
    #[must_use]
    pub fn new(catalog: ArrayCatalog, members: Arc<DeviceRegistry>) -> Self {
        Self { catalog, members }
    }

    fn info(&self, definition: &ArrayDefinition, probed: &[DeviceInfo]) -> DeviceInfo {
        let profiles: Vec<&DeviceProfile> = definition
            .members
            .iter()
            .filter_map(|member| {
                probed
                    .iter()
                    .find(|info| info.id() == *member)?
                    .profile
                    .as_ref()
            })
            .collect();
        DeviceInfo {
            driver: ARRAY_DRIVER_ID.to_owned(),
            key: definition.key.clone(),
            label: definition.label.clone(),
            serial: None,
            profile: (profiles.len() == definition.members.len())
                .then(|| caps::composite_profile(&profiles, definition)),
        }
    }
}

impl DeviceDriver for ArrayDriver {
    fn id(&self) -> &'static str {
        ARRAY_DRIVER_ID
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        let definitions = self.catalog.all();
        if definitions.is_empty() {
            return Vec::new();
        }
        let probed = self.members.probe_all();
        definitions
            .iter()
            .filter(|definition| definition.valid())
            .filter(|definition| {
                definition
                    .members
                    .iter()
                    .all(|member| probed.iter().any(|info| info.id() == *member))
            })
            .map(|definition| self.info(definition, &probed))
            .collect()
    }

    fn resolve(&self, key: &str) -> Option<DeviceInfo> {
        let definition = self.catalog.get(key).filter(ArrayDefinition::valid)?;
        Some(self.info(&definition, &[]))
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        let definition = self
            .catalog
            .get(&info.key)
            .filter(ArrayDefinition::valid)
            .ok_or_else(|| DeviceError::NotFound(info.id()))?;
        let mut children = Vec::with_capacity(definition.members.len());
        for member in &definition.members {
            let (_, device) = self.members.open(member)?;
            children.push(device);
        }
        Ok(Box::new(ArrayDevice::new(definition, children)))
    }
}

/// Holds every lane back until all of them are running.
///
/// Separate radios do not start together, and the head each one delivered before the last joined
/// belongs to no common moment. Dropping it leaves the lanes within one block of each other,
/// which the calibration can measure and correct; keeping it would leave an offset nothing could.
struct Gate {
    wanted: usize,
    arrived: AtomicUsize,
}

impl Gate {
    fn new(wanted: usize) -> Self {
        Self {
            wanted,
            arrived: AtomicUsize::new(0),
        }
    }

    fn arrive(&self) {
        self.arrived.fetch_add(1, Ordering::AcqRel);
    }

    fn open(&self) -> bool {
        self.arrived.load(Ordering::Acquire) >= self.wanted
    }
}

pub struct ArrayDevice {
    definition: ArrayDefinition,
    capabilities: Capabilities,
    settings: DeviceSettings,
    children: Vec<Box<dyn SdrDevice>>,
    /// How many lanes each member contributes, so a lane number can be traced back to a radio.
    lanes: Vec<u32>,
}

impl ArrayDevice {
    fn new(definition: ArrayDefinition, children: Vec<Box<dyn SdrDevice>>) -> Self {
        let lanes: Vec<u32> = children
            .iter()
            .map(|child| child.capabilities().rx_streams.max(1))
            .collect();
        let capabilities = caps::composite(
            &children
                .iter()
                .map(|child| child.capabilities())
                .collect::<Vec<_>>(),
            &definition,
        );
        let settings = DeviceSettings {
            center_hz: children
                .first()
                .and_then(|child| child.settings().center_hz),
            sample_rate: children
                .first()
                .and_then(|child| child.settings().sample_rate),
            ..DeviceSettings::default()
        };
        Self {
            definition,
            capabilities,
            settings,
            children,
            lanes,
        }
    }

    /// Which member a lane belongs to, and which of that member's own lanes it is.
    fn locate(&self, lane: u32) -> Option<(usize, u32)> {
        let mut base = 0u32;
        for (index, count) in self.lanes.iter().enumerate() {
            if lane < base + count {
                return Some((index, lane - base));
            }
            base += count;
        }
        None
    }

    /// Splits one settings table into one per member, so each radio is told only about itself.
    fn fan_out(&self, settings: &DeviceSettings) -> Vec<DeviceSettings> {
        let mut out: Vec<DeviceSettings> = self
            .children
            .iter()
            .map(|_| DeviceSettings {
                center_hz: settings.center_hz,
                sample_rate: settings.sample_rate,
                ppm: settings.ppm,
                bandwidth: settings.bandwidth,
                antenna: settings.antenna.clone(),
                gains: settings.gains.clone(),
                ..DeviceSettings::default()
            })
            .collect();
        for entry in &settings.streams {
            let Some((child, stream)) = self.locate(entry.stream) else {
                continue;
            };
            let Some(target) = out.get_mut(child) else {
                continue;
            };
            target.streams.push(StreamSettings {
                stream,
                ..entry.clone()
            });
        }
        out
    }
}

impl SdrDevice for ArrayDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        sdrmm_device::check_stream_settings(settings, &self.capabilities)?;
        let fanned = self.fan_out(settings);
        for (child, wanted) in self.children.iter_mut().zip(&fanned) {
            child.apply(wanted)?;
        }
        self.settings.merge_from(settings);
        if let Some(first) = self.children.first() {
            self.settings.sample_rate = first.settings().sample_rate;
            if self.definition.shared_tuning {
                self.settings.center_hz = first.settings().center_hz;
            }
        }
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let expected = self.capabilities.rx_streams as usize;
        if sinks.len() != expected {
            return Err(DeviceError::Unsupported(format!(
                "this device has {expected} rx streams, got {} sinks",
                sinks.len()
            )));
        }
        let gate = Arc::new(Gate::new(expected));
        let mut sinks = sinks;
        let mut per_child: Vec<Vec<RxSink>> = Vec::with_capacity(self.children.len());
        let mut handles: Vec<FatalHandle> = Vec::with_capacity(self.children.len());
        for count in &self.lanes {
            let mut forwarded = Vec::with_capacity(*count as usize);
            let mut first: Option<FatalHandle> = None;
            for _ in 0..*count {
                let mut parent = sinks.remove(0);
                let handle = parent.share_failure();
                first.get_or_insert(handle);
                forwarded.push(forwarding_sink(parent, gate.clone()));
            }
            handles.push(first.unwrap_or_else(|| RxSink::new(|_, _| {}).share_failure()));
            per_child.push(forwarded);
        }
        for (index, (child, forwarded)) in self.children.iter_mut().zip(per_child).enumerate() {
            let handle = handles[index].clone();
            let armed = forwarded
                .into_iter()
                .map(|sink| with_relay(sink, handle.clone()))
                .collect();
            if let Err(error) = child.rx_start(armed) {
                for started in self.children.iter_mut().take(index) {
                    started.rx_stop();
                }
                return Err(error);
            }
        }
        Ok(())
    }

    fn rx_stop(&mut self) {
        for child in &mut self.children {
            child.rx_stop();
        }
    }
}

fn forwarding_sink(mut parent: RxSink, gate: Arc<Gate>) -> RxSink {
    let mut arrived = false;
    let mut open = false;
    RxSink::new(move |samples, _index| {
        if !open {
            if !arrived {
                arrived = true;
                gate.arrive();
            }
            if !gate.open() {
                return;
            }
            open = true;
        }
        parent.push(samples);
    })
}

/// Wraps a forwarding sink so a member's fatal error reaches the engine through the parent's own
/// handler rather than dying inside the child.
fn with_relay(sink: RxSink, handle: FatalHandle) -> RxSink {
    let mut sink = sink;
    RxSink::with_fatal_handler(
        move |samples, _index| sink.push(samples),
        move |error| handle.fail(error),
    )
}

/// A range every member can reach.
#[must_use]
pub fn intersect_range(a: Range, b: Range) -> Option<Range> {
    let min = a.min.max(b.min);
    let max = a.max.min(b.max);
    (min <= max).then_some(Range {
        min,
        max,
        step: a.step.or(b.step),
    })
}

#[must_use]
pub fn composite_duplex() -> Duplex {
    Duplex::RxOnly
}

#[must_use]
pub fn composite_dc_artifact() -> DcArtifact {
    DcArtifact::Operator
}

#[cfg(test)]
mod tests;
