use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use sdrmm_device::{
    CaptureConfig, DeviceDriver, DeviceError, LaneMark, MarkPoster, RxSink, SdrDevice, lock,
};
use sdrmm_usb_stream::RxStream;
use sdrmm_wire::{
    AgcGain, AgcSetting, BandwidthSetting, Capabilities, DeviceInfo, DeviceSettings, GainKind,
    GainValue, StreamSettings,
};

use crate::{
    DEFAULT_CENTER_HZ, apply_to_hardware, caps, convert,
    driver::{DeviceDescriptors, IN_FLIGHT_SAMPLES, RtlSdr, StreamGate},
    map_err,
};

mod apply;
mod bank;
#[cfg(test)]
mod hardware;
mod unit;

use bank::{Bank, BankLane};
pub(crate) use unit::{Model, claimed};

pub(crate) const DRIVER_ID: &str = "kraken";

const DEFAULT_SAMPLE_RATE_HZ: u32 = 2_400_000;
const OPEN_ATTEMPTS: u32 = 5;
const OPENING_GAIN_TENTHS: i32 = 297;
const SETTLE_POLL: Duration = Duration::from_millis(250);
const SETTLE_LIMIT: Duration = Duration::from_secs(8);
const SETTLED_POLLS: u32 = 4;

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
        profile: Some(caps::kraken_capabilities(unit.model, unit.expected_lanes(), &[]).profile()),
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

fn admit(unit: &unit::Unit, wanted: &DeviceInfo) -> Result<(), DeviceError> {
    if !unit.complete() {
        return Err(DeviceError::NotFound(unit.label()));
    }
    let lanes = unit.members.len() as u32;
    if let Some(profile) = &wanted.profile
        && profile.rx_streams != lanes
    {
        return Err(DeviceError::NotFound(format!(
            "{} has {lanes} lanes, {} were expected",
            unit.label(),
            profile.rx_streams
        )));
    }
    Ok(())
}

fn open_bank(wanted: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
    let descriptors = DeviceDescriptors::new().map_err(map_err)?;
    let listed: Vec<_> = descriptors.iter().cloned().collect();
    let unit = unit::units(&listed)
        .into_iter()
        .find(|unit| unit.key == wanted.key)
        .ok_or_else(|| DeviceError::NotFound(wanted.id()))?;
    admit(&unit, wanted)?;
    let mut lanes = Vec::with_capacity(unit.members.len());
    for member in &unit.members {
        lanes.push(descriptors.open(*member).map_err(map_err)?);
    }
    tracing::info!(model = unit.model.name(), key = %unit.key, lanes = lanes.len(), "opened a coherent bank");
    Ok(Box::new(KrakenDevice::new(unit.model, lanes)?))
}

impl BankLane for RtlSdr {
    type Stream = RxStream;
    type Gate = StreamGate;

    fn hold(&mut self) -> Result<(RxStream, StreamGate), DeviceError> {
        self.hold_stream().map_err(map_err)
    }

    fn release(gate: &StreamGate) -> Result<(), DeviceError> {
        gate.release().map_err(map_err)
    }

    fn rehold(gate: &StreamGate) -> Result<(), DeviceError> {
        gate.rehold().map_err(map_err)
    }
}

fn announce(posters: &[MarkPoster], mark: LaneMark) -> Result<(), DeviceError> {
    let mut outcome = Ok(());
    for poster in posters {
        if let Err(error) = poster.post(mark)
            && outcome.is_ok()
        {
            outcome = Err(error);
        }
    }
    outcome
}

pub struct KrakenDevice {
    model: Model,
    lanes: Vec<Arc<Mutex<RtlSdr>>>,
    capabilities: Capabilities,
    lane_capabilities: Capabilities,
    settings: DeviceSettings,
    lane_settings: Vec<DeviceSettings>,
    gain_table: Vec<i32>,
    bank: Option<Bank>,
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
    fn new(model: Model, mut lanes: Vec<RtlSdr>) -> Result<Self, DeviceError> {
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
        switch_off(model, &lanes[0], lanes.len()).map_err(map_err)?;
        let count = lanes.len() as u32;
        let mut device = Self {
            model,
            lanes: lanes
                .into_iter()
                .map(|sdr| Arc::new(Mutex::new(sdr)))
                .collect(),
            capabilities: caps::kraken_capabilities(model, count, &gain_table),
            lane_capabilities: caps::kraken_lane_capabilities(&gain_table),
            settings: DeviceSettings::default(),
            lane_settings,
            gain_table,
            bank: None,
        };
        device.settings.bias_tee = (model == Model::Kraken).then_some(false);
        device.republish();
        Ok(device)
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

    fn apply_gpio(&self, gpio: &[(u8, bool)]) -> Option<DeviceError> {
        let control = lock(&self.lanes[0]);
        let mut failure = None;
        for (pin, on) in gpio {
            if let Err(error) = control.set_gpio(*pin, *on) {
                failure.get_or_insert(map_err(error));
            }
        }
        failure
    }

    fn streaming(&self) -> bool {
        self.bank.as_ref().is_some_and(Bank::is_running)
    }
}

impl SdrDevice for KrakenDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn in_flight_samples(&self) -> u64 {
        IN_FLIGHT_SAMPLES
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        let limits = apply::Limits {
            model: self.model,
            capabilities: &self.capabilities,
            lane_caps: &self.lane_capabilities,
            table: &self.gain_table,
        };
        let plan = apply::plan(settings, &limits, &self.lane_settings)?;
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
        if let Some(error) = self.apply_gpio(&plan.gpio) {
            failure.get_or_insert(error);
        }
        if failure.is_some() {
            self.realign(&before);
        }
        self.republish();
        if let (Some(bank), Some(rate)) = (&self.bank, self.settings.sample_rate) {
            bank.retime(rate);
        }
        if failure.is_none() && plan.bias_tee.is_some() {
            self.settings.bias_tee = plan.bias_tee;
        }
        failure.map_or(Ok(()), Err)
    }

