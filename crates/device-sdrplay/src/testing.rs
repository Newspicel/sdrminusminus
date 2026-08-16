use std::{
    cell::UnsafeCell,
    ffi::{c_char, c_int, c_uint, c_void},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};

use sdrmm_device::{DeviceError, lock};

use crate::{
    api::{DevHandle, InitOutcome, Sdrplay},
    ffi,
};

#[derive(Clone, Copy)]
struct ContextPtr(*mut c_void);

unsafe impl Send for ContextPtr {}

pub struct FakeApi {
    devices: Mutex<Vec<ffi::DeviceT>>,
    dev_params: Box<UnsafeCell<ffi::DevParamsT>>,
    channel_a: Box<UnsafeCell<ffi::RxChannelParamsT>>,
    channel_b: Box<UnsafeCell<ffi::RxChannelParamsT>>,
    tree: Box<UnsafeCell<ffi::DeviceParamsT>>,
    path: PathBuf,
    selected: AtomicBool,
    streaming: AtomicBool,
    pending_inits: AtomicU32,
    updates: Mutex<Vec<(c_int, c_uint, c_uint)>>,
    callbacks: Mutex<Option<(ffi::CallbackFnsT, ContextPtr)>>,
}

// The fake owns the parameter tree the real API would own; tests drive it from one thread at a
// time, the same contract the vendor library documents.
unsafe impl Send for FakeApi {}
unsafe impl Sync for FakeApi {}

const FAKE_HANDLE: usize = 0xD1CE;

fn serial_bytes(serial: &str) -> [c_char; ffi::MAX_SER_NO_LEN] {
    let mut bytes = [0; ffi::MAX_SER_NO_LEN];
    for (slot, byte) in bytes.iter_mut().zip(serial.as_bytes()) {
        *slot = *byte as c_char;
    }
    bytes
}

impl FakeApi {
    #[must_use]
    pub fn with_devices(devices: Vec<ffi::DeviceT>) -> Self {
        let mut channel = ffi::RxChannelParamsT::default();
        channel.tuner_params.rf_freq.rf_hz = 200_000_000.0;
        channel.tuner_params.bw_type = ffi::BW_0_200;
        channel.tuner_params.gain.gr_db = 50;
        channel.tuner_params.gain.min_gr = ffi::NORMAL_MIN_GR;
        channel.ctrl_params.agc.enable = ffi::AGC_50HZ;
        channel.ctrl_params.agc.set_point_dbfs = -60;
        channel.ctrl_params.dc_offset.dc_enable = 1;
        channel.ctrl_params.dc_offset.iq_enable = 1;
        channel.ctrl_params.decimation.decimation_factor = 1;
        let mut dev_params = ffi::DevParamsT::default();
        dev_params.fs_freq.fs_hz = 2_000_000.0;

        let fake = Self {
            devices: Mutex::new(devices),
            dev_params: Box::new(UnsafeCell::new(dev_params)),
            channel_a: Box::new(UnsafeCell::new(channel)),
            channel_b: Box::new(UnsafeCell::new(channel)),
            tree: Box::new(UnsafeCell::new(ffi::DeviceParamsT {
                dev_params: std::ptr::null_mut(),
                rx_channel_a: std::ptr::null_mut(),
                rx_channel_b: std::ptr::null_mut(),
            })),
            path: PathBuf::from("/usr/local/lib/libsdrplay_api.so.3"),
            selected: AtomicBool::new(false),
            streaming: AtomicBool::new(false),
            pending_inits: AtomicU32::new(0),
            updates: Mutex::new(Vec::new()),
            callbacks: Mutex::new(None),
        };
        let tree = fake.tree.get();
        unsafe {
            (*tree).dev_params = fake.dev_params.get();
            (*tree).rx_channel_a = fake.channel_a.get();
            (*tree).rx_channel_b = fake.channel_b.get();
        }
        fake
    }

    #[must_use]
    pub fn device(hw_ver: u8, serial: &str) -> ffi::DeviceT {
        ffi::DeviceT {
            ser_no: serial_bytes(serial),
            hw_ver,
            tuner: ffi::TUNER_A,
            valid: 1,
            ..ffi::DeviceT::default()
        }
    }

    #[must_use]
    pub fn duo(serial: &str, modes: c_int, tuners: c_int) -> ffi::DeviceT {
        ffi::DeviceT {
            ser_no: serial_bytes(serial),
            hw_ver: ffi::RSPDUO_ID,
            tuner: tuners,
            rsp_duo_mode: modes,
            valid: 1,
            ..ffi::DeviceT::default()
        }
    }

    #[must_use]
    pub fn rsp1a() -> Self {
        Self::with_devices(vec![Self::device(ffi::RSP1A_ID, "1234567890")])
    }

    #[must_use]
    pub fn dual_tuner_duo() -> Self {
        Self::with_devices(vec![Self::duo(
            "1809001DDD",
            ffi::DUO_MODE_SINGLE_TUNER | ffi::DUO_MODE_DUAL_TUNER | ffi::DUO_MODE_MASTER,
            ffi::TUNER_BOTH,
        )])
    }

