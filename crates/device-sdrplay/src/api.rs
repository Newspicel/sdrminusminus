use std::{
    ffi::{c_char, c_float, c_int, c_uint, c_void},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use sdrmm_device::DeviceError;

use crate::ffi;

pub const MIN_API_VERSION: f32 = 3.15;
pub const MAX_API_VERSION: f32 = 4.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitOutcome {
    Started,
    MasterPending,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DevHandle(pub ffi::Handle);

unsafe impl Send for DevHandle {}
unsafe impl Sync for DevHandle {}

pub trait Sdrplay: Send + Sync {
    fn version(&self) -> f32;
    fn library_path(&self) -> &Path;
    fn get_devices(&self) -> Result<Vec<ffi::DeviceT>, DeviceError>;
    fn select_device(&self, device: &mut ffi::DeviceT) -> Result<(), DeviceError>;
    fn release_device(&self, device: &mut ffi::DeviceT) -> Result<(), DeviceError>;
    fn device_params(&self, dev: DevHandle) -> Result<*mut ffi::DeviceParamsT, DeviceError>;
    fn init(
        &self,
        dev: DevHandle,
        callbacks: &ffi::CallbackFnsT,
        context: *mut c_void,
    ) -> Result<InitOutcome, DeviceError>;
    fn uninit(&self, dev: DevHandle) -> Result<(), DeviceError>;
    fn update(
        &self,
        dev: DevHandle,
        tuner: c_int,
        reason: c_uint,
        ext1: c_uint,
    ) -> Result<(), DeviceError>;
}

pub fn error_message(code: ffi::ErrT) -> &'static str {
    match code {
        ffi::SUCCESS => "success",
        ffi::FAIL => "the API rejected the request",
        2 => "invalid parameter",
        ffi::OUT_OF_RANGE => "value out of range",
        4 => "gain update failed",
        5 => "frequency update failed",
        6 => "sample rate update failed",
        ffi::HW_ERROR => "hardware error",
        8 => "the requested settings would alias",
        ffi::ALREADY_INITIALISED => "the device is already initialised",
        ffi::NOT_INITIALISED => "the device is not initialised",
        11 => "not enabled",
        ffi::HW_VER_ERROR => "unsupported hardware version",
        13 => "out of memory",
        ffi::SERVICE_NOT_RESPONDING => {
            "the SDRplay API service is not running — start sdrplay_apiService and retry"
        }
        ffi::START_PENDING => "waiting for the RSPduo master application to start",
        16 => "stop pending",
        ffi::INVALID_MODE => "invalid mode for this device",
        18..=23 => "the API failed to verify the device",
        ffi::INVALID_SERVICE_VERSION => {
            "the installed SDRplay library and service are different versions — reinstall the API"
        }
        _ => "unknown SDRplay error",
    }
}

pub fn check(code: ffi::ErrT, action: &str) -> Result<(), DeviceError> {
    if code == ffi::SUCCESS {
        return Ok(());
    }
    let message = format!("{action}: {} (code {code})", error_message(code));
    match code {
        ffi::HW_VER_ERROR | ffi::INVALID_MODE => Err(DeviceError::Unsupported(message)),
        ffi::ALREADY_INITIALISED => Err(DeviceError::InUse(message)),
        _ => Err(DeviceError::Io(message)),
    }
}

pub fn claim_error(error: DeviceError) -> DeviceError {
    match error {
        DeviceError::Io(message) => DeviceError::InUse(message),
        other => other,
    }
}

type OpenFn = unsafe extern "C" fn() -> ffi::ErrT;
type CloseFn = unsafe extern "C" fn() -> ffi::ErrT;
type ApiVersionFn = unsafe extern "C" fn(*mut c_float) -> ffi::ErrT;
type LockFn = unsafe extern "C" fn() -> ffi::ErrT;
type UnlockFn = unsafe extern "C" fn() -> ffi::ErrT;
type GetDevicesFn = unsafe extern "C" fn(*mut ffi::DeviceT, *mut c_uint, c_uint) -> ffi::ErrT;
type SelectDeviceFn = unsafe extern "C" fn(*mut ffi::DeviceT) -> ffi::ErrT;
type ReleaseDeviceFn = unsafe extern "C" fn(*mut ffi::DeviceT) -> ffi::ErrT;
type GetDeviceParamsFn =
    unsafe extern "C" fn(ffi::Handle, *mut *mut ffi::DeviceParamsT) -> ffi::ErrT;
type InitFn = unsafe extern "C" fn(ffi::Handle, *mut ffi::CallbackFnsT, *mut c_void) -> ffi::ErrT;
type UninitFn = unsafe extern "C" fn(ffi::Handle) -> ffi::ErrT;
type UpdateFn = unsafe extern "C" fn(ffi::Handle, c_int, c_uint, c_uint) -> ffi::ErrT;
type GetErrorStringFn = unsafe extern "C" fn(ffi::ErrT) -> *const c_char;

struct Entries {
    close: CloseFn,
    lock: LockFn,
    unlock: UnlockFn,
    get_devices: GetDevicesFn,
    select_device: SelectDeviceFn,
    release_device: ReleaseDeviceFn,
    get_device_params: GetDeviceParamsFn,
    init: InitFn,
    uninit: UninitFn,
    update: UpdateFn,
    get_error_string: Option<GetErrorStringFn>,
}

pub struct LoadedApi {
    entries: Entries,
    version: f32,
    path: PathBuf,
    _library: libloading::Library,
}

unsafe impl Send for LoadedApi {}
unsafe impl Sync for LoadedApi {}

macro_rules! entry {
    ($library:expr, $name:literal, $signature:ty) => {{
        let symbol: libloading::Symbol<'_, $signature> =
            unsafe { $library.get($name) }.map_err(|error| {
                let name = String::from_utf8_lossy($name);
                format!("{} is missing: {error}", name.trim_end_matches('\0'))
            })?;
        *symbol
    }};
}

impl LoadedApi {
    fn open(path: &Path) -> Result<Self, String> {
        let library = unsafe { libloading::Library::new(path) }
            .map_err(|error| format!("{} could not be loaded: {error}", path.display()))?;
        let open = entry!(library, b"sdrplay_api_Open\0", OpenFn);
        let api_version = entry!(library, b"sdrplay_api_ApiVersion\0", ApiVersionFn);
        let entries = Entries {
            close: entry!(library, b"sdrplay_api_Close\0", CloseFn),
            lock: entry!(library, b"sdrplay_api_LockDeviceApi\0", LockFn),
            unlock: entry!(library, b"sdrplay_api_UnlockDeviceApi\0", UnlockFn),
            get_devices: entry!(library, b"sdrplay_api_GetDevices\0", GetDevicesFn),
            select_device: entry!(library, b"sdrplay_api_SelectDevice\0", SelectDeviceFn),
            release_device: entry!(library, b"sdrplay_api_ReleaseDevice\0", ReleaseDeviceFn),
            get_device_params: entry!(library, b"sdrplay_api_GetDeviceParams\0", GetDeviceParamsFn),
            init: entry!(library, b"sdrplay_api_Init\0", InitFn),
            uninit: entry!(library, b"sdrplay_api_Uninit\0", UninitFn),
            update: entry!(library, b"sdrplay_api_Update\0", UpdateFn),
            get_error_string: unsafe {
                library
                    .get::<GetErrorStringFn>(b"sdrplay_api_GetErrorString\0")
                    .ok()
                    .map(|symbol| *symbol)
            },
        };

        let code = unsafe { open() };
        if code != ffi::SUCCESS {
            return Err(format!(
                "sdrplay_api_Open failed: {} (code {code})",
                error_message(code)
            ));
        }

        let mut version: c_float = 0.0;
        let code = unsafe { api_version(&raw mut version) };
        if code != ffi::SUCCESS {
            unsafe { (entries.close)() };
            return Err(format!(
                "sdrplay_api_ApiVersion failed: {} (code {code})",
                error_message(code)
            ));
        }
        if !(MIN_API_VERSION..MAX_API_VERSION).contains(&version) {
            unsafe { (entries.close)() };
            return Err(format!(
                "SDRplay API {version} is installed, but this build reads the {MIN_API_VERSION} \
                 structure layout. Install API {MIN_API_VERSION} or newer from \
                 https://www.sdrplay.com/downloads/"
            ));
        }
        if version > MIN_API_VERSION {
            tracing::warn!(
                version,
                "the installed SDRplay API is newer than the {MIN_API_VERSION} structure layout \
                 this build was written against"
            );
        }

        Ok(Self {
            entries,
            version,
            path: path.to_path_buf(),
            _library: library,
        })
    }

    fn describe(&self, code: ffi::ErrT, action: &str) -> Result<(), DeviceError> {
        let Some(get_error_string) = self.entries.get_error_string else {
            return check(code, action);
        };
        if code == ffi::SUCCESS {
            return Ok(());
        }
        let vendor = unsafe { get_error_string(code) };
        if vendor.is_null() {
            return check(code, action);
        }
        let vendor = unsafe { std::ffi::CStr::from_ptr(vendor) }.to_string_lossy();
        check(code, &format!("{action} ({vendor})"))
    }
}

impl Drop for LoadedApi {
    fn drop(&mut self) {
        unsafe { (self.entries.close)() };
    }
}

impl Sdrplay for LoadedApi {
    fn version(&self) -> f32 {
        self.version
    }

    fn library_path(&self) -> &Path {
        &self.path
    }

    fn get_devices(&self) -> Result<Vec<ffi::DeviceT>, DeviceError> {
        let mut devices = [ffi::DeviceT::default(); ffi::MAX_DEVICES];
        let mut count: c_uint = 0;
        self.describe(unsafe { (self.entries.lock)() }, "locking the SDRplay API")?;
        let code = unsafe {
            (self.entries.get_devices)(
                devices.as_mut_ptr(),
                &raw mut count,
                ffi::MAX_DEVICES as c_uint,
            )
        };
        let unlocked = unsafe { (self.entries.unlock)() };
        self.describe(code, "enumerating SDRplay devices")?;
        self.describe(unlocked, "unlocking the SDRplay API")?;
        Ok(devices
            .into_iter()
            .take(count as usize)
            .filter(|device| device.valid != 0)
            .collect())
    }

    fn select_device(&self, device: &mut ffi::DeviceT) -> Result<(), DeviceError> {
        self.describe(unsafe { (self.entries.lock)() }, "locking the SDRplay API")?;
        let code = unsafe { (self.entries.select_device)(&raw mut *device) };
        let unlocked = unsafe { (self.entries.unlock)() };
        self.describe(code, "opening the SDRplay device")
            .map_err(claim_error)?;
        self.describe(unlocked, "unlocking the SDRplay API")
    }

    fn release_device(&self, device: &mut ffi::DeviceT) -> Result<(), DeviceError> {
        self.describe(
            unsafe { (self.entries.release_device)(&raw mut *device) },
            "releasing the SDRplay device",
        )
    }

    fn device_params(&self, dev: DevHandle) -> Result<*mut ffi::DeviceParamsT, DeviceError> {
        let mut params: *mut ffi::DeviceParamsT = std::ptr::null_mut();
        self.describe(
            unsafe { (self.entries.get_device_params)(dev.0, &raw mut params) },
            "reading the SDRplay device parameters",
        )?;
        if params.is_null() {
            return Err(DeviceError::Io(
                "the SDRplay API returned no device parameters".to_string(),
            ));
        }
        Ok(params)
    }

    fn init(
        &self,
        dev: DevHandle,
        callbacks: &ffi::CallbackFnsT,
        context: *mut c_void,
    ) -> Result<InitOutcome, DeviceError> {
        let mut callbacks = *callbacks;
        let code = unsafe { (self.entries.init)(dev.0, &raw mut callbacks, context) };
        if code == ffi::START_PENDING {
            return Ok(InitOutcome::MasterPending);
        }
        self.describe(code, "starting the SDRplay stream")?;
        Ok(InitOutcome::Started)
    }

    fn uninit(&self, dev: DevHandle) -> Result<(), DeviceError> {
        self.describe(
            unsafe { (self.entries.uninit)(dev.0) },
            "stopping the SDRplay stream",
        )
    }

    fn update(
        &self,
        dev: DevHandle,
        tuner: c_int,
        reason: c_uint,
        ext1: c_uint,
    ) -> Result<(), DeviceError> {
        self.describe(
            unsafe { (self.entries.update)(dev.0, tuner, reason, ext1) },
            "applying SDRplay settings",
        )
    }
}

#[must_use]
pub fn library_candidates() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    let names = {
        let mut names = vec![PathBuf::from("sdrplay_api.dll")];
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            names.push(
                Path::new(&program_files)
                    .join("SDRplay")
                    .join("API")
                    .join(if cfg!(target_arch = "aarch64") {
                        "arm64"
                    } else {
                        "x64"
                    })
                    .join("sdrplay_api.dll"),
            );
        }
        names.push(
            Path::new("C:\\Program Files\\SDRplay\\API")
                .join(if cfg!(target_arch = "aarch64") {
                    "arm64"
                } else {
                    "x64"
                })
                .join("sdrplay_api.dll"),
        );
        names
    };
    #[cfg(not(target_os = "windows"))]
    let names = {
        let mut names = vec![
            PathBuf::from("/usr/local/lib/libsdrplay_api.so.3"),
            PathBuf::from("/usr/local/lib/libsdrplay_api.so"),
            PathBuf::from("/usr/lib/libsdrplay_api.so.3"),
            PathBuf::from("/usr/lib64/libsdrplay_api.so.3"),
        ];
        names.extend(versioned_install_dirs());
        names.push(PathBuf::from("libsdrplay_api.so.3"));
        names
    };
    names
}

