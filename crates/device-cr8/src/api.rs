use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use sdrmm_device::DeviceError;

use crate::ffi;

/// How long a failed load is left alone before the library is looked for again, so a machine
/// without the vendor SDK does not pay for a search on every probe.
const RETRY_AFTER: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DevHandle(pub ffi::Handle);

unsafe impl Send for DevHandle {}
unsafe impl Sync for DevHandle {}

/// Everything this driver asks of the CR-8 library, behind a trait so the parts that translate
/// settings can be tested without the vendor library or a radio.
pub trait Cr8Api: Send + Sync {
    fn library_path(&self) -> &Path;
    fn serials(&self) -> Result<Vec<String>, DeviceError>;
    fn open(&self, serial: &str) -> Result<DevHandle, DeviceError>;
    fn close(&self, dev: DevHandle);
    fn versions(&self, dev: DevHandle) -> ffi::DevInfo;
    fn start(
        &self,
        dev: DevHandle,
        buffer: usize,
        callback: ffi::Callback,
        ctx: *mut c_void,
    ) -> Result<(), DeviceError>;
    fn stop(&self, dev: DevHandle) -> Result<(), DeviceError>;
    fn enable(&self, dev: DevHandle, channels: c_int) -> Result<(), DeviceError>;
    fn disable(&self, dev: DevHandle, channels: c_int) -> Result<(), DeviceError>;
    fn set_freq(
        &self,
        dev: DevHandle,
        channels: c_int,
        freq_hz: f64,
        coherent: bool,
    ) -> Result<(), DeviceError>;
    fn set_lna_gain(&self, dev: DevHandle, channels: c_int, gain: i32) -> Result<(), DeviceError>;
    fn set_mixer_gain(&self, dev: DevHandle, channels: c_int, gain: i32)
    -> Result<(), DeviceError>;
    fn set_vga_gain(&self, dev: DevHandle, channels: c_int, gain: i32) -> Result<(), DeviceError>;
    fn set_clock(&self, dev: DevHandle, clock: c_int) -> Result<(), DeviceError>;
}

struct Entries {
    list_devices: ffi::ListDevicesFn,
    free_device_list: ffi::FreeDeviceListFn,
    open: ffi::OpenFn,
    close: ffi::CloseFn,
    get_dev_info: ffi::GetDevInfoFn,
    start: ffi::StartFn,
    stop: ffi::StopFn,
    enable: ffi::ChannelFn,
    disable: ffi::ChannelFn,
    set_freq: ffi::SetFreqFn,
    set_lna_gain: ffi::SetGainFn,
    set_mixer_gain: ffi::SetGainFn,
    set_vga_gain: ffi::SetGainFn,
    set_clock: ffi::SetClockFn,
}

pub struct LoadedApi {
    path: PathBuf,
    entries: Entries,
    _library: libloading::Library,
}

unsafe impl Send for LoadedApi {}
unsafe impl Sync for LoadedApi {}

macro_rules! entry {
    ($library:expr, $name:expr, $signature:ty) => {{
        let symbol: libloading::Symbol<'_, $signature> =
            unsafe { $library.get($name) }.map_err(|error| {
                let name = String::from_utf8_lossy($name);
                format!("{} is missing: {error}", name.trim_end_matches('\0'))
            })?;
        *symbol
    }};
}

impl LoadedApi {
    fn open_library(path: &Path) -> Result<Self, String> {
        let library = unsafe { libloading::Library::new(path) }
            .map_err(|error| format!("{} could not be loaded: {error}", path.display()))?;
        let entries = Entries {
            list_devices: entry!(library, b"dlcr_list_devices\0", ffi::ListDevicesFn),
            free_device_list: entry!(library, b"dlcr_free_device_list\0", ffi::FreeDeviceListFn),
            open: entry!(library, b"dlcr_open\0", ffi::OpenFn),
            close: entry!(library, b"dlcr_close\0", ffi::CloseFn),
            get_dev_info: entry!(library, b"dlcr_get_dev_info\0", ffi::GetDevInfoFn),
            start: entry!(library, b"dlcr_start\0", ffi::StartFn),
            stop: entry!(library, b"dlcr_stop\0", ffi::StopFn),
            enable: entry!(library, b"dlcr_enable_channel\0", ffi::ChannelFn),
            disable: entry!(library, b"dlcr_disable_channel\0", ffi::ChannelFn),
            set_freq: entry!(library, b"dlcr_set_freq\0", ffi::SetFreqFn),
            set_lna_gain: entry!(library, b"dlcr_set_lna_gain\0", ffi::SetGainFn),
            set_mixer_gain: entry!(library, b"dlcr_set_mixer_gain\0", ffi::SetGainFn),
            set_vga_gain: entry!(library, b"dlcr_set_vga_gain\0", ffi::SetGainFn),
            set_clock: entry!(library, b"dlcr_set_clock_source\0", ffi::SetClockFn),
        };
        Ok(Self {
            path: path.to_path_buf(),
            entries,
            _library: library,
        })
    }
}

