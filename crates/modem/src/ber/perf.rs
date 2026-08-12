//! The performance scaffold (MODEM-PLAN §4.2). Three instruments behind one contract: every
//! engine commits a throughput baseline, the nightly fails a run that loses more than
//! [`REGRESSION_FRACTION`] of it, and the steady-state `process()` path is proven — not
//! promised — to allocate nothing, by counting every allocation the thread makes.
//!
//! Wall-clock time is allowed here and nowhere else in the harness: throughput is a property
//! of the machine, and the determinism rule (see [`super`]) governs signal content, never how
//! long the silicon took. What keeps a wall-clock number honest is the baseline protocol
//! instead — measured in release on a stated host, committed as JSON, and compared only on
//! that host, because an arm64 laptop's Msamples/s says nothing about an x86 runner's.
//!
//! The zero-allocation convention for hot paths: warm the object up until its buffers hold
//! their steady-state capacity (two full blocks — streaming stages carry an inter-block
//! remainder, so the block after the first is the first whose buffer must fit remainder plus
//! block), then wrap one more call in [`assert_no_alloc`]. The counting is per binary:
//! `#[global_allocator]` binds when a *binary* is linked, so this library cannot install the
//! counter on behalf of the test binaries that link it — each one declares its own
//! [`CountingAlloc`], and [`measure_allocs`] refuses to believe a binary that forgot.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    path::Path,
    time::Instant,
};

use num_complex::Complex;
use sdrmm_dsp::{RealDecimator, design_rrc};
use serde::{Deserialize, Serialize};

/// Fraction of committed throughput a bench may lose before [`compare_perf`] fails the run
/// (§4.2: the nightly fails on >10% regression; a per-entry override needs a justification
/// in the commit that makes it).
pub const REGRESSION_FRACTION: f64 = 0.10;

thread_local! {
    /// Per-thread rather than process-wide: `cargo test` runs many tests concurrently in one
    /// process, and a shared counter would charge a zero-alloc assertion with whatever its
    /// neighbour threads allocated mid-measurement. `const`-initialised and `Drop`-free, so
    /// touching it from inside the allocator can neither allocate nor re-enter.
    static THREAD_ALLOCS: Cell<u64> = const { Cell::new(0) };
}

fn count_one() {
    // `try_with`, because this runs inside `alloc`, where a panic is an abort: during thread
    // teardown another destructor may still allocate after this key is gone, and that
    // allocation simply goes uncounted.
    let _ = THREAD_ALLOCS.try_with(|count| count.set(count.get() + 1));
}

fn thread_allocs() -> u64 {
    THREAD_ALLOCS.try_with(Cell::get).unwrap_or(0)
}

/// The system allocator with a per-thread allocation counter in front — the instrument behind
/// §4.2's "zero steady-state allocations, asserted". Install one at the root of a test
/// binary:
///
/// ```text
/// #[global_allocator]
/// static ALLOC: sdrmm_modem::ber::perf::CountingAlloc = sdrmm_modem::ber::perf::CountingAlloc::new();
/// ```
///
/// Each binary installs its own instance because `#[global_allocator]` is resolved per
/// binary: this crate's test binary declares one for itself, and every future crate's test
/// binary that asserts zero-alloc declares another. Deallocations are not counted — a hot
/// path is gated on *acquiring* memory, and a free implies an acquisition somewhere that the
/// counter already saw.
#[derive(Clone, Copy, Debug, Default)]
pub struct CountingAlloc;

