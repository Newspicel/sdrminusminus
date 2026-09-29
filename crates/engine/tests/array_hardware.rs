#![cfg(feature = "rtlsdr")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
mod kraken;

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use common::array::{calibrated, df, processor_status, status, wait_status};
use kraken::{ARRAY, Kraken, RADIUS_M, append_csv, csv_path, env_f64, uca, wrap_deg, write_csv};
use sdrmm_dsp::doa::{dominance_count, mdl_count};
use sdrmm_engine::{ArrayEvent, Engine, ProcessorSpec};
use sdrmm_wire::{
    ArrayCal, ArrayCalSource, ArrayFailure, ArrayGain, ArrayStatus, ArrayTuneRequest, CalPhase,
    DfReading, ProcessorParams, ProcessorReading, SyncState, Winding, processor::df::DfParams,
};
use tokio::sync::broadcast::{self, error::TryRecvError};

const UHF_HZ: f64 = 433.92e6;
const GAIN_DB: f64 = 30.0;
const CHECK_S: u32 = 10;
const LOCK_LIMIT: Duration = Duration::from_secs(15);
const HOLD: Duration = Duration::from_secs(180);
const HOLD_LIMIT: Duration = Duration::from_secs(240);
const CHECKS: usize = 18;
const RESIDUAL_DELAY: f32 = 0.1;
const RESIDUAL_PHASE_DEG: f32 = 2.0;
const RESOLVE_LIMIT: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(20);
const DF_REPORTS: usize = 60;
const DF_WAIT: Duration = Duration::from_secs(120);
const DF_OFFSET_HZ: f64 = 200e3;
const DF_MIN_ERROR_DEG: f64 = 5.0;
const DF_SPREAD_LOW: f64 = 0.5;
const DF_SPREAD_HIGH: f64 = 2.0;
const DF_AXIS_GUARD_DEG: f64 = 10.0;
const DF_MIN_BEARINGS: usize = 3;
const DF_SINGLE_SHARE: f64 = 0.9;
const DF_CSV: &str = "kraken_df_known_bearing";

fn noise_cal(check_s: u32) -> ArrayCal {
    ArrayCal {
        source: ArrayCalSource::Noise,
        check_s,
        ..ArrayCal::default()
    }
}

#[derive(Default)]
struct Hold {
    solves: Vec<(Duration, ArrayStatus)>,
    failures: BTreeMap<String, usize>,
    noise_not_seen: usize,
    readings: Vec<DfReading>,
}

impl Hold {
    fn watch(&mut self, now: &ArrayStatus, at: Duration) {
        if let Some(failure) = &now.failure {
            *self.failures.entry(format!("{failure:?}")).or_default() += 1;
            if *failure == ArrayFailure::NoiseNotSeen {
                self.noise_not_seen += 1;
            }
        }
        let fresh = self
            .solves
            .last()
            .is_none_or(|(_, last)| last.last_solve_at != now.last_solve_at);
        if fresh && calibrated(now) {
            self.solves.push((at, now.clone()));
        }
    }

    fn take(&mut self, events: &mut broadcast::Receiver<ArrayEvent>) {
        loop {
            match events.try_recv() {
                Ok(ArrayEvent::Report { processor, reading }) if processor == "df" => {
                    if let ProcessorReading::Df(reading) = reading.as_ref() {
                        self.readings.push(reading.clone());
                    }
                }
                Ok(_) | Err(TryRecvError::Lagged(_)) => {}
                Err(TryRecvError::Empty | TryRecvError::Closed) => return,
            }
        }
    }

    fn checks(&self) -> &[(Duration, ArrayStatus)] {
        self.solves.get(1..).unwrap_or_default()
    }
}

fn residual_rows(hold: &Hold) -> Vec<String> {
    let mut rows = Vec::new();
    for (check, (at, solved)) in hold.checks().iter().enumerate() {
        for lane in solved.lanes.iter().skip(1) {
            rows.push(format!(
                "{check},{:.1},{},{:.4},{:.3},{:.3},{:.3},{:?}",
                at.as_secs_f64(),
                lane.lane,
                lane.residual_delay.unwrap_or(f32::NAN),
                lane.residual_phase_deg.unwrap_or(f32::NAN),
                lane.phase_deg,
                lane.coherence,
                lane.sync
            ));
        }
    }
    rows
}

