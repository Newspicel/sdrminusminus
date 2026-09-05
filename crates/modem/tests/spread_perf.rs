#![allow(clippy::unwrap_used, clippy::expect_used)]

use num_complex::Complex;
use sdrmm_modem::{
    constellation::tables,
    soft::Llr,
    spread::{
        CckDemod, CckMod, CckMode, CckParams, ChipShaper, CssDemod, CssMod, CssParams, DsssDemod,
        DsssMod, DsssParams, FhssDemod, FhssMod,
    },
};
use sdrmm_modem_test_support::ber::{
    catalog::spread::{
        self, CHIP_ALPHA, CHIP_SAMPLE_RATE, CHIP_SPAN, CHIP_SPS, CSS_BANDWIDTH, CSS_PREAMBLE, LEAD,
        PREAMBLE, SEARCH, barker, css_payload, hop_sequence,
    },
    perf::{
        CountingAlloc, PerfBaseline, REGRESSION_FRACTION, assert_no_alloc, compare_perf, host_id,
        load_baselines, measure_throughput, save_baselines,
    },
};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

const SYMBOLS: usize = 512;
const CSS_SF: u32 = 7;

fn shaper() -> ChipShaper {
    ChipShaper::root_raised_cosine(CHIP_SPS, CHIP_ALPHA, CHIP_SPAN)
}

fn points(count: usize, table: &sdrmm_modem::constellation::Constellation) -> Vec<Complex<f32>> {
    let mut state = 0x0_5b0fu32;
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            table.points()[state as usize % table.len()]
        })
        .collect()
}

fn labels(count: usize, alphabet: u32) -> Vec<u32> {
    let mut state = 0x0_5c0fu32;
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state % alphabet
        })
        .collect()
}

struct Dsss {
    demod: DsssDemod,
    wave: Vec<Complex<f32>>,
    preamble: Vec<Complex<f32>>,
}

fn dsss_burst() -> Dsss {
    let table = tables::pam(2).unwrap();
    let params = DsssParams::new(barker(11), shaper(), 8);
    let preamble = points(PREAMBLE, &table);
    let payload = points(SYMBOLS, &table);
    let mut wave = vec![Complex::new(0.0, 0.0); LEAD];
    DsssMod::new(params.clone()).frame(&preamble, &payload, &mut wave);
    wave.resize(wave.len() + 512, Complex::new(0.0, 0.0));
    Dsss {
        demod: DsssDemod::new(params),
        wave,
        preamble,
    }
}

struct Cck {
    demod: CckDemod,
    wave: Vec<Complex<f32>>,
    preamble: Vec<u32>,
}

fn cck_burst() -> Cck {
    let params = CckParams::new(CckMode::Bits8, shaper(), 8);
    let preamble = labels(PREAMBLE, CckMode::Bits8.alphabet());
    let payload = labels(SYMBOLS, CckMode::Bits8.alphabet());
    let mut wave = vec![Complex::new(0.0, 0.0); LEAD];
    CckMod::new(params.clone()).frame(&preamble, &payload, &mut wave);
    wave.resize(wave.len() + 512, Complex::new(0.0, 0.0));
    Cck {
        demod: CckDemod::new(params),
        wave,
        preamble,
    }
}

fn css_burst() -> (CssDemod, Vec<Complex<f32>>, Vec<u32>, usize) {
    let params = CssParams::new(CSS_SF);
    let preamble = labels(CSS_PREAMBLE, params.alphabet() as u32);
    let payload = css_payload(CSS_SF);
    let symbols = labels(payload, params.alphabet() as u32);
    let mut wave = vec![Complex::new(0.0, 0.0); 32];
    CssMod::new(params.clone()).frame(&preamble, &symbols, &mut wave);
    wave.resize(wave.len() + 32, Complex::new(0.0, 0.0));
    (CssDemod::new(params), wave, preamble, payload)
}