fn check(code: c_int, action: &str) -> Result<(), DeviceError> {
    if code == 0 {
        return Ok(());
    }
    Err(DeviceError::Io(ffi::message(code, action)))
}

impl Cr8Api for LoadedApi {
    fn library_path(&self) -> &Path {
        &self.path
    }

    fn serials(&self) -> Result<Vec<String>, DeviceError> {
        let mut list: *mut ffi::Info = std::ptr::null_mut();
        let count = unsafe { (self.entries.list_devices)(&raw mut list) };
        if count < 0 {
            return Err(DeviceError::Io(ffi::message(count, "listing CR-8 devices")));
        }
        if list.is_null() {
            return Ok(Vec::new());
        }
        let mut serials = Vec::with_capacity(count as usize);
        for index in 0..count as usize {
            let info = unsafe { &*list.add(index) };
            let serial = unsafe { CStr::from_ptr(info.serial.as_ptr()) }
                .to_string_lossy()
                .trim()
                .to_owned();
            if !serial.is_empty() {
                serials.push(serial);
            }
        }
        unsafe { (self.entries.free_device_list)(list) };
        Ok(serials)
    }

    fn open(&self, serial: &str) -> Result<DevHandle, DeviceError> {
        let wanted =
            CString::new(serial).map_err(|_| DeviceError::NotFound(format!("cr8:{serial}")))?;
        let mut handle: ffi::Handle = std::ptr::null_mut();
        let code =
            unsafe { (self.entries.open)(&raw mut handle, wanted.as_ptr().cast::<c_char>()) };
        check(code, "opening the CR-8")?;
        if handle.is_null() {
            return Err(DeviceError::NotFound(format!("cr8:{serial}")));
        }
        Ok(DevHandle(handle))
    }

    fn close(&self, dev: DevHandle) {
        unsafe { (self.entries.close)(dev.0) };
    }

    fn versions(&self, dev: DevHandle) -> ffi::DevInfo {
        let mut info = ffi::DevInfo::default();
        unsafe { (self.entries.get_dev_info)(dev.0, &raw mut info) };
        info
    }

    fn start(
        &self,
        dev: DevHandle,
        buffer: usize,
        callback: ffi::Callback,
        ctx: *mut c_void,
    ) -> Result<(), DeviceError> {
        check(
            unsafe { (self.entries.start)(dev.0, buffer, callback, ctx) },
            "starting the CR-8 receiver",
        )
    }

    fn stop(&self, dev: DevHandle) -> Result<(), DeviceError> {
        check(
            unsafe { (self.entries.stop)(dev.0) },
            "stopping the CR-8 receiver",
        )
    }

    fn enable(&self, dev: DevHandle, channels: c_int) -> Result<(), DeviceError> {
        check(
            unsafe { (self.entries.enable)(dev.0, channels) },
            "enabling CR-8 channels",
        )
    }

    fn disable(&self, dev: DevHandle, channels: c_int) -> Result<(), DeviceError> {
        check(
            unsafe { (self.entries.disable)(dev.0, channels) },
            "disabling CR-8 channels",
        )
    }

    fn set_freq(
        &self,
        dev: DevHandle,
        channels: c_int,
        freq_hz: f64,
        coherent: bool,
    ) -> Result<(), DeviceError> {
        check(
            unsafe { (self.entries.set_freq)(dev.0, channels, freq_hz, coherent) },
            "tuning the CR-8",
        )
    }

    fn set_lna_gain(&self, dev: DevHandle, channels: c_int, gain: i32) -> Result<(), DeviceError> {
        check(
            unsafe { (self.entries.set_lna_gain)(dev.0, channels, gain) },
            "setting CR-8 LNA gain",
        )
    }

    fn set_mixer_gain(
        &self,
        dev: DevHandle,
        channels: c_int,
        gain: i32,
    ) -> Result<(), DeviceError> {
        check(
            unsafe { (self.entries.set_mixer_gain)(dev.0, channels, gain) },
            "setting CR-8 mixer gain",
        )
    }

    fn set_vga_gain(&self, dev: DevHandle, channels: c_int, gain: i32) -> Result<(), DeviceError> {
        check(
            unsafe { (self.entries.set_vga_gain)(dev.0, channels, gain) },
            "setting CR-8 VGA gain",
        )
    }

