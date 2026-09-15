mod api;
mod args;
mod device;
mod ffi;
mod stream;
mod types;

pub use args::Args;
pub use device::Device;
pub use stream::{RxStream, TxStream};
pub use types::{ArgInfo, ArgType, Direction, Error, ErrorCode, Range};

/// Where the SoapySDR runtime was loaded from, and the version it reports.
pub fn installed() -> Result<(String, std::path::PathBuf), String> {
    api::shared().map(|library| (library.version().to_string(), library.path().to_path_buf()))
}

pub fn enumerate(filter: &str) -> Result<Vec<Args>, Error> {
    let library = api::shared().map_err(Error::other)?;
    let markup = device::cstring(filter)?;
    let mut length: usize = 0;
    let found = unsafe { (library.entries.enumerate)(markup.as_ptr(), &raw mut length) };
    if found.is_null() {
        return Ok(Vec::new());
    }
    let args = unsafe { std::slice::from_raw_parts(found, length) }
        .iter()
        .map(|raw| unsafe { Args::from_kwargs(raw) })
        .collect();
    unsafe { (library.entries.kwargs_list_clear)(found, length) };
    Ok(args)
}

/// The module directories the installed SoapySDR core will search.
#[must_use]
pub fn module_search_paths() -> Vec<String> {
    string_list(|library, length| unsafe { (library.entries.list_search_paths)(length) })
}

/// The loadable modules found in those search paths.
#[must_use]
pub fn list_modules() -> Vec<String> {
    string_list(|library, length| unsafe { (library.entries.list_modules)(length) })
}

fn string_list(
    call: impl FnOnce(&api::Library, *mut usize) -> *mut *mut std::ffi::c_char,
) -> Vec<String> {
    let Ok(library) = api::shared() else {
        return Vec::new();
    };
    let mut length: usize = 0;
    let mut found = call(&library, &raw mut length);
    if found.is_null() {
        return Vec::new();
    }
    let list = unsafe { std::slice::from_raw_parts(found, length) }
        .iter()
        .map(|&text| unsafe { types::required_string(text) })
        .collect();
    unsafe { (library.entries.strings_clear)(&raw mut found, length) };
    list
}
