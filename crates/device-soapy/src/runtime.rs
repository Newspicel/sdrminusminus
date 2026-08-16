use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use sdrmm_device::DeviceError;

const EXTRA_MODULE_PATH: &str = "SDRMM_SOAPY_MODULE_PATH";

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
    let search = search_path(modules, &extra, |path| path.is_dir())
        .map_err(|error| DeviceError::Io(error.to_string()))?;
    unsafe { std::env::set_var("SOAPY_SDR_ROOT", root) };
    unsafe { std::env::set_var("SOAPY_SDR_PLUGIN_PATH", &search) };
    Ok(())
}

fn search_path(
    bundled: &Path,
    extra: &[PathBuf],
    exists: impl Fn(&Path) -> bool,
) -> Result<OsString, std::env::JoinPathsError> {
    let mut ordered: Vec<&Path> = Vec::new();
    let candidates = extra
        .iter()
        .map(PathBuf::as_path)
        .chain(std::iter::once(bundled));
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
        let joined = search_path(&bundled, &extra, |_| true).expect("join");
        assert_eq!(parts(&joined), vec![PathBuf::from("/opt/extra"), bundled]);
    }

    #[test]
    fn search_path_drops_missing_and_duplicate_directories() {
        let bundled = PathBuf::from("/app/modules0.8");
        let extra = vec![bundled.clone(), PathBuf::from("/gone")];
        let joined =
            search_path(&bundled, &extra, |path| path != Path::new("/gone")).expect("join");
        assert_eq!(parts(&joined), vec![bundled]);
    }

    #[test]
    fn search_path_leaves_host_directories_out_of_a_bundled_install() {
        let bundled = PathBuf::from("/app/modules0.8");
        let joined = search_path(&bundled, &[], |_| true).expect("join");
        assert_eq!(
            parts(&joined),
            vec![bundled],
            "a bundled install must load only its own modules unless an operator opts in"
        );
    }
}
