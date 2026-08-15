#![allow(clippy::expect_used)]

use mfsk_core::{
    ft4::encode as ft4, ft8::wave_gen as ft8, msg::wsjt77::pack77, wspr::synthesize_type1,
};
use num_complex::Complex;

const RATE: usize = 12_000;

fn analytic_fixture(audio: &[i16], slot_samples: usize, start_samples: usize) -> Vec<Complex<f32>> {
    let mut iq = vec![Complex::new(0.0, 0.0); slot_samples];
    for (target, &sample) in iq[start_samples..].iter_mut().zip(audio) {
        target.re = f32::from(sample) / f32::from(i16::MAX);
    }
    iq
}

#[must_use]
pub fn ft8_slot(call: &str, grid: &str, audio_hz: f32) -> Vec<Complex<f32>> {
    let message = pack77("CQ", call, grid).expect("test FT8 message must pack");
    let tones = ft8::message_to_tones(&message);
    let audio = ft8::tones_to_i16(&tones, audio_hz, 20_000);
    analytic_fixture(&audio, 15 * RATE, RATE / 2)
}

#[must_use]
pub fn ft4_slot(call: &str, grid: &str, audio_hz: f32) -> Vec<Complex<f32>> {
    let message = pack77("CQ", call, grid).expect("test FT4 message must pack");
    let tones = ft4::message_to_tones(&message);
    let audio = ft4::tones_to_i16(&tones, audio_hz, 20_000);
    analytic_fixture(&audio, 15 * RATE / 2, RATE / 2)
}

#[must_use]
pub fn wspr_slot(call: &str, grid: &str, power_dbm: i32, audio_hz: f32) -> Vec<Complex<f32>> {
    let audio = synthesize_type1(call, grid, power_dbm, RATE as u32, audio_hz, 0.7)
        .expect("test WSPR message must pack");
    let mut iq = vec![Complex::new(0.0, 0.0); 120 * RATE];
    for (target, sample) in iq[RATE..].iter_mut().zip(audio) {
        target.re = sample;
    }
    iq
}
