#![allow(clippy::unwrap_used, clippy::expect_used)]

use num_complex::Complex;
use sdrmm_modem::multicarrier::{
    FbmcDemod, FbmcMod, FbmcParams, GfdmDemod, GfdmDetector, GfdmMod, OtfsGrid, OtfsPrecoder,
    UfmcDemod, UfmcMod, UfmcParams,
};
use sdrmm_modem_test_support::ber::{
    catalog::{multicarrier::gfdm_params, ofdm::RATE},
    perf::{
        CountingAlloc, PerfBaseline, REGRESSION_FRACTION, assert_no_alloc, compare_perf, host_id,
        load_baselines, measure_throughput, save_baselines,
    },
};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

const STEM: &str = "multicarrier/multicarrier_perf";

const BLOCKS: usize = 8;
const SYMBOLS: usize = 8;

fn points(count: usize) -> Vec<Complex<f32>> {
    let mut state = 0x0_9c9cu32;
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let i = if state & 1 == 0 { 1.0 } else { -1.0 };
            let q = if state & 2 == 0 { 1.0 } else { -1.0 };
            Complex::new(i, q) / 2f32.sqrt()
        })
        .collect()
}

struct Chain<M, D> {
    modulator: M,
    demodulator: D,
    points: Vec<Complex<f32>>,
    wave: Vec<Complex<f32>>,
}

fn gfdm_chain(detector: GfdmDetector) -> Chain<GfdmMod, GfdmDemod> {
    let params = gfdm_params();
    let points = points(BLOCKS * params.block());
    let mut wave = Vec::with_capacity(BLOCKS * params.samples());
    let mut modulator = GfdmMod::new(params);
    modulator.modulate(&points, &mut wave);
    Chain {
        modulator,
        demodulator: GfdmDemod::new(params, detector),
        points,
        wave,
    }
}

fn ufmc_chain() -> Chain<UfmcMod, UfmcDemod> {
    let params = UfmcParams::reference();
    let points = points(SYMBOLS * params.points());
    let mut wave = Vec::with_capacity(SYMBOLS * params.samples());
    let mut modulator = UfmcMod::new(params);
    modulator.modulate(&points, &mut wave);
    Chain {
        modulator,
        demodulator: UfmcDemod::new(params),
        points,
        wave,
    }
}

fn fbmc_chain() -> Chain<FbmcMod, FbmcDemod> {
    let params = FbmcParams::reference();
    let points = points(SYMBOLS * params.allocated);
    let mut wave = Vec::with_capacity(params.frame_samples(points.len()));
    let mut modulator = FbmcMod::new(params);
    modulator.modulate(&points, &mut wave);
    Chain {
        modulator,
        demodulator: FbmcDemod::new(params),
        points,
        wave,
    }
}

