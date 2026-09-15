use std::{
    ffi::{c_int, c_long, c_void},
    marker::PhantomData,
};

use super::{device::Device, ffi, types::Error};

/// A sample type the SoapySDR stream API can carry, named by the format string it is sent under.
///
/// # Safety
/// `FORMAT` must name the SoapySDR format whose element layout is exactly `Self`, or the driver
/// writes a differently sized element into buffers laid out for this one.
pub unsafe trait StreamSample: Copy {
    const FORMAT: &'static [u8];
}

unsafe impl StreamSample for num_complex::Complex<f32> {
    const FORMAT: &'static [u8] = ffi::FORMAT_CF32;
}

fn timeout(micros: i64) -> c_long {
    c_long::try_from(micros).unwrap_or(c_long::MAX)
}

struct Handle {
    device: Device,
    stream: ffi::Stream,
}

// The C API documents stream handles as usable from the thread that drives them; the engine
// hands each stream to exactly one capture thread.
unsafe impl Send for Handle {}

impl Handle {
    fn open(
        device: &Device,
        direction: super::Direction,
        channels: &[usize],
        format: &[u8],
    ) -> Result<Self, Error> {
        let args = ffi::Kwargs::empty();
        let stream = unsafe {
            (device.library().entries.setup_stream)(
                device.handle(),
                direction.into(),
                format.as_ptr().cast::<std::ffi::c_char>(),
                channels.as_ptr(),
                channels.len(),
                &raw const args,
            )
        };
        let stream = device.check(stream)?;
        if stream.is_null() {
            return Err(Error::other("SoapySDR returned no stream"));
        }
        Ok(Self {
            device: device.clone(),
            stream,
        })
    }

    fn mtu(&self) -> Result<usize, Error> {
        self.device.check(unsafe {
            (self.device.library().entries.stream_mtu)(self.device.handle(), self.stream)
        })
    }

    fn activate(&mut self, time_ns: Option<i64>) -> Result<(), Error> {
        let flags = if time_ns.is_some() { ffi::HAS_TIME } else { 0 };
        self.device.returned(unsafe {
            (self.device.library().entries.activate_stream)(
                self.device.handle(),
                self.stream,
                flags,
                time_ns.unwrap_or(0),
                0,
            )
        })
    }

    fn deactivate(&mut self, time_ns: Option<i64>) -> Result<(), Error> {
        let flags = if time_ns.is_some() { ffi::HAS_TIME } else { 0 };
        self.device.returned(unsafe {
            (self.device.library().entries.deactivate_stream)(
                self.device.handle(),
                self.stream,
                flags,
                time_ns.unwrap_or(0),
            )
        })
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            (self.device.library().entries.close_stream)(self.device.handle(), self.stream);
        }
    }
}

fn count(device: &Device, result: c_int) -> Result<usize, Error> {
    usize::try_from(result).map_err(|_| device.failure(result))
}

pub struct RxStream<S: StreamSample> {
    handle: Handle,
    channels: usize,
    sample: PhantomData<S>,
}

impl<S: StreamSample> RxStream<S> {
    pub(crate) fn open(device: &Device, channels: &[usize]) -> Result<Self, Error> {
        Ok(Self {
            handle: Handle::open(device, super::Direction::Rx, channels, S::FORMAT)?,
            channels: channels.len(),
            sample: PhantomData,
        })
    }

    pub fn mtu(&self) -> Result<usize, Error> {
        self.handle.mtu()
    }

    pub fn activate(&mut self, time_ns: Option<i64>) -> Result<(), Error> {
        self.handle.activate(time_ns)
    }

    pub fn deactivate(&mut self, time_ns: Option<i64>) -> Result<(), Error> {
        self.handle.deactivate(time_ns)
    }