    fn set_noise_source(&mut self, on: bool) -> Result<(), DeviceError> {
        lock(&self.lanes[0])
            .set_gpio(apply::NOISE_SOURCE_PIN, on)
            .map_err(map_err)?;
        let posters = self.bank.as_ref().map_or(&[][..], Bank::posters);
        announce(
            posters,
            LaneMark::NoiseSource {
                on,
                in_flight: self.in_flight_samples(),
            },
        )
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        if self.streaming() {
            return Err(DeviceError::AlreadyStreaming);
        }
        self.rx_stop();
        let config = CaptureConfig::new("sdrmm-kraken-rx", DRIVER_ID)
            .with_sample_rate(self.settings.sample_rate);
        self.bank = Some(Bank::start(
            self.lanes.clone(),
            sinks,
            config,
            convert::converter,
        )?);
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
        if let Some(mut bank) = self.bank.take() {
            bank.stop();
        }
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

fn switch_off(model: Model, control: &RtlSdr, lanes: usize) -> Result<(), crate::driver::Error> {
    control.set_gpio(apply::NOISE_SOURCE_PIN, false)?;
    if model == Model::Kraken {
        for lane in 0..lanes {
            control.set_gpio(apply::bias_tee_pin(lane), false)?;
        }
    }
    Ok(())
}

impl Drop for KrakenDevice {
    fn drop(&mut self) {
        self.rx_stop();
        let Some(control) = self.lanes.first() else {
            return;
        };
        if let Err(error) = switch_off(self.model, &lock(control), self.lanes.len()) {
            tracing::warn!(%error, "the bank's noise source and bias tees may still be on");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, time::Instant};

    use sdrmm_device::LaneEvent;

    use super::{
        bank::tests::{FakeBank, Seen},
        *,
    };

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

    fn found(lanes: &[u32]) -> unit::Unit {
        let descriptors = unit::tests::lanes_behind("0", 3, Some(unit::KRAKEN_HUB), lanes);
        unit::units(&descriptors)
            .into_iter()
            .next()
            .expect("a unit")
    }

    #[test]
    fn an_incomplete_kraken_is_retried_then_not_found() {
        let incomplete = found(&[0, 1, 2, 3]);
        let wanted = info(&incomplete);
        assert_eq!(wanted.label, "KrakenSDR (1004 missing)");
        assert_eq!(
            wanted.profile.as_ref().map(|profile| profile.rx_streams),
            Some(5)
        );
        let settles = Cell::new(0);
        let opens = Cell::new(0);
        let result = with_retries(
            OPEN_ATTEMPTS,
            || settles.set(settles.get() + 1),
            || {
                opens.set(opens.get() + 1);
                admit(&incomplete, &wanted)
            },
        );
        let Err(DeviceError::NotFound(message)) = result else {
            panic!("an incomplete bank must not open, got {result:?}");
        };
        assert!(message.contains("1004 missing"), "{message}");
        assert_eq!(opens.get(), OPEN_ATTEMPTS as usize);
        assert_eq!(settles.get(), OPEN_ATTEMPTS as usize - 1);
    }

    #[test]
    fn a_bank_with_another_lane_count_than_asked_for_is_not_the_one_asked_for() {
        let complete = found(&[0, 1, 2, 3, 4]);
        let mut wanted = info(&complete);
        admit(&complete, &wanted).expect("the unit that was probed");
        if let Some(profile) = wanted.profile.as_mut() {
            profile.rx_streams = 4;
        }
        assert!(matches!(
            admit(&complete, &wanted),
            Err(DeviceError::NotFound(_))
        ));
    }

    #[test]
    fn the_noise_switch_marks_every_lane() {
        let fake = FakeBank::new(5);
        let (mut bank, seen) = fake.start();
        fake.wait_for_samples(&seen, 1);
        let mark = LaneMark::NoiseSource {
            on: true,
            in_flight: IN_FLIGHT_SAMPLES,
        };
        announce(bank.posters(), mark).expect("every lane takes the mark");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let marked = seen
                .iter()
                .filter(|lane| {
                    lock(lane).iter().any(|item| {
                        matches!(item, Seen::Event(LaneEvent::Mark { mark: got, .. }) if *got == mark)
                    })
                })
                .count();
            if marked == seen.len() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{marked} of 5 lanes saw the switch"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        bank.stop();
    }

    #[test]
    fn a_mark_for_a_stopped_bank_is_an_error() {
        let fake = FakeBank::new(2);
        let (mut bank, _seen) = fake.start();
        let posters = bank.posters().to_vec();
        bank.stop();
        drop(bank);
        assert!(announce(&posters, LaneMark::Retuned { in_flight: 0 }).is_err());
    }
}