fn measured_baselines() -> Vec<PerfBaseline> {
    let params = gfdm_params();
    let mut zf = gfdm_chain(GfdmDetector::ZeroForcing);
    let mut sink = Vec::with_capacity(zf.points.len());
    let gfdm_msps = measure_throughput(400, zf.wave.len() as u64, || {
        sink.clear();
        zf.demodulator.demodulate(&zf.wave, &mut sink);
    });

    let mut ufmc = ufmc_chain();
    let mut sink = Vec::with_capacity(ufmc.points.len());
    let ufmc_msps = measure_throughput(400, ufmc.wave.len() as u64, || {
        sink.clear();
        ufmc.demodulator.demodulate(&ufmc.wave, &mut sink);
    });

    let mut fbmc = fbmc_chain();
    let mut sink = Vec::with_capacity(fbmc.points.len());
    let fbmc_msps = measure_throughput(200, fbmc.wave.len() as u64, || {
        sink.clear();
        fbmc.demodulator.demodulate(&fbmc.wave, SYMBOLS, &mut sink);
    });

    let grid = OtfsGrid::new(48, 16);
    let mut precoder = OtfsPrecoder::new(grid);
    let dd = points(grid.points());
    let mut tf = vec![Complex::new(0.0, 0.0); grid.points()];
    let carrier_samples = 16 * 80;
    let otfs_msps = measure_throughput(4_000, carrier_samples as u64, || {
        precoder.spread(&dd, &mut tf);
    });

    let mut modulator = GfdmMod::new(params);
    let mut wave = Vec::with_capacity(BLOCKS * params.samples());
    let gfdm_tx_msps = measure_throughput(400, (BLOCKS * params.samples()) as u64, || {
        wave.clear();
        modulator.modulate(&zf.points, &mut wave);
    });

    vec![
        PerfBaseline {
            bench: "gfdm_zf_20m".into(),
            msamples_per_s: gfdm_msps,
            realtime_factor: gfdm_msps * 1e6 / RATE,
            config: format!(
                "{}×{} block, roll-off {}, {}-sample prefix, {BLOCKS} blocks, dense A⁻¹ per block",
                params.subcarriers, params.subsymbols, params.rolloff, params.cp
            ),
            host: host_id(),
        },
        PerfBaseline {
            bench: "gfdm_tx_20m".into(),
            msamples_per_s: gfdm_tx_msps,
            realtime_factor: gfdm_tx_msps * 1e6 / RATE,
            config: format!(
                "{}×{} block, dense A per block — the transmitter pays exactly what the \
                 zero-forcing receiver does",
                params.subcarriers, params.subsymbols
            ),
            host: host_id(),
        },
        PerfBaseline {
            bench: "ufmc_20m".into(),
            msamples_per_s: ufmc_msps,
            realtime_factor: ufmc_msps * 1e6 / RATE,
            config: format!(
                "{}-point transform, 4 subbands of 12, 33-tap prototype, {SYMBOLS} symbols, \
                 zero-pad-to-2N receiver",
                UfmcParams::reference().fft
            ),
            host: host_id(),
        },
        PerfBaseline {
            bench: "fbmc_20m".into(),
            msamples_per_s: fbmc_msps,
            realtime_factor: fbmc_msps * 1e6 / RATE,
            config: format!(
                "64-subcarrier bank, 48 allocated, PHYDYAS K=4, {SYMBOLS} symbols, direct-form \
                 analysis bank"
            ),
            host: host_id(),
        },
        PerfBaseline {
            bench: "otfs_precoder_20m".into(),
            msamples_per_s: otfs_msps,
            realtime_factor: otfs_msps * 1e6 / RATE,
            config: "48×16 delay–Doppler grid, ISFFT only — the carrier's own cost is the \
                     CP-OFDM row's"
                .into(),
            host: host_id(),
        },
    ]
}

fn path(stem: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("baselines/{stem}.json"))
}

#[test]
fn the_gfdm_paths_allocate_nothing() {
    let params = gfdm_params();
    for detector in [GfdmDetector::ZeroForcing, GfdmDetector::Matched] {
        let mut chain = gfdm_chain(detector);
        let mut sink = Vec::with_capacity(chain.points.len());
        for _ in 0..2 {
            sink.clear();
            chain.demodulator.demodulate(&chain.wave, &mut sink);
        }
        sink.clear();
        assert_no_alloc("GfdmDemod::demodulate", || {
            chain.demodulator.demodulate(&chain.wave, &mut sink);
        });
        assert_eq!(sink.len(), chain.points.len());

        let mut wave = Vec::with_capacity(BLOCKS * params.samples());
        for _ in 0..2 {
            wave.clear();
            chain.modulator.modulate(&chain.points, &mut wave);
        }
        wave.clear();
        assert_no_alloc("GfdmMod::modulate", || {
            chain.modulator.modulate(&chain.points, &mut wave);
        });
    }
}

