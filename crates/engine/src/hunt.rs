use std::{
    sync::{
        Arc, Mutex, MutexGuard, PoisonError, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use sdrmm_channels::hunt_sweep::SweepDf;
use sdrmm_wire::{
    DecodedRecord, DecoderEvent, DfBearing, EventOrigin, HuntSettings, HuntStatus, PositionFix,
    ServerEvent, StateScope,
};
use tokio::sync::broadcast::error::TryRecvError;

use crate::{DeviceSetStatus, Engine, EngineError};

const SMOOTHING: f32 = 0.25;
const SPECTRUM_TIMEOUT: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(2);
const MIN_INTERVAL: Duration = Duration::from_millis(20);
const MAX_INTERVAL: Duration = Duration::from_millis(1_000);
const CLOSING_DB: f32 = 0.5;
const MIN_RANGE_DB: f32 = 6.0;
const POSE_CAPACITY: usize = 256;

type SharedSweep = Arc<Mutex<Option<SweepSide>>>;

pub(crate) struct HuntState {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<HuntStatus>>,
    pose_drops: Arc<AtomicU64>,
    poses: Option<PoseInbox>,
    sweep: SharedSweep,
    thread: Option<JoinHandle<()>>,
}

struct PoseSample {
    fix: PositionFix,
    received_ms: u64,
}

struct PoseInbox {
    producer: rtrb::Producer<PoseSample>,
    drops: Arc<AtomicU64>,
}

impl PoseInbox {
    fn push(&mut self, pose: PoseSample) -> Result<(), EngineError> {
        self.producer.push(pose).map_err(|_| {
            let drops = self.drops.fetch_add(1, Ordering::Relaxed) + 1;
            if drops.is_power_of_two() {
                tracing::warn!(drops, "hunt pose queue full");
            }
            EngineError::Processor("Pose queue full".to_owned())
        })
    }
}

struct SweepSide {
    poses: rtrb::Consumer<PoseSample>,
    df: SweepDf,
}

impl SweepSide {
    fn for_settings(
        settings: &HuntSettings,
        drops: &Arc<AtomicU64>,
    ) -> Result<Option<(PoseInbox, Self)>, EngineError> {
        let Some(node) = &settings.node else {
            return Ok(None);
        };
        let df = SweepDf::new(node.clone(), node.clone(), &settings.sweep)?;
        let (producer, poses) = rtrb::RingBuffer::new(POSE_CAPACITY);
        let inbox = PoseInbox {
            producer,
            drops: drops.clone(),
        };
        Ok(Some((inbox, Self { poses, df })))
    }

    fn drain(&mut self) {
        while let Ok(pose) = self.poses.pop() {
            if let Err(error) = self.df.pose(&pose.fix, pose.received_ms as f64 / 1_000.0) {
                tracing::warn!(%error, "hunt pose refused");
            }
        }
    }
}

impl HuntState {
    pub(crate) fn status(&self) -> HuntStatus {
        let mut status = lock_status(&self.status).clone();
        status.pose_drops = self.pose_drops.load(Ordering::Relaxed);
        status
    }

    fn sweeping(&self) -> bool {
        lock_side(&self.sweep)
            .as_ref()
            .is_some_and(|side| side.df.is_on())
    }

    fn switch_sweep(
        &mut self,
        on: bool,
        settings: Option<&HuntSettings>,
    ) -> Result<HuntStatus, EngineError> {
        let snapshot = {
            let mut guard = lock_side(&self.sweep);
            let side = match &mut *guard {
                Some(side) => {
                    if let Some(settings) = settings {
                        side.df.apply(&settings.sweep)?;
                    }
                    side
                }
                None => {
                    let (inbox, side) = settings
                        .map(|settings| SweepSide::for_settings(settings, &self.pose_drops))
                        .transpose()?
                        .flatten()
                        .ok_or_else(no_hunt_node)?;
                    self.poses = Some(inbox);
                    guard.insert(side)
                }
            };
            side.df.set_on(on);
            side.df.status().clone()
        };
        let mut status = lock_status(&self.status);
        status.sweep = Some(snapshot);
        if let Some(settings) = settings {
            status.settings.sweep = settings.sweep;
            if status.settings.node.is_none() {
                status.settings.node.clone_from(&settings.node);
            }
        }
        drop(status);
        Ok(self.status())
    }

    pub(crate) fn stop_and_join(mut self) -> HuntStatus {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::error!("hunt thread panicked");
        }
        self.status()
    }
}

fn lock_status(status: &Mutex<HuntStatus>) -> MutexGuard<'_, HuntStatus> {
    status.lock().unwrap_or_else(PoisonError::into_inner)
}