impl CountingAlloc {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

// SAFETY: every method forwards verbatim to `System`, whose contract the caller already
// carries; the counter touches no allocator state.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count_one();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count_one();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // A growth on the hot path is memory acquired on the hot path; it counts exactly as a
        // fresh allocation would.
        count_one();
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

/// Allocations the current thread performs while `f` runs. A zero is only believed from an
/// installed counter: the canary allocation proves [`CountingAlloc`] is this binary's
/// `#[global_allocator]`, because an uninstalled counter reads zero for everything and would
/// green every zero-alloc gate vacuously.
///
/// # Panics
/// If the counting allocator is not installed in the calling binary.
#[must_use]
pub fn measure_allocs(f: impl FnOnce()) -> u64 {
    let canary = thread_allocs();
    std::hint::black_box(Box::new(0u8));
    assert!(
        thread_allocs() > canary,
        "CountingAlloc is not this binary's #[global_allocator]; a zero-alloc assertion \
         would pass vacuously"
    );
    let before = thread_allocs();
    f();
    thread_allocs() - before
}

/// The hot-path gate (§4.2): after warm-up, one steady-state call must allocate nothing.
/// `label` names the path in the failure, because "1 allocation" is only actionable with a
/// culprit attached.
///
/// # Panics
/// If `f` allocated, or if the counting allocator is not installed (see [`measure_allocs`]).
pub fn assert_no_alloc(label: &str, f: impl FnOnce()) {
    let allocs = measure_allocs(f);
    assert_eq!(
        allocs, 0,
        "{label}: {allocs} allocation(s) on the steady-state path"
    );
}

/// Wall-clock throughput of `iters` calls of `f`, each consuming `samples_per_iter` input
/// samples, in Msamples/s. Plain [`Instant`] rather than criterion, so the baseline tests and
/// any quick probe measure exactly the work the criterion benches measure, without the
/// statistical machinery. Callers warm `f` up first: a cold first call carries one-off buffer
/// growth, and that is allocation to assert on, not throughput to average in.
#[must_use]
pub fn measure_throughput(iters: u64, samples_per_iter: u64, mut f: impl FnMut()) -> f64 {
    let start = Instant::now();
    for _ in 0..iters {
        f();
    }
    // Floored so a degenerate zero-length measurement stays finite instead of feeding an
    // infinity into the comparison arithmetic downstream.
    let elapsed = start.elapsed().as_secs_f64().max(1e-9);
    (iters as f64) * (samples_per_iter as f64) / elapsed / 1e6
}

/// One engine configuration's committed performance number (§4.2). `bench` matches the
/// criterion bench id; `config` states the measured configuration in words; `host` is the
/// coarse `arch-os` pair from [`host_id`] — enough to refuse a cross-machine comparison, free
/// of hostname lookups that would differ per developer laptop. `realtime_factor` is
/// throughput over the entry's required processing rate: how many channels of this type one
/// core sustains.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerfBaseline {
    pub bench: String,
    pub msamples_per_s: f64,
    pub realtime_factor: f64,
    pub config: String,
    pub host: String,
}

/// The `arch-os` pair a baseline is stamped with and compared under.
#[must_use]
pub fn host_id() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

/// Writes baselines as pretty-printed JSON — the committed artifact reviewers read in diffs.
pub fn save_baselines(path: &Path, baselines: &[PerfBaseline]) -> std::io::Result<()> {
    let mut json = serde_json::to_string_pretty(baselines).map_err(std::io::Error::other)?;
    json.push('\n');
    std::fs::write(path, json)
}

/// Reads a committed baseline list back. A malformed file is an error, never an empty list —
/// a gate with nothing to compare against has to say so, not pass.
pub fn load_baselines(path: &Path) -> std::io::Result<Vec<PerfBaseline>> {
    serde_json::from_str(&std::fs::read_to_string(path)?).map_err(std::io::Error::other)
}

/// One bench's measured-vs-committed outcome. `change_fraction` is signed: +0.08 is 8% faster
/// than the baseline, −0.12 is the regression the gate exists for. −1.0 marks a committed
/// bench the measurement produced no comparable number for — missing, config string moved, or
/// a zero baseline — treated as a full loss, because a bench that silently vanishes is how a
/// perf gate dies.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerfChange {
    pub bench: String,
    pub committed_msamples_per_s: f64,
    pub measured_msamples_per_s: f64,
    pub change_fraction: f64,
}

