use std::sync::{Arc, Mutex, MutexGuard};

use caps::{GainMode, Plan};
use dongle::{Catalog, Dongle, Listing};
use sdrmm_device::{
    Capture, CaptureConfig, CaptureRadio, DeviceDriver, DeviceError, RxSink, SdrDevice, lock,
    single_rx_sink,
};
use sdrmm_usb_stream::RxStream;
use sdrmm_wire::{
    AgcGain, AgcSetting, BandwidthSetting, Capabilities, DeviceInfo, DeviceSettings, ExtraValue,
};

mod caps;
mod convert;
mod dongle;
mod kraken;

pub use kraken::KrakenDriver;

pub(crate) const DRIVER_ID: &str = "rtlsdr";

const DEFAULT_SAMPLE_RATE_HZ: u32 = 2_048_000;
pub(crate) const DEFAULT_CENTER_HZ: u32 = 100_000_000;

fn enumerate() -> Result<Vec<Listing>, dongle::Error> {
    Ok(Catalog::scan()?.listings().cloned().collect())
}

fn standalone() -> Result<Vec<Listing>, dongle::Error> {
    Ok(without_banks(enumerate()?))
}

fn without_banks(attached: Vec<Listing>) -> Vec<Listing> {
    let claimed = kraken::claimed(&attached);
    attached
        .into_iter()
        .enumerate()
        .filter(|(position, _)| !claimed.contains(position))
        .map(|(_, listing)| listing)
        .collect()
}

#[derive(Default)]
pub struct RtlSdrDriver {
    written: Mutex<Vec<String>>,
}

impl RtlSdrDriver {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl DeviceDriver for RtlSdrDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        match standalone() {
            Ok(descriptors) => caps::device_infos(&descriptors),
            Err(e) => {
                tracing::warn!("rtlsdr enumerate failed: {e}");
                Vec::new()
            }
        }
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        let descriptors = without_banks(scanned()?);
        let index = located(&descriptors, &info.key)
            .ok_or_else(|| DeviceError::NotFound(info.id()))?
            .index;
        let dongle = Dongle::open(index)?;
        Ok(Box::new(RtlSdrDevice::from_dongle(dongle)?))
    }

    fn write_serial(&self, key: &str, serial: Option<&str>) -> Result<String, DeviceError> {
        let attached = scanned()?;
        let descriptors = without_banks(attached.clone());
        let target = located(&descriptors, key)
            .ok_or_else(|| DeviceError::NotFound(format!("{DRIVER_ID}:{key}")))?;
        let mut written = lock(&self.written);
        let taken: Vec<&str> = attached
            .iter()
            .filter(|listing| listing.index != target.index)
            .filter_map(|listing| listing.serial.as_deref())
            .chain(written.iter().map(String::as_str))
            .collect();
        let serial = chosen_serial(serial, &taken)?;
        Dongle::open(target.index)?.write_serial(&serial)?;
        tracing::info!(key, serial, "wrote RTL-SDR serial");
        written.push(serial.clone());
        Ok(serial)
    }
}

fn scanned() -> Result<Vec<Listing>, DeviceError> {
    enumerate().map_err(|e| DeviceError::Io(format!("rtlsdr enumerate: {e}")))
}

fn located<'a>(descriptors: &'a [Listing], key: &str) -> Option<&'a Listing> {
    let position = caps::device_infos(descriptors)
        .iter()
        .position(|probed| probed.key == key)?;
    descriptors.get(position)
}

fn chosen_serial(wanted: Option<&str>, taken: &[&str]) -> Result<String, DeviceError> {
    match wanted {
        Some(serial) if !dongle::valid_serial(serial) => Err(DeviceError::Unsupported(
            "a serial is 1 to 16 letters or digits".to_owned(),
        )),
        Some(serial) if taken.contains(&serial) => Err(DeviceError::Unsupported(format!(
            "another RTL-SDR here already has serial {serial}"
        ))),
        Some(serial) => Ok(serial.to_owned()),
        None => random_serial(taken, || {
            getrandom::u32().map_err(|e| DeviceError::Io(format!("random serial: {e}")))
        }),
    }
}

const FACTORY_SERIALS: [&str; 2] = ["00000000", "00000001"];
const SERIAL_DRAWS: usize = 16;

