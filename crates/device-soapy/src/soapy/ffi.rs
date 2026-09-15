use std::ffi::{c_char, c_double, c_int, c_void};

pub type Device = *mut c_void;
pub type Stream = *mut c_void;

pub const TX: c_int = 0;
pub const RX: c_int = 1;

pub const END_BURST: c_int = 1 << 1;
pub const HAS_TIME: c_int = 1 << 2;

pub const TIMEOUT: c_int = -1;
pub const STREAM_ERROR: c_int = -2;
pub const CORRUPTION: c_int = -3;
pub const OVERFLOW: c_int = -4;
pub const NOT_SUPPORTED: c_int = -5;
pub const TIME_ERROR: c_int = -6;
pub const UNDERFLOW: c_int = -7;

pub const ARG_INFO_BOOL: c_int = 0;
pub const ARG_INFO_INT: c_int = 1;
pub const ARG_INFO_FLOAT: c_int = 2;

pub const ABI_VERSION: &str = "0.8";
pub const FORMAT_CF32: &[u8] = b"CF32\0";

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Range {
    pub minimum: c_double,
    pub maximum: c_double,
    pub step: c_double,
}

#[repr(C)]
pub struct Kwargs {
    pub size: usize,
    pub keys: *mut *mut c_char,
    pub vals: *mut *mut c_char,
}

impl Kwargs {
    pub const fn empty() -> Self {
        Self {
            size: 0,
            keys: std::ptr::null_mut(),
            vals: std::ptr::null_mut(),
        }
    }
}

#[repr(C)]
pub struct ArgInfo {
    pub key: *mut c_char,
    pub value: *mut c_char,
    pub name: *mut c_char,
    pub description: *mut c_char,
    pub units: *mut c_char,
    pub data_type: c_int,
    pub range: Range,
    pub num_options: usize,
    pub options: *mut *mut c_char,
    pub option_names: *mut *mut c_char,
}

pub type StringList = unsafe extern "C" fn(Device, c_int, usize, *mut usize) -> *mut *mut c_char;
pub type RangeList = unsafe extern "C" fn(Device, c_int, usize, *mut usize) -> *mut Range;
pub type ArgInfoList = unsafe extern "C" fn(Device, c_int, usize, *mut usize) -> *mut ArgInfo;
pub type ChannelQuery = unsafe extern "C" fn(Device, c_int, usize) -> c_double;
pub type ChannelFlag = unsafe extern "C" fn(Device, c_int, usize) -> bool;
pub type ChannelString = unsafe extern "C" fn(Device, c_int, usize) -> *mut c_char;
pub type ChannelSet = unsafe extern "C" fn(Device, c_int, usize, c_double) -> c_int;
pub type ChannelKwargs = unsafe extern "C" fn(Device, c_int, usize) -> Kwargs;
pub type NamedSet = unsafe extern "C" fn(Device, c_int, usize, *const c_char, c_double) -> c_int;
pub type NamedQuery = unsafe extern "C" fn(Device, c_int, usize, *const c_char) -> c_double;
pub type DeviceStringList = unsafe extern "C" fn(Device, *mut usize) -> *mut *mut c_char;
pub type DeviceString = unsafe extern "C" fn(Device) -> *mut c_char;
pub type ModuleStringList = unsafe extern "C" fn(*mut usize) -> *mut *mut c_char;
pub type StaticString = unsafe extern "C" fn() -> *const c_char;
