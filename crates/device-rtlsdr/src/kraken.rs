use std::sync::{
    Arc, Condvar, Mutex, PoisonError,
    atomic::{AtomicBool, Ordering},
};

use sdrmm_device::{
    CaptureConfig, DeviceDriver, DeviceError, RxSink, SdrDevice, Worker, drain_stream, lock,
};
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings, ExtraValue, StreamSettings};

use crate::{
    DEFAULT_CENTER_HZ, apply_to_hardware, caps, convert,
    driver::{BoardVariant, DeviceDescriptors, RtlSdr},
    map_err,
};

mod apply;
mod unit;

pub(crate) use unit::claimed;

pub(crate) const DRIVER_ID: &str = "kraken";

const THREAD_NAME: &str = "sdrmm-kraken-rx";

/// The rate the vendor's own acquisition chain runs at, and the widest the five chains keep up
/// with over one shared USB host controller.
const DEFAULT_SAMPLE_RATE_HZ: u32 = 2_400_000;

#[derive(Default)]
pub struct KrakenDriver;

impl KrakenDriver {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

fn info(unit: &unit::Unit) -> DeviceInfo {
    DeviceInfo {
        driver: DRIVER_ID.to_owned(),
        key: unit.key.clone(),
        label: unit.label(),
        serial: None,
        profile: Some(caps::kraken_capabilities(unit.lanes(), &[]).profile()),
    }
}

impl DeviceDriver for KrakenDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        match crate::enumerate() {
            Ok(descriptors) => unit::units(&descriptors).iter().map(info).collect(),
            Err(error) => {
                tracing::warn!("kraken enumerate failed: {error}");
                Vec::new()
            }
        }
    }

    fn open(&self, wanted: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        let descriptors = DeviceDescriptors::new().map_err(map_err)?;
        let listed: Vec<_> = descriptors.iter().cloned().collect();
        let unit = unit::units(&listed)
            .into_iter()
            .find(|unit| unit.key == wanted.key)
            .ok_or_else(|| DeviceError::NotFound(wanted.id()))?;
        let mut lanes = Vec::with_capacity(unit.members.len());
        for member in &unit.members {
            lanes.push(descriptors.open(*member).map_err(map_err)?);
        }
        tracing::info!(model = unit.model, key = %unit.key, lanes = lanes.len(), "opened a coherent bank");
        Ok(Box::new(KrakenDevice::new(lanes)?))
    }
}

/// Holds every lane until the last one is ready to run.
///
/// The dongles are set up one after another, but their sample counts only line up if they leave
/// reset together, so the bank starts on one release rather than on whenever each dongle happened
/// to be armed. What is left over is thread wake-up, which the calibration measures away.
#[derive(Default)]
struct StartGate {
    go: Mutex<Option<bool>>,
    signal: Condvar,
}

impl StartGate {
    fn wait(&self) -> bool {
        let mut go = lock(&self.go);
        while go.is_none() {
            go = self.signal.wait(go).unwrap_or_else(PoisonError::into_inner);
        }
        go.unwrap_or(false)
    }

    fn open(&self, go: bool) {
        *lock(&self.go) = Some(go);
        self.signal.notify_all();
    }
}

pub struct KrakenDevice {
    lanes: Vec<Arc<Mutex<RtlSdr>>>,
    capabilities: Capabilities,
    lane_capabilities: Capabilities,
    settings: DeviceSettings,
    lane_settings: Vec<DeviceSettings>,
    gain_table: Vec<i32>,
    running: Arc<AtomicBool>,
    workers: Vec<Worker>,
}

fn settled_from(sdr: &RtlSdr) -> DeviceSettings {
    DeviceSettings {
        center_hz: Some(f64::from(sdr.center_freq())),
        sample_rate: Some(f64::from(sdr.sample_rate())),
        ppm: Some(f64::from(sdr.freq_correction())),
        antenna: Some("RX".to_owned()),
        extra: vec![ExtraValue {
            name: caps::AGC.to_owned(),
            value: true.into(),
        }],
        ..DeviceSettings::default()
    }
}

