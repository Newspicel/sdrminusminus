use num_complex::Complex;
use sdrmm_modem::{
    constellation::{Constellation, tables},
    spread::{
        CckDemod, CckMod, CckMode, CckParams, ChipShaper, CssDemod, CssMod, CssParams, DsssDemod,
        DsssMod, DsssParams, FhssDemod, FhssMod, HopSequence, PnSequence,
    },
    symbolcode::{DifferentialSymbolDecoder, DifferentialSymbolEncoder},
};

use super::{
    Measurement, Reference, Tier,
    linear::{bits_to_labels, labels_to_bits, table},
};
use crate::ber::{sweep::Link, theory};

pub const CHIP_RATE: f64 = 11_000_000.0;

pub const CHIP_SPS: usize = 4;
pub const CHIP_SAMPLE_RATE: f64 = CHIP_RATE * CHIP_SPS as f64;

pub const CHIP_ALPHA: f64 = 0.35;
pub const CHIP_SPAN: usize = 8;

pub const PREAMBLE: usize = 64;

pub const SEARCH_GROUP: usize = 8;

pub const LEAD: usize = 96;

pub const TAIL: usize = 512;

pub const SEARCH: usize = 192;

pub const FULL_CAP: u64 = 3_000_000;

pub const DSSS_PAYLOAD: usize = 2_048;

#[must_use]
pub fn dsss_overhead_db() -> f64 {
    DsssParams::framing_overhead_db(PREAMBLE, DSSS_PAYLOAD)
}

fn shaper() -> ChipShaper {
    ChipShaper::root_raised_cosine(CHIP_SPS, CHIP_ALPHA, CHIP_SPAN)
}

fn points_by_label(table: &Constellation) -> Vec<Complex<f32>> {
    let mut by_label = vec![Complex::new(0.0, 0.0); table.len()];
    for (index, &label) in table.labels().iter().enumerate() {
        by_label[label as usize] = table.points()[index];
    }
    by_label
}

fn preamble_points(table: &Constellation, len: usize) -> Vec<Complex<f32>> {
    let by_label = points_by_label(table);
    let mut state = 0x9e37_79b9u32;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            by_label[state as usize % by_label.len()]
        })
        .collect()
}

fn preamble_labels(alphabet: u32, len: usize) -> Vec<u32> {
    let mut state = 0x9e37_79b9u32;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state % alphabet
        })
        .collect()
}

#[must_use]
pub fn dsss_link(name: &str, pn: PnSequence, constellation: Constellation) -> Link {
    let bits_per_symbol = constellation.bits_per_symbol();
    let chips = pn.len();
    let params = DsssParams::new(pn, shaper(), SEARCH_GROUP);
    let by_label = points_by_label(&constellation);
    let preamble = preamble_points(&constellation, PREAMBLE);
    let modulator = DsssMod::new(params.clone());
    let demod = DsssDemod::new(params);
    let known = preamble.clone();

    let label = format!(
        "{name} uncoded, DSSS {chips} chips/symbol at {} Mchip/s, {CHIP_SPS} samples/chip, \
         RRC α={CHIP_ALPHA}, {PREAMBLE}-symbol preamble, {DSSS_PAYLOAD}-symbol payload, \
         correlator + gain anchor",
        CHIP_RATE / 1e6
    );
    Link {
        label,
        bits_per_trial: DSSS_PAYLOAD * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let mut modulator = modulator.clone();
            let mut wave = vec![Complex::new(0.0, 0.0); LEAD];
            let points: Vec<Complex<f32>> = bits_to_labels(bits, bits_per_symbol)
                .into_iter()
                .map(|label| by_label[label as usize])
                .collect();
            modulator.frame(&preamble, &points, &mut wave);
            wave.resize(wave.len() + TAIL, Complex::new(0.0, 0.0));
            wave
        }),
        demodulate: Box::new(move |wave| {
            let mut demod = demod.clone();
            if demod.acquire(wave, &known, SEARCH).is_none() {
                return Vec::new();
            }
            let mut points = Vec::with_capacity(DSSS_PAYLOAD);
            demod.demodulate(PREAMBLE, DSSS_PAYLOAD, &mut points);
            let labels: Vec<u32> = points
                .iter()
                .map(|&p| constellation.hard_slice(p))
                .collect();
            labels_to_bits(&labels, bits_per_symbol)
        }),
    }
}

