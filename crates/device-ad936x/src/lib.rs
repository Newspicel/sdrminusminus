use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use sdrmm_device::{
    Capture, CaptureConfig, DeviceDriver, DeviceError, Direction, DuplexState, RxSink, SdrDevice,
    TxStream, lock,
    net::{Adopted, Endpoint},
};
use sdrmm_wire::{
    AgcGain, Capabilities, DeviceInfo, DeviceSettings, Duplex, GainKind, StreamSettings,
};

use crate::{
    caps::{Front, parse_range},
    convert::IqConverter,
    discovery::USB_PREFIX,
    iio::{Client, DEFAULT_PORT, Direction as Way, UsbBus},
    layout::{HARDWAREGAIN, Layout, available},
    rx::{RxRadio, fan_out},
    source::Source,
    tx::Ad936xTx,
};

mod apply;
mod caps;
mod convert;
mod discovery;
mod iio;
mod layout;
mod pace;
mod rx;
mod source;
mod tx;

const DRIVER_ID: &str = "ad936x";

const SINK_BLOCK_SAMPLES: usize = 32_768;

const SWEEP_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub struct Ad936xDriver {
    adopted: Adopted,
    serials: Mutex<BTreeMap<Endpoint, String>>,
    well_known: Vec<Endpoint>,
    swept: Mutex<Option<Instant>>,
}

impl Default for Ad936xDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl Ad936xDriver {
    #[must_use]
    pub fn new() -> Self {
        Self::searching(discovery::WELL_KNOWN.iter().map(|host| host.to_string()))
    }

    #[must_use]
    pub fn searching(hosts: impl IntoIterator<Item = String>) -> Self {
        Self {
            adopted: Adopted::default(),
            serials: Mutex::new(BTreeMap::new()),
            well_known: discovery::endpoints(hosts),
            swept: Mutex::new(None),
        }
    }

    fn endpoints(&self, listed: &[DeviceInfo]) -> Vec<DeviceInfo> {
        let serials = lock(&self.serials);
        let mut known: Vec<String> = listed
            .iter()
            .filter_map(|info| info.serial.clone())
            .collect();
        self.adopted
            .list()
            .into_iter()
            .filter_map(|endpoint| {
                let serial = serials.get(&endpoint).cloned();
                if let Some(serial) = &serial {
                    if known.contains(serial) {
                        return None;
                    }
                    known.push(serial.clone());
                }
                Some(net_info(&endpoint, serial))
            })
            .collect()
    }

    fn remember(&self, endpoint: Endpoint, serial: Option<&str>) {
        if let Some(serial) = serial {
            lock(&self.serials).insert(endpoint, serial.to_string());
        }
    }

    fn due_for_a_sweep(&self) -> bool {
        let mut swept = lock(&self.swept);
        if swept.is_some_and(|at| at.elapsed() < SWEEP_INTERVAL) {
            return false;
        }
        *swept = Some(Instant::now());
        true
    }
}

fn net_info(endpoint: &Endpoint, serial: Option<String>) -> DeviceInfo {
    DeviceInfo {
        driver: DRIVER_ID.to_string(),
        key: endpoint.to_string(),
        label: format!("AD936x {endpoint}"),
        serial,
        profile: None,
    }
}

fn usb_info(radio: &discovery::UsbRadio) -> DeviceInfo {
    DeviceInfo {
        driver: DRIVER_ID.to_string(),
        key: radio.key.clone(),
        label: radio.label.clone(),
        serial: radio.serial.clone(),
        profile: None,
    }
}

impl DeviceDriver for Ad936xDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        let mut found: Vec<DeviceInfo> = discovery::usb_radios().iter().map(usb_info).collect();
        found.extend(self.endpoints(&found));
        found
    }

    fn probe_deep(&self) -> Vec<DeviceInfo> {
        if self.due_for_a_sweep() {
            let known = self.adopted.list();
            let untried: Vec<Endpoint> = self
                .well_known
                .iter()
                .filter(|endpoint| !known.contains(endpoint))
                .cloned()
                .collect();
            for found in discovery::sweep(untried) {
                tracing::info!(endpoint = %found.endpoint, "found an AD936x radio at a well-known address");
                self.adopted.adopt(found.endpoint.clone());
                self.remember(found.endpoint, found.serial.as_deref());
            }
        }
        self.probe()
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        let device = Ad936xDevice::open(source(&info.key)?)?;
        if let Source::Net(endpoint) = &device.source {
            self.remember(endpoint.clone(), device.serial.as_deref());
        }
        Ok(Box::new(device))
    }

    fn resolve(&self, key: &str) -> Option<DeviceInfo> {
        if key.starts_with(USB_PREFIX) {
            return discovery::attached(key).map(|radio| usb_info(&radio));
        }
        let endpoint = Endpoint::parse(key, DEFAULT_PORT)
            .inspect_err(|e| tracing::warn!("ad936x endpoint: {e}"))
            .ok()?;
        if !self.adopted.adopt(endpoint.clone()) {
            tracing::warn!(%endpoint, "too many ad936x endpoints; refusing to adopt another");
            return None;
        }
        let serial = lock(&self.serials).get(&endpoint).cloned();
        Some(net_info(&endpoint, serial))
    }
}

