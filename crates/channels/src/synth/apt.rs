use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_wire::AvhrrChannel;

use crate::apt::geometry::{
    CHANNEL_WEDGE, DEVIATION_HZ, FRAME_LINES, IMAGE_START, LINE_WORDS, MARKER_LINES, MINUTE_LINES,
    SPACE_START, SUBCARRIER_HZ, SYNC_A, SYNC_B, SYNC_WORDS, Side, WEDGE_LINES, WORD_RATE_HZ,
    ZERO_WEDGE, wedge_level,
};

pub const FRAME_PHASE: u16 = 64;
const MODULATION_FLOOR: f64 = 0.13;
const SIDE_A_TELEMETRY: [u8; 6] = [104, 106, 103, 105, 80, 30];
const SIDE_B_TELEMETRY: [u8; 6] = [120, 118, 121, 119, 82, 150];
const WHITE: u8 = 255;
const BLACK: u8 = 0;

fn wave(x: u16, y: u16, x_period: f64, y_period: f64) -> f64 {
    (TAU * (f64::from(x) / x_period + f64::from(y) / y_period)).sin()
}

#[must_use]
pub fn image_a(line: u16, x: u16) -> u8 {
    (128.0 + 100.0 * wave(x, line, 300.0, 64.0)).round() as u8
}

#[must_use]
pub fn image_b(line: u16, x: u16) -> u8 {
    let ripple = (TAU * f64::from(x) / 180.0).cos() * (TAU * f64::from(line) / 96.0).cos();
    (128.0 + 60.0 * ripple + 40.0 * wave(x, line, 450.0, 200.0)).round() as u8
}

fn wedge(line: u16) -> usize {
    (usize::from(line) + usize::from(FRAME_PHASE)) % FRAME_LINES / WEDGE_LINES + 1
}

fn telemetry_level(line: u16, channel: AvhrrChannel, extra: &[u8; 6]) -> u8 {
    match wedge(line) {
        CHANNEL_WEDGE => wedge_level(usize::from(channel.wedge())),
        index if index > ZERO_WEDGE => extra[index - ZERO_WEDGE - 1],
        index => wedge_level(index),
    }
}

fn minute_marker(line: u16) -> bool {
    usize::from(line) % MINUTE_LINES < MARKER_LINES
}

type Pattern = fn(u16, u16) -> u8;

fn fill_side(
    words: &mut [u8; LINE_WORDS],
    side: Side,
    line: u16,
    channel: AvhrrChannel,
    extra: &[u8; 6],
) {
    let base = side.offset();
    let (sync, space, image): (&[bool; SYNC_WORDS], u8, Pattern) = match side {
        Side::A => (&SYNC_A, BLACK, image_a),
        Side::B => (&SYNC_B, WHITE, image_b),
    };
    for (word, &high) in sync.iter().enumerate() {
        words[base + word] = if high { WHITE } else { BLACK };
    }
    let space = if minute_marker(line) {
        WHITE - space
    } else {
        space
    };
    words[base + SPACE_START..base + IMAGE_START].fill(space);
    for (x, word) in words[side.image()].iter_mut().enumerate() {
        *word = image(line, x as u16);
    }
    words[side.telemetry()].fill(telemetry_level(line, channel, extra));
}

#[must_use]
pub fn line_words(line: u16, channel_a: AvhrrChannel, channel_b: AvhrrChannel) -> [u8; LINE_WORDS] {
    let mut words = [0u8; LINE_WORDS];
    fill_side(&mut words, Side::A, line, channel_a, &SIDE_A_TELEMETRY);
    fill_side(&mut words, Side::B, line, channel_b, &SIDE_B_TELEMETRY);
    words
}

#[must_use]
pub fn audio(lines: u16, channel_a: AvhrrChannel, channel_b: AvhrrChannel, rate: f64) -> Vec<f32> {
    let total_words = usize::from(lines) * LINE_WORDS;
    let len = (total_words as f64 / WORD_RATE_HZ * rate).round() as usize;
    let mut current = u16::MAX;
    let mut words = [0u8; LINE_WORDS];
    (0..len)
        .map(|k| {
            let t = k as f64 / rate;
            let index = ((t * WORD_RATE_HZ) as usize).min(total_words - 1);
            let line = (index / LINE_WORDS) as u16;
            if line != current {
                current = line;
                words = line_words(line, channel_a, channel_b);
            }
            let level = f64::from(words[index % LINE_WORDS]) / f64::from(WHITE);
            let amplitude = MODULATION_FLOOR + (1.0 - MODULATION_FLOOR) * level;
            let phase = (TAU * SUBCARRIER_HZ * t).rem_euclid(TAU);
            (amplitude * phase.sin()) as f32
        })
        .collect()
}

#[must_use]
pub fn transmission(
    lines: u16,
    channel_a: AvhrrChannel,
    channel_b: AvhrrChannel,
    rate: f64,
) -> Vec<Complex<f32>> {
    super::fm_modulate(
        &audio(lines, channel_a, channel_b, rate),
        DEVIATION_HZ,
        rate,
    )
}

pub fn doppler(iq: &mut [Complex<f32>], start_hz: f64, end_hz: f64, rate: f64) {
    let len = iq.len().max(1) as f64;
    let mut phase = 0.0f64;
    for (k, sample) in iq.iter_mut().enumerate() {
        let freq = start_hz + (end_hz - start_hz) * k as f64 / len;
        phase = (phase + TAU * freq / rate).rem_euclid(TAU);
        *sample *= Complex::from_polar(1.0, phase as f32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_lasts_half_a_second() {
        let iq = transmission(2, AvhrrChannel::Ch2, AvhrrChannel::Ch4, 60_000.0);
        assert_eq!(iq.len(), 60_000);
    }

    #[test]
    fn wedge_16_carries_the_channel_level() {
        let line = (FRAME_LINES - WEDGE_LINES) as u16 - FRAME_PHASE;
        let words = line_words(line, AvhrrChannel::Ch3b, AvhrrChannel::Ch4);
        assert_eq!(words[Side::A.telemetry().start], wedge_level(6));
        assert_eq!(words[Side::B.telemetry().start], wedge_level(4));
    }

    #[test]
    fn minute_markers_invert_the_space() {
        let marked = line_words(0, AvhrrChannel::Ch2, AvhrrChannel::Ch4);
        let plain = line_words(5, AvhrrChannel::Ch2, AvhrrChannel::Ch4);
        assert_eq!(marked[SPACE_START], WHITE);
        assert_eq!(plain[SPACE_START], BLACK);
    }
}