#[must_use]
pub fn barker11_link() -> Link {
    dsss_link(
        "bpsk-dsss barker-11",
        barker(11),
        table("dsss bpsk", tables::pam(2)),
    )
}

#[must_use]
pub fn barker11_qpsk_link() -> Link {
    dsss_link(
        "qpsk-dsss barker-11",
        barker(11),
        table("dsss qpsk", tables::qam_square(4)),
    )
}

#[must_use]
pub fn m31_link() -> Link {
    dsss_link(
        "bpsk-dsss m31",
        maximal_length(5),
        table("dsss bpsk", tables::pam(2)),
    )
}

pub const CCK_PAYLOAD: usize = 2_816;

#[must_use]
pub fn cck_overhead_db() -> f64 {
    CckParams::framing_overhead_db(PREAMBLE, CCK_PAYLOAD)
}

#[must_use]
pub fn cck_link(name: &str, mode: CckMode) -> Link {
    let bits_per_symbol = mode.bits_per_symbol();
    let params = CckParams::new(mode, shaper(), SEARCH_GROUP);
    let preamble = preamble_labels(mode.alphabet(), PREAMBLE);
    let modulator = CckMod::new(params.clone());
    let demod = CckDemod::new(params);
    let known = preamble.clone();

    let label = format!(
        "{name} uncoded, CCK {bits_per_symbol} bits/symbol over 8 chips at {} Mchip/s, \
         {CHIP_SPS} samples/chip, RRC α={CHIP_ALPHA}, differential φ1, {PREAMBLE}-symbol \
         preamble, {CCK_PAYLOAD}-symbol payload, correlator bank + gain anchor",
        CHIP_RATE / 1e6
    );
    Link {
        label,
        bits_per_trial: CCK_PAYLOAD * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let mut modulator = modulator.clone();
            let labels = differentially_encode(&bits_to_labels(bits, bits_per_symbol));
            let mut wave = vec![Complex::new(0.0, 0.0); LEAD];
            modulator.frame(&preamble, &labels, &mut wave);
            wave.resize(wave.len() + TAIL, Complex::new(0.0, 0.0));
            wave
        }),
        demodulate: Box::new(move |wave| {
            let mut demod = demod.clone();
            if demod.acquire(wave, &known, SEARCH).is_none() {
                return Vec::new();
            }
            let mut labels = Vec::with_capacity(CCK_PAYLOAD);
            demod.demodulate(PREAMBLE, CCK_PAYLOAD, &mut labels);
            labels_to_bits(&differentially_decode(&labels), bits_per_symbol)
        }),
    }
}

fn differentially_encode(labels: &[u32]) -> Vec<u32> {
    let mut encoder = DifferentialSymbolEncoder::new(4);
    labels
        .iter()
        .map(|&label| (label & !3) | encoder.encode(label & 3))
        .collect()
}

fn differentially_decode(labels: &[u32]) -> Vec<u32> {
    let mut decoder = DifferentialSymbolDecoder::new(4);
    labels
        .iter()
        .map(|&label| (label & !3) | decoder.decode(label & 3))
        .collect()
}

#[must_use]
pub fn cck11_link() -> Link {
    cck_link("cck-8bit", CckMode::Bits8)
}

#[must_use]
pub fn cck55_link() -> Link {
    cck_link("cck-4bit", CckMode::Bits4)
}

pub const CSS_BANDWIDTH: f64 = 125_000.0;

pub const CSS_PREAMBLE: usize = 16;

#[must_use]
pub fn css_payload(spreading_factor: u32) -> usize {
    let _ = spreading_factor;
    CSS_PREAMBLE * 12
}

#[must_use]
pub fn css_overhead_db() -> f64 {
    CssParams::framing_overhead_db(CSS_PREAMBLE, css_payload(7))
}