fn assert_residuals(hold: &Hold) {
    for (check, (_, solved)) in hold.checks().iter().enumerate() {
        for lane in solved.lanes.iter().skip(1) {
            let delay = lane
                .residual_delay
                .expect("a check reports its residual delay");
            let phase = lane
                .residual_phase_deg
                .expect("a check reports its residual phase");
            assert!(
                delay.abs() < RESIDUAL_DELAY,
                "check {check} lane {}: residual delay {delay}",
                lane.lane
            );
            assert!(
                phase.abs() < RESIDUAL_PHASE_DEG,
                "check {check} lane {}: residual phase {phase}",
                lane.lane
            );
        }
    }
}

fn df_counts(readings: &[DfReading]) -> String {
    let mut dominance = BTreeMap::<usize, usize>::new();
    let mut mdl = BTreeMap::<usize, usize>::new();
    let mut reported = BTreeMap::<u32, usize>::new();
    for reading in readings {
        *reported.entry(reading.sources).or_default() += 1;
        let values: Vec<f32> = reading
            .eigenvalues_db
            .iter()
            .map(|db| 10f32.powf(db / 10.0))
            .collect();
        if values.is_empty() {
            continue;
        }
        *dominance.entry(dominance_count(&values)).or_default() += 1;
        *mdl.entry(mdl_count(&values, 1e4)).or_default() += 1;
    }
    format!(
        "{} DF reports; sources reported {reported:?}; dominance {dominance:?}; MDL {mdl:?}",
        readings.len()
    )
}

#[cfg(feature = "probe")]
mod probe {
    use common::array::{processor, wait_for};
    use sdrmm_channels::array_processor::probe::{ProbeBlock, ProbeLog, take_probe_log};
    use sdrmm_wire::processor::ProbeParams;

    use super::*;

    pub struct Probe {
        log: ProbeLog,
        blocks: Vec<ProbeBlock>,
    }

    impl Probe {
        pub fn start(engine: &Engine) -> Self {
            engine
                .apply_processor(processor(
                    "probe",
                    ProcessorParams::Probe(ProbeParams {
                        time: true,
                        phase: true,
                        ..ProbeParams::default()
                    }),
                ))
                .unwrap();
            let log = wait_for("the probe log", LOCK_LIMIT, || take_probe_log("probe"));
            Self {
                log,
                blocks: Vec::new(),
            }
        }

        pub fn take(&mut self) {
            self.blocks.extend(self.log.drain());
        }

        pub fn verdict(&self) -> Result<String, String> {
            let power = |block: &ProbeBlock| {
                block.power[..block.lanes]
                    .iter()
                    .copied()
                    .fold(0.0, f32::max)
            };
            let mut levels: Vec<f32> = self.blocks.iter().map(power).collect();
            levels.sort_by(f32::total_cmp);
            let median = levels.get(levels.len() / 2).copied().unwrap_or(0.0);
            let loudest = levels.last().copied().unwrap_or(0.0);
            let lost: u64 = self.blocks.iter().map(|block| block.lost).sum();
            let mut loud: Vec<&ProbeBlock> = self
                .blocks
                .iter()
                .filter(|block| power(block) >= 2.0 * median)
                .collect();
            loud.truncate(8);
            let first = self.blocks.first().map_or(0, |block| block.unix_ns);
            for block in &loud {
                println!(
                    "loud block at {:.3} s seq {} index {} generation {} gap {} power {:?}",
                    block.unix_ns.saturating_sub(first) as f64 * 1e-9,
                    block.seq,
                    block.first_index,
                    block.generation,
                    block.gap_before,
                    &block.power[..block.lanes]
                );
            }
            let text = format!(
                "probe: {} blocks, median power {median:.3e}, loudest {loudest:.3e}, lost {lost}",
                self.blocks.len()
            );
            if self.blocks.is_empty() || loudest >= 2.0 * median || lost > 0 {
                Err(text)
            } else {
                Ok(text)
            }
        }
    }
}

#[cfg(not(feature = "probe"))]
mod probe {
    use super::Engine;

    pub struct Probe;

    impl Probe {
        pub const fn start(_engine: &Engine) -> Self {
            Self
        }

        pub const fn take(&mut self) {}

        pub fn verdict(&self) -> Result<String, String> {
            Ok(
                "probe: not built, run with --features probe to check for noise-level blocks"
                    .to_owned(),
            )
        }
    }
}

