use std::{
    sync::{Arc, Mutex, atomic::Ordering},
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;
use num_complex::Complex;
use sdrmm_channels::testgen;
use sdrmm_device::{
    DeviceDriver, DeviceError, RxSink, SdrDevice, Worker, check_stream_settings, lock,
    single_rx_sink,
};
use sdrmm_wire::{
    Capabilities, Coherence, DcArtifact, DeviceInfo, DeviceSettings, Duplex, ExtraSetting,
    ExtraValue, Range, SIGGEN_DRIVER_ID, StreamScope,
};

pub mod signals;

pub use signals::{DEFAULT_SIGNAL, SIGNALS, Signal};

pub const SIGNAL_SETTING: &str = "signal";
pub const OFFSET_SETTING: &str = "offset_hz";
pub const LEVEL_SETTING: &str = "level_db";
pub const NOISE_SETTING: &str = "noise_db";
pub const GAP_SETTING: &str = "gap_s";

pub const MAX_GENERATORS: usize = 64;
pub const NOISE_OFF_DB: f64 = -140.0;

const DRIVER_ID: &str = SIGGEN_DRIVER_ID;
const BLOCK_SECS: f64 = 0.025;
const DEFAULT_CENTER_HZ: f64 = 145_000_000.0;
const DEFAULT_LEVEL_DB: f64 = -12.0;
const DEFAULT_NOISE_DB: f64 = -60.0;
const MAX_OFFSET_HZ: f64 = 10_000_000.0;
const MAX_GAP_S: f64 = 10.0;
const GAP_STEP_S: f64 = 0.1;

#[derive(Default)]
pub struct SigGenDriver {
    adopted: Mutex<Vec<String>>,
}

impl SigGenDriver {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn info(key: &str) -> DeviceInfo {
        DeviceInfo {
            driver: DRIVER_ID.to_string(),
            key: key.to_string(),
            label: "Signal generator".to_string(),
            serial: None,
            profile: Some(capabilities().profile()),
        }
    }

    fn valid(key: &str) -> bool {
        !key.is_empty()
            && key.len() <= 64
            && key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    }
}

impl DeviceDriver for SigGenDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        lock(&self.adopted)
            .iter()
            .map(|key| Self::info(key))
            .collect()
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        if !Self::valid(&info.key) {
            return Err(DeviceError::NotFound(format!("{DRIVER_ID}:{}", info.key)));
        }
        Ok(Box::new(SignalGen::new()))
    }

    fn resolve(&self, key: &str) -> Option<DeviceInfo> {
        if !Self::valid(key) {
            return None;
        }
        let mut adopted = lock(&self.adopted);
        if !adopted.iter().any(|held| held == key) {
            if adopted.len() >= MAX_GENERATORS {
                return None;
            }
            adopted.push(key.to_string());
        }
        Some(Self::info(key))
    }
}

#[must_use]
pub fn capabilities() -> Capabilities {
    Capabilities {
        freq_ranges: vec![Range {
            min: 0.0,
            max: 6_000_000_000.0,
            step: None,
        }],
        sample_rates: signals::rates(),
        sample_rate_ranges: Vec::new(),
        gains: Vec::new(),
        antennas: Vec::new(),
        bandwidths: Vec::new(),
        bandwidth_ranges: Vec::new(),
        bandwidth_auto: false,
        bias_tee: false,
        agc: sdrmm_wire::Agc::None,
        extra: vec![
            ExtraSetting::choice(SIGNAL_SETTING, "Signal", signals::options(), DEFAULT_SIGNAL),
            ExtraSetting::range(
                OFFSET_SETTING,
                "Offset",
                Range {
                    min: -MAX_OFFSET_HZ,
                    max: MAX_OFFSET_HZ,
                    step: None,
                },
                "Hz",
            ),
            ExtraSetting::range(
                LEVEL_SETTING,
                "Level",
                Range {
                    min: -60.0,
                    max: 0.0,
                    step: Some(1.0),
                },
                "dB",
            ),
            ExtraSetting::range(
                NOISE_SETTING,
                "Noise",
                Range {
                    min: NOISE_OFF_DB,
                    max: 0.0,
                    step: Some(1.0),
                },
                "dB",
            ),
            ExtraSetting::range(
                GAP_SETTING,
                "Gap",
                Range {
                    min: 0.0,
                    max: MAX_GAP_S,
                    step: Some(GAP_STEP_S),
                },
                "s",
            ),
        ],
        ppm: false,
        duplex: Duplex::RxOnly,
        rx_streams: 1,
        tx_streams: 0,
        per_stream: StreamScope::default(),
        directional: None,
        dc_artifact: DcArtifact::None,
        hardware_sweep: false,
        coherence: Coherence::None,
        noise_source: false,
    }
}

