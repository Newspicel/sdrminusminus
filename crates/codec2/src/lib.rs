use std::{
    ffi::{c_int, c_void},
    ptr::null_mut,
};

use codec2::{Codec2, Codec2Mode};

pub const ABI_VERSION: u32 = 1;

unsafe extern "C" {
    fn fdmdv_1600_create() -> *mut c_void;
    fn fdmdv_1600_destroy(modem: *mut c_void);
    fn fdmdv_1600_demod(
        modem: *mut c_void,
        input: *const f32,
        nin: c_int,
        output: *mut u8,
        sync: *mut c_int,
        reliable_sync: *mut c_int,
    ) -> c_int;
}

#[unsafe(no_mangle)]
extern "C" fn sdrmm_codec2_abi_version() -> u32 {
    ABI_VERSION
}

#[unsafe(no_mangle)]
extern "C" fn sdrmm_fdmdv_create() -> *mut c_void {
    unsafe { fdmdv_1600_create() }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn sdrmm_fdmdv_destroy(modem: *mut c_void) {
    unsafe { fdmdv_1600_destroy(modem) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn sdrmm_fdmdv_demod(
    modem: *mut c_void,
    input: *const f32,
    nin: c_int,
    output: *mut u8,
    sync: *mut c_int,
    reliable_sync: *mut c_int,
) -> c_int {
    unsafe { fdmdv_1600_demod(modem, input, nin, output, sync, reliable_sync) }
}

#[unsafe(no_mangle)]
extern "C" fn sdrmm_codec2_create(bit_rate: c_int) -> *mut c_void {
    let mode = match bit_rate {
        3200 => Codec2Mode::MODE_3200,
        1600 => Codec2Mode::MODE_1600,
        1300 => Codec2Mode::MODE_1300,
        _ => return null_mut(),
    };
    Box::into_raw(Box::new(Codec2::new(mode))).cast()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn sdrmm_codec2_destroy(codec: *mut c_void) {
    if !codec.is_null() {
        drop(unsafe { Box::from_raw(codec.cast::<Codec2>()) });
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn sdrmm_codec2_samples_per_frame(codec: *const c_void) -> c_int {
    let codec = unsafe { &*codec.cast::<Codec2>() };
    c_int::try_from(codec.samples_per_frame()).unwrap_or(0)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn sdrmm_codec2_bytes_per_frame(codec: *const c_void) -> c_int {
    let codec = unsafe { &*codec.cast::<Codec2>() };
    c_int::try_from(codec.bits_per_frame().div_ceil(8)).unwrap_or(0)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn sdrmm_codec2_decode(codec: *mut c_void, bits: *const u8, pcm: *mut i16) {
    let codec = unsafe { &mut *codec.cast::<Codec2>() };
    let samples = codec.samples_per_frame();
    let bytes = codec.bits_per_frame().div_ceil(8);
    let bits = unsafe { std::slice::from_raw_parts(bits, bytes) };
    let pcm = unsafe { std::slice::from_raw_parts_mut(pcm, samples) };
    codec.decode(pcm, bits);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_sizes_follow_the_bit_rate() {
        for (rate, samples, bytes) in [(3200, 160, 8), (1600, 320, 8), (1300, 320, 7)] {
            let codec = sdrmm_codec2_create(rate);
            assert!(!codec.is_null());
            unsafe {
                assert_eq!(sdrmm_codec2_samples_per_frame(codec), samples);
                assert_eq!(sdrmm_codec2_bytes_per_frame(codec), bytes);
                sdrmm_codec2_destroy(codec);
            }
        }
        assert!(sdrmm_codec2_create(700).is_null());
    }

    #[test]
    fn decoding_an_encoded_tone_yields_speech_energy() {
        let mut encoder = Codec2::new(Codec2Mode::MODE_3200);
        let tone: Vec<i16> = (0..160)
            .map(|n| (8_000.0 * (n as f32 * 0.2).sin()) as i16)
            .collect();
        let mut bits = [0u8; 8];
        let codec = sdrmm_codec2_create(3200);
        let mut pcm = [0i16; 160];
        let mut energy = 0i64;
        for _ in 0..10 {
            encoder.encode(&mut bits, &tone);
            unsafe { sdrmm_codec2_decode(codec, bits.as_ptr(), pcm.as_mut_ptr()) };
            energy += pcm.iter().map(|&s| i64::from(s).pow(2)).sum::<i64>();
        }
        unsafe { sdrmm_codec2_destroy(codec) };
        assert!(energy > 0);
    }

    #[test]
    fn the_modem_asks_for_a_nominal_frame_after_noise() {
        let modem = sdrmm_fdmdv_create();
        assert!(!modem.is_null());
        let input = [0.0f32; 320];
        let mut bits = [0u8; 32];
        let (mut sync, mut reliable) = (1, 1);
        let next = unsafe {
            sdrmm_fdmdv_demod(
                modem,
                input.as_ptr(),
                160,
                bits.as_mut_ptr(),
                &mut sync,
                &mut reliable,
            )
        };
        unsafe { sdrmm_fdmdv_destroy(modem) };
        assert!((1..=200).contains(&next));
        assert_eq!(sync, 0);
    }
}