/// The §4.2 regression gate: every committed bench, matched by name *and* config (a number
/// measured at another configuration is a different number), against its measured throughput.
/// `Ok` carries every change so improvements are reported, not just tolerated; `Err` carries
/// the benches that lost more than `regression_fraction` (default [`REGRESSION_FRACTION`]).
/// Measured benches with no committed counterpart pass silently — they are committed when the
/// baseline is next rewritten.
///
/// # Errors
/// The regressed entries, worst offenders included, when any committed bench fails the gate.
pub fn compare_perf(
    measured: &[PerfBaseline],
    committed: &[PerfBaseline],
    regression_fraction: f64,
) -> Result<Vec<PerfChange>, Vec<PerfChange>> {
    let mut changes = Vec::new();
    let mut regressions = Vec::new();
    for reference in committed {
        let found = measured
            .iter()
            .find(|m| m.bench == reference.bench && m.config == reference.config);
        let change = match found {
            Some(m) if reference.msamples_per_s > 0.0 => PerfChange {
                bench: reference.bench.clone(),
                committed_msamples_per_s: reference.msamples_per_s,
                measured_msamples_per_s: m.msamples_per_s,
                change_fraction: (m.msamples_per_s - reference.msamples_per_s)
                    / reference.msamples_per_s,
            },
            _ => PerfChange {
                bench: reference.bench.clone(),
                committed_msamples_per_s: reference.msamples_per_s,
                measured_msamples_per_s: 0.0,
                change_fraction: -1.0,
            },
        };
        if change.change_fraction < -regression_fraction {
            regressions.push(change.clone());
        }
        changes.push(change);
    }
    if regressions.is_empty() {
        Ok(changes)
    } else {
        Err(regressions)
    }
}

/// The xorshift32 the dsp fixtures use. A bench signal only has to be busy and reproducible;
/// statistical care is the harness RNG's job ([`super::rng`]), not the signal generator's.
struct XorShift32(u32);

impl XorShift32 {
    /// Zero is xorshift's one fixed point, so it is mapped away rather than trusted not to
    /// be passed.
    fn seeded(seed: u32) -> Self {
        Self(seed | 1)
    }

    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
}

/// Deterministic pseudo-random dibits for bench signals.
#[must_use]
pub fn test_dibits(len: usize, seed: u32) -> Vec<u8> {
    let mut rng = XorShift32::seeded(seed);
    (0..len).map(|_| (rng.next() & 3) as u8).collect()
}

