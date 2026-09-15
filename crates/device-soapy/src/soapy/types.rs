use std::ffi::{c_char, c_int};

pub use ffi::Range;

use super::ffi;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Direction {
    Tx,
    Rx,
}

impl From<Direction> for c_int {
    fn from(direction: Direction) -> Self {
        match direction {
            Direction::Tx => ffi::TX,
            Direction::Rx => ffi::RX,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ErrorCode {
    Timeout,
    StreamError,
    Corruption,
    Overflow,
    NotSupported,
    TimeError,
    Underflow,
    Other,
}

impl ErrorCode {
    pub(crate) const fn from_raw(code: c_int) -> Self {
        match code {
            ffi::TIMEOUT => Self::Timeout,
            ffi::STREAM_ERROR => Self::StreamError,
            ffi::CORRUPTION => Self::Corruption,
            ffi::OVERFLOW => Self::Overflow,
            ffi::NOT_SUPPORTED => Self::NotSupported,
            ffi::TIME_ERROR => Self::TimeError,
            ffi::UNDERFLOW => Self::Underflow,
            _ => Self::Other,
        }
    }
}

#[derive(Clone, Debug, Hash)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
}

impl Error {
    pub(crate) fn other(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::Other,
            message: message.into(),
        }
    }

    pub(crate) fn unsupported(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::NotSupported,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArgType {
    Bool,
    Float,
    Int,
    String,
}

impl ArgType {
    /// SoapySDR up to and including 0.8.1 ships an `ArgInfo` default constructor that leaves
    /// `type` uninitialised, so a driver pushing a default-constructed `ArgInfo` hands over
    /// whatever was on its stack. Anything unrecognised reads as the string the later
    /// constructor settled on.
    const fn from_raw(raw: c_int) -> Self {
        match raw {
            ffi::ARG_INFO_BOOL => Self::Bool,
            ffi::ARG_INFO_INT => Self::Int,
            ffi::ARG_INFO_FLOAT => Self::Float,
            _ => Self::String,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ArgInfo {
    pub key: String,
    pub value: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub units: Option<String>,
    pub data_type: ArgType,
    pub range: Option<Range>,
    pub options: Vec<(String, Option<String>)>,
}

pub(crate) unsafe fn required_string(pointer: *mut c_char) -> String {
    unsafe { optional_string(pointer) }.unwrap_or_default()
}

pub(crate) unsafe fn optional_string(pointer: *mut c_char) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    Some(
        unsafe { std::ffi::CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned(),
    )
}

unsafe fn options(raw: &ffi::ArgInfo) -> Vec<(String, Option<String>)> {
    if raw.num_options == 0 || raw.options.is_null() {
        return Vec::new();
    }
    let values = unsafe { std::slice::from_raw_parts(raw.options, raw.num_options) };
    let labels = if raw.option_names.is_null() {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(raw.option_names, raw.num_options) }
    };
    values
        .iter()
        .enumerate()
        .map(|(index, &value)| {
            let label = labels.get(index).copied().unwrap_or(std::ptr::null_mut());
            (unsafe { required_string(value) }, unsafe {
                optional_string(label)
            })
        })
        .collect()
}

pub(crate) unsafe fn arg_info(raw: &ffi::ArgInfo) -> ArgInfo {
    ArgInfo {
        key: unsafe { required_string(raw.key) },
        value: unsafe { required_string(raw.value) },
        name: unsafe { optional_string(raw.name) },
        description: unsafe { optional_string(raw.description) },
        units: unsafe { optional_string(raw.units) },
        data_type: ArgType::from_raw(raw.data_type),
        range: (raw.range.minimum != 0.0 || raw.range.maximum != 0.0 || raw.range.step != 0.0)
            .then_some(raw.range),
        options: unsafe { options(raw) },
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use super::*;

    fn owned(text: &str) -> *mut c_char {
        CString::new(text).expect("no null byte").into_raw()
    }

    unsafe fn release(pointer: *mut c_char) {
        if !pointer.is_null() {
            drop(unsafe { CString::from_raw(pointer) });
        }
    }

    fn blank() -> ffi::ArgInfo {
        ffi::ArgInfo {
            key: std::ptr::null_mut(),
            value: std::ptr::null_mut(),
            name: std::ptr::null_mut(),
            description: std::ptr::null_mut(),
            units: std::ptr::null_mut(),
            data_type: 0,
            range: ffi::Range::default(),
            num_options: 0,
            options: std::ptr::null_mut(),
            option_names: std::ptr::null_mut(),
        }
    }

    #[test]
    fn known_argument_types_map_across() {
        assert_eq!(ArgType::from_raw(ffi::ARG_INFO_BOOL), ArgType::Bool);
        assert_eq!(ArgType::from_raw(ffi::ARG_INFO_INT), ArgType::Int);
        assert_eq!(ArgType::from_raw(ffi::ARG_INFO_FLOAT), ArgType::Float);
    }

    #[test]
    fn an_uninitialised_argument_type_reads_as_a_string() {
        assert_eq!(ArgType::from_raw(0xdead_beefu32 as c_int), ArgType::String);
        assert_eq!(ArgType::from_raw(4), ArgType::String);
    }

    #[test]
    fn an_all_zero_range_is_no_range() {
        let raw = blank();
        assert!(unsafe { arg_info(&raw) }.range.is_none());
    }

    #[test]
    fn a_declared_range_survives() {
        let mut raw = blank();
        raw.range = ffi::Range {
            minimum: 0.0,
            maximum: 2.0,
            step: 1.0,
        };
        assert_eq!(unsafe { arg_info(&raw) }.range.expect("range").step, 1.0);
    }

    #[test]
    fn options_survive_a_missing_label_list() {
        let mut values = vec![owned("auto"), owned("meta")];
        let mut raw = blank();
        raw.key = owned("direct_samp");
        raw.num_options = values.len();
        raw.options = values.as_mut_ptr();
        let info = unsafe { arg_info(&raw) };
        assert_eq!(
            info.options,
            vec![("auto".to_string(), None), ("meta".to_string(), None)]
        );
        unsafe {
            release(raw.key);
            for value in values.drain(..) {
                release(value);
            }
        }
    }

    #[test]
    fn a_null_option_list_is_no_options() {
        let mut raw = blank();
        raw.num_options = 3;
        assert!(unsafe { arg_info(&raw) }.options.is_empty());
    }

    #[test]
    fn error_codes_map_from_the_c_constants() {
        assert_eq!(ErrorCode::from_raw(ffi::TIMEOUT), ErrorCode::Timeout);
        assert_eq!(ErrorCode::from_raw(ffi::OVERFLOW), ErrorCode::Overflow);
        assert_eq!(ErrorCode::from_raw(ffi::UNDERFLOW), ErrorCode::Underflow);
        assert_eq!(
            ErrorCode::from_raw(ffi::NOT_SUPPORTED),
            ErrorCode::NotSupported
        );
        assert_eq!(ErrorCode::from_raw(1), ErrorCode::Other);
    }

    #[test]
    fn a_direction_carries_the_constant_the_c_api_expects() {
        assert_eq!(c_int::from(Direction::Rx), ffi::RX);
        assert_eq!(c_int::from(Direction::Tx), ffi::TX);
    }
}
