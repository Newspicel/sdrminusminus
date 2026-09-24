use std::{
    sync::{
        Arc, Condvar, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use sdrmm_device::{
    CaptureConfig, DeviceDriver, DeviceError, RxSink, SdrDevice, Worker, drain_stream, lock,
};
use sdrmm_wire::{
    AgcGain, AgcSetting, BandwidthSetting, Capabilities, DeviceInfo, DeviceSettings, GainKind,
    GainValue, StreamSettings,
};

use crate::{
    DEFAULT_CENTER_HZ, apply_to_hardware, caps, convert,
    driver::{DeviceDescriptors, RtlSdr},
    map_err,
};

mod apply;
#[cfg(test)]
mod hardware;
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
        with_retries(OPEN_ATTEMPTS, || settle(&wanted.key), || open_bank(wanted))
    }
}

const OPEN_ATTEMPTS: u32 = 5;
const OPENING_GAIN_TENTHS: i32 = 297;
const SETTLE_POLL: Duration = Duration::from_millis(250);
const SETTLE_LIMIT: Duration = Duration::from_secs(8);
const SETTLED_POLLS: u32 = 4;

fn bank_addresses(key: &str) -> Option<Vec<(String, u8)>> {
    let listed = crate::enumerate().ok()?;
    let unit = unit::units(&listed)
        .into_iter()
        .find(|unit| unit.key == key)?;
    Some(
        unit.members
            .iter()
            .filter_map(|member| listed.get(*member))
            .map(|lane| (lane.bus.clone(), lane.address))
            .collect(),
    )
}

fn settle(key: &str) {
    let deadline = std::time::Instant::now() + SETTLE_LIMIT;
    let mut last = None;
    let mut steady = 0;
    while std::time::Instant::now() < deadline && steady < SETTLED_POLLS {
        std::thread::sleep(SETTLE_POLL);
        let now = bank_addresses(key);
        steady = if now.is_some() && now == last {
            steady + 1
        } else {
            0
        };
        last = now;
    }
}

fn with_retries<T>(
    attempts: u32,
    mut settle: impl FnMut(),
    mut open: impl FnMut() -> Result<T, DeviceError>,
) -> Result<T, DeviceError> {
    let mut attempt = 1;
    loop {
        match open() {
            Err(error) if attempt < attempts && re_enumerating(&error) => {
                tracing::warn!(%error, attempt, "a lane dropped off the bus while opening");
                settle();
                attempt += 1;
            }
            Err(error) if attempt > 1 => {
                tracing::warn!(%error, attempt, "the bank did not open");
                return Err(error);
            }
            result => return result,
        }
    }
}

fn re_enumerating(error: &DeviceError) -> bool {
    matches!(
        error,
        DeviceError::Disconnected(_) | DeviceError::InUse(_) | DeviceError::NotFound(_)
    )
}

