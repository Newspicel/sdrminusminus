use std::ffi::{c_char, c_double, c_int, c_void};

pub const CHANNEL_COUNT: usize = 8;
pub const SERIAL_LEN: usize = 16;
pub const SAMPLE_RATE_HZ: f64 = 12.5e6;

pub const CHAN_ALL: c_int = 0xFF;

pub const CLOCK_INTERNAL: c_int = 0x00;
pub const CLOCK_EXTERNAL: c_int = 0x01;

pub type Handle = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Info {
    pub serial: [c_char; SERIAL_LEN + 1],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DevInfo {
    pub hw_ver_major: u8,
    pub hw_ver_minor: u8,
    pub fw_ver_major: u8,
    pub fw_ver_minor: u8,
    pub fw_ver_build: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Complex {
    pub re: f32,
    pub im: f32,
}

pub type Callback =
    unsafe extern "C" fn(samples: *mut *mut Complex, count: usize, drops: usize, ctx: *mut c_void);

pub type ListDevicesFn = unsafe extern "C" fn(*mut *mut Info) -> c_int;
pub type FreeDeviceListFn = unsafe extern "C" fn(*mut Info);
pub type OpenFn = unsafe extern "C" fn(*mut Handle, *const c_char) -> c_int;
pub type CloseFn = unsafe extern "C" fn(Handle);
pub type GetDevInfoFn = unsafe extern "C" fn(Handle, *mut DevInfo);
pub type StartFn = unsafe extern "C" fn(Handle, usize, Callback, *mut c_void) -> c_int;
pub type StopFn = unsafe extern "C" fn(Handle) -> c_int;
pub type ChannelFn = unsafe extern "C" fn(Handle, c_int) -> c_int;
pub type SetFreqFn = unsafe extern "C" fn(Handle, c_int, c_double, bool) -> c_int;
pub type SetGainFn = unsafe extern "C" fn(Handle, c_int, c_int) -> c_int;
pub type SetClockFn = unsafe extern "C" fn(Handle, c_int) -> c_int;

/// The vendor library returns zero for success and a negative code otherwise, with no table of
/// meanings published, so the code is passed through rather than translated into a guess.
pub fn message(code: c_int, action: &str) -> String {
    format!("{action}: the CR-8 library returned {code}")
}