fn random_serial(
    taken: &[&str],
    mut draw: impl FnMut() -> Result<u32, DeviceError>,
) -> Result<String, DeviceError> {
    for _ in 0..SERIAL_DRAWS {
        let serial = format!("{:08}", draw()? % 100_000_000);
        if !taken.contains(&serial.as_str()) && !FACTORY_SERIALS.contains(&serial.as_str()) {
            return Ok(serial);
        }
    }
    Err(DeviceError::Io("no free random serial".to_owned()))
}

struct RtlRadio {
    dongle: Mutex<Dongle>,
}

impl RtlRadio {
    fn lock(&self) -> MutexGuard<'_, Dongle> {
        lock(&self.dongle)
    }
}

impl CaptureRadio for RtlRadio {
    type Stream = RxStream;

    fn arm(&self) -> Result<RxStream, DeviceError> {
        Ok(self.lock().start_stream()?)
    }
}

pub struct RtlSdrDevice {
    radio: Arc<RtlRadio>,
    capabilities: Capabilities,
    settings: DeviceSettings,
    gain_table: Vec<i32>,
    capture: Capture<RtlRadio>,
}

impl RtlSdrDevice {
    fn from_dongle(mut dongle: Dongle) -> Result<Self, DeviceError> {
        let capabilities = caps::capabilities(dongle.board(), dongle.gain_table());
        let gain_table = dongle.gain_table().to_vec();
        tracing::info!(
            tuner = ?dongle.tuner_kind(),
            board = ?dongle.board(),
            gain_steps = gain_table.len(),
            "opened rtlsdr device"
        );

        dongle.set_sample_rate(DEFAULT_SAMPLE_RATE_HZ)?;
        dongle.set_center(DEFAULT_CENTER_HZ)?;
        dongle.set_auto_gain()?;
        let bias_tee = dongle.bias_tee_at_start();
        dongle.set_bias_tee(bias_tee)?;

        let extra = (!dongle.board().has_upconverter())
            .then(|| ExtraValue {
                name: caps::DIRECT_SAMPLING.to_string(),
                value: dongle.direct_sampling().wire_name().into(),
            })
            .into_iter()
            .collect();

        let settings = DeviceSettings {
            center_hz: dongle.center_hz().map(f64::from),
            sample_rate: Some(f64::from(dongle.sample_rate())),
            ppm: Some(f64::from(dongle.ppm())),
            antenna: Some("RX".to_string()),
            bandwidth: Some(BandwidthSetting::Auto),
            bias_tee: Some(bias_tee),
            agc: Some(AgcSetting::switched(true)),
            extra,
            ..DeviceSettings::default()
        };

        Ok(Self {
            radio: Arc::new(RtlRadio {
                dongle: Mutex::new(dongle),
            }),
            capabilities,
            settings,
            gain_table,
            capture: Capture::new(),
        })
    }
}

fn apply_to_hardware(dongle: &mut Dongle, plan: &Plan) -> Result<(), DeviceError> {
    if let Some(mode) = plan.direct_sampling {
        dongle.set_direct_sampling(mode)?;
    }
    if let Some(rate) = plan.sample_rate {
        dongle.set_sample_rate(rate)?;
    }
    if let Some(hz) = plan.center_hz {
        dongle.set_center(hz)?;
    }
    if let Some(bandwidth) = plan.bandwidth {
        dongle.set_bandwidth(bandwidth)?;
    }
    if let Some(ppm) = plan.ppm {
        dongle.set_ppm(ppm)?;
    }
    match plan.gain {
        Some(GainMode::Auto) => dongle.set_auto_gain()?,
        Some(GainMode::Manual(tenths)) => dongle.set_manual_gain(tenths)?,
        None => {}
    }
    if let Some(on) = plan.bias_tee {
        dongle.set_bias_tee(on)?;
    }
    Ok(())
}