impl KrakenDevice {
    fn new(mut lanes: Vec<RtlSdr>) -> Result<Self, DeviceError> {
        let gain_table = lanes
            .first()
            .ok_or_else(|| DeviceError::NotFound("an empty bank".to_owned()))?
            .gains()
            .to_vec();
        let mut lane_settings = Vec::with_capacity(lanes.len());
        for sdr in &mut lanes {
            sdr.set_dither(false).map_err(map_err)?;
            sdr.set_sample_rate(DEFAULT_SAMPLE_RATE_HZ)
                .map_err(map_err)?;
            sdr.set_center_freq(DEFAULT_CENTER_HZ).map_err(map_err)?;
            sdr.set_gain_auto().map_err(map_err)?;
            lane_settings.push(settled_from(sdr));
        }
        let control = &lanes[0];
        control
            .set_gpio(apply::NOISE_SOURCE_PIN, false)
            .map_err(map_err)?;
        for lane in 0..lanes.len() {
            control
                .set_gpio(apply::bias_tee_pin(lane), false)
                .map_err(map_err)?;
        }
        let count = lanes.len() as u32;
        let mut device = Self {
            lanes: lanes
                .into_iter()
                .map(|sdr| Arc::new(Mutex::new(sdr)))
                .collect(),
            capabilities: caps::kraken_capabilities(count, &gain_table),
            lane_capabilities: caps::capabilities(BoardVariant::Generic, &gain_table),
            settings: DeviceSettings::default(),
            lane_settings,
            gain_table,
            running: Arc::new(AtomicBool::new(false)),
            workers: Vec::new(),
        };
        device.settings.extra = vec![ExtraValue {
            name: caps::BIAS_TEE.to_owned(),
            value: false.into(),
        }];
        device.republish();
        Ok(device)
    }

    /// Restates the bank from what its lanes settled on, so what a client reads back is what the
    /// radios are actually set to rather than what was asked for.
    fn republish(&mut self) {
        let Some(first) = self.lane_settings.first() else {
            return;
        };
        self.settings.center_hz = first.center_hz;
        self.settings.sample_rate = first.sample_rate;
        self.settings.ppm = first.ppm;
        self.settings.bandwidth = first.bandwidth;
        self.settings.antenna.clone_from(&first.antenna);
        self.settings.gains.clone_from(&first.gains);
        let agc = first
            .extra
            .iter()
            .find(|value| value.name == caps::AGC)
            .cloned();
        self.settings.extra.retain(|value| value.name != caps::AGC);
        self.settings.extra.extend(agc);
        self.settings.streams = self
            .lane_settings
            .iter()
            .enumerate()
            .map(|(lane, settled)| StreamSettings {
                stream: lane as u32,
                center_hz: None,
                tuning: None,
                gains: settled.gains.clone(),
                antenna: None,
            })
            .collect();
    }
}