fn split_gains(delta: &DeviceSettings) -> (DeviceSettings, DeviceSettings) {
    let front = DeviceSettings {
        gains: Vec::new(),
        streams: delta
            .streams
            .iter()
            .map(|stream| StreamSettings {
                gains: Vec::new(),
                ..stream.clone()
            })
            .filter(|stream| *stream != lane_only(stream.stream))
            .collect(),
        ..delta.clone()
    };
    let gains = DeviceSettings {
        gains: delta.gains.clone(),
        streams: delta
            .streams
            .iter()
            .filter(|stream| !stream.gains.is_empty())
            .map(|stream| StreamSettings {
                gains: stream.gains.clone(),
                ..lane_only(stream.stream)
            })
            .collect(),
        ..DeviceSettings::default()
    };
    (front, gains)
}

fn lane_only(stream: u32) -> StreamSettings {
    StreamSettings {
        stream,
        ..StreamSettings::default()
    }
}

fn source(key: &str) -> Result<Source, DeviceError> {
    if key.starts_with(USB_PREFIX) {
        let info = discovery::find_usb(key)?;
        return Ok(Source::Usb(UsbBus::open(&info)?));
    }
    Ok(Source::Net(Endpoint::parse(key, DEFAULT_PORT)?))
}

pub struct Ad936xDevice {
    client: Arc<Client>,
    source: Source,
    serial: Option<String>,
    layout: Layout,
    front: Front,
    capabilities: Capabilities,
    settings: DeviceSettings,
    duplex: Arc<Mutex<DuplexState>>,
    capture: Capture<RxRadio>,
}

impl Ad936xDevice {
    fn open(source: Source) -> Result<Self, DeviceError> {
        let client = Client::new(source.open()?);
        match client.version() {
            Ok(version) => tracing::debug!(%version, "iiod answered"),
            Err(e) => tracing::debug!("iiod version: {e}"),
        }
        let context = client.context()?;
        let layout = Layout::read(&context)?;
        let front = Front::read(&client, &context, &layout)?;
        let mut capabilities = caps::capabilities(&front, &layout);
        if capabilities.duplex == Duplex::Full && !source.full_duplex() {
            capabilities.duplex = Duplex::Half;
        }
        let mut settings = apply::read_settings(&client, &capabilities, &front, &layout);
        settings.rx_streams =
            (!capabilities.rx_stream_choices.is_empty()).then_some(capabilities.rx_streams);
        tracing::info!(
            radio = context
                .attribute("hw_model")
                .unwrap_or(context.description.as_str()),
            at = %source,
            rx = capabilities.rx_streams,
            tx = capabilities.tx_streams,
            "opened an AD936x radio"
        );
        Ok(Self {
            duplex: Arc::new(Mutex::new(DuplexState::new(capabilities.duplex))),
            client: Arc::new(client),
            source,
            serial: context.attribute("hw_serial").map(str::to_string),
            layout,
            front,
            capabilities,
            settings,
            capture: Capture::new(),
        })
    }

    fn apply_planned(&mut self, delta: &DeviceSettings) -> Result<(), DeviceError> {
        let (next, writes) = apply::plan(
            delta,
            &self.capabilities,
            &self.front,
            &self.layout,
            &self.settings,
        )?;
        match apply::execute(&self.client, &self.layout.phy, &writes) {
            Ok(()) => {
                self.settings = next;
                Ok(())
            }
            Err(e) => {
                self.reread();
                Err(e)
            }
        }
    }

    fn reread(&mut self) {
        let held =
            apply::read_settings(&self.client, &self.capabilities, &self.front, &self.layout);
        self.settings.merge_from(&held);
    }

    fn follow_band(&mut self) {
        let Some(port) = self.layout.port(false, 0) else {
            return;
        };
        let range = self
            .client
            .read_channel_attr(&self.layout.phy, Way::In, port, &available(HARDWAREGAIN))
            .inspect_err(|e| tracing::debug!("{port} gain range: {e}"))
            .ok()
            .and_then(|text| parse_range(&text));
        let Some(stage) = self
            .capabilities
            .gains
            .iter_mut()
            .find(|stage| stage.kind == GainKind::Tuner)
        else {
            return;
        };
        if let Some(range) = range.filter(|range| *range != stage.range) {
            stage.range = range;
            self.front.rx_gain = range;
            self.reread();
        }
    }