fn measured() -> Vec<PerfBaseline> {
    let chip_geometry =
        format!("11 chips/symbol, {CHIP_SPS} samples/chip, RRC α={CHIP_ALPHA}, {SYMBOLS} symbols");

    let mut dsss = dsss_burst();
    dsss.demod
        .acquire(&dsss.wave, &dsss.preamble, SEARCH)
        .unwrap();
    let mut sink = Vec::with_capacity(SYMBOLS);
    let dsss_msps = measure_throughput(200, dsss.wave.len() as u64, || {
        sink.clear();
        dsss.demod
            .acquire(&dsss.wave, &dsss.preamble, SEARCH)
            .unwrap();
        dsss.demod.demodulate(PREAMBLE, SYMBOLS, &mut sink);
    });

    let mut cck = cck_burst();
    cck.demod.acquire(&cck.wave, &cck.preamble, SEARCH).unwrap();
    let mut cck_sink = Vec::with_capacity(SYMBOLS);
    let cck_msps = measure_throughput(200, cck.wave.len() as u64, || {
        cck_sink.clear();
        cck.demod.acquire(&cck.wave, &cck.preamble, SEARCH).unwrap();
        cck.demod.demodulate(PREAMBLE, SYMBOLS, &mut cck_sink);
    });

    let (mut css_demod, css_wave, css_preamble, css_symbols) = css_burst();
    let mut css_sink = Vec::with_capacity(css_symbols);
    let css_msps = measure_throughput(200, css_wave.len() as u64, || {
        css_sink.clear();
        let origin = css_demod.estimate_origin(&css_wave, &css_preamble);
        css_demod.demodulate(
            &css_wave,
            origin + CSS_PREAMBLE * (1 << CSS_SF),
            css_symbols,
            &mut css_sink,
        );
    });

    let sequence = hop_sequence(11);
    let dehopper = FhssDemod::new(sequence.clone());
    let mut hopped = dsss.wave.clone();
    FhssMod::new(sequence.clone()).hop(&mut hopped);
    let hop_msps = measure_throughput(2_000, hopped.len() as u64, || {
        dehopper.dehop(&mut hopped);
    });

    vec![
        PerfBaseline {
            bench: "dsss_barker11_44m".into(),
            msamples_per_s: dsss_msps,
            realtime_factor: dsss_msps * 1e6 / CHIP_SAMPLE_RATE,
            config: format!("{chip_geometry}, burst search over {SEARCH} samples + despread"),
            host: host_id(),
        },
        PerfBaseline {
            bench: "cck_8bit_44m".into(),
            msamples_per_s: cck_msps,
            realtime_factor: cck_msps * 1e6 / CHIP_SAMPLE_RATE,
            config: format!(
                "8 chips/symbol, {CHIP_SPS} samples/chip, RRC α={CHIP_ALPHA}, {SYMBOLS} symbols, \
                 burst search over {SEARCH} samples + 64-word correlator bank"
            ),
            host: host_id(),
        },
        PerfBaseline {
            bench: "css_sf7_125k".into(),
            msamples_per_s: css_msps,
            realtime_factor: css_msps * 1e6 / CSS_BANDWIDTH,
            config: format!(
                "SF{CSS_SF}, critically sampled, {CSS_PREAMBLE}-symbol preamble, {css_symbols} \
                 symbols, preamble-bin origin + dechirp/transform argmax"
            ),
            host: host_id(),
        },
        PerfBaseline {
            bench: "fhss_dehop_44m".into(),
            msamples_per_s: hop_msps,
            realtime_factor: hop_msps * 1e6 / CHIP_SAMPLE_RATE,
            config: format!(
                "{}-channel plan, {} cycles/sample spacing, de-hop only",
                sequence.channels(),
                sequence.spacing_cycles()
            ),
            host: host_id(),
        },
    ]
}

fn path(stem: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("baselines/{stem}.json"))
}

#[test]
fn the_direct_sequence_receive_path_allocates_nothing() {
    let mut burst = dsss_burst();
    let mut sink = Vec::with_capacity(SYMBOLS);
    for _ in 0..2 {
        burst
            .demod
            .acquire(&burst.wave, &burst.preamble, SEARCH)
            .unwrap();
        sink.clear();
        burst.demod.demodulate(PREAMBLE, SYMBOLS, &mut sink);
    }
    sink.clear();
    assert_no_alloc("DsssDemod::demodulate", || {
        burst.demod.demodulate(PREAMBLE, SYMBOLS, &mut sink);
    });
    assert_eq!(sink.len(), SYMBOLS);

    let table = tables::pam(2).unwrap();
    let mut llrs = vec![Llr(0.0); sink.len() * table.bits_per_symbol()];
    burst.demod.llrs(&sink, &table, &mut llrs);
    assert_no_alloc("DsssDemod::llrs", || {
        burst.demod.llrs(&sink, &table, &mut llrs);
    });
}

#[test]
fn the_burst_search_allocates_nothing() {
    let mut burst = dsss_burst();
    for _ in 0..2 {
        burst
            .demod
            .acquire(&burst.wave, &burst.preamble, SEARCH)
            .unwrap();
    }
    assert_no_alloc("DsssDemod::acquire", || {
        burst
            .demod
            .acquire(&burst.wave, &burst.preamble, SEARCH)
            .unwrap();
    });
}

