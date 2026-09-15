use std::{
    ffi::{CStr, OsString, c_char, c_double, c_int, c_long, c_longlong, c_void},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use super::ffi;

pub const LIBRARY_ENV: &str = "SDRMM_SOAPY_LIBRARY";
const RETRY_AFTER: Duration = Duration::from_secs(5);

macro_rules! api {
    ($($field:ident: $symbol:literal => $signature:ty,)+) => {
        pub struct Entries {
            $(pub $field: $signature,)+
        }

        impl Entries {
            fn load(library: &libloading::Library) -> Result<Self, String> {
                $(
                    let $field: $signature = {
                        let symbol: libloading::Symbol<'_, $signature> =
                            unsafe { library.get(concat!($symbol, "\0").as_bytes()) }
                                .map_err(|error| format!("{} is missing: {error}", $symbol))?;
                        *symbol
                    };
                )+
                Ok(Self { $($field,)+ })
            }
        }
    };
}

api! {
    abi_version: "SoapySDR_getABIVersion" => ffi::StaticString,
    lib_version: "SoapySDR_getLibVersion" => ffi::StaticString,
    list_modules: "SoapySDR_listModules" => ffi::ModuleStringList,
    list_search_paths: "SoapySDR_listSearchPaths" => ffi::ModuleStringList,

    free: "SoapySDR_free" => unsafe extern "C" fn(*mut c_void),
    strings_clear: "SoapySDRStrings_clear" => unsafe extern "C" fn(*mut *mut *mut c_char, usize),
    kwargs_clear: "SoapySDRKwargs_clear" => unsafe extern "C" fn(*mut ffi::Kwargs),
    kwargs_list_clear: "SoapySDRKwargsList_clear" => unsafe extern "C" fn(*mut ffi::Kwargs, usize),
    arg_info_list_clear: "SoapySDRArgInfoList_clear" => unsafe extern "C" fn(*mut ffi::ArgInfo, usize),

    enumerate: "SoapySDRDevice_enumerateStrArgs" => unsafe extern "C" fn(*const c_char, *mut usize) -> *mut ffi::Kwargs,
    make: "SoapySDRDevice_makeStrArgs" => unsafe extern "C" fn(*const c_char) -> ffi::Device,
    unmake: "SoapySDRDevice_unmake" => unsafe extern "C" fn(ffi::Device) -> c_int,
    last_status: "SoapySDRDevice_lastStatus" => unsafe extern "C" fn() -> c_int,
    last_error: "SoapySDRDevice_lastError" => ffi::StaticString,

    hardware_info: "SoapySDRDevice_getHardwareInfo" => unsafe extern "C" fn(ffi::Device) -> ffi::Kwargs,
    channel_info: "SoapySDRDevice_getChannelInfo" => ffi::ChannelKwargs,
    num_channels: "SoapySDRDevice_getNumChannels" => unsafe extern "C" fn(ffi::Device, c_int) -> usize,
    full_duplex: "SoapySDRDevice_getFullDuplex" => ffi::ChannelFlag,

    stream_formats: "SoapySDRDevice_getStreamFormats" => ffi::StringList,
    native_stream_format: "SoapySDRDevice_getNativeStreamFormat" => unsafe extern "C" fn(ffi::Device, c_int, usize, *mut c_double) -> *mut c_char,
    stream_args_info: "SoapySDRDevice_getStreamArgsInfo" => ffi::ArgInfoList,

    setup_stream: "SoapySDRDevice_setupStream" => unsafe extern "C" fn(ffi::Device, c_int, *const c_char, *const usize, usize, *const ffi::Kwargs) -> ffi::Stream,
    close_stream: "SoapySDRDevice_closeStream" => unsafe extern "C" fn(ffi::Device, ffi::Stream) -> c_int,
    stream_mtu: "SoapySDRDevice_getStreamMTU" => unsafe extern "C" fn(ffi::Device, ffi::Stream) -> usize,
    activate_stream: "SoapySDRDevice_activateStream" => unsafe extern "C" fn(ffi::Device, ffi::Stream, c_int, c_longlong, usize) -> c_int,
    deactivate_stream: "SoapySDRDevice_deactivateStream" => unsafe extern "C" fn(ffi::Device, ffi::Stream, c_int, c_longlong) -> c_int,
    read_stream: "SoapySDRDevice_readStream" => unsafe extern "C" fn(ffi::Device, ffi::Stream, *const *mut c_void, usize, *mut c_int, *mut c_longlong, c_long) -> c_int,
    write_stream: "SoapySDRDevice_writeStream" => unsafe extern "C" fn(ffi::Device, ffi::Stream, *const *const c_void, usize, *mut c_int, c_longlong, c_long) -> c_int,
    read_stream_status: "SoapySDRDevice_readStreamStatus" => unsafe extern "C" fn(ffi::Device, ffi::Stream, *mut usize, *mut c_int, *mut c_longlong, c_long) -> c_int,

    list_antennas: "SoapySDRDevice_listAntennas" => ffi::StringList,
    set_antenna: "SoapySDRDevice_setAntenna" => unsafe extern "C" fn(ffi::Device, c_int, usize, *const c_char) -> c_int,
    antenna: "SoapySDRDevice_getAntenna" => ffi::ChannelString,

    list_gains: "SoapySDRDevice_listGains" => ffi::StringList,
    has_gain_mode: "SoapySDRDevice_hasGainMode" => ffi::ChannelFlag,
    set_gain_mode: "SoapySDRDevice_setGainMode" => unsafe extern "C" fn(ffi::Device, c_int, usize, bool) -> c_int,
    gain_mode: "SoapySDRDevice_getGainMode" => ffi::ChannelFlag,
    set_gain_element: "SoapySDRDevice_setGainElement" => ffi::NamedSet,
    gain_element: "SoapySDRDevice_getGainElement" => ffi::NamedQuery,
    gain_element_range: "SoapySDRDevice_getGainElementRange" => unsafe extern "C" fn(ffi::Device, c_int, usize, *const c_char) -> ffi::Range,

    set_frequency: "SoapySDRDevice_setFrequency" => unsafe extern "C" fn(ffi::Device, c_int, usize, c_double, *const ffi::Kwargs) -> c_int,
    set_frequency_component: "SoapySDRDevice_setFrequencyComponent" => unsafe extern "C" fn(ffi::Device, c_int, usize, *const c_char, c_double, *const ffi::Kwargs) -> c_int,
    frequency: "SoapySDRDevice_getFrequency" => ffi::ChannelQuery,
    list_frequencies: "SoapySDRDevice_listFrequencies" => ffi::StringList,
    frequency_range: "SoapySDRDevice_getFrequencyRange" => ffi::RangeList,
    frequency_args_info: "SoapySDRDevice_getFrequencyArgsInfo" => ffi::ArgInfoList,

    set_sample_rate: "SoapySDRDevice_setSampleRate" => ffi::ChannelSet,
    sample_rate: "SoapySDRDevice_getSampleRate" => ffi::ChannelQuery,
    sample_rate_range: "SoapySDRDevice_getSampleRateRange" => ffi::RangeList,

    set_bandwidth: "SoapySDRDevice_setBandwidth" => ffi::ChannelSet,
    bandwidth: "SoapySDRDevice_getBandwidth" => ffi::ChannelQuery,
    bandwidth_range: "SoapySDRDevice_getBandwidthRange" => ffi::RangeList,

    has_dc_offset_mode: "SoapySDRDevice_hasDCOffsetMode" => ffi::ChannelFlag,
    has_iq_balance: "SoapySDRDevice_hasIQBalance" => ffi::ChannelFlag,

    list_clock_sources: "SoapySDRDevice_listClockSources" => ffi::DeviceStringList,
    clock_source: "SoapySDRDevice_getClockSource" => ffi::DeviceString,
    list_time_sources: "SoapySDRDevice_listTimeSources" => ffi::DeviceStringList,
    time_source: "SoapySDRDevice_getTimeSource" => ffi::DeviceString,
    has_hardware_time: "SoapySDRDevice_hasHardwareTime" => unsafe extern "C" fn(ffi::Device, *const c_char) -> bool,
    hardware_time: "SoapySDRDevice_getHardwareTime" => unsafe extern "C" fn(ffi::Device, *const c_char) -> c_longlong,
    master_clock_rate: "SoapySDRDevice_getMasterClockRate" => unsafe extern "C" fn(ffi::Device) -> c_double,

    setting_info: "SoapySDRDevice_getSettingInfo" => unsafe extern "C" fn(ffi::Device, *mut usize) -> *mut ffi::ArgInfo,
    write_setting: "SoapySDRDevice_writeSetting" => unsafe extern "C" fn(ffi::Device, *const c_char, *const c_char) -> c_int,
    read_setting: "SoapySDRDevice_readSetting" => unsafe extern "C" fn(ffi::Device, *const c_char) -> *mut c_char,
    channel_setting_info: "SoapySDRDevice_getChannelSettingInfo" => ffi::ArgInfoList,
}

pub struct Library {
    pub entries: Entries,
    path: PathBuf,
    version: String,
    _library: libloading::Library,
}

unsafe impl Send for Library {}
unsafe impl Sync for Library {}

impl Library {
    fn open(path: &Path) -> Result<Self, String> {
        let library = open_globally(path)
            .map_err(|error| format!("{} could not be loaded: {error}", path.display()))?;
        let entries = Entries::load(&library)?;
        let abi = unsafe { text((entries.abi_version)()) };
        if abi != ffi::ABI_VERSION {
            return Err(format!(
                "{} reports SoapySDR ABI {abi}, and this build speaks {}. Install a SoapySDR \
                 {} runtime, or point {LIBRARY_ENV} at one.",
                path.display(),
                ffi::ABI_VERSION,
                ffi::ABI_VERSION
            ));
        }
        let version = unsafe { text((entries.lib_version)()) };
        Ok(Self {
            entries,
            path: path.to_path_buf(),
            version,
            _library: library,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}

/// Loads the core into the process-wide symbol namespace.
///
/// SoapySDR's plugin modules are linked against the core and resolve its symbols from the flat
/// namespace at load time, so a core opened `RTLD_LOCAL` leaves every module unloadable with
/// "symbol not found in flat namespace". Windows resolves imports by module name and needs no
/// equivalent.
#[cfg(unix)]
fn open_globally(path: &Path) -> Result<libloading::Library, libloading::Error> {
    use libloading::os::unix::{Library, RTLD_GLOBAL, RTLD_NOW};

    unsafe { Library::open(Some(path), RTLD_NOW | RTLD_GLOBAL) }.map(Into::into)
}

#[cfg(not(unix))]
fn open_globally(path: &Path) -> Result<libloading::Library, libloading::Error> {
    unsafe { libloading::Library::new(path) }
}

unsafe fn text(pointer: *const c_char) -> String {
    if pointer.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(pointer) }
        .to_string_lossy()
        .into_owned()
}

#[must_use]
pub fn library_candidates() -> Vec<PathBuf> {
    candidates(
        std::env::var_os(LIBRARY_ENV),
        std::env::var_os("SOAPY_SDR_ROOT"),
    )
}

fn candidates(override_path: Option<OsString>, root: Option<OsString>) -> Vec<PathBuf> {
    if let Some(override_path) = override_path {
        return vec![PathBuf::from(override_path)];
    }
    let mut names: Vec<PathBuf> = Vec::new();
    if let Some(root) = root {
        let root = Path::new(&root);
        names.push(root.join(library_dir()).join(versioned_name()));
        names.push(root.join(library_dir()).join(bare_name()));
    }
    names.extend(
        install_prefixes()
            .into_iter()
            .map(|prefix| Path::new(prefix).join(library_dir()).join(versioned_name())),
    );
    // A leaf name reaches the loader's own search, which on Linux includes the `DT_RUNPATH` a
    // packager set on this binary. macOS searches `LC_RPATH` only for a path that asks for it,
    // so an `@rpath` spelling is what lets a Homebrew-style rpath answer there too.
    if cfg!(target_os = "macos") {
        names.push(PathBuf::from(format!("@rpath/{}", versioned_name())));
        names.push(PathBuf::from(format!("@rpath/{}", bare_name())));
    }
    names.push(PathBuf::from(versioned_name()));
    names.push(PathBuf::from(bare_name()));
    names
}

const fn library_dir() -> &'static str {
    if cfg!(target_os = "windows") {
        "bin"
    } else {
        "lib"
    }
}

const fn versioned_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "SoapySDR.dll"
    }
    #[cfg(target_os = "macos")]
    {
        "libSoapySDR.0.8.dylib"
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        "libSoapySDR.so.0.8"
    }
}