    fn read_gain(&self, lane: usize) -> Result<f64, DeviceError> {
        let port = self.layout.port(false, lane).ok_or_else(|| {
            DeviceError::Unsupported(format!("this radio has no receive lane {lane}"))
        })?;
        let text = self
            .client
            .read_channel_attr(&self.layout.phy, Way::In, port, HARDWAREGAIN)?;
        text.split_whitespace()
            .next()
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| DeviceError::Io(format!("{port} gain reads {text:?}")))
    }

    fn stream_lanes(&mut self, lanes: u32) -> Result<(), DeviceError> {
        if lanes == self.capabilities.rx_streams {
            return Ok(());
        }
        if !self.capabilities.rx_stream_choices.contains(&lanes) {
            return Err(DeviceError::Unsupported(format!(
                "this radio streams {:?} lanes, got {lanes}",
                self.capabilities.rx_stream_choices
            )));
        }
        caps::stream_lanes(&mut self.capabilities, lanes);
        self.settings.rx_streams = Some(lanes);
        self.settings.streams.retain(|lane| lane.stream < lanes);
        self.reread();
        Ok(())
    }

    fn lanes(&self, output: bool, wanted: usize) -> Result<usize, DeviceError> {
        let have = if output {
            self.layout.tx_streams()
        } else {
            self.capabilities.rx_streams as usize
        };
        if wanted == 0 || wanted > have {
            return Err(DeviceError::Unsupported(format!(
                "this radio has {have} {} streams, got {wanted}",
                if output { "tx" } else { "rx" }
            )));
        }
        Ok(wanted)
    }
}

impl SdrDevice for Ad936xDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn in_flight_samples(&self) -> u64 {
        self.layout.rx.as_ref().map_or(0, |stream| {
            rx::in_flight_samples(
                self.settings.sample_rate.unwrap_or(0.0),
                stream.sample_bytes(self.layout.rx_streams()),
            )
        })
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        if let Some(lanes) = settings.rx_streams {
            self.stream_lanes(lanes)?;
        }
        let (front, gains) = split_gains(settings);
        self.apply_planned(&front)?;
        if settings.center_hz.is_some() {
            self.follow_band();
        }
        if gains != DeviceSettings::default() {
            self.apply_planned(&gains)?;
        }
        Ok(())
    }

    fn agc_gains(&self) -> Result<Vec<AgcGain>, DeviceError> {
        let mut gains = Vec::new();
        for lane in 0..self.capabilities.rx_streams as usize {
            let resolved = self
                .settings
                .for_stream(lane as u32, &self.capabilities.per_stream);
            if resolved.agc.as_ref().is_some_and(|agc| agc.on) {
                gains.push(AgcGain {
                    stream: lane as u32,
                    value_db: self.read_gain(lane)?,
                });
            }
        }
        Ok(gains)
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let lanes = self.lanes(false, sinks.len())?;
        let stream =
            self.layout.rx.clone().ok_or_else(|| {
                DeviceError::Unsupported("this radio does not receive".to_string())
            })?;
        let format = stream.format;
        lock(&self.duplex).claim(Direction::Rx)?;
        let radio = Arc::new(RxRadio::new(
            self.source.clone(),
            stream,
            lanes,
            self.settings.sample_rate,
        ));
        let converter = IqConverter::new(format, radio.buffer_samples() * lanes);
        let started = self.capture.start(
            radio,
            converter,
            fan_out(sinks, SINK_BLOCK_SAMPLES),
            CaptureConfig {
                block_samples: SINK_BLOCK_SAMPLES * lanes,
                ..CaptureConfig::new("sdrmm-ad936x-rx", DRIVER_ID)
                    .with_sample_rate(self.settings.sample_rate.map(|rate| rate * lanes as f64))
            },
        );
        if started.is_err() {
            lock(&self.duplex).release(Direction::Rx);
        }
        started
    }

    fn rx_stop(&mut self) {
        self.capture.stop();
        lock(&self.duplex).release(Direction::Rx);
    }

    fn tx_start_channels(&mut self, channels: &[u32]) -> Result<Box<dyn TxStream>, DeviceError> {
        let lanes = self.lanes(true, channels.len())?;
        if channels.iter().copied().ne(0..lanes as u32) {
            return Err(DeviceError::Unsupported(format!(
                "this radio transmits on its lanes in order, got channels {channels:?}"
            )));
        }
        let stream =
            self.layout.tx.clone().ok_or_else(|| {
                DeviceError::Unsupported("this radio does not transmit".to_string())
            })?;
        let samples = rx::buffer_samples(
            self.settings.sample_rate.unwrap_or(0.0),
            stream.sample_bytes(lanes),
        );
        lock(&self.duplex).claim(Direction::Tx)?;
        match Ad936xTx::open(&self.source, &stream, lanes, samples, self.duplex.clone()) {
            Ok(stream) => Ok(Box::new(stream)),
            Err(e) => {
                lock(&self.duplex).release(Direction::Tx);
                Err(e)
            }
        }
    }
}