#[test]
#[ignore = "requires an idle KrakenSDR; runs a calibrated Array on it for three minutes"]
fn kraken_array_locks_and_holds() {
    let kraken = Kraken::open();
    let engine = &kraken.engine;
    let mut events = engine.subscribe_arrays();
    let started = Instant::now();
    engine
        .apply_array(kraken.array(UHF_HZ, GAIN_DB, noise_cal(CHECK_S)))
        .unwrap();
    engine.apply_processor(df("df")).unwrap();
    let mut probe = probe::Probe::start(engine);
    let locked = wait_status(engine, "Locked and Solved", LOCK_LIMIT, calibrated);
    let lock_time = started.elapsed();
    let mut hold = Hold::default();
    let held = Instant::now();
    while (held.elapsed() < HOLD || hold.checks().len() < CHECKS) && held.elapsed() < HOLD_LIMIT {
        hold.watch(&status(engine), held.elapsed());
        hold.take(&mut events);
        probe.take();
        std::thread::sleep(POLL);
    }
    let end = status(engine);
    let probed = probe.verdict();
    let counts = df_counts(&hold.readings);
    let summary = [
        format!("locked and solved in {:.2} s", lock_time.as_secs_f64()),
        format!(
            "{} checks in {:.0} s",
            hold.checks().len(),
            held.elapsed().as_secs_f64()
        ),
        format!("failures seen {:?}", hold.failures),
        format!(
            "df gated {:?} at the end, dropped {} samples",
            processor_status(&end, "df").gated,
            processor_status(&end, "df").dropped_samples
        ),
        counts,
        probed.clone().unwrap_or_else(|error| error),
    ];
    for line in &summary {
        println!("{line}");
    }
    write_csv(
        "kraken_array_locks_and_holds",
        "check,at_s,lane,residual_delay,residual_phase_deg,phase_deg,coherence,sync",
        &residual_rows(&hold),
    );
    write_csv("kraken_array_locks_and_holds_summary", "summary", &summary);
    assert!(lock_time < LOCK_LIMIT, "{lock_time:?}");
    assert_eq!(locked.sync, SyncState::Locked);
    assert_eq!(locked.cal, CalPhase::Solved);
    assert!(
        hold.checks().len() >= CHECKS,
        "{} checks",
        hold.checks().len()
    );
    assert_residuals(&hold);
    assert_eq!(hold.noise_not_seen, 0, "{:?}", hold.failures);
    assert_eq!(end.failure, None);
    probed.unwrap();
}

enum Change {
    Tune(f64),
    Gain(f64),
}

impl Change {
    fn request(&self) -> ArrayTuneRequest {
        match *self {
            Self::Tune(center_hz) => ArrayTuneRequest {
                center_hz: Some(center_hz),
                ..ArrayTuneRequest::default()
            },
            Self::Gain(db) => ArrayTuneRequest {
                gain: Some(ArrayGain::Manual { db }),
                ..ArrayTuneRequest::default()
            },
        }
    }

    fn label(&self) -> String {
        match *self {
            Self::Tune(center_hz) => format!("tune {:.2} MHz", center_hz / 1e6),
            Self::Gain(db) => format!("gain {db} dB"),
        }
    }
}

fn resolve(engine: &Engine, change: &Change) -> (Duration, Duration, ArrayStatus) {
    let before = status(engine);
    let started = Instant::now();
    engine.tune_array(ARRAY, change.request()).unwrap();
    let mut stale = None;
    let solved = wait_status(engine, &change.label(), RESOLVE_LIMIT, |now| {
        if stale.is_none() && !now.phase_ready {
            stale = Some(started.elapsed());
        }
        stale.is_some() && calibrated(now) && now.last_solve_at != before.last_solve_at
    });
    (stale.unwrap_or_default(), started.elapsed(), solved)
}

