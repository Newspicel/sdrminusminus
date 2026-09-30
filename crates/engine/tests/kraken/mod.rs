#![allow(dead_code)]

use std::{
    io::Write as _,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use sdrmm_device::{DeviceDriver, DeviceRegistry};
use sdrmm_device_rtlsdr::KrakenDriver;
use sdrmm_engine::{ArraySpec, Engine, LaneRef};
use sdrmm_wire::{
    ArrayCal, ArrayGain, ArrayGeometry, ArrayNode, ArrayTune, DeviceInfo, DeviceSettings, Winding,
};

pub const ARRAY: &str = "array";
pub const RATE: f64 = 2_400_000.0;
pub const LANES: u32 = 5;
pub const RADIUS_M: f64 = 0.35;
const REGISTRY_PRIORITY: u8 = 25;
const FIND_WAIT: Duration = Duration::from_secs(20);
const FIND_POLL: Duration = Duration::from_millis(250);

pub struct Kraken {
    pub engine: Arc<Engine>,
    pub ds: u32,
    info: DeviceInfo,
}

fn find() -> DeviceInfo {
    let started = Instant::now();
    loop {
        if let Some(info) = KrakenDriver::new().probe().into_iter().find(|info| {
            info.profile
                .as_ref()
                .is_some_and(|profile| profile.rx_streams == LANES)
                && !info.label.contains("missing")
        }) {
            return info;
        }
        assert!(
            started.elapsed() < FIND_WAIT,
            "no complete KrakenSDR is attached"
        );
        std::thread::sleep(FIND_POLL);
    }
}

impl Kraken {
    pub fn open() -> Self {
        let info = find();
        let mut registry = DeviceRegistry::new();
        registry.register(REGISTRY_PRIORITY, Box::new(KrakenDriver::new()));
        let engine = Engine::with_registry(registry, None);
        let ds = engine
            .create_device_set(&info.id())
            .unwrap_or_else(|error| panic!("{} opens: {error}", info.label));
        engine
            .patch_device(
                ds,
                DeviceSettings {
                    sample_rate: Some(RATE),
                    ..DeviceSettings::default()
                },
            )
            .unwrap_or_else(|error| panic!("{} runs at {RATE}: {error}", info.label));
        Self { engine, ds, info }
    }

    pub fn lanes(&self) -> Vec<Option<LaneRef>> {
        (0..LANES)
            .map(|stream| {
                Some(LaneRef {
                    device_set: self.ds,
                    stream,
                })
            })
            .collect()
    }

    pub fn array(&self, center_hz: f64, gain_db: f64, cal: ArrayCal) -> ArraySpec {
        self.array_on(uca(RADIUS_M, Winding::Clockwise), center_hz, gain_db, cal)
    }

    pub fn array_on(
        &self,
        geometry: ArrayGeometry,
        center_hz: f64,
        gain_db: f64,
        cal: ArrayCal,
    ) -> ArraySpec {
        ArraySpec {
            node: ARRAY.to_owned(),
            lanes: self.lanes(),
            settings: ArrayNode {
                geometry,
                cal,
                ..ArrayNode::default()
            },
            tune: Some(ArrayTune {
                center_hz,
                gain: ArrayGain::Manual { db: gain_db },
            }),
            warm: None,
        }
    }
}

impl Drop for Kraken {
    fn drop(&mut self) {
        self.engine.shutdown();
        match KrakenDriver::new().open(&self.info) {
            Ok(device) => drop(device),
            Err(error) => eprintln!(
                "{} did not reopen to switch its noise source off: {error}",
                self.info.label
            ),
        }
    }
}

pub fn uca(radius_m: f64, winding: Winding) -> ArrayGeometry {
    ArrayGeometry::Uca {
        radius_m,
        first_deg: 0.0,
        winding,
    }
}

pub fn wrap_deg(degrees: f64) -> f64 {
    (degrees + 180.0).rem_euclid(360.0) - 180.0
}

fn hardware_dir() -> PathBuf {
    let binary = std::env::current_exe().expect("the test binary");
    binary
        .ancestors()
        .nth(3)
        .expect("the target directory")
        .join("hardware")
}

pub fn csv_path(name: &str) -> PathBuf {
    hardware_dir().join(format!("{name}.csv"))
}

pub fn write_csv(name: &str, header: &str, rows: &[String]) {
    let path = csv_path(name);
    std::fs::create_dir_all(hardware_dir()).expect("target/hardware");
    let mut text = format!("{header}\n");
    for row in rows {
        text.push_str(row);
        text.push('\n');
    }
    std::fs::write(&path, text).expect("the CSV is written");
    println!("wrote {}", path.display());
}

pub fn append_csv(name: &str, header: &str, rows: &[String]) {
    let path = csv_path(name);
    std::fs::create_dir_all(hardware_dir()).expect("target/hardware");
    let fresh = !path.exists();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .expect("the CSV opens");
    if fresh {
        writeln!(file, "{header}").expect("the CSV header is written");
    }
    for row in rows {
        writeln!(file, "{row}").expect("a CSV row is written");
    }
    println!("appended to {}", path.display());
}

pub fn env_f64(name: &str) -> Option<f64> {
    let text = std::env::var(name).ok()?;
    Some(
        text.trim()
            .parse()
            .unwrap_or_else(|_| panic!("{name} must be a number, got {text}")),
    )
}
