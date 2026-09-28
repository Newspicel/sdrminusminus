mod device;
mod lane;
mod render;
mod scene;
#[cfg(test)]
mod tests;

use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

use arc_swap::{ArcSwap, ArcSwapOption};
use sdrmm_device::{DeviceError, lock};
use sdrmm_wire::{DeviceInfo, NoiseSource};

pub(crate) use device::BenchDevice;
pub use scene::{
    BenchDeviceSpec, Clutter, Echo, Emitter, LaneImpairments, MAX_BENCH_LANES,
    MAX_CLUTTER_DELAY_SAMPLES, MAX_CLUTTER_ECHOES, MAX_PPM, Path, Pilot, ReportedGap, Scene, Slip,
    Waveform, default_devices, default_scene,
};

pub const BLOCK_LEN: usize = 8_192;
pub const NOISE_SWITCH_LEAD_S: f64 = 0.005;
pub const BEARING_SETTING: &str = "wavefront_bearing_deg";
pub const RADIUS_SETTING: &str = "array_radius_m";

const KEPT_SWITCHES: usize = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LaneTruth {
    pub phase_deg: f64,
    pub gain_db: f64,
    pub first_sample_s: f64,
    pub rate_hz: f64,
    pub ppm: f64,
}

impl LaneTruth {
    #[must_use]
    pub fn offset_samples(&self, reference: &Self) -> f64 {
        (self.first_sample_s - reference.first_sample_s) * reference.rate_hz
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct NoiseSwitch {
    pub(crate) at_s: f64,
    pub(crate) on: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct NoiseSchedule {
    pub(crate) switches: Vec<NoiseSwitch>,
}

struct Slot {
    spec: BenchDeviceSpec,
    lanes: Vec<ArcSwap<LaneImpairments>>,
    truth: Vec<ArcSwapOption<LaneTruth>>,
    schedule: ArcSwap<NoiseSchedule>,
}

pub struct BenchWorld {
    origin: Instant,
    scene: ArcSwap<Scene>,
    slots: Vec<Slot>,
    edits: Mutex<()>,
}

fn ordered(mut impairments: LaneImpairments) -> LaneImpairments {
    impairments.slips.sort_by_key(|slip| slip.at);
    impairments.gaps.sort_by_key(|gap| gap.at);
    impairments
}

#[must_use]
pub fn default_world() -> Arc<BenchWorld> {
    BenchWorld::new(default_scene(), default_devices())
}

impl BenchWorld {
    #[must_use]
    pub fn new(scene: Scene, devices: Vec<BenchDeviceSpec>) -> Arc<Self> {
        let slots = devices
            .into_iter()
            .map(|spec| Slot {
                lanes: spec
                    .lanes
                    .iter()
                    .map(|lane| ArcSwap::from_pointee(ordered(lane.clone())))
                    .collect(),
                truth: spec.lanes.iter().map(|_| ArcSwapOption::empty()).collect(),
                schedule: ArcSwap::from_pointee(NoiseSchedule::default()),
                spec,
            })
            .collect();
        Arc::new(Self {
            origin: Instant::now(),
            scene: ArcSwap::from_pointee(scene),
            slots,
            edits: Mutex::new(()),
        })
    }

    pub fn set_scene(&self, scene: Scene) -> Result<(), DeviceError> {
        let _edit = lock(&self.edits);
        self.store_scene(scene)
    }

    pub fn set_impairments(
        &self,
        key: &str,
        lane: usize,
        impairments: LaneImpairments,
    ) -> Result<(), DeviceError> {
        let slot = self.find(key)?;
        let cell = self.slots[slot]
            .lanes
            .get(lane)
            .ok_or_else(|| DeviceError::NotFound(format!("{key} has no lane {lane}")))?;
        if let Some(problem) = impairments.problem() {
            return Err(DeviceError::Unsupported(problem.to_owned()));
        }
        cell.store(Arc::new(ordered(impairments)));
        Ok(())
    }

    #[must_use]
    pub fn true_time_s(&self) -> f64 {
        self.origin.elapsed().as_secs_f64()
    }

    #[must_use]
    pub fn scene(&self) -> Arc<Scene> {
        self.scene.load_full()
    }

    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        self.slots
            .iter()
            .map(|slot| slot.spec.key.clone())
            .collect()
    }

    #[must_use]
    pub fn lane_truth(&self, key: &str, lane: usize) -> Option<LaneTruth> {
        let slot = self.find(key).ok()?;
        self.slots[slot].truth.get(lane)?.load().as_deref().copied()
    }

    pub(crate) fn find(&self, key: &str) -> Result<usize, DeviceError> {
        self.slots
            .iter()
            .position(|slot| slot.spec.key == key)
            .ok_or_else(|| DeviceError::NotFound(format!("virtual:{key}")))
    }

    pub(crate) fn spec(&self, slot: usize) -> &BenchDeviceSpec {
        &self.slots[slot].spec
    }

    pub(crate) fn infos(&self) -> Vec<DeviceInfo> {
        self.slots
            .iter()
            .map(|slot| device::info(&slot.spec))
            .collect()
    }

    pub(crate) fn problem(&self, slot: usize) -> Option<String> {
        let scene = self.scene();
        if let Some(problem) = scene.problem() {
            return Some(problem);
        }
        let spec = self.spec(slot);
        if self.slots[..slot]
            .iter()
            .any(|other| other.spec.key == spec.key)
        {
            return Some(format!("{} is on the bench twice", spec.key));
        }
        if let Some(feed) = &spec.noise_from {
            let fed = self.find(feed).ok().map(|s| self.spec(s).noise_source);
            if fed.is_none_or(|source| source == NoiseSource::None) {
                return Some(format!(
                    "{} takes noise from {feed}, which has no noise source",
                    spec.key
                ));
            }
        }
        spec.problem(&scene)
    }

    pub(crate) fn feed_of(&self, slot: usize) -> Option<usize> {
        let spec = self.spec(slot);
        match &spec.noise_from {
            Some(key) => self.find(key).ok(),
            None => (spec.noise_source != NoiseSource::None).then_some(slot),
        }
    }

    pub(crate) fn impairments(&self, slot: usize, lane: usize) -> Arc<LaneImpairments> {
        self.slots[slot]
            .lanes
            .get(lane)
            .map_or_else(|| Arc::new(LaneImpairments::default()), ArcSwap::load_full)
    }

    pub(crate) fn schedule(&self, slot: usize) -> Arc<NoiseSchedule> {
        self.slots[slot].schedule.load_full()
    }

    pub(crate) fn publish_truth(&self, slot: usize, lane: usize, truth: LaneTruth) {
        if let Some(cell) = self.slots[slot].truth.get(lane) {
            cell.store(Some(Arc::new(truth)));
        }
    }

    pub(crate) fn switch_noise(&self, slot: usize, on: bool) -> f64 {
        let _edit = lock(&self.edits);
        let at_s = self.true_time_s() + NOISE_SWITCH_LEAD_S;
        let mut schedule = NoiseSchedule::clone(&self.schedule(slot));
        schedule.switches.push(NoiseSwitch { at_s, on });
        let excess = schedule.switches.len().saturating_sub(KEPT_SWITCHES);
        schedule.switches.drain(..excess);
        self.slots[slot].schedule.store(Arc::new(schedule));
        at_s
    }

    pub(crate) fn place(
        &self,
        slot: usize,
        bearing_deg: Option<f64>,
        radius_m: Option<f64>,
    ) -> Result<(), DeviceError> {
        if bearing_deg.is_none() && radius_m.is_none() {
            return Ok(());
        }
        let _edit = lock(&self.edits);
        let mut scene = Scene::clone(&self.scene());
        if let Some(azimuth_deg) = bearing_deg {
            let emitter = scene.emitters.first_mut().ok_or_else(|| {
                DeviceError::Unsupported("the bench scene has no emitter to move".to_owned())
            })?;
            emitter.azimuth_deg = azimuth_deg;
        }
        if let Some(radius_m) = radius_m {
            let spec = self.spec(slot);
            let circle = scene::uca_positions(radius_m, spec.lanes.len())
                .map_err(DeviceError::Unsupported)?;
            let end = spec.first_element + circle.len();
            let elements = scene
                .positions
                .get_mut(spec.first_element..end)
                .ok_or_else(|| {
                    DeviceError::Unsupported(format!("{} has no elements to move", spec.key))
                })?;
            elements.copy_from_slice(&circle);
        }
        self.store_scene(scene)
    }

    fn store_scene(&self, scene: Scene) -> Result<(), DeviceError> {
        let problem = scene.problem().or_else(|| {
            self.slots
                .iter()
                .find_map(|slot| slot.spec.coverage(&scene))
        });
        if let Some(problem) = problem {
            return Err(DeviceError::Unsupported(problem));
        }
        self.scene.store(Arc::new(scene));
        Ok(())
    }
}