#[test]
#[ignore = "requires an idle KrakenSDR; retunes and regains a calibrated Array"]
fn kraken_sync_after_burst_and_retune() {
    let kraken = Kraken::open();
    let engine = &kraken.engine;
    engine
        .apply_array(kraken.array(100e6, 20.0, noise_cal(0)))
        .unwrap();
    engine.apply_processor(df("df")).unwrap();
    wait_status(engine, "the first solve", LOCK_LIMIT, calibrated);
    let changes = [
        Change::Tune(UHF_HZ),
        Change::Tune(868e6),
        Change::Gain(40.0),
    ];
    let mut rows = Vec::new();
    let mut outcomes = Vec::new();
    for change in &changes {
        let (stale, solved_in, solved) = resolve(engine, change);
        let line = format!(
            "{}: stale after {:.0} ms, solved after {:.0} ms, center {:.2} MHz, gain {:?}",
            change.label(),
            stale.as_secs_f64() * 1e3,
            solved_in.as_secs_f64() * 1e3,
            solved.center_hz / 1e6,
            solved.gain_db
        );
        println!("{line}");
        rows.push(format!(
            "{},{:.0},{:.0},{},{:?}",
            change.label(),
            stale.as_secs_f64() * 1e3,
            solved_in.as_secs_f64() * 1e3,
            solved.center_hz,
            solved.gain_db
        ));
        outcomes.push((solved_in, solved));
    }
    write_csv(
        "kraken_sync_after_burst_and_retune",
        "change,stale_ms,solved_ms,center_hz,gain_db",
        &rows,
    );
    for (solved_in, solved) in &outcomes {
        assert!(*solved_in < RESOLVE_LIMIT, "{solved_in:?}");
        assert_eq!(solved.failure, None);
        assert_eq!(solved.sync, SyncState::Locked);
    }
    assert_eq!(outcomes[1].1.center_hz, 868e6);
    let gain = outcomes[2].1.gain_db.expect("the array reports its gain");
    assert!((gain - 40.0).abs() < 1.0, "{gain} dB");
    assert_eq!(
        processor_status(&status(engine), "df").error,
        None,
        "the direction finder runs"
    );
}

struct Transmitter {
    hz: f64,
    bearings: Vec<f64>,
    radius_m: f64,
    winding: Winding,
    gain_db: f64,
    bandwidth_hz: f64,
}

const NO_TRANSMITTER: &str = "kraken_df_known_bearing needs a transmitter at known bearings: \
    set SDRMM_DF_TX_HZ, SDRMM_DF_BEARINGS (for example 40,130,250) and SDRMM_DF_RADIUS_M, \
    then move the transmitter when asked";

fn transmitter() -> Transmitter {
    let hz = env_f64("SDRMM_DF_TX_HZ").expect(NO_TRANSMITTER);
    let bearings: Vec<f64> = std::env::var("SDRMM_DF_BEARINGS")
        .expect(NO_TRANSMITTER)
        .split(',')
        .map(|bearing| {
            bearing
                .trim()
                .parse()
                .unwrap_or_else(|_| panic!("a bearing in degrees, got {bearing}"))
        })
        .collect();
    let winding = match std::env::var("SDRMM_DF_WINDING").as_deref() {
        Ok("counter_clockwise") => Winding::CounterClockwise,
        _ => Winding::Clockwise,
    };
    Transmitter {
        hz,
        bearings,
        radius_m: env_f64("SDRMM_DF_RADIUS_M").unwrap_or(RADIUS_M),
        winding,
        gain_db: env_f64("SDRMM_DF_GAIN_DB").unwrap_or(GAIN_DB),
        bandwidth_hz: env_f64("SDRMM_DF_BANDWIDTH_HZ").unwrap_or(20e3),
    }
}

fn assert_survey_plan(bearings: &[f64]) {
    assert!(
        bearings.len() >= DF_MIN_BEARINGS,
        "survey at least {DF_MIN_BEARINGS} bearings"
    );
    for (index, a) in bearings.iter().enumerate() {
        let on_axis = wrap_deg(*a).abs().min(180.0 - wrap_deg(*a).abs());
        assert!(
            on_axis > DF_AXIS_GUARD_DEG,
            "{a} deg lies on the element-0 axis and cannot show a winding error"
        );
        for b in &bearings[index + 1..] {
            assert!(
                wrap_deg(a + b).abs() > DF_AXIS_GUARD_DEG,
                "{a} and {b} deg mirror each other about the element-0 axis"
            );
        }
    }
}

fn operator_moves_to(bearing: f64) {
    println!("Place the transmitter at {bearing} deg relative and press Enter");
    let mut line = String::new();
    let read = std::io::stdin()
        .read_line(&mut line)
        .expect("the terminal answers");
    assert!(read > 0, "{NO_TRANSMITTER}: no operator at the terminal");
}