#[must_use]
pub fn css_link(spreading_factor: u32) -> Link {
    let params = CssParams::new(spreading_factor);
    let bits = params.bits_per_symbol();
    let payload = css_payload(spreading_factor);
    let preamble = preamble_labels(params.alphabet() as u32, CSS_PREAMBLE);
    let modulator = CssMod::new(params.clone());
    let demod = CssDemod::new(params.clone());
    let known = preamble.clone();
    let lead = CSS_LEAD.min(params.chips() / 4);

    let label = format!(
        "css-sf{spreading_factor} uncoded, {} chirp shifts at {} kHz bandwidth, critically \
         sampled, {CSS_PREAMBLE}-symbol preamble, {payload}-symbol payload, dechirp + \
         transform argmax",
        params.alphabet(),
        CSS_BANDWIDTH / 1e3
    );
    Link {
        label,
        bits_per_trial: payload * bits,
        modulate: Box::new(move |bit_slice| {
            let symbols: Vec<u32> = bits_to_labels(bit_slice, bits);
            let mut wave = vec![Complex::new(0.0, 0.0); lead];
            modulator.frame(&preamble, &symbols, &mut wave);
            wave.resize(wave.len() + lead, Complex::new(0.0, 0.0));
            wave
        }),
        demodulate: Box::new(move |wave| {
            let mut demod = demod.clone();
            let origin = demod.estimate_origin(wave, &known);
            let mut symbols = Vec::with_capacity(payload);
            demod.demodulate(
                wave,
                origin + CSS_PREAMBLE * (1 << spreading_factor),
                payload,
                &mut symbols,
            );
            labels_to_bits(&symbols, bits)
        }),
    }
}

pub const CSS_LEAD: usize = 32;

pub const HOP_CHANNELS: usize = 3;

pub const HOP_SPACING_CYCLES: f64 = 1.0 / CHIP_SPS as f64;

pub const HOP_DWELL_SYMBOLS: usize = 64;

#[must_use]
pub fn hop_sequence(chips_per_symbol: usize) -> HopSequence {
    let dwell = HOP_DWELL_SYMBOLS * chips_per_symbol * CHIP_SPS;
    let hops = (LEAD + (PREAMBLE + DSSS_PAYLOAD) * chips_per_symbol * CHIP_SPS + TAIL) / dwell + 2;
    match HopSequence::from_m_sequence(HOP_CHANNELS, HOP_SPACING_CYCLES, dwell, hops, 9) {
        Ok(sequence) => sequence,
        Err(why) => panic!("catalog entry `fhss`: {why}"),
    }
}

#[must_use]
pub fn fhss_link() -> Link {
    let inner = barker11_link();
    let sequence = hop_sequence(11);
    let hopper = FhssMod::new(sequence.clone());
    let dehopper = FhssDemod::new(sequence);
    let modulate = inner.modulate;
    let demodulate = inner.demodulate;
    Link {
        label: format!(
            "{}, hopped over {HOP_CHANNELS} channels at {} MHz spacing, \
             {HOP_DWELL_SYMBOLS}-symbol dwells",
            inner.label,
            HOP_SPACING_CYCLES * CHIP_SAMPLE_RATE / 1e6
        ),
        bits_per_trial: inner.bits_per_trial,
        modulate: Box::new(move |bits| {
            let mut wave = modulate(bits);
            hopper.hop(&mut wave);
            wave
        }),
        demodulate: Box::new(move |wave| {
            let mut wave = wave.to_vec();
            dehopper.dehop(&mut wave);
            demodulate(&wave)
        }),
    }
}

#[must_use]
pub fn barker(n: usize) -> PnSequence {
    match PnSequence::barker(n) {
        Ok(pn) => pn,
        Err(why) => panic!("catalog entry: Barker-{n}: {why}"),
    }
}

#[must_use]
pub fn maximal_length(degree: u32) -> PnSequence {
    match PnSequence::maximal_length(degree) {
        Ok(pn) => pn,
        Err(why) => panic!("catalog entry: m-sequence of degree {degree}: {why}"),
    }
}

