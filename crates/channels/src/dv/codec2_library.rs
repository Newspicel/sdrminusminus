use std::{
    ffi::{OsString, c_int, c_void},
    path::{Path, PathBuf},
    ptr::NonNull,
    sync::LazyLock,
};

use crate::ChannelError;

pub const LIBRARY_ENV: &str = "SDRMM_CODEC2_LIBRARY";
const ABI_VERSION: u32 = 1;

macro_rules! api {
    ($($field:ident: $symbol:literal => $signature:ty,)+) => {
        pub(crate) struct Entries {
            $(pub(crate) $field: $signature,)+
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

pub struct Library {
    entries: Entries,
    path: PathBuf,
    _library: libloading::Library,
}

impl Library {
    fn open(path: &Path) -> Result<Self, String> {
        let library = unsafe { libloading::Library::new(path) }
            .map_err(|error| format!("{} could not be loaded: {error}", path.display()))?;
        let entries = Entries::load(&library)?;
        let abi = unsafe { (entries.abi_version)() };
        if abi != ABI_VERSION {
            return Err(format!(
                "{} speaks ABI {abi}, this build needs {ABI_VERSION}",
                path.display()
            ));
        }
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
        if !path.is_file() {
            continue;
        }
        match Library::open(path) {
            Ok(library) => return Ok(library),
            Err(error) => failures.push(error),
        }
    }
    if failures.is_empty() {
        failures.push(format!(
            "{} not found, set {LIBRARY_ENV} or install it beside the executable",
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
    let Some(dir) = exe_dir else {
        return Vec::new();
    };
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

pub(crate) struct Fdmdv {
    api: &'static Entries,
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
        let state = unsafe { (api.fdmdv_create)() };
        let state = NonNull::new(state).ok_or_else(|| {
            ChannelError::InvalidSettings("FreeDV modem allocation failed".to_owned())
        })?;
        Ok(Self { api, state })
    }

    pub(crate) fn demod(
        &mut self,
        input: &[num_complex::Complex<f32>],
        bits: &mut [u8; 32],
    ) -> FdmdvResult {
        let (mut sync, mut reliable_sync) = (0, 0);
        let next_nin = unsafe {
            (self.api.fdmdv_demod)(
                self.state.as_ptr(),
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
}

impl Drop for Fdmdv {
    fn drop(&mut self) {
        unsafe { (self.api.fdmdv_destroy)(self.state.as_ptr()) };
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
        let codec = NonNull::new(unsafe { (api.codec_create)(bit_rate) }).ok_or_else(|| {
            ChannelError::LibraryUnavailable(format!("Codec2 {bit_rate} is not available"))
        })?;
        let codec = Self {
            api,
            bit_rate,
            codec,
        };
        let samples = unsafe { (api.codec_samples)(codec.codec.as_ptr()) };
        let bytes = unsafe { (api.codec_bytes)(codec.codec.as_ptr()) };
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
        unsafe { (self.api.codec_decode)(self.codec.as_ptr(), bits.as_ptr(), pcm.as_mut_ptr()) };
    }
}

impl<const SAMPLES: usize, const BYTES: usize> Drop for Codec2<SAMPLES, BYTES> {
    fn drop(&mut self) {
        unsafe { (self.api.codec_destroy)(self.codec.as_ptr()) };
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
}
