use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use sdrmm_device::{DeviceDriver, DeviceError, DeviceRegistry, SdrDevice};
use sdrmm_device_recording::RecordingDriver;
use sdrmm_device_virtual::{
    BenchDeviceSpec, BenchWorld, LaneImpairments, LaneTruth, Scene, VirtualDriver, default_devices,
    default_scene,
};
use sdrmm_engine::{ArrayEvent, ArraySpec, Engine, LaneRef, ProcessorSpec};
use sdrmm_wire::{
    ArrayGeometry, ArrayNode, ArrayStatus, CalPhase, DeviceInfo, DeviceSettings, ProcessorParams,
    ProcessorReading, ProcessorStatus, SyncState, Winding, processor::df::DfParams,
};
use tokio::sync::broadcast::{self, error::TryRecvError};

pub const ARRAY: &str = "array";
pub const KRAKEN: &str = "kraken5";
pub const ARRAY4: &str = "array4";
pub const DONGLE1: &str = "dongle1";
pub const DONGLE2: &str = "dongle2";
pub const RATE: f64 = 1_024_000.0;
pub const FULL_RATE: f64 = 2_400_000.0;
pub const BENCH_RADIUS_M: f64 = 0.35;
pub const DONGLE_SPACING_M: f64 = 0.5;
pub const LOCK_WAIT: Duration = Duration::from_secs(10);
pub const WAIT: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(2);
const REGISTRY_PRIORITY: u8 = 10;

type Pulled = Arc<Mutex<HashSet<String>>>;

struct Pluggable {
    inner: VirtualDriver,
    pulled: Pulled,
}

impl Pluggable {
    fn present(&self, info: &DeviceInfo) -> bool {
        !self
            .pulled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&info.key)
    }
}

impl DeviceDriver for Pluggable {
    fn id(&self) -> &'static str {
        self.inner.id()
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        self.inner
            .probe()
            .into_iter()
            .filter(|info| self.present(info))
            .collect()
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        if self.present(info) {
            self.inner.open(info)
        } else {
            Err(DeviceError::NotFound(info.id()))
        }
    }
}

pub struct Bench {
    pub engine: Arc<Engine>,
    pub world: Arc<BenchWorld>,
    devices: Mutex<Vec<BenchDeviceSpec>>,
    pulled: Pulled,
    hotplug: Mutex<(Option<Vec<String>>, HashSet<u32>)>,
}

impl Bench {
    pub fn new() -> Self {
        Self::with(default_scene(), default_devices(), None)
    }

    pub fn recording_into(dir: PathBuf) -> Self {
        Self::with(default_scene(), default_devices(), Some(dir))
    }

    pub fn with(scene: Scene, devices: Vec<BenchDeviceSpec>, recordings: Option<PathBuf>) -> Self {
        let world = BenchWorld::new(scene, devices.clone());
        let pulled = Pulled::default();
        let mut registry = DeviceRegistry::new();
        registry.register(
            REGISTRY_PRIORITY,
            Box::new(Pluggable {
                inner: VirtualDriver::with_world(world.clone()),
                pulled: pulled.clone(),
            }),
        );
        registry.register(
            REGISTRY_PRIORITY,
            Box::new(RecordingDriver::new(recordings.clone())),
        );
        Self {
            engine: Engine::with_registry(registry, recordings),
            world,
            devices: Mutex::new(devices),
            pulled,
            hotplug: Mutex::new((None, HashSet::new())),
        }
    }

    pub fn open(&self, key: &str) -> u32 {
        self.open_at(key, RATE)
    }

    pub fn open_at(&self, key: &str, sample_rate: f64) -> u32 {
        let ds = self
            .engine
            .create_device_set(&format!("virtual:{key}"))
            .unwrap_or_else(|error| panic!("the bench {key} opens: {error}"));
        self.engine
            .patch_device(
                ds,
                DeviceSettings {
                    sample_rate: Some(sample_rate),
                    ..DeviceSettings::default()
                },
            )
            .unwrap_or_else(|error| panic!("{key} runs at {sample_rate}: {error}"));
        ds
    }

    pub fn impair(&self, key: &str, lane: usize, edit: impl FnOnce(&mut LaneImpairments)) {
        let mut devices = self.devices.lock().unwrap_or_else(PoisonError::into_inner);
        let spec = devices
            .iter_mut()
            .find(|spec| spec.key == key)
            .unwrap_or_else(|| panic!("{key} is on the bench"));
        let impairments = &mut spec.lanes[lane];
        edit(impairments);
        self.world
            .set_impairments(key, lane, impairments.clone())
            .unwrap_or_else(|error| panic!("{key} lane {lane} takes the impairment: {error}"));
    }

    pub fn truth(&self, key: &str, lane: usize) -> LaneTruth {
        wait_for(&format!("{key} lane {lane} truth"), WAIT, || {
            self.world.lane_truth(key, lane)
        })
    }

    pub fn pull(&self, key: &str) {
        self.pulled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.to_owned());
        self.hotplug_tick();
        self.hotplug_tick();
    }

    pub fn plug(&self, key: &str) {
        self.pulled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(key);
        self.hotplug_tick();
    }

    fn hotplug_tick(&self) {
        let mut held = self.hotplug.lock().unwrap_or_else(PoisonError::into_inner);
        let (known, missing) = &mut *held;
        self.engine.hotplug_tick_for_test(known, missing);
    }
}

