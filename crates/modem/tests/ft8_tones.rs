#![allow(clippy::unwrap_used, clippy::expect_used)]

use num_complex::Complex;
use sdrmm_modem::{
    cpm::{CpmMod, CpmParams, Mapping},
    orthogonal::{MfskDemod, MfskParams},
    pulse::{self, Norm},
    soft::argmax,
};
use sdrmm_modem_test_support::ber::rng::Rng;

const RATE_HZ: f64 = 12_000.0;
const TONE_SPACING_HZ: f64 = 6.25;
const SYMBOL_S: f64 = 0.16;
const SPS: f64 = RATE_HZ * SYMBOL_S;
const TONES: usize = 8;
const FRAME_SYMBOLS: usize = 79;
const COSTAS: [u8; 7] = [3, 1, 4, 0, 6, 5, 2];
const COSTAS_AT: [usize; 3] = [0, 36, 72];
const GRAY: [u8; 8] = [0, 1, 3, 2, 5, 6, 4, 7];
const BT: f64 = 2.0;
const PULSE_SPAN: usize = 3;
const DELAY_SYMBOLS: usize = 1;

fn plan() -> MfskParams {
    MfskParams::orthogonal(TONES, SPS)
}

fn ft8_params() -> CpmParams {
    CpmParams::from_h(
        Mapping::natural(TONES),
        1.0,
        pulse::gaussian_freq(SPS, BT, PULSE_SPAN, Norm::Area),
        SPS,
    )
}

fn transmit(symbols: &[u8]) -> Vec<Complex<f32>> {
    let mut modulator = CpmMod::new(ft8_params());
    let mut out = Vec::new();
    modulator.modulate(symbols, &mut out);
    modulator.flush(&mut out);
    out
}

fn frame(seed: u32) -> Vec<u8> {
    let mut state = seed | 1;
    let mut symbols = vec![0u8; FRAME_SYMBOLS];
    for (i, s) in symbols.iter_mut().enumerate() {
        let sync = COSTAS_AT
            .iter()
            .find(|&&at| (at..at + COSTAS.len()).contains(&i));
        *s = match sync {
            Some(&at) => COSTAS[i - at],
            None => {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state % TONES as u32) as u8
            }
        };
    }
    symbols
}

fn add_noise(wave: &mut [Complex<f32>], seed: u64, noise_var: f64) {
    let mut rng = Rng::new(seed);
    let sigma = (noise_var / 2.0).sqrt();
    for s in wave.iter_mut() {
        *s += Complex::new((rng.normal() * sigma) as f32, (rng.normal() * sigma) as f32);
    }
}

fn delay_samples() -> usize {
    DELAY_SYMBOLS * SPS as usize
}

#[test]
fn the_published_costas_array_is_one() {
    let mut seen = std::collections::HashSet::new();
    for (i, &from) in COSTAS.iter().enumerate() {
        for (j, &to) in COSTAS.iter().enumerate().skip(i + 1) {
            let displacement = ((j - i) as i32, i32::from(to) - i32::from(from));
            assert!(
                seen.insert(displacement),
                "displacement {displacement:?} occurs twice: {COSTAS:?} is not a Costas array"
            );
        }
    }
    let mut tones = COSTAS;
    tones.sort_unstable();
    assert_eq!(tones, [0, 1, 2, 3, 4, 5, 6]);
}

#[test]
fn the_published_gray_map_makes_neighbouring_tones_one_bit_apart() {
    let mut sorted = GRAY;
    sorted.sort_unstable();
    assert_eq!(sorted, [0, 1, 2, 3, 4, 5, 6, 7], "not a permutation");
    let mut bits_of = [0u8; 8];
    for (group, &tone) in GRAY.iter().enumerate() {
        bits_of[tone as usize] = group as u8;
    }
    for tone in 0..7 {
        let differing = (bits_of[tone] ^ bits_of[tone + 1]).count_ones();
        assert_eq!(
            differing,
            1,
            "tones {tone} and {} carry groups {} and {}, which differ in {differing} bits",
            tone + 1,
            bits_of[tone],
            bits_of[tone + 1]
        );
    }
}

#[test]
fn the_tone_plan_reproduces_the_published_geometry() {
    let plan = plan();
    assert_eq!(plan.window(), 1_920, "1920 samples per symbol at 12 kHz");
    let hz = |k: usize| plan.tone_cycles_per_sample(k) * RATE_HZ;
    let spacing = hz(1) - hz(0);
    assert!(
        (spacing - TONE_SPACING_HZ).abs() < 1e-9,
        "tone spacing {spacing} Hz vs the published {TONE_SPACING_HZ}"
    );
    assert!((spacing * SYMBOL_S - 1.0).abs() < 1e-12);
    let bandwidth = hz(TONES - 1) - hz(0) + TONE_SPACING_HZ;
    assert!(
        (bandwidth - 50.0).abs() < 1e-9,
        "occupied {bandwidth} Hz vs 50"
    );
    let transmission_s = FRAME_SYMBOLS as f64 * SYMBOL_S;
    assert!(
        (transmission_s - 12.64).abs() < 1e-9,
        "{transmission_s} s vs 12.64"
    );
}

