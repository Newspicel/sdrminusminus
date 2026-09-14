use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use sdrmm_device::DeviceError;

const EXTRA_MODULE_PATH: &str = "SDRMM_SOAPY_MODULE_PATH";

/// Vendor modules whose device search reaches onto the network are staged beside the others, so
/// that finding a radio on this machine does not wait out a DNS-SD sweep meant for another one.
const NETWORK_SUFFIX: &str = "-network";

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
    let extra: Vec<PathBuf> = [EXTRA_MODULE_PATH, "SOAPY_SDR_PLUGIN_PATH"]
        .iter()
        .filter_map(std::env::var_os)
        .flat_map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .collect();
    let network = network_modules(modules);
    let search = search_path(modules, &extra, network.as_deref(), |path| path.is_dir())
        .map_err(|error| DeviceError::Io(error.to_string()))?;
    unsafe { std::env::set_var("SOAPY_SDR_ROOT", root) };
    unsafe { std::env::set_var("SOAPY_SDR_PLUGIN_PATH", &search) };
    Ok(())
}

fn network_modules(modules: &Path) -> Option<PathBuf> {
    let name = modules.file_name()?.to_str()?;
    Some(modules.with_file_name(format!("{name}{NETWORK_SUFFIX}")))
}

/// Takes the network-searching modules back out of the ones this process will load, for a search
/// that is only meant to reach this machine.
///
/// # Safety
/// The caller must invoke this during single-threaded process startup, before any other thread
/// can read or write the process environment and before the first SoapySDR call, which is what
/// loads the modules.
pub(crate) unsafe fn hide_network_modules() {
    let Some(value) = std::env::var_os("SOAPY_SDR_PLUGIN_PATH") else {
        return;
    };
    let kept: Vec<PathBuf> = std::env::split_paths(&value)
        .filter(|path| !is_network_modules(path))
        .collect();
    let Ok(joined) = std::env::join_paths(&kept) else {
        tracing::warn!("cannot take the network modules out of the soapy module path");
        return;
    };
    unsafe { std::env::set_var("SOAPY_SDR_PLUGIN_PATH", joined) };
}

fn is_network_modules(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(NETWORK_SUFFIX))
}

fn search_path(
    bundled: &Path,
    extra: &[PathBuf],
    network: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
) -> Result<OsString, std::env::JoinPathsError> {
    let mut ordered: Vec<&Path> = Vec::new();
    let candidates = extra
        .iter()
        .map(PathBuf::as_path)
        .chain(std::iter::once(bundled))
        .chain(network);
    for candidate in candidates {
        if exists(candidate) && !ordered.contains(&candidate) {
            ordered.push(candidate);
        }
    }
    std::env::join_paths(ordered)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(path: &OsString) -> Vec<PathBuf> {
        std::env::split_paths(path).collect()
    }

    #[test]
    fn search_path_puts_operator_dirs_before_the_bundled_tree() {
        let bundled = PathBuf::from("/app/soapy/lib/SoapySDR/modules0.8");
        let extra = vec![PathBuf::from("/opt/extra")];
        let joined = search_path(&bundled, &extra, None, |_| true).expect("join");
        assert_eq!(parts(&joined), vec![PathBuf::from("/opt/extra"), bundled]);
    }

    #[test]
    fn search_path_drops_missing_and_duplicate_directories() {
        let bundled = PathBuf::from("/app/modules0.8");
        let extra = vec![bundled.clone(), PathBuf::from("/gone")];
        let joined =
            search_path(&bundled, &extra, None, |path| path != Path::new("/gone")).expect("join");
        assert_eq!(parts(&joined), vec![bundled]);
    }

    #[test]
    fn the_network_modules_sit_beside_the_ones_every_search_loads() {
        assert_eq!(
            network_modules(Path::new("/app/soapy/lib/SoapySDR/modules0.8")),
            Some(PathBuf::from("/app/soapy/lib/SoapySDR/modules0.8-network"))
        );
        assert_eq!(network_modules(Path::new("/")), None);
    }

    #[test]
    fn search_path_leaves_host_directories_out_of_a_bundled_install() {
        let bundled = PathBuf::from("/app/modules0.8");
        let joined = search_path(&bundled, &[], None, |_| true).expect("join");
        assert_eq!(
            parts(&joined),
            vec![bundled],
            "a bundled install must load only its own modules unless an operator opts in"
        );
    }

    #[test]
    fn a_network_staged_module_is_on_the_path_that_opens_a_radio() {
        let bundled = PathBuf::from("/app/modules0.8");
        let network = PathBuf::from("/app/modules0.8-network");
        let joined = search_path(&bundled, &[], Some(&network), |_| true).expect("join");
        assert_eq!(
            parts(&joined),
            vec![bundled, network],
            "a device a search can find must be one this process can open"
        );
    }

    #[test]
    fn a_network_directory_that_is_not_staged_is_left_off() {
        let bundled = PathBuf::from("/app/modules0.8");
        let network = PathBuf::from("/app/modules0.8-network");
        let joined =
            search_path(&bundled, &[], Some(&network), |path| path == bundled).expect("join");
        assert_eq!(parts(&joined), vec![bundled]);
    }

    #[test]
    fn only_the_network_staging_directory_is_taken_out_of_a_local_search() {
        assert!(is_network_modules(Path::new("/app/modules0.8-network")));
        assert!(!is_network_modules(Path::new("/app/modules0.8")));
        assert!(!is_network_modules(Path::new("/opt/extra")));
    }
}