#[test]
fn the_ufmc_paths_allocate_nothing() {
    let params = UfmcParams::reference();
    let mut chain = ufmc_chain();
    let mut sink = Vec::with_capacity(chain.points.len());
    for _ in 0..2 {
        sink.clear();
        chain.demodulator.demodulate(&chain.wave, &mut sink);
    }
    sink.clear();
    assert_no_alloc("UfmcDemod::demodulate", || {
        chain.demodulator.demodulate(&chain.wave, &mut sink);
    });
    assert_eq!(sink.len(), chain.points.len());

    let mut wave = Vec::with_capacity(SYMBOLS * params.samples());
    for _ in 0..2 {
        wave.clear();
        chain.modulator.modulate(&chain.points, &mut wave);
    }
    wave.clear();
    assert_no_alloc("UfmcMod::modulate", || {
        chain.modulator.modulate(&chain.points, &mut wave);
    });
}

#[test]
fn the_fbmc_paths_allocate_nothing() {
    let params = FbmcParams::reference();
    let mut chain = fbmc_chain();
    let mut sink = Vec::with_capacity(chain.points.len());
    for _ in 0..2 {
        sink.clear();
        chain
            .demodulator
            .demodulate(&chain.wave, SYMBOLS, &mut sink);
    }
    sink.clear();
    assert_no_alloc("FbmcDemod::demodulate", || {
        chain
            .demodulator
            .demodulate(&chain.wave, SYMBOLS, &mut sink);
    });
    assert_eq!(sink.len(), chain.points.len());

    let mut wave = Vec::with_capacity(params.frame_samples(chain.points.len()));
    for _ in 0..2 {
        wave.clear();
        chain.modulator.modulate(&chain.points, &mut wave);
    }
    wave.clear();
    assert_no_alloc("FbmcMod::modulate", || {
        chain.modulator.modulate(&chain.points, &mut wave);
    });
}

#[test]
fn the_otfs_precoder_allocates_nothing() {
    let grid = OtfsGrid::new(48, 16);
    let mut precoder = OtfsPrecoder::new(grid);
    let dd = points(grid.points());
    let mut tf = vec![Complex::new(0.0, 0.0); grid.points()];
    let mut back = vec![Complex::new(0.0, 0.0); grid.points()];
    for _ in 0..2 {
        precoder.spread(&dd, &mut tf);
        precoder.despread(&tf, &mut back);
    }
    assert_no_alloc("OtfsPrecoder::spread", || precoder.spread(&dd, &mut tf));
    assert_no_alloc("OtfsPrecoder::despread", || {
        precoder.despread(&tf, &mut back);
    });
}

#[test]
#[ignore = "rewrites the committed baseline; run explicitly in release on the reference host"]
fn write_multicarrier_perf_baseline() {
    if cfg!(debug_assertions) {
        panic!("a debug-profile number must never become the committed baseline");
    }
    let path = path(STEM);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let rows = measured_baselines();
    for row in &rows {
        println!(
            "{}: {:.1} Msamples/s, {:.2}x real time at {} MHz",
            row.bench,
            row.msamples_per_s,
            row.realtime_factor,
            RATE / 1e6
        );
    }
    save_baselines(&path, &rows).unwrap();
}

#[test]
#[ignore = "nightly perf gate; run in release"]
fn compare_multicarrier_perf_baseline() {
    if cfg!(debug_assertions) {
        eprintln!("skipping the perf gate: throughput is only comparable in release");
        return;
    }
    let committed = load_baselines(&path(STEM)).unwrap();
    if committed.iter().any(|b| b.host != host_id()) {
        eprintln!("skipping the perf gate: baseline host is not {}", host_id());
        return;
    }
    match compare_perf(&measured_baselines(), &committed, REGRESSION_FRACTION) {
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

#[test]
fn the_cost_ordering_is_the_structural_one() {
    if cfg!(debug_assertions) {
        return;
    }
    let rows = measured_baselines();
    let rate = |bench: &str| {
        rows.iter()
            .find(|r| r.bench == bench)
            .map(|r| r.msamples_per_s)
            .unwrap()
    };
    let order = ["fbmc_20m", "gfdm_zf_20m", "ufmc_20m", "otfs_precoder_20m"];
    for pair in order.windows(2) {
        assert!(
            rate(pair[0]) < rate(pair[1]),
            "{} at {:.1} is not below {} at {:.1} Msamples/s",
            pair[0],
            rate(pair[0]),
            pair[1],
            rate(pair[1])
        );
    }
}