#[test]
fn a_full_frame_round_trips_tone_for_tone() {
    let symbols = frame(0x5eed);
    let wave = transmit(&symbols);
    assert_eq!(
        wave.len(),
        (FRAME_SYMBOLS + PULSE_SPAN) * SPS as usize,
        "frame length"
    );
    let mut decoded = Vec::new();
    MfskDemod::new(plan()).demodulate(&wave, delay_samples(), FRAME_SYMBOLS, &mut decoded);
    assert_eq!(decoded, symbols);
    for &at in &COSTAS_AT {
        assert_eq!(&decoded[at..at + COSTAS.len()], &COSTAS, "sync at {at}");
    }
}

#[test]
fn the_costas_arrays_locate_the_frame_under_noise() {
    let symbols = frame(0xc057);
    let lead = 5usize;
    let mut wave = transmit(&[vec![0u8; lead], symbols.clone()].concat());
    let es: f64 = SPS;
    add_noise(&mut wave, 0xc057, es / 10f64.powf(0.8));

    let demod = MfskDemod::new(plan());
    let mut energies = [0.0f32; TONES];
    let score = |at: usize, energies: &mut [f32]| -> f64 {
        COSTAS
            .iter()
            .enumerate()
            .map(|(k, &tone)| {
                demod.energies(&wave, delay_samples(), at + k, energies);
                let total: f64 = energies.iter().map(|&e| f64::from(e)).sum();
                let want = f64::from(energies[tone as usize]);
                want - (total - want) / (TONES - 1) as f64
            })
            .sum()
    };
    let mut scored: Vec<(usize, f64)> = (0..FRAME_SYMBOLS + lead)
        .map(|at| (at, score(at, &mut energies)))
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut best: Vec<usize> = scored[..3].iter().map(|&(at, _)| at).collect();
    best.sort_unstable();
    let expected: Vec<usize> = COSTAS_AT.iter().map(|&at| at + lead).collect();
    assert_eq!(best, expected, "the three best sync positions");
}

#[test]
fn the_raw_symbol_error_rate_at_the_published_threshold_matches_theory() {
    const CODED_BITS: f64 = 174.0;
    let s_over_n0 = 2500.0 * 10f64.powf(-21.0 / 10.0);
    let es_over_n0 = s_over_n0 * SYMBOL_S;
    let ebn0_db = 10.0 * (es_over_n0 / 3.0).log10();
    let coded_ebn0_db =
        10.0 * (s_over_n0 / (CODED_BITS / (FRAME_SYMBOLS as f64 * SYMBOL_S))).log10();
    assert!(
        (ebn0_db - 0.24).abs() < 0.05 && (coded_ebn0_db - 1.58).abs() < 0.05,
        "the published threshold works out to {ebn0_db} dB raw / {coded_ebn0_db} dB coded"
    );

    let symbols = frame(0x7e57);
    let clean = transmit(&symbols);
    let es: f64 = clean
        .iter()
        .skip(delay_samples())
        .take(FRAME_SYMBOLS * SPS as usize)
        .map(|s| f64::from(s.norm_sqr()))
        .sum::<f64>()
        / FRAME_SYMBOLS as f64;

    let demod = MfskDemod::new(plan());
    let mut energies = [0.0f32; TONES];
    let (mut errors, mut trials) = (0u32, 0u32);
    for trial in 0..40u64 {
        let mut wave = clean.clone();
        add_noise(&mut wave, 0x7e57 + trial, es / es_over_n0);
        for (k, &sent) in symbols.iter().enumerate() {
            demod.energies(&wave, delay_samples(), k, &mut energies);
            trials += 1;
            if argmax(&energies) != sent {
                errors += 1;
            }
        }
    }
    let measured = f64::from(errors) / f64::from(trials);
    let theory = sdrmm_modem_test_support::ber::theory::mfsk_noncoherent_ser(8, ebn0_db);
    assert!(
        (measured / theory - 1.0).abs() < 0.15,
        "measured raw SER {measured:.4} vs exact noncoherent 8-FSK {theory:.4} at {ebn0_db:.2} dB"
    );
    assert!(
        measured > 0.1,
        "raw SER {measured} at the published threshold"
    );
}