fn open_bank(wanted: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
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
        bandwidth: Some(BandwidthSetting::Auto),
        agc: Some(AgcSetting::switched(false)),
        gains: vec![GainValue::new(
            GainKind::Tuner,
            f64::from(OPENING_GAIN_TENTHS) / 10.0,
        )],
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
            sdr.set_gain_manual(OPENING_GAIN_TENTHS).map_err(map_err)?;
            lane_settings.push(settled_from(sdr));
        }
        switch_off(&lanes[0], lanes.len()).map_err(map_err)?;
        let count = lanes.len() as u32;
        let mut device = Self {
            lanes: lanes
                .into_iter()
                .map(|sdr| Arc::new(Mutex::new(sdr)))
                .collect(),
            capabilities: caps::kraken_capabilities(count, &gain_table),
            lane_capabilities: caps::kraken_lane_capabilities(&gain_table),
            settings: DeviceSettings::default(),
            lane_settings,
            gain_table,
            running: Arc::new(AtomicBool::new(false)),
            workers: Vec::new(),
        };
        device.settings.bias_tee = Some(false);
        device.republish();
        Ok(device)
    }

    /// Restates the bank from what its lanes settled on, so what a client reads back is what the
    /// radios are actually set to rather than what was asked for.
    fn hold_calibration_gain(&self) -> Result<(), DeviceError> {
        for (lane, settled) in self.lanes.iter().zip(&self.lane_settings) {
            let center = settled.center_hz.unwrap_or(f64::from(DEFAULT_CENTER_HZ));
            let Some(tenths) = apply::calibration_gain(center, &self.gain_table) else {
                continue;
            };
            lock(lane).set_gain_manual(tenths).map_err(map_err)?;
        }
        Ok(())
    }

    fn restore_gains(&self) -> Result<(), DeviceError> {
        for (lane, settled) in self.lanes.iter().zip(&self.lane_settings) {
            let mut sdr = lock(lane);
            let agc = settled.agc.as_ref().is_some_and(|agc| agc.on);
            match caps::current_manual_tenths(settled) {
                _ if agc => sdr.set_gain_auto(),
                Some(tenths) => sdr.set_gain_manual(tenths),
                None => Ok(()),
            }
            .map_err(map_err)?;
        }
        Ok(())
    }

    fn realign(&mut self, before: &[(Option<f64>, Option<f64>)]) {
        for ((sdr, settled), tuning) in self.lanes.iter().zip(&mut self.lane_settings).zip(before) {
            let (Some(center), Some(rate)) = *tuning else {
                continue;
            };
            let mut sdr = lock(sdr);
            if let Err(error) = realign_lane(&mut sdr, center as u32, rate as u32) {
                tracing::error!(%error, "a lane is left off its previous tuning");
            }
            settled.center_hz = Some(f64::from(sdr.center_freq()));
            settled.sample_rate = Some(f64::from(sdr.sample_rate()));
        }
    }

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
        self.settings.agc.clone_from(&first.agc);
        self.settings.streams = self
            .lane_settings
            .iter()
            .enumerate()
            .map(|(lane, settled)| StreamSettings {
                stream: lane as u32,
                center_hz: settled.center_hz,
                tuning: None,
                gains: settled.gains.clone(),
                antenna: None,
                agc: settled.agc.clone(),
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
        let before: Vec<(Option<f64>, Option<f64>)> = self
            .lane_settings
            .iter()
            .map(|settled| (settled.center_hz, settled.sample_rate))
            .collect();
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
        }
        let control = lock(&self.lanes[0]);
        for (pin, on) in &plan.gpio {
            if let Err(error) = control.set_gpio(*pin, *on) {
                failure.get_or_insert(map_err(error));
            }
        }
        drop(control);
        if failure.is_some() {
            self.realign(&before);
        }
        self.republish();
        if failure.is_none() && plan.bias_tee.is_some() {
            self.settings.bias_tee = plan.bias_tee;
        }
        failure.map_or(Ok(()), Err)
    }

    /// Switches the bank's own noise source into every lane, through the one dongle whose GPIO
    /// the switch hangs off.
    fn set_noise_source(&mut self, on: bool) -> Result<(), DeviceError> {
        if on {
            self.hold_calibration_gain()?;
        }
        lock(&self.lanes[0])
            .set_gpio(apply::NOISE_SOURCE_PIN, on)
            .map_err(map_err)?;
        if on { Ok(()) } else { self.restore_gains() }
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

    fn agc_gains(&self) -> Result<Vec<AgcGain>, DeviceError> {
        let mut read = Vec::new();
        for (stream, (lane, settled)) in self.lanes.iter().zip(&self.lane_settings).enumerate() {
            if !settled.agc.as_ref().is_some_and(|agc| agc.on) {
                continue;
            }
            let tenths = lock(lane).tuner_gain().map_err(map_err)?;
            read.push(AgcGain {
                stream: stream as u32,
                value_db: f64::from(tenths) / 10.0,
            });
        }
        Ok(read)
    }

    fn rx_stop(&mut self) {
        self.running.store(false, Ordering::Release);
        for worker in &mut self.workers {
            worker.stop();
        }
        self.workers.clear();
    }
}

fn realign_lane(sdr: &mut RtlSdr, center: u32, rate: u32) -> Result<(), crate::driver::Error> {
    if sdr.sample_rate() != rate {
        sdr.set_sample_rate(rate)?;
    }
    if sdr.center_freq() != center {
        sdr.set_center_freq(center)?;
    }
    Ok(())
}

fn switch_off(control: &RtlSdr, lanes: usize) -> Result<(), crate::driver::Error> {
    control.set_gpio(apply::NOISE_SOURCE_PIN, false)?;
    for lane in 0..lanes {
        control.set_gpio(apply::bias_tee_pin(lane), false)?;
    }
    Ok(())
}

impl Drop for KrakenDevice {
    fn drop(&mut self) {
        self.rx_stop();
        let Some(control) = self.lanes.first() else {
            return;
        };
        if let Err(error) = switch_off(&lock(control), self.lanes.len()) {
            tracing::warn!(%error, "the bank's noise source and bias tees may still be on");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, sync::mpsc};

    use super::*;

    fn attempts_until(outcomes: &[Result<(), DeviceError>]) -> (Result<(), DeviceError>, usize) {
        let calls = Cell::new(0);
        let result = with_retries(
            3,
            || {},
            || {
                let at = calls.get();
                calls.set(at + 1);
                outcomes[at].clone()
            },
        );
        (result, calls.get())
    }

    #[test]
    fn a_bank_opens_on_a_gain_every_tuner_can_hold() {
        assert!(crate::driver::GAIN_VALUES.contains(&OPENING_GAIN_TENTHS));
    }

    #[test]
    fn a_lane_that_dropped_off_is_given_time_to_come_back() {
        let (result, calls) = attempts_until(&[
            Err(DeviceError::Disconnected("gone".to_owned())),
            Err(DeviceError::InUse("re-enumerating".to_owned())),
            Ok(()),
        ]);
        assert!(result.is_ok());
        assert_eq!(calls, 3);
    }

    #[test]
    fn a_bank_that_stays_gone_fails_after_the_last_attempt() {
        let gone = || Err(DeviceError::Disconnected("gone".to_owned()));
        let (result, calls) = attempts_until(&[gone(), gone(), gone()]);
        assert!(matches!(result, Err(DeviceError::Disconnected(_))));
        assert_eq!(calls, 3);
    }

    #[test]
    fn a_fault_that_is_not_the_bus_is_not_retried() {
        let (result, calls) = attempts_until(&[Err(DeviceError::Io("pll".to_owned()))]);
        assert!(matches!(result, Err(DeviceError::Io(_))));
        assert_eq!(calls, 1);
    }

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