pub const BARKER11_GRID: &[f64] = &[4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
pub const BARKER11_QPSK_GRID: &[f64] = &[4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
pub const M31_GRID: &[f64] = &[4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
pub const CCK11_GRID: &[f64] = &[3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
pub const CCK55_GRID: &[f64] = &[2.0, 3.0, 4.0, 5.0, 6.0, 7.0];
pub const CSS_SF7_GRID: &[f64] = &[2.0, 3.0, 4.0, 5.0, 6.0, 7.0];
pub const CSS_SF10_GRID: &[f64] = &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
pub const CSS_SF12_GRID: &[f64] = &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0];
pub const FHSS_GRID: &[f64] = &[4.0, 5.0, 6.0, 7.0, 8.0, 9.0];

pub const BARKER11_SEED: u64 = 0x0_5b11;
pub const BARKER11_QPSK_SEED: u64 = 0x0_5b12;
pub const M31_SEED: u64 = 0x0_5b31;
pub const CCK11_SEED: u64 = 0x0_5cc8;
pub const CCK55_SEED: u64 = 0x0_5cc4;
pub const CSS_SF7_SEED: u64 = 0x0_5c07;
pub const CSS_SF10_SEED: u64 = 0x0_5c10;
pub const CSS_SF12_SEED: u64 = 0x0_5c12;
pub const FHSS_SEED: u64 = 0x0_5f45;

pub const BARKER11_AWGN: &str = "spread/dsss_barker11_awgn";
pub const BARKER11_QPSK_AWGN: &str = "spread/dsss_barker11_qpsk_awgn";
pub const M31_AWGN: &str = "spread/dsss_m31_awgn";
pub const CCK11_AWGN: &str = "spread/cck_8bit_awgn";
pub const CCK55_AWGN: &str = "spread/cck_4bit_awgn";
pub const CSS_SF7_AWGN: &str = "spread/css_sf7_awgn";
pub const CSS_SF10_AWGN: &str = "spread/css_sf10_awgn";
pub const CSS_SF12_AWGN: &str = "spread/css_sf12_awgn";
pub const FHSS_AWGN: &str = "spread/fhss_barker11_awgn";

pub const DSSS_LIMITS: &str = "spread/dsss_barker11_limits";
pub const M31_LIMITS: &str = "spread/dsss_m31_limits";
pub const CCK_LIMITS: &str = "spread/cck_8bit_limits";
pub const CSS_LIMITS: &str = "spread/css_sf7_limits";
pub const FHSS_LIMITS: &str = "spread/fhss_limits";
pub const PERF: &str = "spread/spread_perf";

pub const ORACLE_TOLERANCE_DB: f64 = 0.5;

fn dsss_bpsk_ber(ebn0_db: f64) -> f64 {
    theory::bpsk_ber(ebn0_db - dsss_overhead_db())
}

fn dsss_qpsk_ber(ebn0_db: f64) -> f64 {
    theory::qpsk_ber(ebn0_db - dsss_overhead_db())
}

fn css_ber(spreading_factor: u32, ebn0_db: f64) -> f64 {
    theory::mfsk_noncoherent_ber(1 << spreading_factor, ebn0_db - css_overhead_db())
}

fn css_sf7_ber(ebn0_db: f64) -> f64 {
    css_ber(7, ebn0_db)
}

fn css_sf10_ber(ebn0_db: f64) -> f64 {
    css_ber(10, ebn0_db)
}

fn css_sf12_ber(ebn0_db: f64) -> f64 {
    css_ber(12, ebn0_db)
}

const fn oracle_row(
    stem: &'static str,
    link: fn() -> Link,
    grid: &'static [f64],
    seed: u64,
    name: &'static str,
    ber: fn(f64) -> f64,
) -> Measurement {
    Measurement {
        stem,
        link,
        full: Tier {
            grid,
            seed,
            min_errors: super::FULL_ERRORS,
            max_trial_bits: FULL_CAP,
        },
        smoke_points: super::SMOKE_POINTS,
        reference: Reference::Oracle {
            name,
            ber,
            tolerance_db: ORACLE_TOLERANCE_DB,
        },
    }
}

fn css_sf7_link() -> Link {
    css_link(7)
}

fn css_sf10_link() -> Link {
    css_link(10)
}

fn css_sf12_link() -> Link {
    css_link(12)
}

pub const DSSS: &[Measurement] = &[
    oracle_row(
        BARKER11_AWGN,
        barker11_link,
        BARKER11_GRID,
        BARKER11_SEED,
        "exact ½·erfc(√γ) + the frame's own overhead",
        dsss_bpsk_ber,
    ),
    oracle_row(
        BARKER11_QPSK_AWGN,
        barker11_qpsk_link,
        BARKER11_QPSK_GRID,
        BARKER11_QPSK_SEED,
        "exact Gray QPSK + the frame's own overhead",
        dsss_qpsk_ber,
    ),
    oracle_row(
        M31_AWGN,
        m31_link,
        M31_GRID,
        M31_SEED,
        "exact ½·erfc(√γ) + the frame's own overhead",
        dsss_bpsk_ber,
    ),
];

pub const CCK: &[Measurement] = &[
    Measurement::committed(CCK11_AWGN, cck11_link, CCK11_GRID, CCK11_SEED, FULL_CAP),
    Measurement::committed(CCK55_AWGN, cck55_link, CCK55_GRID, CCK55_SEED, FULL_CAP),
];

pub const CSS: &[Measurement] = &[
    oracle_row(
        CSS_SF7_AWGN,
        css_sf7_link,
        CSS_SF7_GRID,
        CSS_SF7_SEED,
        "exact 128-ary noncoherent orthogonal + the frame's own overhead",
        css_sf7_ber,
    ),
    oracle_row(
        CSS_SF10_AWGN,
        css_sf10_link,
        CSS_SF10_GRID,
        CSS_SF10_SEED,
        "exact 1024-ary noncoherent orthogonal + the frame's own overhead",
        css_sf10_ber,
    ),
    oracle_row(
        CSS_SF12_AWGN,
        css_sf12_link,
        CSS_SF12_GRID,
        CSS_SF12_SEED,
        "exact 4096-ary noncoherent orthogonal + the frame's own overhead",
        css_sf12_ber,
    ),
];

pub const FHSS: &[Measurement] = &[oracle_row(
    FHSS_AWGN,
    fhss_link,
    FHSS_GRID,
    FHSS_SEED,
    "exact ½·erfc(√γ) + the carried entry's own overhead",
    dsss_bpsk_ber,
)];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_framing_overheads_are_symbol_count_ratios() {
        let dsss = dsss_overhead_db();
        assert!((dsss - 10.0 * (2112.0f64 / 2048.0).log10()).abs() < 1e-12);
        assert!((dsss - 0.1335).abs() < 5e-3, "{dsss}");
        let cck = cck_overhead_db();
        assert!((cck - 10.0 * (2880.0f64 / 2816.0).log10()).abs() < 1e-12);
        let css = css_overhead_db();
        assert!((css - 10.0 * (104.0f64 / 96.0).log10()).abs() < 1e-12);
        assert!((css - 0.3475).abs() < 5e-3, "{css}");
        for sf in 7..=12u32 {
            assert_eq!(css_payload(sf), css_payload(7));
        }
    }

    #[test]
    fn every_chain_round_trips_on_a_clean_channel() {
        for (name, link) in [
            ("barker-11 bpsk", barker11_link()),
            ("barker-11 qpsk", barker11_qpsk_link()),
            ("m31 bpsk", m31_link()),
            ("cck 8-bit", cck11_link()),
            ("cck 4-bit", cck55_link()),
            ("css sf7", css_link(7)),
            ("css sf10", css_link(10)),
            ("fhss", fhss_link()),
        ] {
            let bits: Vec<bool> = (0..link.bits_per_trial).map(|i| i % 7 < 3).collect();
            let wave = (link.modulate)(&bits);
            let decoded = (link.demodulate)(&wave);
            let errors = bits
                .iter()
                .enumerate()
                .filter(|(i, b)| decoded.get(*i) != Some(b))
                .count();
            assert_eq!(errors, 0, "{name}: {errors} of {} bits", bits.len());
        }
    }

    #[test]
    fn the_hopped_row_carries_the_row_it_hops() {
        let bits: Vec<bool> = (0..barker11_link().bits_per_trial)
            .map(|i| i % 5 < 2)
            .collect();
        let plain = (barker11_link().modulate)(&bits);
        let hopped = (fhss_link().modulate)(&bits);
        assert_eq!(plain.len(), hopped.len());
        let moved = plain
            .iter()
            .zip(&hopped)
            .filter(|&(&a, &b)| (a - b).norm() > 1e-4)
            .count();
        assert!(
            moved > plain.len() / 3,
            "only {moved} of {} samples were hopped",
            plain.len()
        );
        assert_eq!(
            (fhss_link().demodulate)(&hopped),
            (barker11_link().demodulate)(&plain)
        );
    }

    #[test]
    fn the_committed_hop_schedule_spreads_a_trial_across_its_plan() {
        let sequence = hop_sequence(11);
        let hops = sequence.order().len();
        assert!(
            hops > 2 * HOP_CHANNELS,
            "{hops} hops over {HOP_CHANNELS} channels"
        );
        assert!(
            sequence.visits(hops) >= 3 * HOP_CHANNELS / 4,
            "a trial visits only {} of {HOP_CHANNELS} channels",
            sequence.visits(hops)
        );
        let demod = FhssDemod::new(sequence);
        for channel in 0..HOP_CHANNELS {
            let dwells = demod.dwells_on(channel, hops);
            assert!(
                dwells < 2 * hops / HOP_CHANNELS,
                "channel {channel} takes {dwells} of {hops} dwells"
            );
        }
    }
}
