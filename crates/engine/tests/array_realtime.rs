#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::{
    collections::BTreeMap,
    fmt::Write as _,
    io::Write as _,
    sync::Arc,
    time::{Duration, Instant},
};

use common::array::{
    Bench, FULL_RATE, KRAKEN, LOCK_WAIT, WAIT, df, kraken_array, next_reading, processor,
    processor_status, wait_calibrated, wait_status,
};
use sdrmm_engine::Engine;
use sdrmm_wire::{
    ArrayStatus, ProcessorParams, ProcessorReading,
    processor::{beamformer::BeamformerParams, spatial::SpatialSpectrumParams},
    radar::{GpuUse, PassiveRadarParams, RadarHealth},
};

const RUN: Duration = Duration::from_secs(30);
const AGGREGATOR_BUDGET: f64 = 0.5;
const THREAD_BUDGET: f64 = 1.0;
const SCALE_VAR: &str = "SDRMM_RT_SCALE";
const SUMMARY_VAR: &str = "GITHUB_STEP_SUMMARY";
const SUMMARY_MIN_SHARE: f64 = 0.001;
const APPLE_SILICON_SCALE: f64 = 5.0;
const PROCESSORS: [&str; 4] = ["df", "beam", "spatial", "radar"];

#[derive(Clone, Debug)]
struct ThreadTime {
    id: u64,
    name: String,
    cpu_ns: u64,
}

#[cfg(target_os = "macos")]
fn thread_times() -> Vec<ThreadTime> {
    const PROC_PIDLISTTHREADS: libc::c_int = 6;
    const MAX_THREADS: usize = 4_096;
    let pid = libc::c_int::try_from(std::process::id()).unwrap();
    let mut handles = vec![0u64; MAX_THREADS];
    let room = libc::c_int::try_from(handles.len() * size_of::<u64>()).unwrap();
    let filled = unsafe {
        libc::proc_pidinfo(
            pid,
            PROC_PIDLISTTHREADS,
            0,
            handles.as_mut_ptr().cast(),
            room,
        )
    };
    handles.truncate(usize::try_from(filled).unwrap_or(0) / size_of::<u64>());
    let size = libc::c_int::try_from(size_of::<libc::proc_threadinfo>()).unwrap();
    handles
        .into_iter()
        .filter_map(|handle| {
            let mut info: libc::proc_threadinfo = unsafe { std::mem::zeroed() };
            let got = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDTHREADINFO,
                    handle,
                    (&raw mut info).cast(),
                    size,
                )
            };
            (got == size).then(|| ThreadTime {
                id: handle,
                name: info
                    .pth_name
                    .iter()
                    .take_while(|byte| **byte != 0)
                    .map(|byte| char::from(*byte as u8))
                    .collect(),
                cpu_ns: info.pth_user_time + info.pth_system_time,
            })
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn thread_times() -> Vec<ThreadTime> {
    const USER_HZ: u64 = 100;
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else {
        return Vec::new();
    };
    tasks
        .flatten()
        .filter_map(|task| {
            let id: u64 = task.file_name().to_str()?.parse().ok()?;
            let path = task.path();
            let name = std::fs::read_to_string(path.join("comm")).ok()?;
            let scheduled = std::fs::read_to_string(path.join("schedstat"))
                .ok()
                .and_then(|text| text.split_whitespace().next()?.parse::<u64>().ok());
            let cpu_ns = match scheduled {
                Some(ns) => ns,
                None => {
                    let stat = std::fs::read_to_string(path.join("stat")).ok()?;
                    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
                    let ticks: u64 = fields.get(11)?.parse::<u64>().ok()?
                        + fields.get(12)?.parse::<u64>().ok()?;
                    ticks * 1_000_000_000 / USER_HZ
                }
            };
            Some(ThreadTime {
                id,
                name: name.trim().to_owned(),
                cpu_ns,
            })
        })
        .collect()
}

fn busy_between(before: &[ThreadTime], after: &[ThreadTime], wall: Duration) -> Vec<(String, f64)> {
    let earlier: BTreeMap<u64, &ThreadTime> = before.iter().map(|time| (time.id, time)).collect();
    let mut busy: Vec<(String, f64)> = after
        .iter()
        .filter_map(|time| {
            let start = earlier.get(&time.id)?;
            let used = time.cpu_ns.saturating_sub(start.cpu_ns) as f64;
            Some((time.name.clone(), used / wall.as_nanos() as f64))
        })
        .collect();
    busy.sort_by(|a, b| b.1.total_cmp(&a.1));
    busy
}

fn busy_table(busy: &[(String, f64)], scale: f64) -> String {
    let mut table = format!(
        "### Array realtime\n\n| Thread | Busy | Pi 5 at {SCALE_VAR}={scale} |\n|---|---:|---:|\n"
    );
    for (name, share) in busy.iter().filter(|(_, share)| *share >= SUMMARY_MIN_SHARE) {
        writeln!(
            table,
            "| `{name}` | {:.2} % | {:.1} % |",
            share * 100.0,
            share * scale * 100.0
        )
        .unwrap();
    }
    table
}

fn publish_summary(table: &str) {
    let Some(path) = std::env::var_os(SUMMARY_VAR).map(std::path::PathBuf::from) else {
        return;
    };
    let mut summary = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap_or_else(|error| panic!("open {}: {error}", path.display()));
    summary.write_all(table.as_bytes()).unwrap();
}

fn spin(name: &str, busy: Duration) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let started = Instant::now();
            let mut x = 1u64;
            while started.elapsed() < busy {
                x = std::hint::black_box(x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1));
            }
        })
        .unwrap()
}

