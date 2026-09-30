use std::{
    ffi::{OsString, c_int, c_void},
    path::{Path, PathBuf},
    ptr::NonNull,
    sync::LazyLock,
};

use num_complex::Complex;

use crate::ChannelError;

pub const LIBRARY_ENV: &str = "SDRMM_CODEC2_LIBRARY";
const ABI_VERSION: u32 = 1;
const FREEDV_1600_CARRIERS: c_int = 16;
const FDMDV_BITS: usize = 32;

macro_rules! api {
    ($name:ident { $($field:ident: $symbol:literal => $signature:ty,)+ }) => {
        pub(crate) struct $name {
            $($field: $signature,)+
        }

        impl $name {
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
    Bundled {
        abi_version: "sdrmm_codec2_abi_version" => unsafe extern "C" fn() -> u32,
        fdmdv_create: "sdrmm_fdmdv_create" => unsafe extern "C" fn() -> *mut c_void,
        fdmdv_destroy: "sdrmm_fdmdv_destroy" => unsafe extern "C" fn(*mut c_void),
        fdmdv_demod: "sdrmm_fdmdv_demod" => unsafe extern "C" fn(*mut c_void, *const f32, c_int, *mut u8, *mut c_int, *mut c_int) -> c_int,
        codec_create: "sdrmm_codec2_create" => unsafe extern "C" fn(c_int) -> *mut c_void,
        codec_destroy: "sdrmm_codec2_destroy" => unsafe extern "C" fn(*mut c_void),
        codec_samples: "sdrmm_codec2_samples_per_frame" => unsafe extern "C" fn(*const c_void) -> c_int,
        codec_bytes: "sdrmm_codec2_bytes_per_frame" => unsafe extern "C" fn(*const c_void) -> c_int,
        codec_decode: "sdrmm_codec2_decode" => unsafe extern "C" fn(*mut c_void, *const u8, *mut i16),
    }
}

api! {
    Upstream {
        fdmdv_create: "fdmdv_create" => unsafe extern "C" fn(c_int) -> *mut c_void,
        fdmdv_destroy: "fdmdv_destroy" => unsafe extern "C" fn(*mut c_void),
        fdmdv_demod: "fdmdv_demod" => unsafe extern "C" fn(*mut c_void, *mut c_int, *mut c_int, *mut Complex<f32>, *mut c_int),
        fdmdv_stats: "fdmdv_get_demod_stats" => unsafe extern "C" fn(*mut c_void, *mut ModemStats),
        codec_create: "codec2_create" => unsafe extern "C" fn(c_int) -> *mut c_void,
        codec_destroy: "codec2_destroy" => unsafe extern "C" fn(*mut c_void),
        codec_samples: "codec2_samples_per_frame" => unsafe extern "C" fn(*mut c_void) -> c_int,
        codec_bytes: "codec2_bytes_per_frame" => unsafe extern "C" fn(*mut c_void) -> c_int,
        codec_decode: "codec2_decode" => unsafe extern "C" fn(*mut c_void, *mut i16, *const u8),
    }
}

#[repr(C)]
pub(crate) struct ModemStats {
    nc: c_int,
    snr_est: f32,
    rx_symbols: [[Complex<f32>; 51]; 320],
    nr: c_int,
    sync: c_int,
    foff: f32,
    rx_timing: f32,
    clock_offset: f32,
    sync_metric: f32,
    pre: c_int,
    post: c_int,
    uw_fails: c_int,
    rx_eye: [[f32; 160]; 8],
    neyetr: c_int,
    neyesamp: c_int,
    f_est: [f32; 4],
    fft_buf: [f32; 1024],
    fft_cfg: *mut c_void,
}

impl ModemStats {
    fn boxed() -> Box<Self> {
        unsafe { Box::<Self>::new_zeroed().assume_init() }
    }
}

pub(crate) enum Entries {
    Bundled(Bundled),
    Upstream(Upstream),
}

impl Entries {
    fn load(library: &libloading::Library, path: &Path) -> Result<Self, String> {
        let bundled_error = match Bundled::load(library) {
            Ok(bundled) => return Self::checked(bundled, path),
            Err(error) => error,
        };
        Upstream::load(library)
            .map(Self::Upstream)
            .map_err(|error| {
                format!(
                    "{} is neither sdrmm_codec2 ({bundled_error}) nor codec2 ({error})",
                    path.display()
                )
            })
    }

    fn checked(bundled: Bundled, path: &Path) -> Result<Self, String> {
        let abi = unsafe { (bundled.abi_version)() };
        if abi != ABI_VERSION {
            return Err(format!(
                "{} speaks ABI {abi}, this build needs {ABI_VERSION}",
                path.display()
            ));
        }
        Ok(Self::Bundled(bundled))
    }

    fn codec_create(&self, bit_rate: c_int) -> *mut c_void {
        match self {
            Self::Bundled(api) => unsafe { (api.codec_create)(bit_rate) },
            Self::Upstream(api) => match upstream_mode(bit_rate) {
                Some(mode) => unsafe { (api.codec_create)(mode) },
                None => std::ptr::null_mut(),
            },
        }
    }

    fn codec_destroy(&self, codec: *mut c_void) {
        match self {
            Self::Bundled(api) => unsafe { (api.codec_destroy)(codec) },
            Self::Upstream(api) => unsafe { (api.codec_destroy)(codec) },
        }
    }

    fn codec_frame(&self, codec: *mut c_void) -> (c_int, c_int) {
        match self {
            Self::Bundled(api) => unsafe { ((api.codec_samples)(codec), (api.codec_bytes)(codec)) },
            Self::Upstream(api) => unsafe {
                ((api.codec_samples)(codec), (api.codec_bytes)(codec))
            },
        }
    }

    fn codec_decode(&self, codec: *mut c_void, bits: *const u8, pcm: *mut i16) {
        match self {
            Self::Bundled(api) => unsafe { (api.codec_decode)(codec, bits, pcm) },
            Self::Upstream(api) => unsafe { (api.codec_decode)(codec, pcm, bits) },
        }
    }

    fn fdmdv_create(&self) -> *mut c_void {
        match self {
            Self::Bundled(api) => unsafe { (api.fdmdv_create)() },
            Self::Upstream(api) => unsafe { (api.fdmdv_create)(FREEDV_1600_CARRIERS) },
        }
    }

    fn fdmdv_destroy(&self, modem: *mut c_void) {
        match self {
            Self::Bundled(api) => unsafe { (api.fdmdv_destroy)(modem) },
            Self::Upstream(api) => unsafe { (api.fdmdv_destroy)(modem) },
        }
    }
}

fn upstream_mode(bit_rate: c_int) -> Option<c_int> {
    match bit_rate {
        3200 => Some(0),
        1600 => Some(2),
        1300 => Some(4),
        _ => None,
    }
}

pub struct Library {
    entries: Entries,
    path: PathBuf,
    _library: libloading::Library,
}

impl Library {
    fn open(path: &Path) -> Result<Self, String> {
        let library = unsafe { libloading::Library::new(path) }
            .map_err(|error| format!("{} could not be loaded: {error}", path.display()))?;
        let entries = Entries::load(&library, path)?;
        Ok(Self {
            entries,
            path: path.to_path_buf(),
            _library: library,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

static LIBRARY: LazyLock<Result<Library, String>> = LazyLock::new(load);

pub fn library() -> Result<&'static Library, String> {
    LIBRARY.as_ref().map_err(Clone::clone)
}

pub(crate) fn entries() -> Result<&'static Entries, ChannelError> {
    library()
        .map(|library| &library.entries)
        .map_err(ChannelError::LibraryUnavailable)
}

fn load() -> Result<Library, String> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let candidates = candidates(std::env::var_os(LIBRARY_ENV), exe_dir.as_deref());
    let mut failures = Vec::new();
    for path in &candidates {
        if path.is_absolute() && !path.is_file() {
            continue;
        }
        match Library::open(path) {
            Ok(library) => return Ok(library),
            Err(error) if path.is_absolute() => failures.push(error),
            Err(_) => {}
        }
    }
    if failures.is_empty() {
        failures.push(format!(
            "neither {} nor codec2 {SYSTEM_VERSION} found, set {LIBRARY_ENV} or install codec2",
            file_name().to_string_lossy()
        ));
    }
    Err(failures.join("; "))
}

fn file_name() -> OsString {
    libloading::library_filename("sdrmm_codec2")
}

fn candidates(override_path: Option<OsString>, exe_dir: Option<&Path>) -> Vec<PathBuf> {
    if let Some(path) = override_path {
        return vec![PathBuf::from(path)];
    }
    let mut found = exe_dir.map(bundled_candidates).unwrap_or_default();
    found.extend(system_candidates());
    found
}

fn bundled_candidates(dir: &Path) -> Vec<PathBuf> {
    let name = file_name();
    [
        dir.to_path_buf(),
        dir.join("../Frameworks"),
        dir.join("../lib/sdrmm"),
        dir.join("../lib/sdrminusminus"),
        dir.join("deps"),
    ]
    .into_iter()
    .map(|dir| dir.join(&name))
    .collect()
}

const SYSTEM_VERSION: &str = "1.2";

#[cfg(target_os = "macos")]
fn system_candidates() -> Vec<PathBuf> {
    let name = format!("libcodec2.{SYSTEM_VERSION}.dylib");
    ["/opt/homebrew/lib", "/usr/local/lib", "/opt/local/lib"]
        .into_iter()
        .map(|dir| Path::new(dir).join(&name))
        .collect()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn system_candidates() -> Vec<PathBuf> {
    let name = format!("libcodec2.so.{SYSTEM_VERSION}");
    vec![
        PathBuf::from(&name),
        Path::new("/home/linuxbrew/.linuxbrew/lib").join(&name),
    ]
}

#[cfg(not(unix))]
fn system_candidates() -> Vec<PathBuf> {
    Vec::new()
}

enum Modem {
    Bundled(&'static Bundled),
    Upstream(&'static Upstream, Box<ModemStats>),
}

pub(crate) struct Fdmdv {
    api: &'static Entries,
    modem: Modem,
    state: NonNull<c_void>,
}

unsafe impl Send for Fdmdv {}

pub(crate) struct FdmdvResult {
    pub(crate) next_nin: i32,
    pub(crate) sync: bool,
    pub(crate) reliable_sync: bool,
}

impl Fdmdv {
    pub(crate) fn new() -> Result<Self, ChannelError> {
        let api = entries()?;
        let state = NonNull::new(api.fdmdv_create()).ok_or_else(|| {
            ChannelError::InvalidSettings("FreeDV modem allocation failed".to_owned())
        })?;
        let modem = match api {
            Entries::Bundled(bundled) => Modem::Bundled(bundled),
            Entries::Upstream(upstream) => Modem::Upstream(upstream, ModemStats::boxed()),
        };
        Ok(Self { api, modem, state })
    }

    pub(crate) fn demod(
        &mut self,
        input: &[Complex<f32>],
        bits: &mut [u8; FDMDV_BITS],
    ) -> FdmdvResult {
        let state = self.state.as_ptr();
        match &mut self.modem {
            Modem::Bundled(api) => bundled_demod(api, state, input, bits),
            Modem::Upstream(api, stats) => upstream_demod(api, stats, state, input, bits),
        }
    }
}

fn bundled_demod(
    api: &Bundled,
    state: *mut c_void,
    input: &[Complex<f32>],
    bits: &mut [u8; FDMDV_BITS],
) -> FdmdvResult {
    let (mut sync, mut reliable_sync) = (0, 0);
    let next_nin = unsafe {
        (api.fdmdv_demod)(
            state,
            input.as_ptr().cast(),
            input.len() as c_int,
            bits.as_mut_ptr(),
            &mut sync,
            &mut reliable_sync,
        )
    };
    FdmdvResult {
        next_nin,
        sync: sync != 0,
        reliable_sync: reliable_sync != 0,
    }
}

fn upstream_demod(
    api: &Upstream,
    stats: &mut ModemStats,
    state: *mut c_void,
    input: &[Complex<f32>],
    bits: &mut [u8; FDMDV_BITS],
) -> FdmdvResult {
    let mut decoded = [0 as c_int; FDMDV_BITS];
    let mut reliable_sync = 0;
    let mut nin = input.len() as c_int;
    unsafe {
        (api.fdmdv_demod)(
            state,
            decoded.as_mut_ptr(),
            &mut reliable_sync,
            input.as_ptr().cast_mut(),
            &mut nin,
        );
        (api.fdmdv_stats)(state, stats);
    }
    for (bit, value) in bits.iter_mut().zip(decoded) {
        *bit = u8::from(value != 0);
    }
    FdmdvResult {
        next_nin: nin,
        sync: stats.sync != 0,
        reliable_sync: reliable_sync != 0,
    }
}

impl Drop for Fdmdv {
    fn drop(&mut self) {
        self.api.fdmdv_destroy(self.state.as_ptr());
    }
}

pub(crate) struct Codec2<const SAMPLES: usize, const BYTES: usize> {
    api: &'static Entries,
    bit_rate: c_int,
    codec: NonNull<c_void>,
}

unsafe impl<const SAMPLES: usize, const BYTES: usize> Send for Codec2<SAMPLES, BYTES> {}

impl<const SAMPLES: usize, const BYTES: usize> Codec2<SAMPLES, BYTES> {
    pub(crate) fn new(bit_rate: c_int) -> Result<Self, ChannelError> {
        let api = entries()?;
        let codec = NonNull::new(api.codec_create(bit_rate)).ok_or_else(|| {
            ChannelError::LibraryUnavailable(format!("Codec2 {bit_rate} is not available"))
        })?;
        let codec = Self {
            api,
            bit_rate,
            codec,
        };
        let (samples, bytes) = api.codec_frame(codec.codec.as_ptr());
        if usize::try_from(samples) != Ok(SAMPLES) || usize::try_from(bytes) != Ok(BYTES) {
            return Err(ChannelError::LibraryUnavailable(format!(
                "Codec2 {bit_rate} frames are {samples} samples from {bytes} bytes, \
                 expected {SAMPLES} from {BYTES}"
            )));
        }
        Ok(codec)
    }

    pub(crate) fn reset(&mut self) {
        if let Ok(fresh) = Self::new(self.bit_rate) {
            *self = fresh;
        }
    }

    pub(crate) fn decode(&mut self, bits: &[u8; BYTES], pcm: &mut [i16; SAMPLES]) {
        self.api
            .codec_decode(self.codec.as_ptr(), bits.as_ptr(), pcm.as_mut_ptr());
    }
}

impl<const SAMPLES: usize, const BYTES: usize> Drop for Codec2<SAMPLES, BYTES> {
    fn drop(&mut self) {
        self.api.codec_destroy(self.codec.as_ptr());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_override_is_the_only_candidate() {
        assert_eq!(
            candidates(Some("/opt/codec2.so".into()), Some(Path::new("/app"))),
            [PathBuf::from("/opt/codec2.so")]
        );
    }

    #[test]
    fn bundles_and_cargo_builds_are_searched() {
        let found = candidates(None, Some(Path::new("/app/bin")));
        let name = file_name();
        for dir in [
            "",
            "../Frameworks",
            "../lib/sdrmm",
            "../lib/sdrminusminus",
            "deps",
        ] {
            assert!(found.contains(&Path::new("/app/bin").join(dir).join(&name)));
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_bundled_library_wins_over_a_system_codec2() {
        let found = candidates(None, Some(Path::new("/app/bin")));
        let bundled = found.iter().position(|path| path.ends_with(file_name()));
        let system = found
            .iter()
            .position(|path| path.to_string_lossy().contains("libcodec2"));
        assert!(
            bundled.is_some() && system.is_some() && bundled < system,
            "{found:?}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_homebrew_codec2_is_searched_by_prefix() {
        assert!(
            system_candidates().contains(&PathBuf::from("/opt/homebrew/lib/libcodec2.1.2.dylib"))
        );
    }

    #[test]
    fn modem_stats_matches_the_codec2_header() {
        assert_eq!(std::mem::size_of::<ModemStats>(), 139_856);
        assert_eq!(std::mem::offset_of!(ModemStats, sync), 130_572);
    }

    #[test]
    fn only_freedv_bit_rates_map_to_codec2_modes() {
        assert_eq!(upstream_mode(3200), Some(0));
        assert_eq!(upstream_mode(1600), Some(2));
        assert_eq!(upstream_mode(1300), Some(4));
        assert_eq!(upstream_mode(700), None);
    }

    #[test]
    fn the_cargo_build_loads() {
        let library = library().expect("codec2 library");
        assert!(library.path().is_file());
        Codec2::<160, 8>::new(3200).expect("codec2 3200");
        assert!(matches!(
            Codec2::<160, 8>::new(1600),
            Err(ChannelError::LibraryUnavailable(_))
        ));
    }

    #[test]
    fn a_system_codec2_decodes_and_demodulates() {
        let Some(path) = system_candidates()
            .into_iter()
            .find(|path| Library::open(path).is_ok())
        else {
            assert!(
                std::env::var_os("SDRMM_REQUIRE_SYSTEM_CODEC2").is_none(),
                "no codec2 {SYSTEM_VERSION} in {:?}",
                system_candidates()
            );
            return;
        };
        let library = Box::leak(Box::new(Library::open(&path).expect("system codec2")));
        let api = &library.entries;
        assert!(matches!(api, Entries::Upstream(_)));

        let codec = api.codec_create(3200);
        assert!(!codec.is_null());
        assert_eq!(api.codec_frame(codec), (160, 8));
        let mut pcm = [0i16; 160];
        api.codec_decode(codec, [0x55u8; 8].as_ptr(), pcm.as_mut_ptr());
        api.codec_destroy(codec);
        assert!(api.codec_create(700).is_null());

        let Entries::Upstream(upstream) = api else {
            unreachable!()
        };
        let state = api.fdmdv_create();
        let mut stats = ModemStats::boxed();
        let mut bits = [1u8; FDMDV_BITS];
        let result = upstream_demod(
            upstream,
            &mut stats,
            state,
            &[Complex::default(); 160],
            &mut bits,
        );
        api.fdmdv_destroy(state);
        assert!((1..=200).contains(&result.next_nin));
        assert!(!result.sync);
        assert_eq!(stats.nc, FREEDV_1600_CARRIERS);
        assert!(bits.iter().all(|&bit| bit <= 1));
    }
}