    fn set_clock(&self, dev: DevHandle, clock: c_int) -> Result<(), DeviceError> {
        check(
            unsafe { (self.entries.set_clock)(dev.0, clock) },
            "selecting the CR-8 clock source",
        )
    }
}

#[must_use]
pub fn library_candidates() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    let names = ["dlcr.dll", "libdlcr.dll"];
    #[cfg(target_os = "macos")]
    let names = ["libdlcr.dylib", "/usr/local/lib/libdlcr.dylib"];
    #[cfg(all(unix, not(target_os = "macos")))]
    let names = ["libdlcr.so", "/usr/local/lib/libdlcr.so"];
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(override_path) = std::env::var_os("SDRMM_DLCR_LIBRARY") {
        candidates.push(PathBuf::from(override_path));
    }
    candidates.extend(names.iter().map(PathBuf::from));
    candidates
}

#[derive(Default)]
pub struct Loader {
    loaded: Option<Arc<LoadedApi>>,
    failed_at: Option<Instant>,
    error: Option<String>,
}

impl Loader {
    fn get(
        &mut self,
        now: Instant,
        mut open: impl FnMut() -> Result<Arc<LoadedApi>, String>,
    ) -> Option<Arc<LoadedApi>> {
        if let Some(api) = &self.loaded {
            return Some(api.clone());
        }
        if self
            .failed_at
            .is_some_and(|at| now.duration_since(at) < RETRY_AFTER)
        {
            return None;
        }
        match open() {
            Ok(api) => {
                self.loaded = Some(api.clone());
                self.error = None;
                Some(api)
            }
            Err(error) => {
                self.failed_at = Some(now);
                self.error = Some(error);
                None
            }
        }
    }
}

fn load() -> Result<Arc<LoadedApi>, String> {
    let mut failures = Vec::new();
    for candidate in library_candidates() {
        if candidate.is_absolute() && !candidate.exists() {
            continue;
        }
        match LoadedApi::open_library(&candidate) {
            Ok(api) => {
                tracing::info!(library = %api.path.display(), "loaded the CR-8 library");
                return Ok(Arc::new(api));
            }
            Err(error) => failures.push(error),
        }
    }
    Err(if failures.is_empty() {
        "the Dragon Labs CR-8 library was not found".to_owned()
    } else {
        failures.join("; ")
    })
}

static LOADER: Mutex<Option<Loader>> = Mutex::new(None);

/// The library if it is installed, and nothing at all if it is not — a machine without a CR-8
/// simply finds no CR-8, exactly as it finds no SDRplay without that vendor's API.
#[must_use]
pub fn shared() -> Option<Arc<LoadedApi>> {
    let mut guard = LOADER.lock().unwrap_or_else(PoisonError::into_inner);
    guard
        .get_or_insert_with(Loader::default)
        .get(Instant::now(), load)
}

/// Why the library could not be loaded, for the doctor page to repeat.
#[must_use]
pub fn load_error() -> Option<String> {
    let mut guard = LOADER.lock().unwrap_or_else(PoisonError::into_inner);
    let loader = guard.get_or_insert_with(Loader::default);
    let _ = loader.get(Instant::now(), load);
    loader.error.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_are_offered_for_this_platform() {
        assert!(!library_candidates().is_empty());
    }

    #[test]
    fn an_override_is_looked_at_before_the_usual_places() {
        // SAFETY: single-threaded test, and the variable is read again below in the same test.
        unsafe { std::env::set_var("SDRMM_DLCR_LIBRARY", "/tmp/libdlcr.dylib") };
        assert_eq!(
            library_candidates().first(),
            Some(&PathBuf::from("/tmp/libdlcr.dylib"))
        );
        unsafe { std::env::remove_var("SDRMM_DLCR_LIBRARY") };
    }

    #[test]
    fn a_failed_load_is_held_briefly_and_then_tried_again() {
        let attempts = std::cell::Cell::new(0);
        let mut loader = Loader::default();
        let mut open = || {
            attempts.set(attempts.get() + 1);
            Err::<Arc<LoadedApi>, String>("no library".to_owned())
        };
        let start = Instant::now();
        assert!(loader.get(start, &mut open).is_none());
        assert!(
            loader
                .get(start + Duration::from_secs(1), &mut open)
                .is_none()
        );
        assert_eq!(
            attempts.get(),
            1,
            "a missing library is not searched for again at once"
        );
        assert!(loader.get(start + RETRY_AFTER, &mut open).is_none());
        assert_eq!(attempts.get(), 2, "but it is looked for again later");
        assert_eq!(loader.error.as_deref(), Some("no library"));
    }
}
