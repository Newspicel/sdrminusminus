use std::ffi::{c_char, c_float, c_int, c_uint, c_void};

pub type Handle = *mut c_void;
pub type ErrT = c_int;

pub const SUCCESS: ErrT = 0;
pub const FAIL: ErrT = 1;
pub const OUT_OF_RANGE: ErrT = 3;
pub const HW_ERROR: ErrT = 7;
pub const ALREADY_INITIALISED: ErrT = 9;
pub const NOT_INITIALISED: ErrT = 10;
pub const HW_VER_ERROR: ErrT = 12;
pub const SERVICE_NOT_RESPONDING: ErrT = 14;
pub const START_PENDING: ErrT = 15;
pub const INVALID_MODE: ErrT = 17;
pub const INVALID_SERVICE_VERSION: ErrT = 24;

pub const MAX_DEVICES: usize = 16;
pub const MAX_SER_NO_LEN: usize = 64;
pub const MAX_BB_GR: i32 = 59;

pub const RSP1_ID: u8 = 1;
pub const RSP1A_ID: u8 = 255;
pub const RSP2_ID: u8 = 2;
pub const RSPDUO_ID: u8 = 3;
pub const RSPDX_ID: u8 = 4;
pub const RSP1B_ID: u8 = 6;
pub const RSPDXR2_ID: u8 = 7;

pub const TUNER_NEITHER: c_int = 0;
pub const TUNER_A: c_int = 1;
pub const TUNER_B: c_int = 2;
pub const TUNER_BOTH: c_int = 3;

pub const DUO_MODE_UNKNOWN: c_int = 0;
pub const DUO_MODE_SINGLE_TUNER: c_int = 1;
pub const DUO_MODE_DUAL_TUNER: c_int = 2;
pub const DUO_MODE_MASTER: c_int = 4;
pub const DUO_MODE_SLAVE: c_int = 8;

pub const DUO_AMPORT_1: c_int = 1;
pub const DUO_AMPORT_2: c_int = 0;

pub const RSP2_ANTENNA_A: c_int = 5;
pub const RSP2_ANTENNA_B: c_int = 6;
pub const RSP2_AMPORT_1: c_int = 1;
pub const RSP2_AMPORT_2: c_int = 0;

pub const RSPDX_ANTENNA_A: c_int = 0;
pub const RSPDX_ANTENNA_B: c_int = 1;
pub const RSPDX_ANTENNA_C: c_int = 2;

pub const BW_0_200: c_int = 200;
pub const BW_0_300: c_int = 300;
pub const BW_0_600: c_int = 600;
pub const BW_1_536: c_int = 1536;
pub const BW_5_000: c_int = 5000;
pub const BW_6_000: c_int = 6000;
pub const BW_7_000: c_int = 7000;
pub const BW_8_000: c_int = 8000;

pub const IF_ZERO: c_int = 0;
pub const IF_1_620: c_int = 1620;

pub const NORMAL_MIN_GR: c_int = 20;

pub const AGC_DISABLE: c_int = 0;
pub const AGC_100HZ: c_int = 1;
pub const AGC_50HZ: c_int = 2;
pub const AGC_5HZ: c_int = 3;

pub const UPDATE_NONE: c_uint = 0x0000_0000;
pub const UPDATE_DEV_FS: c_uint = 0x0000_0001;
pub const UPDATE_DEV_PPM: c_uint = 0x0000_0002;
pub const UPDATE_RSP1A_BIAS_T: c_uint = 0x0000_0010;
pub const UPDATE_RSP1A_RF_NOTCH: c_uint = 0x0000_0020;
pub const UPDATE_RSP1A_RF_DAB_NOTCH: c_uint = 0x0000_0040;
pub const UPDATE_RSP2_BIAS_T: c_uint = 0x0000_0080;
pub const UPDATE_RSP2_AM_PORT: c_uint = 0x0000_0100;
pub const UPDATE_RSP2_ANTENNA: c_uint = 0x0000_0200;
pub const UPDATE_RSP2_RF_NOTCH: c_uint = 0x0000_0400;
pub const UPDATE_RSP2_EXT_REF: c_uint = 0x0000_0800;
pub const UPDATE_RSPDUO_EXT_REF: c_uint = 0x0000_1000;
pub const UPDATE_TUNER_GR: c_uint = 0x0000_8000;
pub const UPDATE_TUNER_FRF: c_uint = 0x0002_0000;
pub const UPDATE_TUNER_BW_TYPE: c_uint = 0x0004_0000;
pub const UPDATE_TUNER_IF_TYPE: c_uint = 0x0008_0000;
pub const UPDATE_CTRL_DC_OFFSET_IQ_IMBALANCE: c_uint = 0x0040_0000;
pub const UPDATE_CTRL_DECIMATION: c_uint = 0x0080_0000;
pub const UPDATE_CTRL_AGC: c_uint = 0x0100_0000;
pub const UPDATE_CTRL_OVERLOAD_MSG_ACK: c_uint = 0x0400_0000;
pub const UPDATE_RSPDUO_BIAS_T: c_uint = 0x0800_0000;
pub const UPDATE_RSPDUO_AM_PORT: c_uint = 0x1000_0000;
pub const UPDATE_RSPDUO_TUNER1_AM_NOTCH: c_uint = 0x2000_0000;
pub const UPDATE_RSPDUO_RF_NOTCH: c_uint = 0x4000_0000;
pub const UPDATE_RSPDUO_RF_DAB_NOTCH: c_uint = 0x8000_0000;