const fn bare_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "SoapySDR.dll"
    }
    #[cfg(target_os = "macos")]
    {
        "libSoapySDR.dylib"
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        "libSoapySDR.so"
    }
}

/// Prefixes a package manager installs into that the dynamic loader does not search by default:
/// on macOS it falls back to `/usr/local/lib` and `/usr/lib` only, so a Homebrew SoapySDR on
/// Apple silicon is invisible to a bare `dlopen` of the library's name.
fn install_prefixes() -> Vec<&'static str> {
    #[cfg(target_os = "macos")]
    {
        vec!["/opt/homebrew", "/usr/local", "/opt/local"]
    }
    #[cfg(target_os = "windows")]
    {
        vec![
            "C:\\Program Files\\PothosSDR",
            "C:\\Program Files\\SoapySDR",
        ]
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        vec!["/usr", "/usr/local", "/home/linuxbrew/.linuxbrew"]
    }
}

#[derive(Default)]
struct Loader {
    library: Option<Arc<Library>>,
    failure: Option<(String, Instant)>,
}

impl Loader {
    fn resolve(&mut self, now: Instant) -> Result<Arc<Library>, String> {
        self.resolve_with(now, load)
    }

    fn resolve_with(
        &mut self,
        now: Instant,
        load: impl FnOnce() -> Result<Arc<Library>, String>,
    ) -> Result<Arc<Library>, String> {
        if let Some(library) = &self.library {
            return Ok(library.clone());
        }
        if let Some((error, at)) = &self.failure
            && now.duration_since(*at) < RETRY_AFTER
        {
            return Err(error.clone());
        }
        match load() {
            Ok(library) => {
                self.library = Some(library.clone());
                self.failure = None;
                Ok(library)
            }
            Err(error) => {
                self.failure = Some((error.clone(), now));
                Err(error)
            }
        }
    }
}

