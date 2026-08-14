use num_complex::Complex;
use sdrmm_dsp::window::hann;

use super::ook;

/// Longest edge transition, in seconds. Real CW transmitters shape the key line at roughly
/// this rate to keep the click sidebands inside a few hundred Hz.
const RISE_S: f64 = 5e-3;

const TABLE: &[(char, &str)] = &[
    ('A', ".-"),
    ('B', "-..."),
    ('C', "-.-."),
    ('D', "-.."),
    ('E', "."),
    ('F', "..-."),
    ('G', "--."),
    ('H', "...."),
    ('I', ".."),
    ('J', ".---"),
    ('K', "-.-"),
    ('L', ".-.."),
    ('M', "--"),
    ('N', "-."),
    ('O', "---"),
    ('P', ".--."),
    ('Q', "--.-"),
    ('R', ".-."),
    ('S', "..."),
    ('T', "-"),
    ('U', "..-"),
    ('V', "...-"),
    ('W', ".--"),
    ('X', "-..-"),
    ('Y', "-.--"),
    ('Z', "--.."),
    ('0', "-----"),
    ('1', ".----"),
    ('2', "..---"),
    ('3', "...--"),
    ('4', "....-"),
    ('5', "....."),
    ('6', "-...."),
    ('7', "--..."),
    ('8', "---.."),
    ('9', "----."),
    ('.', ".-.-.-"),
    (',', "--..--"),
    ('?', "..--.."),
    ('\'', ".----."),
    ('!', "-.-.--"),
    ('/', "-..-."),
    ('(', "-.--."),
    (')', "-.--.-"),
    ('&', ".-..."),
    (':', "---..."),
    (';', "-.-.-."),
    ('=', "-...-"),
    ('+', ".-.-."),
    ('-', "-....-"),
    ('_', "..--.-"),
    ('"', ".-..-."),
    ('$', "...-..-"),
    ('@', ".--.-."),
];

fn pattern(ch: char) -> Option<&'static str> {
    let upper = ch.to_ascii_uppercase();
    TABLE.iter().find(|(c, _)| *c == upper).map(|(_, p)| *p)
}

/// Ideal on/off keying envelope for `text` at `wpm`: marks at 1.0, gaps at 0.0, with the
/// standard 1/3/7 dot-unit element, letter and word spacing. Characters outside the table are
/// skipped. No lead-in or lead-out — callers add their own silence.
#[must_use]
pub fn envelope(text: &str, wpm: f32, rate: f64) -> Vec<f32> {
    assert!(wpm > 0.0 && wpm.is_finite(), "wpm must be positive");
    assert!(rate > 0.0, "sample rate must be positive");
    let dot = 1.2 * rate / f64::from(wpm);

    let mut out = Vec::new();
    let mut t = 0.0f64;
    let mut run = |out: &mut Vec<f32>, units: f64, level: f32| {
        t += units * dot;
        // Accumulating in time and rounding at each boundary keeps the element grid free of
        // the drift a per-element `round(units * dot)` would compound.
        out.resize((t.round() as usize).max(out.len()), level);
    };

    let mut gap_units = 0.0;
    for ch in text.chars() {
        if ch == ' ' {
            if !out.is_empty() {
                gap_units = 7.0;
            }
            continue;
        }
        let Some(pat) = pattern(ch) else { continue };
        if !out.is_empty() {
            run(&mut out, gap_units, 0.0);
        }
        for (i, e) in pat.chars().enumerate() {
            if i > 0 {
                run(&mut out, 1.0, 0.0);
            }
            run(&mut out, if e == '-' { 3.0 } else { 1.0 }, 1.0);
        }
        gap_units = 3.0;
    }
    out
}

/// `text` keyed onto a `tone_hz` carrier as complex baseband IQ, with raised-cosine edges so
/// the spectrum stays inside a CW filter.
#[must_use]
pub fn transmission(text: &str, wpm: f32, tone_hz: f64, rate: f64) -> Vec<Complex<f32>> {
    let key = envelope(text, wpm, rate);
    ook(&shape_edges(&key, wpm, rate), tone_hz, rate)
}

/// Smooth the hard keying envelope with a symmetric raised-cosine kernel. Symmetry matters:
/// it puts the half-amplitude point exactly on the ideal element boundary, so shaping does
/// not bias the durations a decoder measures.
fn shape_edges(key: &[f32], wpm: f32, rate: f64) -> Vec<f32> {
    let dot = 1.2 * rate / f64::from(wpm);
    let taps = ((RISE_S * rate).min(dot / 4.0).round() as usize).max(1) | 1;
    let kernel: Vec<f32> = hann(taps + 1).into_iter().skip(1).collect();
    let norm: f32 = kernel.iter().sum();
    let half = taps / 2;
    key.iter()
        .enumerate()
        .map(|(k, _)| {
            let acc: f32 = kernel
                .iter()
                .enumerate()
                .map(|(j, &w)| {
                    w * (k + j)
                        .checked_sub(half)
                        .and_then(|i| key.get(i))
                        .unwrap_or(&0.0)
                })
                .sum();
            acc / norm
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 8_000.0;
    const WPM: f32 = 20.0;
    /// 20 wpm ⇒ 60 ms dots.
    const DOT: usize = 480;

    #[test]
    fn envelope_uses_paris_element_and_letter_timing() {
        let env = envelope("EE", WPM, RATE);
        assert_eq!(env.len(), 5 * DOT);
        assert!(env[..DOT].iter().all(|&v| v == 1.0));
        assert!(env[DOT..4 * DOT].iter().all(|&v| v == 0.0));
        assert!(env[4 * DOT..].iter().all(|&v| v == 1.0));
    }

    #[test]
    fn word_gap_is_seven_dots_and_leading_space_is_dropped() {
        assert_eq!(envelope("E E", WPM, RATE).len(), 9 * DOT);
        assert_eq!(envelope(" E", WPM, RATE).len(), DOT);
    }

    #[test]
    fn shaped_edges_keep_the_half_amplitude_point_on_the_boundary() {
        let iq = transmission("E", WPM, 0.0, RATE);
        assert_eq!(iq.len(), DOT);
        assert!((iq[0].norm() - 0.5).abs() < 0.05, "start {}", iq[0].norm());
        assert!((iq[DOT / 2].norm() - 1.0).abs() < 1e-3);
        let last = iq[DOT - 1].norm();
        assert!((0.45..0.6).contains(&last), "end {last}");
    }
}
