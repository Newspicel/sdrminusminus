use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use sdrmm_device::DeviceError;

/// Extra module directories to search, ahead of the bundled ones.
const EXTRA_MODULE_PATH: &str = "SDRMM_SOAPY_MODULE_PATH";

/// Details from the loaded SoapySDR core, used by `sdrmm --doctor` and package smoke tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeInfo {
    pub core_version: String,
    pub search_paths: Vec<String>,
    pub modules: Vec<String>,
}

#[must_use]
pub fn runtime_info() -> RuntimeInfo {
    RuntimeInfo {
        core_version: soapysdr::library_version(),
        search_paths: soapysdr::module_search_paths(),
        modules: soapysdr::list_modules(),
    }
}

/// Select the application's private Soapy tree before the first enumerate or device open.
///
/// The bundled modules are not the only ones searched. A module the package cannot carry — a
/// distribution's UHD, a SoapySDRPlay3 built against a newer vendor API — is picked up from the
/// host's own module directories, and `SDRMM_SOAPY_MODULE_PATH` names directories that come
/// first. A module built against a different SoapySDR ABI is refused by the core with a logged
/// error rather than loaded, so an incompatible host module costs a log line.
///
/// # Safety
/// The caller must invoke this during single-threaded process startup, before any other thread
/// can read or write the process environment and before constructing a [`SoapyDriver`].
///
/// [`SoapyDriver`]: crate::SoapyDriver
pub unsafe fn configure_bundled_runtime(root: &Path, modules: &Path) -> Result<(), DeviceError> {
    if !modules.is_dir() {
        return Err(DeviceError::Io(format!(
            "bundled Soapy module directory is missing: {}",
            modules.display()
        )));
    }
    // A search path the operator already set is kept rather than replaced: this function runs
    // inside a packaged app, and a container or a Nix wrapper that pointed SoapySDR somewhere
    // deliberate should not lose that by being packaged.
    let extra: Vec<PathBuf> = [EXTRA_MODULE_PATH, "SOAPY_SDR_PLUGIN_PATH"]
        .iter()
        .filter_map(std::env::var_os)
        .flat_map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .collect();
    let search = search_path(modules, &extra, &host_module_dirs(), |path| path.is_dir())
        .map_err(|error| DeviceError::Io(error.to_string()))?;
    unsafe { std::env::set_var("SOAPY_SDR_ROOT", root) };
    unsafe { std::env::set_var("SOAPY_SDR_PLUGIN_PATH", &search) };
    #[cfg(target_os = "windows")]
    unsafe {
        prepend_vendor_library_dirs();
    }
    Ok(())
}

/// The module search path, in load order: directories the operator named, then the bundled tree,
/// then the host's own. Duplicates and directories that do not exist are dropped — SoapySDR
/// reports its search path verbatim, and a missing entry in `sdrmm --doctor` reads as a packaging
/// fault.
fn search_path(
    bundled: &Path,
    extra: &[PathBuf],
    host: &[PathBuf],
    exists: impl Fn(&Path) -> bool,
) -> Result<OsString, std::env::JoinPathsError> {
    let mut ordered: Vec<&Path> = Vec::new();
    let candidates = extra
        .iter()
        .map(PathBuf::as_path)
        .chain(std::iter::once(bundled))
        .chain(host.iter().map(PathBuf::as_path));
    for candidate in candidates {
        if exists(candidate) && !ordered.contains(&candidate) {
            ordered.push(candidate);
        }
    }
    std::env::join_paths(ordered)
}

/// Where a platform's own SoapySDR installation keeps its modules. The ABI directory is part of
/// the name, so a module for another SoapySDR generation is never even a candidate.
fn host_module_dirs() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    let roots = ["/usr/local/lib", "/opt/homebrew/lib", "/opt/local/lib"];
    #[cfg(target_os = "linux")]
    let roots = [
        "/usr/local/lib",
        "/usr/lib",
        "/usr/lib64",
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib/aarch64-linux-gnu",
        "/usr/lib/arm-linux-gnueabihf",
    ];
    #[cfg(target_os = "windows")]
    let roots = ["C:\\Program Files\\SoapySDR\\lib"];
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let roots: [&str; 0] = [];
    roots
        .iter()
        .map(|root| Path::new(root).join("SoapySDR").join("modules0.8"))
        .collect()
}

/// Windows resolves a module's imports through the process search path, and the SDRplay
/// installer leaves `sdrplay_api.dll` in its own directory. Putting that directory on `PATH` is
/// what lets the staged SoapySDRPlay3 module load; everywhere else the loader's own defaults
/// already cover the vendor library.
#[cfg(target_os = "windows")]
unsafe fn prepend_vendor_library_dirs() {
    let Some(program_files) = std::env::var_os("ProgramFiles") else {
        return;
    };
    let api = Path::new(&program_files)
        .join("SDRplay")
        .join("API")
        .join("x64");
    if !api.is_dir() {
        return;
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs = vec![api];
    dirs.extend(std::env::split_paths(&path));
    if let Ok(joined) = std::env::join_paths(dirs) {
        unsafe { std::env::set_var("PATH", joined) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(path: &OsString) -> Vec<PathBuf> {
        std::env::split_paths(path).collect()
    }

    #[test]
    fn search_path_puts_operator_dirs_first_and_bundled_before_host() {
        let bundled = PathBuf::from("/app/soapy/lib/SoapySDR/modules0.8");
        let extra = vec![PathBuf::from("/opt/extra")];
        let host = vec![PathBuf::from("/usr/local/lib/SoapySDR/modules0.8")];
        let joined = search_path(&bundled, &extra, &host, |_| true).expect("join");
        assert_eq!(
            parts(&joined),
            vec![
                PathBuf::from("/opt/extra"),
                bundled,
                PathBuf::from("/usr/local/lib/SoapySDR/modules0.8"),
            ]
        );
    }

    #[test]
    fn search_path_drops_missing_and_duplicate_directories() {
        let bundled = PathBuf::from("/app/modules0.8");
        let extra = vec![bundled.clone(), PathBuf::from("/gone")];
        let host = vec![
            bundled.clone(),
            PathBuf::from("/usr/lib/SoapySDR/modules0.8"),
        ];
        let joined =
            search_path(&bundled, &extra, &host, |path| path != Path::new("/gone")).expect("join");
        assert_eq!(
            parts(&joined),
            vec![bundled, PathBuf::from("/usr/lib/SoapySDR/modules0.8")]
        );
    }

    #[test]
    fn search_path_holds_the_bundled_tree_when_nothing_else_exists() {
        let bundled = PathBuf::from("/app/modules0.8");
        let host = host_module_dirs();
        let joined = search_path(&bundled, &[], &host, |path| path == bundled).expect("join");
        assert_eq!(parts(&joined), vec![bundled]);
    }

    #[test]
    fn host_module_dirs_all_name_the_abi_directory() {
        for dir in host_module_dirs() {
            assert!(dir.ends_with("SoapySDR/modules0.8"), "{}", dir.display());
        }
    }
}
