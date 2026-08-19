use std::{
    ffi::{c_int, c_void},
    sync::{Arc, Mutex, PoisonError},
};

use sdrmm_device::{DeviceDriver, DeviceError, RxSink, SdrDevice, check_stream_settings};
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings};

mod api;
mod caps;
mod ffi;
mod settings;

pub use api::{Cr8Api, DevHandle, library_candidates, load_error, shared};
pub use caps::{CLOCK_EXTERNAL, CLOCK_INTERNAL, CLOCK_SETTING, capabilities, profile};

pub const DRIVER_ID: &str = "cr8";

/// How many samples per channel the library is asked to hand over at a time. At the CR-8's fixed
/// 12.5 MS/s this is a little over five milliseconds, short enough that the engine's rings never
/// see a step change and long enough that eight channels do not thrash the callback.
const BUFFER_SAMPLES: usize = 65_536;

pub struct Cr8Driver {
    api: Option<Arc<dyn Cr8Api>>,
}

impl Default for Cr8Driver {
    fn default() -> Self {
        Self::new()
    }
}

impl Cr8Driver {
    #[must_use]
    pub fn new() -> Self {
        Self { api: None }
    }

    /// A driver over a given API, which is how the translation is tested without the vendor
    /// library or a radio on the bench.
    #[must_use]
    pub fn with_api(api: Arc<dyn Cr8Api>) -> Self {
        Self { api: Some(api) }
    }

    fn api(&self) -> Option<Arc<dyn Cr8Api>> {
        match &self.api {
            Some(api) => Some(api.clone()),
            None => shared().map(|loaded| loaded as Arc<dyn Cr8Api>),
        }
    }
}

fn info(serial: &str) -> DeviceInfo {
    DeviceInfo {
        driver: DRIVER_ID.to_owned(),
        key: serial.to_owned(),
        label: format!("Dragon Labs CR-8 {serial}"),
        serial: Some(serial.to_owned()),
        profile: Some(caps::profile()),
    }
}

impl DeviceDriver for Cr8Driver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        let Some(api) = self.api() else {
            return Vec::new();
        };
        match api.serials() {
            Ok(serials) => serials.iter().map(|serial| info(serial)).collect(),
            Err(error) => {
                tracing::warn!(%error, "could not list CR-8 devices");
                Vec::new()
            }
        }
    }

    fn open(&self, wanted: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        let api = self
            .api()
            .ok_or_else(|| DeviceError::NotFound(wanted.id()))?;
        let handle = api.open(&wanted.key)?;
        let versions = api.versions(handle);
        tracing::info!(
            serial = %wanted.key,
            hardware = format!("{}.{}", versions.hw_ver_major, versions.hw_ver_minor),
            firmware = format!(
                "{}.{}.{}",
                versions.fw_ver_major, versions.fw_ver_minor, versions.fw_ver_build
            ),
            "opened a CR-8"
        );
        Ok(Box::new(Cr8Device::new(api, handle)))
    }
}

/// The lanes a running receiver is delivering to, shared with the vendor library's callback
/// thread. Nothing else touches it while the receiver runs.
struct Lanes {
    sinks: Vec<RxSink>,
}

pub struct Cr8Device {
    api: Arc<dyn Cr8Api>,
    handle: DevHandle,
    capabilities: Capabilities,
    settings: DeviceSettings,
    lanes: Option<Arc<Mutex<Lanes>>>,
}

impl Cr8Device {
    fn new(api: Arc<dyn Cr8Api>, handle: DevHandle) -> Self {
        Self {
            api,
            handle,
            capabilities: caps::capabilities(),
            settings: DeviceSettings {
                sample_rate: Some(ffi::SAMPLE_RATE_HZ),
                ..DeviceSettings::default()
            },
            lanes: None,
        }
    }
}

/// Hands one buffer of every channel to the lane that owns it.
///
/// `drops` counts the samples the library could not deliver before this buffer. Stepping each
/// lane over them keeps the eight streams on one timeline, which is the whole reason a coherent
/// radio is worth having.
unsafe extern "C" fn deliver(
    samples: *mut *mut ffi::Complex,
    count: usize,
    drops: usize,
    ctx: *mut c_void,
) {
    if ctx.is_null() || samples.is_null() {
        return;
    }
    let lanes = unsafe { &*ctx.cast::<Mutex<Lanes>>() };
    let mut held = lanes.lock().unwrap_or_else(PoisonError::into_inner);
    for (lane, sink) in held.sinks.iter_mut().enumerate() {
        if drops > 0 {
            sink.dropped(drops as u64);
        }
        let channel = unsafe { *samples.add(lane) };
        if channel.is_null() || count == 0 {
            continue;
        }
        let block = unsafe {
            std::slice::from_raw_parts(channel.cast::<num_complex::Complex<f32>>(), count)
        };
        sink.push(block);
    }
}

impl SdrDevice for Cr8Device {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        check_stream_settings(settings, &self.capabilities)?;
        let plan = settings::plan(settings, &self.settings, &self.capabilities)?;
        for step in &plan {
            step.run(self.api.as_ref(), self.handle)?;
        }
        self.settings.merge_from(settings);
        self.settings.sample_rate = Some(ffi::SAMPLE_RATE_HZ);
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
        self.api.enable(self.handle, ffi::CHAN_ALL)?;
        let lanes = Arc::new(Mutex::new(Lanes { sinks }));
        let ctx = Arc::as_ptr(&lanes).cast::<c_void>().cast_mut();
        self.lanes = Some(lanes);
        let started = self.api.start(self.handle, BUFFER_SAMPLES, deliver, ctx);
        if started.is_err() {
            self.lanes = None;
        }
        started
    }

    fn rx_stop(&mut self) {
        if let Err(error) = self.api.stop(self.handle) {
            tracing::warn!(%error, "the CR-8 did not stop cleanly");
        }
        let _ = self.api.disable(self.handle, ffi::CHAN_ALL);
        self.lanes = None;
    }
}

impl Drop for Cr8Device {
    fn drop(&mut self) {
        if self.lanes.is_some() {
            self.rx_stop();
        }
        self.api.close(self.handle);
    }
}

#[must_use]
pub fn channel_mask(lane: usize) -> c_int {
    if lane >= ffi::CHANNEL_COUNT {
        return 0;
    }
    1 << lane
}

#[cfg(test)]
mod tests;
