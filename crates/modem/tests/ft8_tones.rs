//! The orthogonal entry's golden-vector test: FT8 tone
//! demodulation against the published WSJT-X waveform definition.
//!
//! **What is published and therefore golden here** (Franke, Somerville & Taylor, *"The FT4 and
//! FT8 Communication Protocols"*, QEX July/August 2020 — the protocol's own reference
//! description):
//!
//! - 8-GFSK at BT = 2.0, tone spacing 6.25 Hz = 1/T, symbol length T = 0.16 s (1920 samples at
//!   12 000 Hz), so the tones are orthogonal at exactly one cycle per symbol.
//! - 79 symbols per transmission: 58 data symbols and three 7-symbol Costas sync arrays at
//!   symbol positions 0–6, 36–42 and 72–78, each the array **{3, 1, 4, 0, 6, 5, 2}**.
//! - Occupied bandwidth 8 × 6.25 = 50 Hz; transmission length 79 × 0.16 = 12.64 s.
//! - The three data bits of a symbol are Gray-coded through **{0, 1, 3, 2, 5, 6, 4, 7}**.
//! - A decode threshold of about −21 dB SNR measured in 2500 Hz.
//!
//! **What is not here, and why.** The 58 data symbols of a *particular* message are the output
//! of FT8's source encoding — 77 message bits, a CRC-14, and an LDPC(174, 91) whose parity
//! matrix is the protocol's, not this library's. That encoder is squarely out of
//! scope (§1.1: the library ends at recovered bits; channel coding lives beside the FEC in
//! `sdrmm-dsp`), so this file validates the *waveform and its demodulation* against the
//! published definition and stops there: no claim is made that any particular WSJT-X message
//! produces these symbols. Where a published number can still bite — the Costas array, the Gray
//! map, the geometry, the −21 dB threshold's implication for the raw symbol error rate — it is
//! asserted rather than described.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use num_complex::Complex;
use sdrmm_modem::{
    ber::rng::Rng,
    cpm::{CpmMod, CpmParams, Mapping},
    orthogonal::{MfskDemod, MfskParams},
    pulse::{self, Norm},
    soft::argmax,
};

// --- The published definition, as data -------------------------------------------------------

const RATE_HZ: f64 = 12_000.0;
const TONE_SPACING_HZ: f64 = 6.25;
const SYMBOL_S: f64 = 0.16;
const SPS: f64 = RATE_HZ * SYMBOL_S;
const TONES: usize = 8;
const FRAME_SYMBOLS: usize = 79;
const COSTAS: [u8; 7] = [3, 1, 4, 0, 6, 5, 2];
const COSTAS_AT: [usize; 3] = [0, 36, 72];
const GRAY: [u8; 8] = [0, 1, 3, 2, 5, 6, 4, 7];
/// Gaussian smoothing of the frequency transitions, in the BT product the protocol states.
const BT: f64 = 2.0;
/// Symbols the Gaussian frequency pulse spans. Three is what makes the shaping a *transition*
/// smoother rather than a partial-response code: the pulse is centred on its own symbol, so the
/// middle of every symbol holds its pure tone and only the edges are shaped. It costs the
/// receiver one symbol of group delay ([`DELAY_SYMBOLS`]), measured rather than assumed —
/// at span 2 or 4 no whole-symbol delay decodes the frame at all.
const PULSE_SPAN: usize = 3;
/// Group delay the 3-symbol pulse puts between the transmitter's symbol clock and the
/// receiver's window, in symbols.
const DELAY_SYMBOLS: usize = 1;

/// The tone plan: 8 tones, one cycle per symbol apart — the published spacing, expressed in the
/// unit orthogonality is checkable in.
fn plan() -> MfskParams {
    MfskParams::orthogonal(TONES, SPS)
}

/// The published waveform: 8-GFSK at h = 1 over a Gaussian frequency pulse of BT = 2.0, from the
/// crate's *CPM* modulator — which is the point of generating it this way rather than by
/// writing tones directly. The transmitter is one engine, the receiver another, and they agree
/// only because the tone plan says the same thing to both.
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

/// A complete 79-symbol frame: the three Costas arrays at their published positions and
/// pseudo-random data between them. The *data* is arbitrary — see the module docs on what FT8's
/// encoder would put there — but the frame's shape is the protocol's.
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