fn df_reports(events: &mut broadcast::Receiver<ArrayEvent>, wanted: usize) -> Vec<DfReading> {
    let mut hold = Hold::default();
    let started = Instant::now();
    while hold.readings.len() < wanted {
        assert!(
            started.elapsed() < DF_WAIT,
            "{} DF reports",
            hold.readings.len()
        );
        hold.take(events);
        hold.readings
            .retain(|reading| !reading.squelched && !reading.peaks.is_empty());
        std::thread::sleep(POLL);
    }
    hold.readings.truncate(wanted);
    hold.readings
}

struct Report {
    bearing: f64,
    error: f64,
    sigma: f64,
    sources: u32,
}

fn reports(bearing: f64, readings: &[DfReading]) -> Vec<Report> {
    readings
        .iter()
        .map(|reading| {
            let peak = &reading.peaks[0];
            Report {
                bearing,
                error: wrap_deg(f64::from(peak.relative_deg) - bearing),
                sigma: f64::from(peak.sigma_deg),
                sources: reading.sources,
            }
        })
        .collect()
}

fn survey_rows(readings: &[DfReading], reports: &[Report]) -> Vec<String> {
    readings
        .iter()
        .zip(reports)
        .enumerate()
        .map(|(index, (reading, report))| {
            let peak = &reading.peaks[0];
            format!(
                "{},{index},{:.2},{:.2},{:.2},{},{:.3}",
                report.bearing,
                peak.relative_deg,
                report.error,
                report.sigma,
                report.sources,
                peak.confidence
            )
        })
        .collect()
}

fn assert_survey(tx: &Transmitter, rows: &[Report]) {
    for row in rows {
        assert!(
            row.error.abs() < DF_MIN_ERROR_DEG.max(2.0 * row.sigma),
            "{} deg: error {} deg with sigma {} deg",
            row.bearing,
            row.error,
            row.sigma
        );
    }
    let count = rows.len() as f64;
    let rms = (rows.iter().map(|row| row.error * row.error).sum::<f64>() / count).sqrt();
    let sigma = rows.iter().map(|row| row.sigma).sum::<f64>() / count;
    let single = rows.iter().filter(|row| row.sources == 1).count() as f64 / count;
    let line = format!(
        "{} reports at {:?} deg: RMS error {rms:.2} deg, mean sigma {sigma:.2} deg, ratio {:.2}, one source in {:.0}%",
        rows.len(),
        tx.bearings,
        rms / sigma,
        100.0 * single
    );
    println!("{line}");
    write_csv("kraken_df_known_bearing_summary", "summary", &[line]);
    assert!(
        (DF_SPREAD_LOW * sigma..=DF_SPREAD_HIGH * sigma).contains(&rms),
        "RMS {rms} deg against sigma {sigma} deg"
    );
    assert!(
        single >= DF_SINGLE_SHARE,
        "one source counted in {:.0}% of reports",
        100.0 * single
    );
}

#[test]
#[ignore = "requires an idle KrakenSDR with antennas and a transmitter moved to known bearings"]
fn kraken_df_known_bearing() {
    let tx = transmitter();
    assert_survey_plan(&tx.bearings);
    let kraken = Kraken::open();
    let engine = &kraken.engine;
    let mut events = engine.subscribe_arrays();
    engine
        .apply_array(kraken.array_on(
            uca(tx.radius_m, tx.winding),
            tx.hz - DF_OFFSET_HZ,
            tx.gain_db,
            noise_cal(ArrayCal::default().check_s),
        ))
        .unwrap();
    engine
        .apply_processor(ProcessorSpec {
            params: ProcessorParams::Df(DfParams {
                offset_hz: DF_OFFSET_HZ,
                bandwidth_hz: tx.bandwidth_hz,
                ..DfParams::default()
            }),
            ..df("df")
        })
        .unwrap();
    wait_status(engine, "the first solve", LOCK_LIMIT, calibrated);
    if let Err(error) = std::fs::remove_file(csv_path(DF_CSV)) {
        println!("no earlier survey to clear: {error}");
    }
    let mut rows = Vec::new();
    for bearing in &tx.bearings {
        operator_moves_to(*bearing);
        common::array::drain(&mut events);
        let readings = df_reports(&mut events, DF_REPORTS);
        let taken = reports(*bearing, &readings);
        append_csv(
            DF_CSV,
            "bearing_deg,report,relative_deg,error_deg,sigma_deg,sources,confidence",
            &survey_rows(&readings, &taken),
        );
        rows.extend(taken);
    }
    assert_survey(&tx, &rows);
}