impl SdrDevice for RtlSdrDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        let plan = caps::validate(
            settings,
            &self.capabilities,
            &self.settings,
            &self.gain_table,
        )?;
        let (result, center_hz, sample_rate, ppm) = {
            let mut dongle = self.radio.lock();
            let result = apply_to_hardware(&mut dongle, &plan);
            (
                result,
                dongle.center_hz(),
                dongle.sample_rate(),
                dongle.ppm(),
            )
        };
        self.settings.center_hz = center_hz.map(f64::from);
        self.settings.sample_rate = Some(f64::from(sample_rate));
        self.settings.ppm = Some(f64::from(ppm));
        result?;
        self.settings.merge_from(&plan.applied);
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        self.capture.start(
            self.radio.clone(),
            convert::converter(),
            single_rx_sink(sinks)?,
            CaptureConfig::new("sdrmm-rtlsdr-rx", DRIVER_ID)
                .with_sample_rate(self.settings.sample_rate),
        )
    }

    fn rx_stop(&mut self) {
        self.capture.stop();
    }

    fn agc_gains(&self) -> Result<Vec<AgcGain>, DeviceError> {
        if !self.settings.agc.as_ref().is_some_and(|agc| agc.on) {
            return Ok(Vec::new());
        }
        let tenths = self.radio.lock().measured_gain()?;
        Ok(vec![AgcGain {
            stream: 0,
            value_db: f64::from(tenths) / 10.0,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dongle::Board;

    fn dongle(serial: &str, port: u8, hub: Option<u8>) -> Listing {
        Listing {
            index: usize::from(port),
            bus: "001".to_string(),
            address: port,
            manufacturer: None,
            product: None,
            serial: Some(serial.to_string()),
            port_chain: hub.map_or_else(|| vec![port], |hub| vec![hub, port]),
            board: Board::Generic,
            hub: None,
        }
    }

    #[test]
    fn a_banks_dongles_are_not_offered_as_radios_of_their_own() {
        let mut attached: Vec<Listing> = (0..5)
            .map(|lane| dongle(&(1000 + lane).to_string(), lane as u8 + 1, Some(4)))
            .collect();
        attached.push(dongle("00000123", 9, None));
        let standalone = without_banks(attached);
        assert_eq!(standalone.len(), 1);
        assert_eq!(standalone[0].serial.as_deref(), Some("00000123"));
    }

    #[test]
    fn ordinary_dongles_are_all_offered() {
        let attached = vec![dongle("00000123", 1, None), dongle("00000124", 2, None)];
        assert_eq!(without_banks(attached).len(), 2);
    }

    fn draws(values: &[u32]) -> impl FnMut() -> Result<u32, DeviceError> + '_ {
        let mut next = values.iter();
        move || {
            next.next()
                .copied()
                .ok_or(DeviceError::Io("drained".to_owned()))
        }
    }

    #[test]
    fn a_random_serial_is_eight_digits() {
        assert_eq!(random_serial(&[], draws(&[42])).unwrap(), "00000042");
        assert_eq!(random_serial(&[], draws(&[u32::MAX])).unwrap(), "94967295");
    }

    #[test]
    fn a_random_serial_skips_factory_and_taken_ones() {
        assert_eq!(
            random_serial(&["00000042"], draws(&[100_000_000, 1, 42, 7])).unwrap(),
            "00000007"
        );
        assert!(random_serial(&[], draws(&[1; SERIAL_DRAWS])).is_err());
    }

    #[test]
    fn a_chosen_serial_must_be_valid_and_free() {
        assert_eq!(chosen_serial(Some("Roof"), &["00000001"]).unwrap(), "Roof");
        let random = chosen_serial(None, &[]).unwrap();
        assert!(
            dongle::valid_serial(&random) && random.len() == 8,
            "{random}"
        );
        for wanted in ["00000001", "has space", ""] {
            assert!(matches!(
                chosen_serial(Some(wanted), &["00000001"]),
                Err(DeviceError::Unsupported(_))
            ));
        }
    }

    #[test]
    fn a_serialless_dongle_is_found_by_its_location() {
        let mut bare = dongle("x", 3, None);
        bare.serial = None;
        let attached = vec![dongle("00000123", 1, None), bare];
        assert_eq!(located(&attached, "001/3").map(|l| l.index), Some(3));
        assert_eq!(located(&attached, "00000123").map(|l| l.index), Some(1));
        assert!(located(&attached, "001/9").is_none());
    }

    #[test]
    fn driver_id_is_the_wire_id() {
        assert_eq!(RtlSdrDriver::new().id(), "rtlsdr");
    }
}