/// Antipodal ±1 symbols through a root-raised-cosine (α = 0.35) at `sps` samples per symbol,
/// as complex baseband with zero quadrature — the densest diet a Gardner loop gets, since
/// every symbol change is an equal-and-opposite transition it takes an error from. That makes
/// this the *expensive* case for `SymbolSync`, which is the right one to baseline.
///
/// # Panics
/// If `sps` is not a whole number of at least two.
#[must_use]
pub fn shaped_bpsk_iq(symbols: usize, sps: f64, seed: u32) -> Vec<Complex<f32>> {
    assert!(
        sps >= 2.0 && (sps - sps.round()).abs() < 1e-9,
        "need a whole number of samples per symbol, at least two"
    );
    let step = sps as usize;
    let taps = design_rrc(sps, 0.35, 8);
    let mut impulses = vec![0.0f32; symbols * step + taps.len()];
    let mut rng = XorShift32::seeded(seed);
    for i in 0..symbols {
        let sign = if rng.next() & 1 == 0 { 1.0 } else { -1.0 };
        impulses[i * step] = sign * sps as f32;
    }
    let mut shaped = Vec::new();
    RealDecimator::new(&taps, 1).process(&impulses, &mut shaped);
    shaped.iter().map(|&re| Complex::new(re, 0.0)).collect()
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;

    use sdrmm_dsp::SymbolSync;

    use super::*;

    /// This test binary's counter (see [`CountingAlloc`]: `#[global_allocator]` binds per
    /// binary, so the library cannot install it for anyone else).
    #[global_allocator]
    static ALLOC: CountingAlloc = CountingAlloc::new();

    #[test]
    fn counts_a_known_vec_allocation() {
        let allocs = measure_allocs(|| {
            black_box(Vec::<u8>::with_capacity(4096));
        });
        assert_eq!(allocs, 1, "one with_capacity is exactly one allocation");
    }

    #[test]
    fn pure_arithmetic_reads_zero() {
        let mut acc = 0.0f32;
        let allocs = measure_allocs(|| {
            for i in 0..10_000 {
                acc += (i as f32).sqrt();
            }
        });
        black_box(acc);
        assert_eq!(allocs, 0);
    }

    fn baseline(bench: &str, msamples_per_s: f64) -> PerfBaseline {
        PerfBaseline {
            bench: bench.into(),
            msamples_per_s,
            realtime_factor: msamples_per_s * 1e6 / 48_000.0,
            config: "test configuration".into(),
            host: host_id(),
        }
    }

    #[test]
    fn baselines_round_trip_through_json() {
        let path = std::env::temp_dir().join(format!(
            "sdrmm-modem-perf-roundtrip-{}.json",
            std::process::id()
        ));
        let committed = vec![
            baseline("cpm_demod_m4_48k", 21.5),
            baseline("symbol_sync_8sps", 84.25),
        ];
        save_baselines(&path, &committed).unwrap();
        let loaded = load_baselines(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(loaded, committed);
    }

    #[test]
    fn a_doctored_regression_fails_the_gate() {
        let committed = vec![baseline("cpm_demod_m4_48k", 100.0)];
        let measured = vec![baseline("cpm_demod_m4_48k", 85.0)];
        let regressions = compare_perf(&measured, &committed, REGRESSION_FRACTION).unwrap_err();
        assert_eq!(regressions.len(), 1);
        assert!((regressions[0].change_fraction + 0.15).abs() < 1e-12);
    }

    #[test]
    fn an_improvement_passes_and_is_reported() {
        let committed = vec![baseline("cpm_demod_m4_48k", 100.0)];
        let measured = vec![baseline("cpm_demod_m4_48k", 130.0)];
        let changes = compare_perf(&measured, &committed, REGRESSION_FRACTION).unwrap();
        assert_eq!(changes.len(), 1);
        assert!((changes[0].change_fraction - 0.30).abs() < 1e-12);
    }

    #[test]
    fn a_wobble_inside_the_tolerance_passes() {
        let committed = vec![baseline("cpm_demod_m4_48k", 100.0)];
        let measured = vec![baseline("cpm_demod_m4_48k", 91.0)];
        assert!(compare_perf(&measured, &committed, REGRESSION_FRACTION).is_ok());
    }

    #[test]
    fn a_vanished_bench_is_a_full_regression() {
        let committed = vec![baseline("cpm_demod_m4_48k", 100.0)];
        let regressions = compare_perf(&[], &committed, REGRESSION_FRACTION).unwrap_err();
        assert_eq!(regressions[0].change_fraction, -1.0);
    }

    #[test]
    fn throughput_helper_measures_something_finite() {
        let mut acc = 0u64;
        let msamples_per_s = measure_throughput(8, 1_024, || {
            for i in 0..1_024u64 {
                acc = acc.wrapping_add(i * i);
            }
        });
        black_box(acc);
        assert!(
            msamples_per_s.is_finite() && msamples_per_s > 0.0,
            "measured {msamples_per_s} Msamples/s"
        );
    }

    /// §4.2's zero-alloc gate on the shared timing stack. Two warm-up blocks, per the
    /// module-level convention: streaming stages carry an inter-block remainder, so the second
    /// block is the first whose buffers must fit remainder plus block, and only after it has
    /// the capacity envelope stopped growing. The engines that compose it carry their own —
    /// `cpm::demod`'s tests gate both input domains.
    #[test]
    fn symbol_sync_steady_state_allocates_nothing() {
        let iq = shaped_bpsk_iq(4_096, 8.0, 0x0dd5);
        let mut sync = SymbolSync::new(8.0, 0.01);
        let mut symbols = Vec::with_capacity(iq.len());
        sync.process(&iq, &mut symbols);
        symbols.clear();
        sync.process(&iq, &mut symbols);
        symbols.clear();
        assert_no_alloc("SymbolSync::process", || sync.process(&iq, &mut symbols));
        assert!(
            !symbols.is_empty(),
            "the measured call recovered no symbols"
        );
    }

    /// Reference processing rate the real-time factor divides by (§4.2): the input sample rate
    /// the entry consumes in its named configuration — `SymbolSync` at 8 samples per symbol of
    /// a 4800 baud channel eats 38.4 kHz.
    const SYMBOL_SYNC_RATE_HZ: f64 = 8.0 * 4_800.0;

    /// Committed rows whose chain no longer exists. `fsk4_dmr_48k` measured `Fsk4Demod`, which
    /// phase 3 deleted when the `cpm` engine replaced it (MODEM-PLAN §7). A committed
    /// measurement is never regenerated or edited (§8), so the number stays in the file as the
    /// pre-migration reference the migration was judged against — but nothing can measure it
    /// again, and neither the writer nor the gate may treat that as a change. The engine that
    /// replaced it carries its own baseline, `baselines/cpm/mfsk_perf.json`.
    const RETIRED: [&str; 1] = ["fsk4_dmr_48k"];

    fn measured_baselines() -> Vec<PerfBaseline> {
        let bpsk = shaped_bpsk_iq(4_096, 8.0, 0x0dd5);
        let mut sync = SymbolSync::new(8.0, 0.01);
        let mut symbols = Vec::with_capacity(bpsk.len());
        sync.process(&bpsk, &mut symbols);
        let sync_msps = measure_throughput(1_500, bpsk.len() as u64, || {
            symbols.clear();
            sync.process(&bpsk, &mut symbols);
        });

        vec![PerfBaseline {
            bench: "symbol_sync_8sps".into(),
            msamples_per_s: sync_msps,
            realtime_factor: sync_msps * 1e6 / SYMBOL_SYNC_RATE_HZ,
            config: "8 sps, loop_bw 0.01, RRC α=0.35 shaped BPSK".into(),
            host: host_id(),
        }]
    }

    /// The committed rows this scaffold can still measure — the file minus [`RETIRED`].
    fn live_committed(committed: &[PerfBaseline]) -> Vec<PerfBaseline> {
        committed
            .iter()
            .filter(|b| !RETIRED.contains(&b.bench.as_str()))
            .cloned()
            .collect()
    }

    fn committed_baseline_path() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("baselines/perf_phase0.json")
    }

    /// The committed file still carries the deleted chain's row, and the gate still compares
    /// every row that is not it — the two halves of the retirement, so neither the history nor
    /// the live gate can be lost without a failure here.
    #[test]
    fn the_committed_file_keeps_its_history_and_gates_the_rest() {
        let committed = load_baselines(&committed_baseline_path()).unwrap();
        assert!(
            committed.iter().any(|b| b.bench == "fsk4_dmr_48k"),
            "the pre-migration reference row was removed from the committed baseline"
        );
        let live: Vec<String> = live_committed(&committed)
            .into_iter()
            .map(|b| b.bench)
            .collect();
        assert_eq!(live, ["symbol_sync_8sps"]);
    }

    /// Rewrites the committed phase-0 baseline. Run deliberately, on the reference machine:
    /// `cargo test -p sdrmm-modem --release write_perf_baseline -- --ignored`. The [`RETIRED`]
    /// rows are carried through untouched — a rerun must not quietly erase the history the
    /// migration was measured against.
    #[test]
    #[ignore = "rewrites the committed baseline; run explicitly in release on the reference host"]
    fn write_perf_baseline() {
        if cfg!(debug_assertions) {
            panic!("a debug-profile number must never become the committed baseline");
        }
        let path = committed_baseline_path();
        let mut rows: Vec<PerfBaseline> = load_baselines(&path)
            .unwrap_or_default()
            .into_iter()
            .filter(|b| RETIRED.contains(&b.bench.as_str()))
            .collect();
        rows.extend(measured_baselines());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        save_baselines(&path, &rows).unwrap();
    }

    /// The nightly perf gate: measured against committed, failing past
    /// [`REGRESSION_FRACTION`]. Compared only in release and only on the host that wrote the
    /// baseline — a debug build or another machine's silicon would flag its own slowness as
    /// an engine regression.
    #[test]
    #[ignore = "nightly perf gate; run in release: cargo test -p sdrmm-modem --release compare_perf_baseline -- --ignored"]
    fn compare_perf_baseline() {
        if cfg!(debug_assertions) {
            eprintln!("skipping the perf gate: throughput is only comparable in release");
            return;
        }
        let committed = load_baselines(&committed_baseline_path()).unwrap();
        if committed.iter().any(|b| b.host != host_id()) {
            eprintln!("skipping the perf gate: baseline host is not {}", host_id());
            return;
        }
        let committed = live_committed(&committed);
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
}