impl Drop for Ad936xDevice {
    fn drop(&mut self) {
        self.capture.stop();
        self.client.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_driver_offers_nothing_until_it_finds_or_is_told_about_a_radio() {
        let driver = Ad936xDriver::new();
        assert_eq!(driver.id(), DRIVER_ID);
        assert!(
            driver
                .probe()
                .iter()
                .all(|found| found.key.starts_with(USB_PREFIX)),
            "only attached radios appear before an address is given"
        );
    }

    #[test]
    fn an_address_is_adopted_and_then_listed_under_the_iiod_port() {
        let driver = Ad936xDriver::new();
        let info = driver.resolve("192.168.1.10").expect("addressable");
        assert_eq!(info.id(), "ad936x:192.168.1.10:30431");
        assert_eq!(info.label, "AD936x 192.168.1.10:30431");
        assert!(driver.probe().contains(&info));
        assert_eq!(
            driver.resolve("192.168.1.10:30431"),
            Some(info),
            "two spellings of one address are one radio"
        );
    }

    #[test]
    fn a_key_that_addresses_nothing_is_not_adopted() {
        let driver = Ad936xDriver::new();
        assert!(driver.resolve("192.168.1.10:not-a-port").is_none());
        assert!(driver.resolve("usb-nothing-is-plugged-in-here").is_none());
    }

    #[test]
    fn two_addresses_that_answered_with_one_serial_are_listed_as_one_radio() {
        let driver = Ad936xDriver::searching([]);
        let by_name = driver.resolve("pluto.local").expect("addressable");
        let by_address = driver.resolve("192.168.2.1").expect("addressable");
        driver.remember(
            Endpoint::parse("pluto.local", DEFAULT_PORT).expect("endpoint"),
            Some("1044734c960500111e002e0041984fc267"),
        );
        driver.remember(
            Endpoint::parse("192.168.2.1", DEFAULT_PORT).expect("endpoint"),
            Some("1044734c960500111e002e0041984fc267"),
        );
        let listed = driver.probe();
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert_eq!(
            listed[0].serial.as_deref(),
            Some("1044734c960500111e002e0041984fc267")
        );
        assert!(
            driver.resolve(&by_name.key).is_some() && driver.resolve(&by_address.key).is_some(),
            "either spelling still opens the radio"
        );
    }

    #[test]
    fn gains_are_set_apart_from_the_rest_so_they_land_in_the_new_bands_limits() {
        let delta = DeviceSettings {
            center_hz: Some(5.8e9),
            gains: vec![sdrmm_wire::GainValue::new(GainKind::Tuner, 70.0)],
            streams: vec![
                StreamSettings {
                    stream: 1,
                    gains: vec![sdrmm_wire::GainValue::new(GainKind::Tuner, 60.0)],
                    ..lane_only(1)
                },
                StreamSettings {
                    agc: Some(sdrmm_wire::AgcSetting::off()),
                    ..lane_only(0)
                },
            ],
            ..DeviceSettings::default()
        };
        let (front, gains) = split_gains(&delta);
        assert_eq!(front.center_hz, Some(5.8e9));
        assert!(front.gains.is_empty());
        assert_eq!(
            front.streams.len(),
            1,
            "a lane left with nothing is dropped"
        );
        assert_eq!(front.streams[0].stream, 0);
        assert_eq!(gains.center_hz, None);
        assert_eq!(gains.gains, delta.gains);
        assert_eq!(gains.streams.len(), 1);
        assert_eq!(gains.streams[0].gains[0].value_db, 60.0);
        assert_eq!(gains.streams[0].agc, None);
    }

    #[test]
    fn a_search_leaves_the_network_alone_for_a_while_after_trying_it() {
        let driver = Ad936xDriver::searching([]);
        assert!(driver.due_for_a_sweep());
        assert!(!driver.due_for_a_sweep(), "one search per interval");
    }

    #[test]
    fn a_key_says_which_way_the_radio_is_reached() {
        assert!(matches!(
            source("192.168.1.10").expect("an address"),
            Source::Net(_)
        ));
        assert!(
            source("usb-0000deadbeef").is_err(),
            "a usb key names a radio that has to be attached"
        );
    }
}