#[derive(Clone, Copy)]
struct Params {
    signal: &'static Signal,
    rate_hz: f64,
    offset_hz: f64,
    level_db: f64,
    noise_db: f64,
    gap_s: f64,
}

impl Params {
    fn default_for(signal: &'static Signal) -> Self {
        Self {
            signal,
            rate_hz: signal.rate_hz,
            offset_hz: 0.0,
            level_db: DEFAULT_LEVEL_DB,
            noise_db: DEFAULT_NOISE_DB,
            gap_s: 0.5,
        }
    }

    fn renders_the_same_as(&self, other: &Self) -> bool {
        self.signal.id == other.signal.id
            && self.rate_hz.to_bits() == other.rate_hz.to_bits()
            && self.offset_hz.to_bits() == other.offset_hz.to_bits()
            && self.level_db.to_bits() == other.level_db.to_bits()
            && self.noise_db.to_bits() == other.noise_db.to_bits()
    }
}

pub struct SignalGen {
    capabilities: Capabilities,
    settings: DeviceSettings,
    params: Params,
    shared: Arc<ArcSwap<Params>>,
    worker: Worker,
}

impl Default for SignalGen {
    fn default() -> Self {
        Self::new()
    }
}

fn default_signal() -> &'static Signal {
    signals::find(DEFAULT_SIGNAL).unwrap_or(&SIGNALS[0])
}

impl SignalGen {
    #[must_use]
    pub fn new() -> Self {
        let params = Params::default_for(default_signal());
        Self {
            capabilities: capabilities(),
            settings: settings_of(&params),
            params,
            shared: Arc::new(ArcSwap::from_pointee(params)),
            worker: Worker::new(),
        }
    }
}

fn settings_of(params: &Params) -> DeviceSettings {
    DeviceSettings {
        center_hz: Some(DEFAULT_CENTER_HZ),
        sample_rate: Some(params.rate_hz),
        extra: vec![
            ExtraValue {
                name: SIGNAL_SETTING.to_string(),
                value: serde_json::Value::String(params.signal.id.to_string()),
            },
            ExtraValue {
                name: OFFSET_SETTING.to_string(),
                value: number(params.offset_hz),
            },
            ExtraValue {
                name: LEVEL_SETTING.to_string(),
                value: number(params.level_db),
            },
            ExtraValue {
                name: NOISE_SETTING.to_string(),
                value: number(params.noise_db),
            },
            ExtraValue {
                name: GAP_SETTING.to_string(),
                value: number(params.gap_s),
            },
        ],
        ..DeviceSettings::default()
    }
}

fn number(value: f64) -> serde_json::Value {
    serde_json::Number::from_f64(value).map_or(serde_json::Value::Null, serde_json::Value::Number)
}

fn ranged(name: &str, value: &serde_json::Value, min: f64, max: f64) -> Result<f64, DeviceError> {
    let number = value
        .as_f64()
        .ok_or_else(|| DeviceError::Unsupported(format!("`{name}` must be a number")))?;
    if !number.is_finite() || number < min || number > max {
        return Err(DeviceError::Unsupported(format!(
            "`{name}` {number} is outside {min}..={max}"
        )));
    }
    Ok(number)
}