pub fn uca(radius_m: f64) -> ArrayGeometry {
    ArrayGeometry::Uca {
        radius_m,
        first_deg: 0.0,
        winding: Winding::Clockwise,
    }
}

pub fn dongle_line() -> ArrayGeometry {
    ArrayGeometry::Ula {
        spacing_m: DONGLE_SPACING_M,
        axis_deg: 90.0,
    }
}

pub fn lanes(ds: u32, streams: impl IntoIterator<Item = u32>) -> Vec<Option<LaneRef>> {
    streams
        .into_iter()
        .map(|stream| {
            Some(LaneRef {
                device_set: ds,
                stream,
            })
        })
        .collect()
}

pub fn array(lanes: Vec<Option<LaneRef>>, settings: ArrayNode) -> ArraySpec {
    ArraySpec {
        node: ARRAY.to_owned(),
        lanes,
        settings,
        tune: None,
        warm: None,
    }
}

pub fn kraken_array(ds: u32) -> ArraySpec {
    array(
        lanes(ds, 0..5),
        ArrayNode {
            geometry: uca(BENCH_RADIUS_M),
            ..ArrayNode::default()
        },
    )
}

pub fn processor(node: &str, params: ProcessorParams) -> ProcessorSpec {
    ProcessorSpec {
        node: node.to_owned(),
        array: ARRAY.to_owned(),
        params,
        lane_ports: Vec::new(),
        steer_from: None,
    }
}

pub fn df(node: &str) -> ProcessorSpec {
    processor(node, ProcessorParams::Df(DfParams::default()))
}

pub fn wait_for<T>(what: &str, timeout: Duration, mut found: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(found) = found() {
            return found;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(POLL);
    }
}

pub fn status(engine: &Engine) -> ArrayStatus {
    engine
        .array_statuses()
        .into_iter()
        .find(|status| status.node == ARRAY)
        .unwrap_or_else(|| panic!("{ARRAY} runs"))
}

pub fn wait_status(
    engine: &Engine,
    what: &str,
    timeout: Duration,
    mut accept: impl FnMut(&ArrayStatus) -> bool,
) -> ArrayStatus {
    let deadline = Instant::now() + timeout;
    loop {
        let now = status(engine);
        if accept(&now) {
            return now;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {now:#?}"
        );
        std::thread::sleep(POLL);
    }
}

pub fn calibrated(status: &ArrayStatus) -> bool {
    status.sync == SyncState::Locked && status.cal == CalPhase::Solved && status.phase_ready
}

pub fn wait_calibrated(engine: &Engine, timeout: Duration) -> ArrayStatus {
    wait_status(engine, "locked and calibrated", timeout, calibrated)
}

pub fn wait_solve_after(engine: &Engine, before: &ArrayStatus, timeout: Duration) -> ArrayStatus {
    wait_status(engine, "a fresh solve", timeout, |now| {
        calibrated(now) && now.last_solve_at != before.last_solve_at
    })
}

pub fn processor_status(status: &ArrayStatus, node: &str) -> ProcessorStatus {
    status
        .processors
        .iter()
        .find(|processor| processor.node == node)
        .cloned()
        .unwrap_or_else(|| panic!("{node} is on the array: {:?}", status.processors))
}

pub fn wrap_deg(degrees: f64) -> f64 {
    (degrees + 180.0).rem_euclid(360.0) - 180.0
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Residual {
    pub delay: f64,
    pub phase_deg: f64,
    pub gain_db: f64,
}

pub fn truth_residual(bench: &Bench, members: &[(&str, usize)], status: &ArrayStatus) -> Residual {
    let (key, lane) = members[0];
    let reference = bench.truth(key, lane);
    let mut worst = Residual::default();
    for (slot, &(key, lane)) in members.iter().enumerate().skip(1) {
        let truth = bench.truth(key, lane);
        let solved = &status.lanes[slot];
        let delay = (reference.first_sample_s - truth.first_sample_s) * reference.rate_hz;
        let phase = wrap_deg(truth.phase_deg - reference.phase_deg);
        let gain = truth.gain_db - reference.gain_db;
        worst.delay = worst.delay.max((solved.delay_samples - delay).abs());
        worst.phase_deg = worst
            .phase_deg
            .max(wrap_deg(f64::from(solved.phase_deg) - phase).abs());
        worst.gain_db = worst.gain_db.max((f64::from(solved.gain_db) - gain).abs());
    }
    worst
}

pub fn kraken_members() -> Vec<(&'static str, usize)> {
    (0..5).map(|lane| (KRAKEN, lane)).collect()
}

pub fn next_reading(
    events: &mut broadcast::Receiver<ArrayEvent>,
    node: &str,
    timeout: Duration,
) -> Arc<ProcessorReading> {
    wait_for(&format!("a reading from {node}"), timeout, || {
        loop {
            match events.try_recv() {
                Ok(ArrayEvent::Report { processor, reading }) if processor == node => {
                    return Some(reading);
                }
                Ok(_) | Err(TryRecvError::Lagged(_)) => {}
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Closed) => panic!("the array events closed"),
            }
        }
    })
}

pub fn drain(events: &mut broadcast::Receiver<ArrayEvent>) {
    while matches!(events.try_recv(), Ok(_) | Err(TryRecvError::Lagged(_))) {}
}