#[test]
fn the_cck_correlator_bank_allocates_nothing() {
    let mut burst = cck_burst();
    let mut sink = Vec::with_capacity(SYMBOLS);
    let mut llrs = Vec::with_capacity(SYMBOLS * 8);
    for _ in 0..2 {
        burst
            .demod
            .acquire(&burst.wave, &burst.preamble, SEARCH)
            .unwrap();
        sink.clear();
        burst.demod.demodulate(PREAMBLE, SYMBOLS, &mut sink);
        llrs.clear();
        burst.demod.llrs(PREAMBLE, SYMBOLS, &mut llrs);
    }
    sink.clear();
    assert_no_alloc("CckDemod::demodulate", || {
        burst.demod.demodulate(PREAMBLE, SYMBOLS, &mut sink);
    });
    llrs.clear();
    assert_no_alloc("CckDemod::llrs", || {
        burst.demod.llrs(PREAMBLE, SYMBOLS, &mut llrs);
    });
    assert_eq!(llrs.len(), SYMBOLS * 8);
}

#[test]
fn the_chirp_receive_path_allocates_nothing() {
    let (mut demod, wave, preamble, symbols) = css_burst();
    let data = CSS_PREAMBLE * (1 << CSS_SF);
    let mut sink = Vec::with_capacity(symbols);
    let mut llrs = Vec::with_capacity(symbols * CSS_SF as usize);
    for _ in 0..2 {
        demod.estimate_origin(&wave, &preamble);
        sink.clear();
        demod.demodulate(&wave, data, symbols, &mut sink);
        llrs.clear();
        demod.llrs(&wave, data, symbols, 1.0, &mut llrs);
    }
    assert_no_alloc("CssDemod::estimate_origin", || {
        demod.estimate_origin(&wave, &preamble);
    });
    sink.clear();
    assert_no_alloc("CssDemod::demodulate", || {
        demod.demodulate(&wave, data, symbols, &mut sink);
    });
    llrs.clear();
    assert_no_alloc("CssDemod::llrs", || {
        demod.llrs(&wave, data, symbols, 1.0, &mut llrs);
    });
    assert_eq!(sink.len(), symbols);
}

#[test]
fn hopping_allocates_nothing() {
    let sequence = hop_sequence(11);
    let hopper = FhssMod::new(sequence.clone());
    let dehopper = FhssDemod::new(sequence);
    let mut wave = vec![Complex::new(0.5f32, -0.25); 20_000];
    for _ in 0..2 {
        hopper.hop(&mut wave);
        dehopper.dehop(&mut wave);
    }
    assert_no_alloc("FhssMod::hop", || hopper.hop(&mut wave));
    assert_no_alloc("FhssDemod::dehop", || dehopper.dehop(&mut wave));
}

#[test]
#[ignore = "rewrites the committed baseline; run explicitly in release on the reference host"]
fn write_spread_perf_baseline() {
    if cfg!(debug_assertions) {
        panic!("a debug-profile number must never become the committed baseline");
    }
    let rows = measured();
    let path = path(spread::PERF);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    save_baselines(&path, &rows).unwrap();
    for row in &rows {
        println!(
            "{}: {:.1} Msamples/s, {:.1}x real time",
            row.bench, row.msamples_per_s, row.realtime_factor
        );
    }
}

#[test]
#[ignore = "nightly perf gate; run in release: cargo test -p sdrmm-modem --release --test spread_perf compare_ -- --ignored"]
fn compare_spread_perf_baseline() {
    if cfg!(debug_assertions) {
        eprintln!("skipping the perf gate: throughput is only comparable in release");
        return;
    }
    let committed = load_baselines(&path(spread::PERF)).unwrap();
    if committed.iter().any(|b| b.host != host_id()) {
        eprintln!("skipping the perf gate: baseline host is not {}", host_id());
        return;
    }
    match compare_perf(&measured(), &committed, REGRESSION_FRACTION) {
        Ok(changes) => {
            for c in changes {
                eprintln!(
                    "{}: {:+.1}% vs baseline ({:.1} -> {:.1} Msamples/s)",
                    c.bench,
                    100.0 * c.change_fraction,
                    c.committed_msamples_per_s,
                    c.measured_msamples_per_s
                );
            }
        }
        Err(regressions) => panic!(
            "throughput regressions past {:.0}%: {regressions:#?}",
            100.0 * REGRESSION_FRACTION
        ),
    }
}