impl SdrDevice for SignalGen {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        check_stream_settings(settings, &self.capabilities)?;
        let mut next = self.params;
        let mut picked_signal = false;
        for extra in &settings.extra {
            match extra.name.as_str() {
                SIGNAL_SETTING => {
                    let id = extra.value.as_str().ok_or_else(|| {
                        DeviceError::Unsupported(format!("`{SIGNAL_SETTING}` must be a name"))
                    })?;
                    next.signal = signals::find(id).ok_or_else(|| {
                        DeviceError::Unsupported(format!("no signal called `{id}`"))
                    })?;
                    picked_signal = true;
                }
                OFFSET_SETTING => {
                    next.offset_hz =
                        ranged(OFFSET_SETTING, &extra.value, -MAX_OFFSET_HZ, MAX_OFFSET_HZ)?;
                }
                LEVEL_SETTING => next.level_db = ranged(LEVEL_SETTING, &extra.value, -60.0, 0.0)?,
                NOISE_SETTING => {
                    next.noise_db = ranged(NOISE_SETTING, &extra.value, NOISE_OFF_DB, 0.0)?;
                }
                GAP_SETTING => next.gap_s = ranged(GAP_SETTING, &extra.value, 0.0, MAX_GAP_S)?,
                other => return Err(DeviceError::Unsupported(format!("extra `{other}`"))),
            }
        }
        if let Some(rate) = settings.sample_rate {
            if !self.capabilities.sample_rates.contains(&rate) {
                return Err(DeviceError::Unsupported(format!("sample_rate {rate}")));
            }
            next.rate_hz = rate;
        } else if picked_signal {
            next.rate_hz = next.signal.rate_hz;
        }
        if let Some(hz) = settings.center_hz
            && !self
                .capabilities
                .freq_ranges
                .iter()
                .any(|range| range.min <= hz && hz <= range.max)
        {
            return Err(DeviceError::Unsupported(format!(
                "center_hz {hz} outside tuner range"
            )));
        }
        self.params = next;
        let center_hz = settings.center_hz.or(self.settings.center_hz);
        self.settings = DeviceSettings {
            center_hz,
            ..settings_of(&next)
        };
        self.shared.store(Arc::new(next));
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let mut sink = single_rx_sink(sinks)?;
        let shared = self.shared.clone();
        self.worker.start("sdrmm-siggen-rx", move |running| {
            let mut rendered: Option<(Params, Vec<Complex<f32>>)> = None;
            let mut at = 0usize;
            let mut next_wake = Instant::now();
            while running.load(Ordering::Acquire) {
                let params = *shared.load_full();
                let stale = rendered
                    .as_ref()
                    .is_none_or(|(built, _)| !built.renders_the_same_as(&params));
                if stale {
                    rendered = Some((params, render(&params)));
                    at = 0;
                    next_wake = Instant::now();
                }
                let Some((_, block)) = rendered.as_ref() else {
                    return;
                };
                let step = ((params.rate_hz * BLOCK_SECS).round() as usize).max(1);
                if block.is_empty() {
                    std::thread::sleep(Duration::from_secs_f64(BLOCK_SECS));
                    continue;
                }
                let end = (at + step).min(block.len());
                sink.push(&block[at..end]);
                let sent = end - at;
                at = end;
                let mut idle = 0.0;
                if at >= block.len() {
                    at = 0;
                    idle = params.gap_s;
                }
                next_wake += Duration::from_secs_f64(sent as f64 / params.rate_hz + idle);
                let now = Instant::now();
                if next_wake > now {
                    std::thread::sleep(next_wake - now);
                } else {
                    next_wake = now;
                }
            }
        })
    }

    fn rx_stop(&mut self) {
        self.worker.stop();
    }
}

fn render(params: &Params) -> Vec<Complex<f32>> {
    let mut iq = (params.signal.render)();
    if iq.is_empty() {
        return iq;
    }
    if params.rate_hz != params.signal.rate_hz {
        iq = testgen::resample(&iq, params.signal.rate_hz, params.rate_hz);
    }
    if params.offset_hz != 0.0 {
        testgen::shift(&mut iq, params.offset_hz, params.rate_hz);
    }
    testgen::scale(&mut iq, amplitude(params.level_db));
    if params.noise_db > NOISE_OFF_DB {
        testgen::add_noise(&mut iq, 0x5DEE_CE66, amplitude(params.noise_db));
    }
    iq
}