#[cfg(target_os = "macos")]
fn versioned_install_dirs() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir("/Library/SDRplayAPI") else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("lib").join("libsdrplay_api.so.3"))
        .filter(|path| path.exists())
        .collect();
    found.sort();
    found.reverse();
    found
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn versioned_install_dirs() -> Vec<PathBuf> {
    Vec::new()
}

pub const RETRY_AFTER: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Loader {
    api: Option<Arc<LoadedApi>>,
    failure: Option<(String, Instant)>,
}

impl Loader {
    fn resolve(&mut self, now: Instant) -> Result<Arc<LoadedApi>, String> {
        self.resolve_with(now, load)
    }

    fn resolve_with(
        &mut self,
        now: Instant,
        load: impl FnOnce() -> Result<Arc<LoadedApi>, String>,
    ) -> Result<Arc<LoadedApi>, String> {
        if let Some(api) = &self.api {
            return Ok(api.clone());
        }
        if let Some((error, at)) = &self.failure
            && now.duration_since(*at) < RETRY_AFTER
        {
            return Err(error.clone());
        }
        match load() {
            Ok(api) => {
                self.api = Some(api.clone());
                self.failure = None;
                Ok(api)
            }
            Err(error) => {
                self.failure = Some((error.clone(), now));
                Err(error)
            }
        }
    }
}