fn measuring_works() {
    let name = "sdrmm-rt-check";
    let before = thread_times();
    let started = Instant::now();
    let worker = spin(name, Duration::from_millis(300));
    std::thread::sleep(Duration::from_millis(100));
    let during = thread_times();
    worker.join().unwrap();
    let wall = started.elapsed();
    let seen = during
        .iter()
        .find(|time| time.name == name)
        .expect("the probe thread is listed by name");
    let used = seen.cpu_ns as f64 / 1e9;
    assert!(
        (0.02..0.4).contains(&used),
        "the probe thread used {used} s of CPU in {wall:?}"
    );
    assert!(!before.is_empty());
}

fn scale() -> f64 {
    match std::env::var(SCALE_VAR) {
        Ok(value) => value
            .parse()
            .unwrap_or_else(|_| panic!("{SCALE_VAR} must be a number, got {value}")),
        Err(_) if cfg!(all(target_os = "macos", target_arch = "aarch64")) => APPLE_SILICON_SCALE,
        Err(_) => {
            eprintln!("{SCALE_VAR} unset, busy fractions are compared unscaled");
            1.0
        }
    }
}

fn is_aggregator(name: &str) -> bool {
    name.strip_prefix("sdrmm-array-")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit()))
}

fn radar_health(engine: &Engine) -> RadarHealth {
    let mut events = engine.subscribe_arrays();
    let reading = next_reading(&mut events, "radar", WAIT);
    match reading.as_ref() {
        ProcessorReading::PassiveRadar(update) => update.health.clone(),
        other => panic!("a radar reading: {other:?}"),
    }
}

fn start_workload(bench: &Bench) -> Arc<Engine> {
    let ds = bench.open_at(KRAKEN, FULL_RATE);
    let engine = bench.engine.clone();
    engine.apply_array(kraken_array(ds)).unwrap();
    engine.apply_processor(df("df")).unwrap();
    let mut beam = processor(
        "beam",
        ProcessorParams::Beamformer(BeamformerParams::default()),
    );
    beam.lane_ports = vec!["beam".to_owned()];
    engine.apply_processor(beam).unwrap();
    engine
        .apply_processor(processor(
            "spatial",
            ProcessorParams::SpatialSpectrum(SpatialSpectrumParams::default()),
        ))
        .unwrap();
    engine
        .apply_processor(processor(
            "radar",
            ProcessorParams::PassiveRadar(PassiveRadarParams {
                gpu: GpuUse::Off,
                ..PassiveRadarParams::default()
            }),
        ))
        .unwrap();
    engine
}