    pub fn require_master_before_start(&self, inits: u32) {
        self.pending_inits.store(inits, Ordering::Release);
    }

    #[must_use]
    pub fn is_streaming(&self) -> bool {
        self.streaming.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn is_selected(&self) -> bool {
        self.selected.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn updates(&self) -> Vec<(c_int, c_uint, c_uint)> {
        lock(&self.updates).clone()
    }

    #[must_use]
    pub fn dev_params(&self) -> ffi::DevParamsT {
        unsafe { *self.dev_params.get() }
    }

    #[must_use]
    pub fn channel(&self, tuner: c_int) -> ffi::RxChannelParamsT {
        if tuner == ffi::TUNER_B {
            unsafe { *self.channel_b.get() }
        } else {
            unsafe { *self.channel_a.get() }
        }
    }

    pub fn emit(&self, tuner: c_int, samples: &[(i16, i16)]) {
        let Some((callbacks, context)) = *lock(&self.callbacks) else {
            return;
        };
        let callback = if tuner == ffi::TUNER_B {
            callbacks.stream_b
        } else {
            callbacks.stream_a
        };
        let Some(callback) = callback else {
            return;
        };
        let mut xi: Vec<i16> = samples.iter().map(|(i, _)| *i).collect();
        let mut xq: Vec<i16> = samples.iter().map(|(_, q)| *q).collect();
        let mut params = ffi::StreamCbParamsT {
            first_sample_num: 0,
            gr_changed: 0,
            rf_changed: 0,
            fs_changed: 0,
            num_samples: samples.len() as c_uint,
        };
        unsafe {
            callback(
                xi.as_mut_ptr(),
                xq.as_mut_ptr(),
                &raw mut params,
                samples.len() as c_uint,
                0,
                context.0,
            );
        }
    }

    pub fn raise(&self, event_id: c_int, tuner: c_int, mut params: ffi::EventParamsT) {
        let Some((callbacks, context)) = *lock(&self.callbacks) else {
            return;
        };
        let Some(event) = callbacks.event else {
            return;
        };
        unsafe { event(event_id, tuner, &raw mut params, context.0) };
    }

    pub fn unplug(&self) {
        self.raise(
            ffi::EVENT_DEVICE_REMOVED,
            ffi::TUNER_A,
            ffi::EventParamsT {
                rsp_duo_mode_params: 0,
            },
        );
    }

    pub fn master_started(&self) {
        self.raise(
            ffi::EVENT_RSPDUO_MODE_CHANGE,
            ffi::TUNER_A,
            ffi::EventParamsT {
                rsp_duo_mode_params: ffi::DUO_EVENT_MASTER_INITIALISED,
            },
        );
    }
}

impl Sdrplay for FakeApi {
    fn version(&self) -> f32 {
        crate::api::MIN_API_VERSION
    }

    fn library_path(&self) -> &Path {
        &self.path
    }

    fn get_devices(&self) -> Result<Vec<ffi::DeviceT>, DeviceError> {
        Ok(lock(&self.devices).clone())
    }

    fn select_device(&self, device: &mut ffi::DeviceT) -> Result<(), DeviceError> {
        if self.selected.swap(true, Ordering::AcqRel) {
            return Err(DeviceError::InUse("already selected".to_string()));
        }
        device.dev = std::ptr::without_provenance_mut(FAKE_HANDLE);
        Ok(())
    }

    fn release_device(&self, _device: &mut ffi::DeviceT) -> Result<(), DeviceError> {
        self.selected.store(false, Ordering::Release);
        Ok(())
    }

    fn device_params(&self, dev: DevHandle) -> Result<*mut ffi::DeviceParamsT, DeviceError> {
        if dev.0.addr() != FAKE_HANDLE {
            return Err(DeviceError::NotFound("no such handle".to_string()));
        }
        Ok(self.tree.get())
    }

    fn init(
        &self,
        _dev: DevHandle,
        callbacks: &ffi::CallbackFnsT,
        context: *mut c_void,
    ) -> Result<InitOutcome, DeviceError> {
        *lock(&self.callbacks) = Some((*callbacks, ContextPtr(context)));
        if self
            .pending_inits
            .try_update(Ordering::AcqRel, Ordering::Acquire, |pending| {
                pending.checked_sub(1)
            })
            .is_ok()
        {
            return Ok(InitOutcome::MasterPending);
        }
        self.streaming.store(true, Ordering::Release);
        Ok(InitOutcome::Started)
    }

    fn uninit(&self, _dev: DevHandle) -> Result<(), DeviceError> {
        self.streaming.store(false, Ordering::Release);
        *lock(&self.callbacks) = None;
        Ok(())
    }

    fn update(
        &self,
        _dev: DevHandle,
        tuner: c_int,
        reason: c_uint,
        ext1: c_uint,
    ) -> Result<(), DeviceError> {
        lock(&self.updates).push((tuner, reason, ext1));
        Ok(())
    }
}