static LOADER: Mutex<Loader> = Mutex::new(Loader {
    api: None,
    failure: None,
});

pub fn shared() -> Result<Arc<dyn Sdrplay>, String> {
    let mut loader = LOADER.lock().unwrap_or_else(PoisonError::into_inner);
    loader
        .resolve(Instant::now())
        .map(|api| api as Arc<dyn Sdrplay>)
}

fn load() -> Result<Arc<LoadedApi>, String> {
    let mut failures = Vec::new();
    for candidate in library_candidates() {
        if candidate.is_absolute() && !candidate.exists() {
            continue;
        }
        match LoadedApi::open(&candidate) {
            Ok(api) => {
                tracing::info!(
                    version = api.version,
                    path = %api.path.display(),
                    "loaded the SDRplay API"
                );
                return Ok(Arc::new(api));
            }
            Err(error) => failures.push(error),
        }
    }
    Err(if failures.is_empty() {
        "the SDRplay API is not installed — install it from https://www.sdrplay.com/downloads/ \
         to use an RSP receiver"
            .to_string()
    } else {
        failures.join("; ")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_service_that_is_down_reads_as_an_io_error_naming_the_service() {
        let error = check(ffi::SERVICE_NOT_RESPONDING, "enumerating").expect_err("service down");
        assert!(matches!(error, DeviceError::Io(_)));
        assert!(error.to_string().contains("sdrplay_apiService"));
    }

    #[test]
    fn a_claimed_device_reads_as_in_use() {
        assert!(matches!(
            check(ffi::ALREADY_INITIALISED, "opening"),
            Err(DeviceError::InUse(_))
        ));
    }

    #[test]
    fn an_unsupported_mode_reads_as_unsupported() {
        assert!(matches!(
            check(ffi::INVALID_MODE, "opening"),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn success_is_not_an_error() {
        assert!(check(ffi::SUCCESS, "opening").is_ok());
    }

    #[test]
    fn a_version_mismatch_names_the_reinstall() {
        assert!(error_message(ffi::INVALID_SERVICE_VERSION).contains("reinstall"));
    }

    #[test]
    fn candidates_are_offered_for_this_platform() {
        assert!(!library_candidates().is_empty());
    }

    #[test]
    fn a_failed_load_is_held_briefly_and_then_tried_again() {
        let attempts = std::cell::Cell::new(0);
        let mut loader = Loader::default();
        let attempt = |loader: &mut Loader, now: Instant| {
            let result = loader.resolve_with(now, || {
                attempts.set(attempts.get() + 1);
                Err("not installed".to_string())
            });
            assert!(result.is_err());
        };
        let start = Instant::now();
        attempt(&mut loader, start);
        attempt(&mut loader, start + RETRY_AFTER / 2);
        assert_eq!(
            attempts.get(),
            1,
            "a retry inside the window reopens nothing"
        );
        attempt(&mut loader, start + RETRY_AFTER);
        assert_eq!(
            attempts.get(),
            2,
            "the window expiring allows another attempt"
        );
    }
}