fn amplitude(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use super::*;

    fn extra(name: &str, value: serde_json::Value) -> DeviceSettings {
        DeviceSettings {
            extra: vec![ExtraValue {
                name: name.to_string(),
                value,
            }],
            ..DeviceSettings::default()
        }
    }

    fn signal(id: &str) -> DeviceSettings {
        extra(SIGNAL_SETTING, serde_json::Value::String(id.to_string()))
    }

    fn power(iq: &[Complex<f32>]) -> f64 {
        if iq.is_empty() {
            return 0.0;
        }
        iq.iter().map(|s| f64::from(s.norm_sqr())).sum::<f64>() / iq.len() as f64
    }

    #[test]
    fn every_signal_renders_something_at_its_own_rate() {
        for signal in SIGNALS {
            let params = Params::default_for(signal);
            let iq = render(&params);
            assert!(
                iq.len() >= 64,
                "{} rendered {} samples",
                signal.id,
                iq.len()
            );
            assert!(power(&iq) > 1e-9, "{} rendered silence", signal.id);
        }
    }

    #[test]
    fn signal_ids_and_labels_are_unique() {
        let mut ids: Vec<&str> = SIGNALS.iter().map(|signal| signal.id).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total, "signal ids must be unique");

        let mut labels: Vec<&str> = SIGNALS.iter().map(|signal| signal.label).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), total, "signal labels must be unique");
    }

    #[test]
    fn the_default_signal_is_one_of_the_catalog() {
        assert!(signals::find(DEFAULT_SIGNAL).is_some());
        assert!(
            capabilities()
                .sample_rates
                .contains(&default_signal().rate_hz)
        );
    }

    #[test]
    fn picking_a_signal_snaps_the_rate_to_the_one_it_is_generated_at() {
        let mut device = SignalGen::new();
        device.apply(&signal("adsb")).expect("adsb is a signal");
        assert_eq!(
            device.settings().sample_rate,
            signals::find("adsb").map(|signal| signal.rate_hz)
        );
    }

    #[test]
    fn a_rate_the_operator_asked_for_outlives_the_next_signal_change() {
        let mut device = SignalGen::new();
        device
            .apply(&DeviceSettings {
                sample_rate: Some(2_000_000.0),
                ..signal("morse")
            })
            .expect("a catalog rate with a signal");
        assert_eq!(device.settings().sample_rate, Some(2_000_000.0));
    }

    #[test]
    fn tuning_is_kept_when_only_a_signal_changes() {
        let mut device = SignalGen::new();
        device
            .apply(&DeviceSettings {
                center_hz: Some(433_920_000.0),
                ..DeviceSettings::default()
            })
            .expect("in range");
        device.apply(&signal("subghz")).expect("subghz is a signal");
        assert_eq!(device.settings().center_hz, Some(433_920_000.0));
    }

    #[test]
    fn refuses_settings_no_generator_can_answer() {
        let mut device = SignalGen::new();
        for bad in [
            signal("no-such-signal"),
            extra(SIGNAL_SETTING, serde_json::json!(7)),
            extra(OFFSET_SETTING, serde_json::json!(1e12)),
            extra(LEVEL_SETTING, serde_json::json!(6.0)),
            extra(NOISE_SETTING, serde_json::json!("loud")),
            extra(GAP_SETTING, serde_json::json!(-1.0)),
            DeviceSettings {
                sample_rate: Some(3_333.0),
                ..DeviceSettings::default()
            },
            DeviceSettings {
                center_hz: Some(9_000_000_000.0),
                ..DeviceSettings::default()
            },
        ] {
            assert!(
                matches!(device.apply(&bad), Err(DeviceError::Unsupported(_))),
                "must refuse {bad:?}"
            );
        }
    }

    #[test]
    fn level_and_noise_move_the_power_the_way_they_are_named() {
        let quiet = Params {
            level_db: -40.0,
            noise_db: NOISE_OFF_DB,
            ..Params::default_for(default_signal())
        };
        let loud = Params {
            level_db: 0.0,
            ..quiet
        };
        let noisy = Params {
            noise_db: -10.0,
            ..quiet
        };
        assert!(power(&render(&loud)) > power(&render(&quiet)) * 100.0);
        assert!(power(&render(&noisy)) > power(&render(&quiet)) * 10.0);
    }

    #[test]
    fn a_running_generator_hands_samples_over_in_real_time() {
        let mut device = SignalGen::new();
        device.apply(&signal("tone")).expect("tone is a signal");
        let (tx, rx) = mpsc::channel();
        device
            .rx_start(vec![RxSink::new(move |samples, _| {
                let _ = tx.send(samples.to_vec());
            })])
            .expect("starts");
        let block = rx.recv_timeout(Duration::from_secs(2)).expect("a block");
        device.rx_stop();
        assert!(!block.is_empty());
        assert!(power(&block) > 1e-9);
    }

    #[test]
    fn a_generator_is_adopted_once_and_then_probed() {
        let driver = SigGenDriver::new();
        assert!(driver.probe().is_empty());
        assert_eq!(
            driver.resolve("signal_gen-a1b2").map(|info| info.id()),
            Some("siggen:signal_gen-a1b2".to_string())
        );
        assert!(driver.resolve("signal_gen-a1b2").is_some());
        assert_eq!(driver.probe().len(), 1);
        assert!(driver.open(&SigGenDriver::info("signal_gen-a1b2")).is_ok());
    }

    #[test]
    fn a_key_that_is_not_a_node_name_is_refused() {
        let driver = SigGenDriver::new();
        for key in ["", "has space", "has:colon", "../escape", &"x".repeat(65)] {
            assert!(driver.resolve(key).is_none(), "{key} must be refused");
            assert!(driver.open(&SigGenDriver::info(key)).is_err());
        }
    }
}