impl SdrDevice for KrakenDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        let plan = apply::plan(
            settings,
            &self.capabilities,
            &self.lane_capabilities,
            &self.lane_settings,
            &self.gain_table,
        )?;
        let mut failure = None;
        for ((sdr, settled), lane) in self
            .lanes
            .iter()
            .zip(&mut self.lane_settings)
            .zip(&plan.lanes)
        {
            let mut sdr = lock(sdr);
            let result = apply_to_hardware(&mut sdr, lane);
            settled.center_hz = Some(f64::from(sdr.center_freq()));
            settled.sample_rate = Some(f64::from(sdr.sample_rate()));
            settled.ppm = Some(f64::from(sdr.freq_correction()));
            drop(sdr);
            if let Err(error) = result {
                failure.get_or_insert(error);
                continue;
            }
            settled.merge_from(&lane.applied);
            if lane.clear_bandwidth {
                settled.bandwidth = None;
            }
        }
        let control = lock(&self.lanes[0]);
        for (pin, on) in &plan.gpio {
            if let Err(error) = control.set_gpio(*pin, *on) {
                failure.get_or_insert(map_err(error));
            }
        }
        drop(control);
        self.republish();
        if failure.is_none() {
            for value in plan.extra {
                self.settings.extra.retain(|held| held.name != value.name);
                self.settings.extra.push(value);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    /// Switches the bank's own noise source into every lane, through the one dongle whose GPIO
    /// the switch hangs off.
    fn set_noise_source(&mut self, on: bool) -> Result<(), DeviceError> {
        lock(&self.lanes[0])
            .set_gpio(apply::NOISE_SOURCE_PIN, on)
            .map_err(map_err)
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let expected = self.lanes.len();
        if sinks.len() != expected {
            return Err(DeviceError::Unsupported(format!(
                "this radio has {expected} rx streams, got {} sinks",
                sinks.len()
            )));
        }
        if self.running.load(Ordering::Acquire) {
            return Err(DeviceError::AlreadyStreaming);
        }
        let mut held = Vec::with_capacity(expected);
        for lane in &self.lanes {
            held.push(lock(lane).hold_stream().map_err(map_err)?);
        }
        self.running = Arc::new(AtomicBool::new(true));
        let start = Arc::new(StartGate::default());
        let config =
            CaptureConfig::new(THREAD_NAME, DRIVER_ID).with_sample_rate(self.settings.sample_rate);
        let mut workers = Vec::with_capacity(expected);
        for ((stream, gate), mut sink) in held.into_iter().zip(sinks) {
            let running = self.running.clone();
            let waiting = start.clone();
            let mut worker = Worker::new();
            let spawned = worker.start(THREAD_NAME, move |_| {
                if !waiting.wait() {
                    return;
                }
                if let Err(error) = gate.release() {
                    running.store(false, Ordering::Release);
                    sink.fail(map_err(error));
                    return;
                }
                let mut converter = convert::converter();
                let Some(failure) =
                    drain_stream(&stream, &running, &mut sink, &mut converter, &config)
                else {
                    return;
                };
                running.store(false, Ordering::Release);
                sink.fail(if failure.gone {
                    DeviceError::Disconnected(failure.reason)
                } else {
                    DeviceError::Io(failure.reason)
                });
            });
            if let Err(error) = spawned {
                start.open(false);
                self.running.store(false, Ordering::Release);
                return Err(error);
            }
            workers.push(worker);
        }
        self.workers = workers;
        start.open(true);
        Ok(())
    }

    fn rx_stop(&mut self) {
        self.running.store(false, Ordering::Release);
        for worker in &mut self.workers {
            worker.stop();
        }
        self.workers.clear();
    }
}

impl Drop for KrakenDevice {
    fn drop(&mut self) {
        self.rx_stop();
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use super::*;

    fn crew(gate: &Arc<StartGate>, lanes: usize) -> mpsc::Receiver<bool> {
        let (tx, rx) = mpsc::channel();
        for _ in 0..lanes {
            let gate = gate.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(gate.wait());
            });
        }
        rx
    }

    #[test]
    fn no_lane_runs_until_the_bank_is_released() {
        let gate = Arc::new(StartGate::default());
        let waiting = crew(&gate, 5);
        assert!(
            waiting.recv_timeout(Duration::from_millis(50)).is_err(),
            "a lane left reset before the bank was released"
        );
        gate.open(true);
        for _ in 0..5 {
            assert!(
                waiting
                    .recv_timeout(Duration::from_secs(5))
                    .expect("every lane is released")
            );
        }
    }

    #[test]
    fn a_bank_that_never_starts_lets_its_lanes_go() {
        let gate = Arc::new(StartGate::default());
        let waiting = crew(&gate, 5);
        gate.open(false);
        for _ in 0..5 {
            assert!(
                !waiting
                    .recv_timeout(Duration::from_secs(5))
                    .expect("every lane is told to stop")
            );
        }
    }

    #[test]
    fn a_lane_that_arrives_late_reads_the_decision_already_made() {
        let gate = Arc::new(StartGate::default());
        gate.open(true);
        assert!(gate.wait());
    }
}
