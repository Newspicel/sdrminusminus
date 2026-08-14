use sdrmm_dsp::Nco;

use super::{
    Measurement,
    framing::{
        Acquisition, UW24, WARMUP_SYMBOLS, cpm_wave, find_uw, framed_symbols, payload_bits,
        real_quiet, uw_levels,
    },
};
use crate::{
    ber::sweep::Link,
    cpm::{CpmDemod, CpmParams, Mapping, RealDetector, TIMING_BW_BURST},
    pulse::{self, Norm},
};

/// Bell-202 tones at 1200 baud, processed at 12 kHz audio (10 sps — the rate an APRS-in-NFM
/// audio tap resamples to). Mark sits *below* the 1700 Hz centre, so its level is −1 and the
/// mapping table — not a sign convention — carries the assignment: index 0 → +1 (2200 Hz),
/// index 1 → −1 (1200 Hz).
pub const BAUD: f64 = 1_200.0;
pub const SPS: f64 = 10.0;
pub const RATE: f64 = BAUD * SPS;
pub const MARK_HZ: f64 = 1_200.0;
pub const SPACE_HZ: f64 = 2_200.0;
pub const CENTRE_HZ: f64 = 1_700.0;

#[must_use]
pub fn params() -> CpmParams {
    CpmParams::from_deviation(
        Mapping::new(vec![1.0, -1.0]),
        (SPACE_HZ - MARK_HZ) / 2.0,
        BAUD,
        pulse::rect(SPS, Norm::Area),
        SPS,
    )
}

#[must_use]
pub fn filterbank() -> RealDetector {
    RealDetector::ToneFilterbank {
        plus_hz: SPACE_HZ,
        minus_hz: MARK_HZ,
    }
}

#[must_use]
pub fn discriminator() -> RealDetector {
    RealDetector::Discriminator {
        centre_hz: CENTRE_HZ,
    }
}

/// Per-detector receive filter, the demod unit tests' measured judgment restated at 10 sps:
/// the tone correlators already integrate a 1.2-symbol window, so the filterbank takes only a
/// half-symbol smoothing rect; the discriminator has no integration of its own and takes the
/// full-symbol matched rect.
#[must_use]
pub fn rx(detector: RealDetector) -> Vec<f32> {
    match detector {
        RealDetector::ToneFilterbank { .. } => pulse::rect(SPS / 2.0, Norm::Area),
        RealDetector::Discriminator { .. } => pulse::rect(SPS, Norm::Area),
    }
}

/// AFSK trials are shorter-framed than the RF entries': the audio detectors lock in tens of
/// symbols (the demod unit tests align within 80), so 64 preamble symbols suffice. The unique
/// word is the shared 24-symbol one — a 16-symbol draft put the false-anchor probability at
/// ~2⁻¹⁶ per payload position, and with ~32 in-window positions per trial the committed curves
/// grew a measured ~1.3e-4 error floor of whole mis-anchored trials at high Eb/N0; 24 symbols
/// and the tighter window below push that below every committed point.
pub const PREAMBLE: usize = 64;
pub const TAIL: usize = 16;
pub const BITS: usize = 1024;

/// Sync search span past the nominal word position: covers the detector group delays
/// (~2 symbols filterbank, ~8 discriminator) and the shift a worst-case sample-clock probe
/// adds by word time (~5 symbols at 50 000 ppm), while keeping payload overlap short enough
/// that the word's own sidelobe guard decides every in-window impostor.
pub const SEARCH: usize = 24;

/// The receive chain: the constructed detector *is* the front end, and the audio the
/// demodulator hears is the waveform's real part — see [`link`] for why the harness carries
/// the analytic signal.
#[must_use]
pub fn soft(detector: RealDetector, wave: &[num_complex::Complex<f32>]) -> Vec<f32> {
    let p = params();
    let mut demod = CpmDemod::real(&p, &rx(detector), TIMING_BW_BURST, RATE, detector);
    let mut discard = Vec::new();
    demod.process_real(
        &real_quiet(0x1157, WARMUP_SYMBOLS * SPS as usize),
        &mut discard,
    );
    let audio: Vec<f32> = wave.iter().map(|s| s.re).collect();
    let mut out = Vec::new();
    demod.process_real(&audio, &mut out);
    out
}

/// One AFSK link. The harness's impairments and Eb/N0 accounting live on complex waveforms, so
/// the link carries the *analytic* audio (baseband CPFSK shifted onto the 1700 Hz subcarrier —
/// positive frequencies only) and the demodulator hears its real part. The projection halves
/// signal power and per-sample noise power together (Re of unit-magnitude analytic carries ½;
/// complex noise of per-component σ² projects to real variance σ²), so the stated Eb/N0 is the
/// ratio at the detector.
#[must_use]
pub fn link(label: &str, detector: RealDetector) -> Link {
    let p = params();
    Link {
        label: label.to_string(),
        bits_per_trial: BITS,
        modulate: Box::new(move |bits| {
            let baseband = cpm_wave(
                &params(),
                &framed_symbols(Acquisition::Alternating, PREAMBLE, &UW24, bits, TAIL),
            );
            let mut carrier = Nco::new(CENTRE_HZ as f32, RATE as f32);
            baseband
                .iter()
                .map(|&s| s * carrier.next_sample())
                .collect()
        }),
        demodulate: Box::new(move |wave| {
            let s = soft(detector, wave);
            let levels = uw_levels(&p, &UW24);
            let Some(at) = find_uw(&s, PREAMBLE, PREAMBLE + SEARCH, &levels) else {
                return Vec::new();
            };
            payload_bits(&p, &s, at, UW24.len(), BITS)
        }),
    }
}

#[must_use]
pub fn filterbank_link() -> Link {
    link(
        "afsk 1200/2200 Hz 1200 baud uncoded, CpmMod on 1700 Hz subcarrier -> CpmDemod::real \
         tone filterbank (half-symbol rx rect, timing bw 0.015), 12 kHz audio, 64+24+16 \
         symbol overhead in Eb, analytic-signal accounting, release",
        filterbank(),
    )
}

#[must_use]
pub fn discriminator_link() -> Link {
    link(
        "afsk 1200/2200 Hz 1200 baud uncoded, CpmMod on 1700 Hz subcarrier -> CpmDemod::real \
         analytic discriminator (full-symbol rx rect, timing bw 0.015), 12 kHz audio, \
         64+24+16 symbol overhead in Eb, analytic-signal accounting, release",
        discriminator(),
    )
}

/// One grid for both detectors: the tier-1 comparison is only a comparison if the two curves
/// are measured at the same points.
pub const GRID: &[f64] = &[
    7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 18.0,
];

pub const FILTERBANK_SEED: u64 = 0xafb1;
pub const DISCRIMINATOR_SEED: u64 = 0xafd1;

pub const FILTERBANK_AWGN: &str = "cpm/afsk_filterbank_awgn";
pub const DISCRIMINATOR_AWGN: &str = "cpm/afsk_discriminator_awgn";
pub const LIMITS: &str = "cpm/afsk_limits";
pub const PERF: &str = "cpm/afsk_perf";

pub const MEASUREMENTS: &[Measurement] = &[
    Measurement::committed(
        FILTERBANK_AWGN,
        filterbank_link,
        GRID,
        FILTERBANK_SEED,
        super::framing::FULL_CAP,
    ),
    Measurement::committed(
        DISCRIMINATOR_AWGN,
        discriminator_link,
        GRID,
        DISCRIMINATOR_SEED,
        super::framing::FULL_CAP,
    ),
];