    pub fn read(&mut self, buffers: &mut [&mut [S]], timeout_us: i64) -> Result<usize, Error> {
        if buffers.len() != self.channels {
            return Err(Error::unsupported(format!(
                "this stream has {} channels, got {} buffers",
                self.channels,
                buffers.len()
            )));
        }
        let elements = buffers.iter().map(|buffer| buffer.len()).min().unwrap_or(0);
        let pointers: Vec<*mut c_void> = buffers
            .iter_mut()
            .map(|buffer| buffer.as_mut_ptr().cast::<c_void>())
            .collect();
        let mut flags: c_int = 0;
        let mut time_ns: i64 = 0;
        let device = &self.handle.device;
        let read = unsafe {
            (device.library().entries.read_stream)(
                device.handle(),
                self.handle.stream,
                pointers.as_ptr(),
                elements,
                &raw mut flags,
                &raw mut time_ns,
                timeout(timeout_us),
            )
        };
        count(device, read)
    }
}

pub struct TxStream<S: StreamSample> {
    handle: Handle,
    channels: usize,
    sample: PhantomData<S>,
}

impl<S: StreamSample> TxStream<S> {
    pub(crate) fn open(device: &Device, channels: &[usize]) -> Result<Self, Error> {
        Ok(Self {
            handle: Handle::open(device, super::Direction::Tx, channels, S::FORMAT)?,
            channels: channels.len(),
            sample: PhantomData,
        })
    }

    pub fn activate(&mut self, time_ns: Option<i64>) -> Result<(), Error> {
        self.handle.activate(time_ns)
    }

    pub fn deactivate(&mut self, time_ns: Option<i64>) -> Result<(), Error> {
        self.handle.deactivate(time_ns)
    }

    pub fn write(
        &mut self,
        buffers: &[&[S]],
        time_ns: Option<i64>,
        end_burst: bool,
        timeout_us: i64,
    ) -> Result<usize, Error> {
        if buffers.len() != self.channels {
            return Err(Error::unsupported(format!(
                "this stream has {} channels, got {} buffers",
                self.channels,
                buffers.len()
            )));
        }
        let elements = buffers.iter().map(|buffer| buffer.len()).min().unwrap_or(0);
        let pointers: Vec<*const c_void> = buffers
            .iter()
            .map(|buffer| buffer.as_ptr().cast::<c_void>())
            .collect();
        let mut flags = c_int::from(time_ns.is_some()) * ffi::HAS_TIME;
        if end_burst {
            flags |= ffi::END_BURST;
        }
        let device = &self.handle.device;
        let written = unsafe {
            (device.library().entries.write_stream)(
                device.handle(),
                self.handle.stream,
                pointers.as_ptr(),
                elements,
                &raw mut flags,
                time_ns.unwrap_or(0),
                timeout(timeout_us),
            )
        };
        count(device, written)
    }

    pub fn read_status(
        &mut self,
        channels: &mut usize,
        flags: &mut i32,
        time_ns: &mut i64,
        timeout_us: i64,
    ) -> Result<usize, Error> {
        let device = &self.handle.device;
        let result = unsafe {
            (device.library().entries.read_stream_status)(
                device.handle(),
                self.handle.stream,
                &raw mut *channels,
                &raw mut *flags,
                &raw mut *time_ns,
                timeout(timeout_us),
            )
        };
        if result == 0 {
            return Ok(0);
        }
        count(device, result)
    }
}

#[cfg(test)]
mod tests {
    use super::{super::types::ErrorCode, *};

    #[test]
    fn a_timeout_larger_than_the_platform_long_saturates() {
        assert_eq!(timeout(0), 0);
        assert_eq!(timeout(100_000), 100_000);
        assert_eq!(timeout(i64::MAX), c_long::MAX);
    }

    #[test]
    fn the_sample_format_is_the_one_the_engine_streams() {
        assert_eq!(
            <num_complex::Complex<f32> as StreamSample>::FORMAT,
            b"CF32\0"
        );
    }

    #[test]
    fn an_error_code_keeps_its_meaning_through_the_count_helper() {
        assert_eq!(ErrorCode::from_raw(ffi::TIMEOUT), ErrorCode::Timeout);
    }
}