// --- Golden vectors --------------------------------------------------------------------------

/// The receiver's first sample of symbol 0 — the pulse's group delay, in samples.
fn delay_samples() -> usize {
    DELAY_SYMBOLS * SPS as usize
}

/// {3, 1, 4, 0, 6, 5, 2} is a Costas array — every displacement vector between two of its points
/// occurs once — which is the property that makes its autocorrelation a single spike and the
/// reason WSJT-X can find a frame in noise it cannot yet decode. Checked here because the
/// sequence is copied from a paper, and a mistyped digit would still *look* like a sync word.
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
    // A Costas array is a permutation of its tone range; FT8's uses 7 of the 8 tones.
    let mut tones = COSTAS;
    tones.sort_unstable();
    assert_eq!(tones, [0, 1, 2, 3, 4, 5, 6]);
}

/// The published Gray map, checked in the direction that carries its meaning: `GRAY[i]` is the
/// *tone* a 3-bit group is sent on, so the property is on the inverse — two tones that are
/// neighbours in frequency must carry bit groups differing in one bit. That is what makes the
/// commonest symbol error, a slip to the adjacent tone, cost one bit of three rather than two.
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

/// The geometry, against the published numbers: spacing, orthogonality, occupied bandwidth and
/// transmission length all follow from the plan this library is handed, so all four are read
/// back out of it rather than restated.
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
    // Orthogonality is spacing × symbol length = 1 cycle, which is what the plan asserts.
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

/// The round trip that is the acceptance itself: the published waveform — 8-GFSK at BT = 2.0,
/// generated by the CPM engine — read back tone-for-tone by the orthogonal engine's filterbank,
/// all 79 symbols of a full frame, including the Costas arrays at their published positions.
#[test]
fn a_full_frame_round_trips_tone_for_tone() {
    let symbols = frame(0x5eed);
    let wave = transmit(&symbols);
    // 79 symbols plus the pulse tail the transmitter flushes, so the last symbol is radiated in
    // full — a receiver's matched filter is built around a whole symbol either way.
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

/// What the Costas arrays are *for*: finding the frame. The filterbank's energies are correlated
/// against the published array at every candidate symbol offset, and the three published
/// positions must be the three that answer — at an SNR where the raw symbols are already
/// unreliable, which is the regime WSJT-X does this in.
#[test]
fn the_costas_arrays_locate_the_frame_under_noise() {
    let symbols = frame(0xc057);
    let lead = 5usize;
    let mut wave = transmit(&[vec![0u8; lead], symbols.clone()].concat());
    // Es/N0 ≈ 8 dB: the raw argmax is already wrong a few percent of the time, which is the
    // regime a sync search has to work in — WSJT-X finds frames it cannot yet decode.
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

/// The published −21 dB decode threshold, turned into a statement this library can be held to.
///
/// −21 dB in 2500 Hz means S/N0 = 2500 · 10^(−2.1) = 19.83 Hz, so a 0.16 s symbol carries
/// Es/N0 = 3.17 (**5.02 dB**) and, at 3 bits per tone, Eb/N0 = 0.24 dB per raw bit. Charged
/// against the 174 *coded* bits the whole 12.64 s transmission carries — sync symbols included
/// — the same power reads 1.58 dB per coded bit, which is the number the LDPC(174, 91) is
/// designed against.
///
/// At the raw operating point the tone stream is nowhere near decodable on its own: the closed
/// form puts one symbol in five wrong, and the entire distance from there to a decode is the
/// LDPC this library does not implement (see the module docs). What *is* checkable is that the
/// measured raw error rate at that point matches the exact noncoherent orthogonal form — if it
/// did not, either the published threshold or this entry's detector would be wrong.
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
    let theory = sdrmm_modem::ber::theory::mfsk_noncoherent_ser(8, ebn0_db);
    assert!(
        (measured / theory - 1.0).abs() < 0.15,
        "measured raw SER {measured:.4} vs exact noncoherent 8-FSK {theory:.4} at {ebn0_db:.2} dB"
    );
    assert!(
        measured > 0.1,
        "raw SER {measured} at the published threshold"
    );
}