pub const UPDATE_EXT1_NONE: c_uint = 0x0000_0000;
pub const UPDATE_EXT1_RSPDX_HDR_ENABLE: c_uint = 0x0000_0001;
pub const UPDATE_EXT1_RSPDX_BIAS_T: c_uint = 0x0000_0002;
pub const UPDATE_EXT1_RSPDX_ANTENNA: c_uint = 0x0000_0004;
pub const UPDATE_EXT1_RSPDX_RF_NOTCH: c_uint = 0x0000_0008;
pub const UPDATE_EXT1_RSPDX_RF_DAB_NOTCH: c_uint = 0x0000_0010;
pub const UPDATE_EXT1_RSPDX_HDR_BW: c_uint = 0x0000_0020;

pub const EVENT_POWER_OVERLOAD_CHANGE: c_int = 1;
pub const EVENT_DEVICE_REMOVED: c_int = 2;
pub const EVENT_RSPDUO_MODE_CHANGE: c_int = 3;
pub const EVENT_DEVICE_FAILURE: c_int = 4;

pub const OVERLOAD_DETECTED: c_int = 0;

pub const DUO_EVENT_MASTER_INITIALISED: c_int = 0;
pub const DUO_EVENT_SLAVE_DETACHED: c_int = 2;
pub const DUO_EVENT_MASTER_DLL_DISAPPEARED: c_int = 5;
pub const DUO_EVENT_SLAVE_DLL_DISAPPEARED: c_int = 6;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GainValuesT {
    pub curr: c_float,
    pub max: c_float,
    pub min: c_float,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GainT {
    pub gr_db: c_int,
    pub lna_state: u8,
    pub sync_update: u8,
    pub min_gr: c_int,
    pub gain_vals: GainValuesT,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RfFreqT {
    pub rf_hz: f64,
    pub sync_update: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct DcOffsetTunerT {
    pub dc_cal: u8,
    pub speed_up: u8,
    pub track_time: c_int,
    pub refresh_rate_time: c_int,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TunerParamsT {
    pub bw_type: c_int,
    pub if_type: c_int,
    pub lo_mode: c_int,
    pub gain: GainT,
    pub rf_freq: RfFreqT,
    pub dc_offset_tuner: DcOffsetTunerT,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct DcOffsetT {
    pub dc_enable: u8,
    pub iq_enable: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct DecimationT {
    pub enable: u8,
    pub decimation_factor: u8,
    pub wide_band_signal: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AgcT {
    pub enable: c_int,
    pub set_point_dbfs: c_int,
    pub attack_ms: u16,
    pub decay_ms: u16,
    pub decay_delay_ms: u16,
    pub decay_threshold_db: u16,
    pub sync_update: c_int,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ControlParamsT {
    pub dc_offset: DcOffsetT,
    pub decimation: DecimationT,
    pub agc: AgcT,
    pub adsb_mode: c_int,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Rsp1aTunerParamsT {
    pub bias_t_enable: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Rsp2TunerParamsT {
    pub bias_t_enable: u8,
    pub am_port_sel: c_int,
    pub antenna_sel: c_int,
    pub rf_notch_enable: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RspDuoResetSlaveFlagsT {
    pub reset_gain_update: u8,
    pub reset_rf_update: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RspDuoTunerParamsT {
    pub bias_t_enable: u8,
    pub tuner1_am_port_sel: c_int,
    pub tuner1_am_notch_enable: u8,
    pub rf_notch_enable: u8,
    pub rf_dab_notch_enable: u8,
    pub reset_slave_flags: RspDuoResetSlaveFlagsT,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RspDxTunerParamsT {
    pub hdr_bw: c_int,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RxChannelParamsT {
    pub tuner_params: TunerParamsT,
    pub ctrl_params: ControlParamsT,
    pub rsp1a_tuner_params: Rsp1aTunerParamsT,
    pub rsp2_tuner_params: Rsp2TunerParamsT,
    pub rsp_duo_tuner_params: RspDuoTunerParamsT,
    pub rsp_dx_tuner_params: RspDxTunerParamsT,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FsFreqT {
    pub fs_hz: f64,
    pub sync_update: u8,
    pub re_cal: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SyncUpdateT {
    pub sample_num: c_uint,
    pub period: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ResetFlagsT {
    pub reset_gain_update: u8,
    pub reset_rf_update: u8,
    pub reset_fs_update: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Rsp1aParamsT {
    pub rf_notch_enable: u8,
    pub rf_dab_notch_enable: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Rsp2ParamsT {
    pub ext_ref_output_en: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RspDuoParamsT {
    pub ext_ref_output_en: c_int,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RspDxParamsT {
    pub hdr_enable: u8,
    pub bias_t_enable: u8,
    pub antenna_sel: c_int,
    pub rf_notch_enable: u8,
    pub rf_dab_notch_enable: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct DevParamsT {
    pub ppm: f64,
    pub fs_freq: FsFreqT,
    pub sync_update: SyncUpdateT,
    pub reset_flags: ResetFlagsT,
    pub mode: c_int,
    pub samples_per_pkt: c_uint,
    pub rsp1a_params: Rsp1aParamsT,
    pub rsp2_params: Rsp2ParamsT,
    pub rsp_duo_params: RspDuoParamsT,
    pub rsp_dx_params: RspDxParamsT,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DeviceT {
    pub ser_no: [c_char; MAX_SER_NO_LEN],
    pub hw_ver: u8,
    pub tuner: c_int,
    pub rsp_duo_mode: c_int,
    pub valid: u8,
    pub rsp_duo_sample_freq: f64,
    pub dev: Handle,
}

impl Default for DeviceT {
    fn default() -> Self {
        Self {
            ser_no: [0; MAX_SER_NO_LEN],
            hw_ver: 0,
            tuner: TUNER_NEITHER,
            rsp_duo_mode: DUO_MODE_UNKNOWN,
            valid: 0,
            rsp_duo_sample_freq: 0.0,
            dev: std::ptr::null_mut(),
        }
    }
}

impl DeviceT {
    #[must_use]
    pub fn serial(&self) -> String {
        let bytes: Vec<u8> = self
            .ser_no
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| *byte as u8)
            .collect();
        String::from_utf8_lossy(&bytes).trim().to_string()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DeviceParamsT {
    pub dev_params: *mut DevParamsT,
    pub rx_channel_a: *mut RxChannelParamsT,
    pub rx_channel_b: *mut RxChannelParamsT,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct StreamCbParamsT {
    pub first_sample_num: c_uint,
    pub gr_changed: c_int,
    pub rf_changed: c_int,
    pub fs_changed: c_int,
    pub num_samples: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GainCbParamT {
    pub gr_db: c_uint,
    pub lna_gr_db: c_uint,
    pub curr_gain: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union EventParamsT {
    pub gain_params: GainCbParamT,
    pub power_overload_params: c_int,
    pub rsp_duo_mode_params: c_int,
}

pub type StreamCallback = unsafe extern "C" fn(
    xi: *mut i16,
    xq: *mut i16,
    params: *mut StreamCbParamsT,
    num_samples: c_uint,
    reset: c_uint,
    context: *mut c_void,
);

pub type EventCallback = unsafe extern "C" fn(
    event_id: c_int,
    tuner: c_int,
    params: *mut EventParamsT,
    context: *mut c_void,
);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CallbackFnsT {
    pub stream_a: Option<StreamCallback>,
    pub stream_b: Option<StreamCallback>,
    pub event: Option<EventCallback>,
}

const fn assert_layout<T>(size: usize, align: usize) {
    assert!(size_of::<T>() == size);
    assert!(align_of::<T>() == align);
}

const _: () = {
    assert_layout::<GainT>(24, 4);
    assert_layout::<RfFreqT>(16, 8);
    assert_layout::<TunerParamsT>(72, 8);
    assert_layout::<AgcT>(20, 4);
    assert_layout::<ControlParamsT>(32, 4);
    assert_layout::<Rsp2TunerParamsT>(16, 4);
    assert_layout::<RspDuoTunerParamsT>(16, 4);
    assert_layout::<RxChannelParamsT>(144, 8);
    assert_layout::<FsFreqT>(16, 8);
    assert_layout::<RspDxParamsT>(12, 4);
    assert_layout::<DevParamsT>(64, 8);
    assert_layout::<DeviceT>(96, 8);
    assert_layout::<StreamCbParamsT>(20, 4);
    assert_layout::<GainCbParamT>(16, 8);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_stops_at_the_first_nul() {
        let mut device = DeviceT::default();
        for (slot, byte) in device.ser_no.iter_mut().zip(b"1234567890\0junk") {
            *slot = *byte as c_char;
        }
        assert_eq!(device.serial(), "1234567890");
    }

    #[test]
    fn an_empty_serial_reads_as_empty() {
        assert_eq!(DeviceT::default().serial(), "");
    }
}