fn assert_no_drops(start: &ArrayStatus, end: &ArrayStatus) {
    assert_eq!(end.failure, None);
    assert_eq!(end.realigns, start.realigns, "the array realigned");
    assert_eq!(end.dropped_samples, start.dropped_samples);
    assert_eq!(end.events_lost, start.events_lost);
    for (was, now) in start.lanes.iter().zip(&end.lanes) {
        assert_eq!(
            (now.gaps, now.gap_samples, now.uncertain),
            (was.gaps, was.gap_samples, was.uncertain),
            "lane {} lost samples",
            now.lane
        );
    }
    for node in PROCESSORS {
        let (was, now) = (processor_status(start, node), processor_status(end, node));
        assert!(now.running && now.gated.is_none(), "{now:?}");
        assert_eq!(
            (
                now.dropped_samples,
                now.dropped_reports,
                now.lane_overflows,
                now.truncated
            ),
            (
                was.dropped_samples,
                was.dropped_reports,
                was.lane_overflows,
                was.truncated
            ),
            "{node} dropped work"
        );
    }
}

fn assert_radar_kept_up(start: &RadarHealth, end: &RadarHealth) {
    assert_eq!(end.dropped_samples, start.dropped_samples, "{end:?}");
    assert_eq!(end.dropped_cpis, start.dropped_cpis, "{end:?}");
    assert_eq!(end.dropped_reports, start.dropped_reports, "{end:?}");
}

#[cfg_attr(
    debug_assertions,
    ignore = "a budget only means something in a release build"
)]
#[test]
fn array_realtime_budget() {
    measuring_works();
    let scale = scale();
    let bench = Bench::new();
    let engine = start_workload(&bench);
    wait_calibrated(&engine, LOCK_WAIT);
    let settled = wait_status(&engine, "every processor to run", WAIT, |now| {
        PROCESSORS.iter().all(|node| {
            let status = processor_status(now, node);
            status.running && status.gated.is_none()
        })
    });
    std::thread::sleep(Duration::from_secs(2));
    let radar_start = radar_health(&engine);
    let start = common::array::status(&engine);
    let before = thread_times();
    let started = Instant::now();
    std::thread::sleep(RUN);
    let after = thread_times();
    let wall = started.elapsed();
    let end = common::array::status(&engine);
    let radar_end = radar_health(&engine);
    let busy = busy_between(&before, &after, wall);
    for (name, share) in &busy {
        eprintln!(
            "{name:<24} {:>6.2} %  Pi 5 {:>6.1} %",
            share * 100.0,
            share * scale * 100.0
        );
    }
    publish_summary(&busy_table(&busy, scale));
    assert!(settled.phase_ready);
    assert_no_drops(&start, &end);
    assert_radar_kept_up(&radar_start, &radar_end);
    let aggregator = busy
        .iter()
        .find(|(name, _)| is_aggregator(name))
        .map(|(_, share)| *share)
        .expect("the aggregator thread is listed");
    assert!(
        aggregator * scale < AGGREGATOR_BUDGET,
        "the aggregator needs {:.1} % of a Pi 5 core",
        aggregator * scale * 100.0
    );
    for (name, share) in busy
        .iter()
        .filter(|(name, _)| name.starts_with("sdrmm-") && !name.starts_with("sdrmm-bench-"))
    {
        assert!(
            share * scale < THREAD_BUDGET,
            "{name} needs {:.1} % of a Pi 5 core",
            share * scale * 100.0
        );
    }
}

#[test]
fn busy_table_lists_busy_threads_scaled_to_a_pi() {
    let busy = [
        ("sdrmm-array-7".to_owned(), 0.1),
        ("sdrmm-df".to_owned(), 0.025),
        ("idle".to_owned(), 0.000_5),
    ];
    let table = busy_table(&busy, 2.0);
    assert!(
        table.contains("| Thread | Busy | Pi 5 at SDRMM_RT_SCALE=2 |"),
        "{table}"
    );
    assert!(
        table.contains("| `sdrmm-array-7` | 10.00 % | 20.0 % |"),
        "{table}"
    );
    assert!(table.contains("| `sdrmm-df` | 2.50 % | 5.0 % |"), "{table}");
    assert!(!table.contains("idle"), "{table}");
}