static LOADER: Mutex<Loader> = Mutex::new(Loader {
    library: None,
    failure: None,
});

pub fn shared() -> Result<Arc<Library>, String> {
    let mut loader = LOADER.lock().unwrap_or_else(PoisonError::into_inner);
    loader.resolve(Instant::now())
}

pub const MISSING: &str = "SoapySDR is not installed. Radios this build drives natively — \
                           RTL-SDR, HackRF, AD936x, SDRplay and CR-8 — do not need it; Airspy, \
                           bladeRF, LimeSDR and other SoapySDR-only hardware do.";

fn load() -> Result<Arc<Library>, String> {
    let mut failures = Vec::new();
    for candidate in library_candidates() {
        if candidate.is_absolute() && !candidate.exists() {
            continue;
        }
        match Library::open(&candidate) {
            Ok(library) => {
                tracing::info!(
                    version = library.version(),
                    path = %library.path().display(),
                    "loaded the SoapySDR runtime"
                );
                return Ok(Arc::new(library));
            }
            Err(error) => failures.push(error),
        }
    }
    Err(if failures.is_empty() {
        MISSING.to_string()
    } else {
        failures.join("; ")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_override_is_the_only_candidate_tried() {
        assert_eq!(
            candidates(Some(OsString::from("/opt/sdr/libSoapySDR.dylib")), None),
            vec![PathBuf::from("/opt/sdr/libSoapySDR.dylib")],
            "an operator naming a library must not have a package manager's chosen instead"
        );
    }

    #[test]
    fn an_override_wins_over_a_configured_root() {
        assert_eq!(
            candidates(
                Some(OsString::from("/opt/sdr/libSoapySDR.dylib")),
                Some(OsString::from("/opt/other")),
            )
            .len(),
            1
        );
    }

    #[test]
    fn the_bare_library_name_is_always_tried_last() {
        let found = candidates(None, None);
        assert_eq!(found.last(), Some(&PathBuf::from(bare_name())));
        assert!(found.contains(&PathBuf::from(versioned_name())));
    }

    #[test]
    fn a_configured_root_is_searched_before_any_package_manager_prefix() {
        let found = candidates(None, Some(OsString::from("/opt/sdr")));
        assert_eq!(
            found.first(),
            Some(
                &Path::new("/opt/sdr")
                    .join(library_dir())
                    .join(versioned_name())
            )
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_rpath_a_packager_set_is_tried_before_the_bare_name() {
        let found = candidates(None, None);
        let rpath = found
            .iter()
            .position(|path| path.starts_with("@rpath"))
            .expect("an @rpath candidate");
        let bare = found
            .iter()
            .position(|path| path == Path::new(bare_name()))
            .expect("the bare name");
        assert!(rpath < bare);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_homebrew_prefix_the_loader_ignores_is_searched_explicitly() {
        assert!(
            candidates(None, None)
                .contains(&PathBuf::from("/opt/homebrew/lib/libSoapySDR.0.8.dylib")),
            "dlopen falls back to /usr/local and /usr/lib only, so this prefix needs naming"
        );
    }

    #[test]
    fn a_failure_is_remembered_until_the_retry_window_passes() {
        let mut loader = Loader::default();
        let start = Instant::now();
        let mut attempts = 0;
        let attempt = |loader: &mut Loader, now: Instant, attempts: &mut i32| {
            loader.resolve_with(now, || {
                *attempts += 1;
                Err("not installed".to_string())
            })
        };
        assert!(attempt(&mut loader, start, &mut attempts).is_err());
        assert!(
            attempt(&mut loader, start + RETRY_AFTER / 2, &mut attempts).is_err(),
            "a probe tick must not retry a missing library every time"
        );
        assert_eq!(attempts, 1);
        assert!(attempt(&mut loader, start + RETRY_AFTER * 2, &mut attempts).is_err());
        assert_eq!(attempts, 2);
    }
}