fn lock_side(side: &Mutex<Option<SweepSide>>) -> MutexGuard<'_, Option<SweepSide>> {
    side.lock().unwrap_or_else(PoisonError::into_inner)
}

fn no_hunt_node() -> EngineError {
    EngineError::Processor("No hunt node".to_owned())
}

fn not_running() -> EngineError {
    EngineError::Processor("Not running".to_owned())
}

fn unix_ms() -> u64 {
    u64::try_from(jiff::Timestamp::now().as_millisecond()).unwrap_or_default()
}

pub(crate) fn start(
    engine: &Arc<Engine>,
    ds: u32,
    settings: HuntSettings,
) -> Result<HuntStatus, EngineError> {
    if let Some(status) = switch_sweep(engine, ds, settings.channel, false, None)? {
        return Ok(status);
    }
    begin(engine, ds, settings, false)
}

fn sweep(
    engine: &Arc<Engine>,
    ds: u32,
    channel: u32,
    settings: Option<HuntSettings>,
) -> Result<HuntStatus, EngineError> {
    let settings = settings.map(|settings| HuntSettings {
        channel,
        ..settings
    });
    if let Some(status) = switch_sweep(engine, ds, channel, true, settings.as_ref())? {
        return Ok(status);
    }
    let settings = settings.unwrap_or_else(|| HuntSettings::for_channel(channel));
    begin(engine, ds, settings, true)
}

fn switch_sweep(
    engine: &Engine,
    ds: u32,
    channel: u32,
    on: bool,
    settings: Option<&HuntSettings>,
) -> Result<Option<HuntStatus>, EngineError> {
    let status = {
        let mut inner = engine.lock();
        let Some(hunt) = inner
            .device_sets
            .get_mut(&ds)
            .and_then(|state| state.hunts.get_mut(&channel))
        else {
            return Ok(None);
        };
        if !on && !hunt.sweeping() {
            return Ok(None);
        }
        let status = hunt.switch_sweep(on, settings)?;
        inner.revision += 1;
        status
    };
    engine.emit(ServerEvent::StateChanged {
        scope: StateScope::DeviceSet(ds),
    });
    Ok(Some(status))
}

fn begin(
    engine: &Arc<Engine>,
    ds: u32,
    settings: HuntSettings,
    sweep_on: bool,
) -> Result<HuntStatus, EngineError> {
    let decoder = admits_a_hunt(engine, ds, &settings)?;
    let settings_channel = settings.channel;
    let hunt = spawn(engine, ds, settings, decoder, sweep_on)?;
    let status = hunt.status();
    {
        let mut inner = engine.lock();
        let Some(state) = inner.device_sets.get_mut(&ds) else {
            drop(inner);
            hunt.stop_and_join();
            return Err(EngineError::DeviceSetNotFound(ds));
        };
        if state.hunts.contains_key(&settings_channel) {
            drop(inner);
            hunt.stop_and_join();
            return Err(EngineError::Scan(format!(
                "decoder {settings_channel} is already being hunted"
            )));
        }
        state.hunts.insert(settings_channel, hunt);
        inner.revision += 1;
    }
    engine.emit(ServerEvent::StateChanged {
        scope: StateScope::DeviceSet(ds),
    });
    Ok(status)
}

