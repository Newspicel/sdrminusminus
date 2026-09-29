use std::f64::consts::{PI, TAU};

use sdrmm_dsp::golay23_encode;

const DCS_BAUD: f64 = 134.4;
const DCS_WORD_BITS: u32 = 23;
const DCS_SIGNATURE: u16 = 0b100;

#[must_use]
pub fn ctcss_audio(hz: f64, deviation: f32, rate: f64, len: usize) -> Vec<f32> {
    super::tone_audio(hz, deviation, rate, len)
}

#[must_use]
pub fn dcs_audio(code: u16, deviation: f32, rate: f64, len: usize) -> Vec<f32> {
    let digits = u32::from(code);
    let raw = (digits / 100 % 10) << 6 | (digits / 10 % 10) << 3 | (digits % 10);
    let word = golay23_encode((u32::from(DCS_SIGNATURE) << 9 | raw) as u16);
    let samples_per_bit = rate / DCS_BAUD;
    (0..len)
        .map(|k| {
            let bit = (k as f64 / samples_per_bit) as u32 % DCS_WORD_BITS;
            if word >> bit & 1 == 1 {
                deviation
            } else {
                -deviation
            }
        })
        .collect()
}

#[must_use]
pub fn mix(a: &[f32], b: &[f32]) -> Vec<f32> {
    a.iter().zip(b).map(|(x, y)| x + y).collect()
}

const FORMANTS: [(f64, f64); 3] = [(500.0, 80.0), (1_500.0, 110.0), (2_500.0, 170.0)];
const SPEECH_LOW_HZ: f64 = 300.0;
const SPEECH_HIGH_HZ: f64 = 3_000.0;

fn resonance(hz: f64, centre: f64, bandwidth: f64) -> f64 {
    let ratio = hz / centre;
    let real = 1.0 - ratio * ratio;
    let imag = ratio * bandwidth / centre;
    1.0 / real.hypot(imag)
}

fn harmonic_gain(hz: f64) -> f64 {
    if hz <= SPEECH_LOW_HZ * 0.5 || hz >= SPEECH_HIGH_HZ * 1.15 {
        return 0.0;
    }
    let edges = taper(hz, SPEECH_LOW_HZ * 0.5, SPEECH_LOW_HZ)
        * taper(SPEECH_HIGH_HZ * 1.15 - hz, 0.0, SPEECH_HIGH_HZ * 0.15);
    let tilt = 1.0 / (1.0 + hz / 400.0);
    let formants: f64 = FORMANTS
        .iter()
        .map(|&(centre, bandwidth)| resonance(hz, centre, bandwidth))
        .product();
    edges * tilt * formants
}

fn taper(hz: f64, start: f64, end: f64) -> f64 {
    let t = ((hz - start) / (end - start)).clamp(0.0, 1.0);
    0.5 - 0.5 * (PI * t).cos()
}

#[must_use]
pub fn speech_audio(rate: f64, len: usize) -> Vec<f32> {
    let mut phase = 0.0;
    let mut out = Vec::with_capacity(len);
    for k in 0..len {
        let t = k as f64 / rate;
        let pitch_hz = 120.0 + 15.0 * (TAU * 1.7 * t).sin();
        phase += TAU * pitch_hz / rate;
        let syllable = 0.55 + 0.45 * (TAU * 3.3 * t).sin();
        let mut sample = 0.0;
        let mut harmonic = 1;
        loop {
            let hz = f64::from(harmonic) * pitch_hz;
            if hz >= SPEECH_HIGH_HZ * 1.15 {
                break;
            }
            sample += harmonic_gain(hz) * (f64::from(harmonic) * phase).sin();
            harmonic += 1;
        }
        out.push((syllable * sample) as f32);
    }
    let peak = out.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
    if peak > 0.0 {
        for s in &mut out {
            *s *= 0.5 / peak;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dcs_waveform_is_the_published_word_sent_backwards() {
        let rate = DCS_BAUD;
        let audio = dcs_audio(23, 1.0, rate, DCS_WORD_BITS as usize);
        let sent: String = audio
            .iter()
            .map(|&s| if s > 0.0 { '1' } else { '0' })
            .collect();
        let word: String = "10000001001111101100011".chars().rev().collect();
        assert_eq!(sent, word);
    }

    #[test]
    fn the_word_repeats_for_as_long_as_it_is_asked_to() {
        let rate = DCS_BAUD * 4.0;
        let bits = DCS_WORD_BITS as usize * 3;
        let audio = dcs_audio(754, 0.2, rate, bits * 4);
        assert_eq!(audio.len(), bits * 4);
        for (k, &s) in audio.iter().enumerate() {
            assert_eq!(s.abs(), 0.2, "sample {k} is not a keyed level");
            let period = DCS_WORD_BITS as usize * 4;
            if k >= period {
                assert_eq!(s, audio[k - period], "word did not repeat at {k}");
            }
        }
    }
}