fn admits_a_hunt(
    engine: &Engine,
    ds: u32,
    settings: &HuntSettings,
) -> Result<Decoder, EngineError> {
    let inner = engine.lock();
    let state = inner
        .device_sets
        .get(&ds)
        .ok_or(EngineError::DeviceSetNotFound(ds))?;
    if state.hunts.contains_key(&settings.channel) {
        return Err(EngineError::Scan(format!(
            "decoder {} is already being hunted",
            settings.channel
        )));
    }
    if state.scanners.contains_key(&settings.channel) {
        return Err(EngineError::Scan(format!(
            "decoder {} is scanning; a hunt needs it held on one frequency",
            settings.channel
        )));
    }
    if state.status != DeviceSetStatus::Running {
        return Err(EngineError::Scan(
            "the device set is not running".to_string(),
        ));
    }
    state
        .channels
        .iter()
        .find(|channel| channel.id == settings.channel)
        .map(Decoder::of)
        .ok_or(EngineError::ChannelNotFound(settings.channel, ds))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Decoder {
    pub(crate) stream: u32,
    pub(crate) freq_hz: f64,
    pub(crate) bw_hz: f64,
}

impl Decoder {
    pub(crate) fn of(channel: &sdrmm_wire::ChannelInfo) -> Self {
        let (low, high) = sdrmm_channels::occupied_band(&channel.settings.params);
        Self {
            stream: channel.stream,
            freq_hz: channel.settings.frequency_hz,
            bw_hz: high - low,
        }
    }
}

pub(crate) fn stop(engine: &Engine, ds: u32, channel: u32) -> Result<HuntStatus, EngineError> {
    let hunt = {
        let mut inner = engine.lock();
        let state = inner
            .device_sets
            .get_mut(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let hunt = state
            .hunts
            .remove(&channel)
            .ok_or_else(|| EngineError::Scan(format!("decoder {channel} is not being hunted")))?;
        inner.revision += 1;
        hunt
    };
    let status = hunt.stop_and_join();
    engine.settle_tuning(ds);
    engine.emit(ServerEvent::StateChanged {
        scope: StateScope::DeviceSet(ds),
    });
    Ok(status)
}

fn sweep_parts(
    settings: &HuntSettings,
    sweep_on: bool,
    drops: &Arc<AtomicU64>,
) -> Result<(Option<PoseInbox>, Option<SweepSide>), EngineError> {
    match SweepSide::for_settings(settings, drops)? {
        Some((inbox, mut side)) => {
            side.df.set_on(sweep_on);
            Ok((Some(inbox), Some(side)))
        }
        None if sweep_on => Err(no_hunt_node()),
        None => Ok((None, None)),
    }
}

pub(crate) fn spawn(
    engine: &Arc<Engine>,
    ds: u32,
    settings: HuntSettings,
    decoder: Decoder,
    sweep_on: bool,
) -> Result<HuntState, EngineError> {
    let pose_drops = Arc::new(AtomicU64::new(0));
    let (poses, side) = sweep_parts(&settings, sweep_on, &pose_drops)?;
    let sweep_status = side.as_ref().map(|side| side.df.status().clone());
    let sweep = Arc::new(Mutex::new(side));
    let status = Arc::new(Mutex::new(HuntStatus {
        settings: settings.clone(),
        freq_hz: decoder.freq_hz,
        bw_hz: decoder.bw_hz,
        level_db: None,
        smooth_db: None,
        floor_db: None,
        best_db: None,
        strength: 0.0,
        closing: false,
        readings: 0,
        at_ms: 0,
        pose_drops: 0,
        sweep: sweep_status,
        error: None,
    }));
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let hunt = Hunt {
            engine: Arc::downgrade(engine),
            ds,
            settings,
            decoder,
            stop: stop.clone(),
            status: status.clone(),
            pose_drops: pose_drops.clone(),
            sweep: sweep.clone(),
            smooth: None,
        };
        std::thread::Builder::new()
            .name(format!("sdrmm-hunt-{ds}"))
            .spawn(move || hunt.run())
            .map_err(|e| EngineError::Scan(format!("spawn hunt thread: {e}")))?
    };
    Ok(HuntState {
        stop,
        status,
        pose_drops,
        poses,
        sweep,
        thread: Some(thread),
    })
}

struct Hunt {
    engine: Weak<Engine>,
    ds: u32,
    settings: HuntSettings,
    decoder: Decoder,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<HuntStatus>>,
    pose_drops: Arc<AtomicU64>,
    sweep: SharedSweep,
    smooth: Option<f32>,
}

enum Halt {
    Stopped,
    Failed(String),
}

impl Hunt {
    fn run(mut self) {
        match self.listen() {
            Ok(()) | Err(Halt::Stopped) => {}
            Err(Halt::Failed(error)) => {
                tracing::warn!(ds = self.ds, %error, "hunt stopped");
                lock_status(&self.status).error = Some(error);
                if let Some(engine) = self.engine.upgrade() {
                    self.publish(&engine);
                    engine.emit(ServerEvent::StateChanged {
                        scope: StateScope::DeviceSet(self.ds),
                    });
                }
            }
        }
    }

    fn listen(&mut self) -> Result<(), Halt> {
        let engine = self.engine.upgrade().ok_or(Halt::Stopped)?;
        let mut rx = engine
            .subscribe_spectrum(self.ds, self.decoder.stream)
            .map_err(|e| match e {
                EngineError::DeviceSetNotFound(_) => Halt::Stopped,
                other => Halt::Failed(other.to_string()),
            })?;

        let interval = Duration::from_millis(u64::from(self.settings.interval_ms))
            .clamp(MIN_INTERVAL, MAX_INTERVAL);
        let mut heard = Instant::now();
        let mut measured = Instant::now();
        let mut window_end = Instant::now() + interval;
        let mut peak = f32::NEG_INFINITY;
        loop {
            if self.stop.load(Ordering::Acquire) {
                return Err(Halt::Stopped);
            }
            match rx.try_recv() {
                Ok(snapshot) => {
                    heard = Instant::now();
                    if let Some(db) =
                        crate::scanner::measure(&snapshot, self.decoder.freq_hz, self.decoder.bw_hz)
                    {
                        measured = heard;
                        peak = peak.max(db);
                    } else if measured.elapsed() >= SPECTRUM_TIMEOUT {
                        return Err(Halt::Failed(format!(
                            "the radio is not tuned over the decoder's {} Hz; unlock its tuning \
                             or move it there",
                            self.decoder.freq_hz
                        )));
                    }
                }
                Err(TryRecvError::Empty) => {
                    if heard.elapsed() >= SPECTRUM_TIMEOUT {
                        return Err(Halt::Failed(format!(
                            "the device produced no spectrum within {SPECTRUM_TIMEOUT:?}"
                        )));
                    }
                    std::thread::sleep(POLL);
                }
                Err(TryRecvError::Lagged(_)) => {}
                Err(TryRecvError::Closed) => return Err(Halt::Stopped),
            }
            if Instant::now() < window_end {
                continue;
            }
            window_end = Instant::now() + interval;
            self.follow_decoder(&engine)?;
            let level = peak.is_finite().then_some(peak);
            let at_ms = unix_ms();
            if let Some(level) = level {
                self.record(level, at_ms);
            }
            self.sweep(&engine, level, at_ms);
            if level.is_some() {
                self.publish(&engine);
            }
            peak = f32::NEG_INFINITY;
        }
    }

    fn follow_decoder(&mut self, engine: &Engine) -> Result<(), Halt> {
        let decoder = engine
            .decoder_of(self.ds, self.settings.channel)
            .ok_or_else(|| Halt::Failed("the decoder being hunted was removed".to_string()))?;
        if decoder == self.decoder {
            return Ok(());
        }
        self.decoder = decoder;
        let mut status = lock_status(&self.status);
        status.freq_hz = decoder.freq_hz;
        status.bw_hz = decoder.bw_hz;
        Ok(())
    }

    fn record(&mut self, level_db: f32, at_ms: u64) {
        let previous = self.smooth;
        let smooth = previous.map_or(level_db, |had| had + (level_db - had) * SMOOTHING);
        self.smooth = Some(smooth);

        let mut status = lock_status(&self.status);
        status.level_db = Some(level_db);
        status.smooth_db = Some(smooth);
        status.floor_db = Some(status.floor_db.map_or(smooth, |had| had.min(smooth)));
        status.best_db = Some(status.best_db.map_or(smooth, |had| had.max(smooth)));
        status.closing = previous.is_some_and(|had| smooth - had >= CLOSING_DB);
        status.strength = strength(smooth, status.floor_db, status.best_db);
        status.readings += 1;
        status.at_ms = at_ms;
    }

    fn sweep(&self, engine: &Engine, level_db: Option<f32>, at_ms: u64) {
        let outcome = lock_side(&self.sweep).as_mut().map(|side| {
            side.drain();
            let at_s = at_ms as f64 / 1_000.0;
            let bearing =
                level_db.and_then(|level| side.df.level(level, at_s, self.decoder.freq_hz));
            (bearing, side.df.status().clone())
        });
        let Some((bearing, snapshot)) = outcome else {
            return;
        };
        lock_status(&self.status).sweep = Some(snapshot);
        if let Some(bearing) = bearing {
            engine.publish_decoded(bearing_record(
                self.ds,
                self.settings.channel,
                self.decoder.freq_hz,
                bearing,
            ));
        }
    }

    fn publish(&self, engine: &Engine) {
        let status = {
            let mut status = lock_status(&self.status);
            status.pose_drops = self.pose_drops.load(Ordering::Relaxed);
            status.clone()
        };
        engine.emit(ServerEvent::HuntUpdate {
            device_set: self.ds,
            status: Box::new(status),
        });
    }
}

fn bearing_record(ds: u32, channel: u32, freq_hz: f64, bearing: DfBearing) -> DecodedRecord {
    DecodedRecord {
        origin: Some(EventOrigin {
            node: bearing.node.clone(),
            transmission: 0,
        }),
        device_set: ds,
        channel,
        at: format!("{:.9}", jiff::Timestamp::now()),
        freq_hz,
        event: DecoderEvent::Df(bearing),
        sinks: Vec::new(),
    }
}

impl Engine {
    pub fn sweep_hunt(
        self: &Arc<Self>,
        ds: u32,
        channel: u32,
        settings: Option<HuntSettings>,
    ) -> Result<HuntStatus, EngineError> {
        sweep(self, ds, channel, settings)
    }

    pub fn hunt_pose(
        &self,
        device_set: u32,
        channel: u32,
        fix: PositionFix,
        received_ms: u64,
    ) -> Result<(), EngineError> {
        let mut inner = self.lock();
        let hunt = inner
            .device_sets
            .get_mut(&device_set)
            .ok_or(EngineError::DeviceSetNotFound(device_set))?
            .hunts
            .get_mut(&channel)
            .ok_or_else(not_running)?;
        hunt.poses
            .as_mut()
            .ok_or_else(no_hunt_node)?
            .push(PoseSample { fix, received_ms })
    }

    pub fn hunt_mark(&self, device_set: u32, channel: u32) -> Result<DfBearing, EngineError> {
        let (side, freq_hz) = {
            let inner = self.lock();
            let hunt = inner
                .device_sets
                .get(&device_set)
                .ok_or(EngineError::DeviceSetNotFound(device_set))?
                .hunts
                .get(&channel)
                .ok_or_else(not_running)?;
            (hunt.sweep.clone(), lock_status(&hunt.status).freq_hz)
        };
        let bearing = {
            let mut guard = lock_side(&side);
            let side = guard.as_mut().ok_or_else(no_hunt_node)?;
            side.drain();
            side.df
                .mark(unix_ms() as f64 / 1_000.0, freq_hz)
                .map_err(|refusal| EngineError::Processor(refusal.to_string()))?
        };
        self.publish_decoded(bearing_record(
            device_set,
            channel,
            freq_hz,
            bearing.clone(),
        ));
        Ok(bearing)
    }
}

fn strength(smooth: f32, floor: Option<f32>, best: Option<f32>) -> f32 {
    let (Some(floor), Some(best)) = (floor, best) else {
        return 0.0;
    };
    let range = (best - floor).max(MIN_RANGE_DB);
    ((smooth - floor) / range).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose() -> PoseSample {
        PoseSample {
            fix: PositionFix {
                latitude: 48.137,
                longitude: 11.575,
                altitude_m: None,
                accuracy_m: None,
                speed_mps: None,
                track_deg: None,
                time: "2026-09-28T12:00:00Z".to_owned(),
                attitude: sdrmm_wire::Attitude::default(),
            },
            received_ms: 1_790_000_000_000,
        }
    }

    fn swept(sweep: sdrmm_wire::HuntSweepParams) -> HuntSettings {
        HuntSettings {
            node: Some("hunt-1".to_owned()),
            sweep,
            ..HuntSettings::for_channel(1)
        }
    }

    #[test]
    fn hunt_pose_ring_counts_drops() {
        let drops = Arc::new(AtomicU64::new(0));
        let (Some(mut inbox), Some(mut side)) =
            sweep_parts(&swept(Default::default()), false, &drops).unwrap()
        else {
            panic!("a hunt node gets a pose ring");
        };
        let refused = (0..300).filter(|_| inbox.push(pose()).is_err()).count();
        assert_eq!(refused, 44);
        assert_eq!(drops.load(Ordering::Relaxed), 44);
        assert!(matches!(
            inbox.push(pose()),
            Err(EngineError::Processor(text)) if text == "Pose queue full"
        ));
        side.drain();
        assert!(
            inbox.push(pose()).is_ok(),
            "a drained ring takes poses again"
        );
    }

    #[test]
    fn a_hunt_sweep_needs_a_node_and_sane_params() {
        let drops = Arc::new(AtomicU64::new(0));
        let plain = HuntSettings::for_channel(1);
        assert!(matches!(
            sweep_parts(&plain, false, &drops),
            Ok((None, None))
        ));
        assert!(matches!(
            sweep_parts(&plain, true, &drops),
            Err(EngineError::Processor(text)) if text == "No hunt node"
        ));
        let narrow = swept(sdrmm_wire::HuntSweepParams {
            beamwidth_deg: 1.0,
            ..Default::default()
        });
        assert!(matches!(
            sweep_parts(&narrow, true, &drops),
            Err(EngineError::Channel(_))
        ));
        let (_, side) = sweep_parts(&swept(Default::default()), true, &drops).unwrap();
        assert!(side.is_some_and(|side| side.df.is_on()));
    }

    #[test]
    fn a_meter_has_a_range_before_the_ground_has_been_walked() {
        assert_eq!(strength(-70.0, None, None), 0.0);
        assert_eq!(
            strength(-70.0, Some(-70.0), Some(-70.0)),
            0.0,
            "one reading is not a range"
        );
        assert!(strength(-68.0, Some(-70.0), Some(-70.0)) < 0.5);
    }

    #[test]
    fn strength_spans_the_ground_actually_covered() {
        assert_eq!(strength(-90.0, Some(-90.0), Some(-30.0)), 0.0);
        assert_eq!(strength(-30.0, Some(-90.0), Some(-30.0)), 1.0);
        assert!((strength(-60.0, Some(-90.0), Some(-30.0)) - 0.5).abs() < 0.01);
        assert_eq!(
            strength(-100.0, Some(-90.0), Some(-30.0)),
            0.0,
            "a reading below the floor must not run the meter backwards"
        );
    }

    #[test]
    fn a_decoder_is_measured_over_the_band_it_occupies() {
        let channel = sdrmm_wire::ChannelInfo {
            id: 3,
            stream: 1,
            node: None,
            settings: sdrmm_wire::ChannelSettings {
                frequency_hz: 433_920_000.0,
                squelch: sdrmm_wire::Squelch::Off,
                params: sdrmm_wire::ChannelParams::Nfm(sdrmm_wire::NfmParams {
                    bandwidth_hz: 25_000.0,
                    ..sdrmm_wire::NfmParams::default()
                }),
                blanker: Default::default(),
            },
            out_of_band: false,
            audio_recordings: Vec::new(),
            baseband_recording: None,
            network_export: None,
        };
        assert_eq!(
            Decoder::of(&channel),
            Decoder {
                stream: 1,
                freq_hz: 433_920_000.0,
                bw_hz: 25_000.0,
            }
        );
    }
}
