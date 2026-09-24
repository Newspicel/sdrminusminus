use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{
    Agc, AgcSetting, ArgumentOption, BandwidthSetting, Capabilities, DcArtifact, DeviceInfo,
    DeviceSettings, Duplex, ExtraSetting, ExtraValue, GainKind, GainStage, GainValue, Range,
    StreamScope,
};

use crate::{
    DEFAULT_CENTER_HZ, DRIVER_ID,
    driver::{BoardVariant, DIRECT_SAMPLING_MAX_HZ, DeviceDescriptor, DirectSampling},
};

const TUNER_MIN_HZ: f64 = 24e6;
const TUNER_MAX_HZ: f64 = 1_766e6;
const V4_HF_MIN_HZ: f64 = 500e3;
const V4_HF_MAX_HZ: f64 = 28.8e6;

const DIRECT_RANGE: Range = Range {
    min: 0.0,
    max: DIRECT_SAMPLING_MAX_HZ as f64,
    step: None,
};

const RATE_WINDOWS: [(f64, f64); 2] = [(225_001.0, 300_000.0), (900_001.0, 3_200_000.0)];

const RATE_MENU: [f64; 9] = [
    250_000.0,
    1_024_000.0,
    1_536_000.0,
    1_920_000.0,
    2_048_000.0,
    2_400_000.0,
    2_560_000.0,
    2_880_000.0,
    3_200_000.0,
];

const PPM_MAX: f64 = 200.0;

const BANDWIDTH_MIN_HZ: f64 = 290e3;
const BANDWIDTH_MAX_HZ: f64 = 8e6;

pub(crate) const DIRECT_SAMPLING: &str = "direct_sampling";

#[derive(Debug, Default, PartialEq)]
pub(crate) struct Plan {
    pub(crate) sample_rate: Option<u32>,
    pub(crate) ppm: Option<i32>,
    pub(crate) center_hz: Option<u32>,
    pub(crate) bandwidth: Option<u32>,
    pub(crate) direct_sampling: Option<DirectSampling>,
    pub(crate) gain: Option<GainMode>,
    pub(crate) bias_tee: Option<bool>,
    pub(crate) applied: DeviceSettings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GainMode {
    Auto,
    Manual(i32),
}

pub(crate) fn device_infos(descriptors: &[DeviceDescriptor]) -> Vec<DeviceInfo> {
    descriptors
        .iter()
        .map(|d| {
            let serial = d.serial.as_ref().filter(|s| {
                descriptors
                    .iter()
                    .filter(|other| other.serial.as_ref() == Some(*s))
                    .count()
                    == 1
            });
            let location = format!("{}/{}", d.bus, d.address);
            let model = match (&d.manufacturer, &d.product) {
                (Some(manufacturer), Some(product)) => format!("{manufacturer} {product}"),
                (Some(name), None) | (None, Some(name)) => name.clone(),
                (None, None) => "RTL-SDR".to_string(),
            };
            let profile = Some(capabilities(d.board_variant, &[]).profile());
            match serial {
                Some(serial) => DeviceInfo {
                    driver: DRIVER_ID.to_string(),
                    key: serial.clone(),
                    label: format!("{model} {serial}"),
                    serial: Some(serial.clone()),
                    profile,
                },
                None => DeviceInfo {
                    driver: DRIVER_ID.to_string(),
                    key: location.clone(),
                    label: format!("{model} ({location})"),
                    serial: None,
                    profile,
                },
            }
        })
        .collect()
}

fn tuner_stage(gains: &[i32]) -> Option<GainStage> {
    let min = gains.iter().copied().min()?;
    let max = gains.iter().copied().max()?;
    let range = Range {
        min: tenths_to_db(min),
        max: tenths_to_db(max),
        step: None,
    };
    Some(
        GainStage::new(GainKind::Tuner, range)
            .with_values(gains.iter().copied().map(tenths_to_db).collect()),
    )
}

pub(crate) fn capabilities(board: BoardVariant, gains: &[i32]) -> Capabilities {
    let mut freq_ranges = Vec::with_capacity(2);
    if board.upconverts_hf() {
        freq_ranges.push(Range {
            min: V4_HF_MIN_HZ,
            max: V4_HF_MAX_HZ,
            step: None,
        });
    } else {
        freq_ranges.push(DIRECT_RANGE);
    }
    freq_ranges.push(Range {
        min: TUNER_MIN_HZ,
        max: TUNER_MAX_HZ,
        step: None,
    });

    Capabilities {
        freq_ranges,
        sample_rates: RATE_MENU.to_vec(),
        sample_rate_ranges: RATE_WINDOWS
            .iter()
            .map(|(min, max)| Range {
                min: *min,
                max: *max,
                step: None,
            })
            .collect(),
        gains: tuner_stage(gains).into_iter().collect(),
        antennas: vec!["RX".to_string()],
        bandwidths: Vec::new(),
        bandwidth_ranges: vec![Range {
            min: BANDWIDTH_MIN_HZ,
            max: BANDWIDTH_MAX_HZ,
            step: None,
        }],
        bandwidth_auto: true,
        bias_tee: true,
        agc: Agc::Switch,
        extra: extra_settings(board),
        ppm: true,
        duplex: Duplex::RxOnly,
        rx_streams: 1,
        tx_streams: 0,
        per_stream: StreamScope::default(),
        directional: None,
        dc_artifact: DcArtifact::Managed,
        hardware_sweep: false,
        coherence: sdrmm_wire::Coherence::None,
        noise_source: false,
    }
}

/// What a bank of dongles in one case, on one clock, can be asked to do.
///
/// The lanes are tuned together because an array measured at two frequencies is not one
/// measurement, and the tuner is never bypassed because elements are wired to antennas rather
/// than to a direct-sampling injection point. Gain stays per lane. The noise source is not a
/// setting: it belongs to the calibration that switches it, not to an operator.
pub(crate) fn kraken_lane_capabilities(gains: &[i32]) -> Capabilities {
    Capabilities {
        extra: Vec::new(),
        ..capabilities(BoardVariant::Generic, gains)
    }
}

pub(crate) fn kraken_capabilities(lanes: u32, gains: &[i32]) -> Capabilities {
    Capabilities {
        freq_ranges: vec![Range {
            min: TUNER_MIN_HZ,
            max: TUNER_MAX_HZ,
            step: None,
        }],
        extra: Vec::new(),
        noise_source: true,
        rx_streams: lanes,
        per_stream: StreamScope {
            tuning: true,
            gain: true,
            antenna: false,
        },
        coherence: sdrmm_wire::Coherence::TimeSync,
        ..capabilities(BoardVariant::Generic, gains)
    }
}

fn extra_settings(board: BoardVariant) -> Vec<ExtraSetting> {
    if board.upconverts_hf() {
        return Vec::new();
    }
    vec![ExtraSetting::choice(
        DIRECT_SAMPLING,
        "Direct sampling",
        DirectSampling::all()
            .iter()
            .map(|mode| ArgumentOption::plain(mode.as_str()))
            .collect(),
        DirectSampling::Off.as_str(),
    )]
}

fn reachable_ranges(caps: &Capabilities, mode: DirectSampling) -> Vec<Range> {
    match mode {
        DirectSampling::Off => caps
            .freq_ranges
            .iter()
            .copied()
            .filter(|range| *range != DIRECT_RANGE)
            .collect(),
        _ => vec![DIRECT_RANGE],
    }
}

fn reaches(ranges: &[Range], hz: f64) -> bool {
    ranges.iter().any(|r| r.min <= hz && hz <= r.max)
}

fn clamp_to_ranges(ranges: &[Range], hz: f64) -> f64 {
    ranges
        .iter()
        .map(|range| hz.clamp(range.min, range.max))
        .min_by(|a, b| (a - hz).abs().total_cmp(&(b - hz).abs()))
        .unwrap_or(hz)
}

fn direct_sampling_of(settings: &DeviceSettings) -> DirectSampling {
    settings
        .extra
        .iter()
        .find(|value| value.name == DIRECT_SAMPLING)
        .and_then(|value| value.value.as_str())
        .and_then(DirectSampling::parse)
        .unwrap_or_default()
}

fn tenths_to_db(tenths: i32) -> f64 {
    f64::from(tenths) / 10.0
}

pub(crate) fn nearest_gain(table: &[i32], tenths: i32) -> Option<i32> {
    table
        .iter()
        .copied()
        .min_by_key(|g| ((i64::from(*g) - i64::from(tenths)).abs(), i64::from(*g)))
}

struct Requested {
    agc: Option<bool>,
    gain_tenths: Option<i32>,
}

fn plan_extras(
    delta: &DeviceSettings,
    caps: &Capabilities,
    plan: &mut Plan,
) -> Result<Option<DirectSampling>, DeviceError> {
    let mut mode = None;
    for value in &delta.extra {
        let setting = caps
            .extra
            .iter()
            .find(|s| s.name() == value.name)
            .ok_or_else(|| DeviceError::Unsupported(format!("extra setting {}", value.name)))?;
        match value.name.as_str() {
            DIRECT_SAMPLING => {
                let requested = extra_direct_sampling(setting, value)?;
                mode = Some(requested);
                plan.applied.extra.push(ExtraValue {
                    name: DIRECT_SAMPLING.to_string(),
                    value: requested.as_str().into(),
                });
            }
            other => return Err(DeviceError::Unsupported(format!("extra setting {other}"))),
        }
    }
    Ok(mode)
}

fn plan_switches(
    delta: &DeviceSettings,
    caps: &Capabilities,
    plan: &mut Plan,
) -> Result<Option<bool>, DeviceError> {
    if let Some(on) = delta.bias_tee {
        if !caps.bias_tee {
            return Err(DeviceError::Unsupported("bias_tee".to_string()));
        }
        plan.bias_tee = Some(on);
        plan.applied.bias_tee = Some(on);
    }
    let Some(agc) = &delta.agc else {
        return Ok(None);
    };
    if !caps.agc.admits(agc) {
        return Err(DeviceError::Unsupported(format!(
            "agc: the tuner's AGC is a switch, mode {:?} does not exist",
            agc.mode
        )));
    }
    Ok(Some(agc.on))
}

fn plan_center(
    delta: &DeviceSettings,
    caps: &Capabilities,
    current: &DeviceSettings,
    mode: DirectSampling,
    plan: &mut Plan,
) -> Result<(), DeviceError> {
    let ranges = reachable_ranges(caps, mode);
    if let Some(f) = delta.center_hz {
        if !reaches(&ranges, f) {
            return Err(DeviceError::Unsupported(unreachable_center(caps, mode, f)));
        }
        plan.center_hz = Some(f.round() as u32);
    } else if plan.direct_sampling.is_some() {
        let hz = current
            .center_hz
            .filter(|hz| hz.is_finite())
            .unwrap_or(f64::from(DEFAULT_CENTER_HZ));
        plan.center_hz = Some(clamp_to_ranges(&ranges, hz).round() as u32);
    }
    Ok(())
}

fn plan_sample_rate(delta: &DeviceSettings, plan: &mut Plan) -> Result<(), DeviceError> {
    let Some(rate) = delta.sample_rate else {
        return Ok(());
    };
    if !RATE_WINDOWS
        .iter()
        .any(|(lo, hi)| *lo <= rate && rate <= *hi)
    {
        return Err(DeviceError::Unsupported(format!(
            "sample_rate {rate} outside the RTL2832U windows \
             225001-300000 and 900001-3200000 Hz"
        )));
    }
    plan.sample_rate = Some(rate.round() as u32);
    Ok(())
}

fn plan_bandwidth(
    delta: &DeviceSettings,
    current: &DeviceSettings,
    mode: DirectSampling,
    plan: &mut Plan,
) -> Result<(), DeviceError> {
    let bandwidth = delta
        .bandwidth
        .or_else(|| {
            plan.sample_rate
                .is_some()
                .then_some(current.bandwidth)
                .flatten()
        })
        .or_else(|| {
            (plan.direct_sampling == Some(DirectSampling::Off))
                .then(|| current.bandwidth.unwrap_or(BandwidthSetting::Auto))
        });
    let Some(bandwidth) = bandwidth else {
        return Ok(());
    };
    let hz = match bandwidth {
        BandwidthSetting::Auto => 0,
        BandwidthSetting::Manual { hz } => {
            if !(BANDWIDTH_MIN_HZ..=BANDWIDTH_MAX_HZ).contains(&hz) {
                return Err(DeviceError::Unsupported(format!(
                    "bandwidth {hz} outside {BANDWIDTH_MIN_HZ}..{BANDWIDTH_MAX_HZ} Hz"
                )));
            }
            hz.round() as u32
        }
    };
    if mode == DirectSampling::Off {
        plan.bandwidth = Some(hz);
    }
    plan.applied.bandwidth = Some(bandwidth);
    Ok(())
}

fn plan_antenna_and_ppm(
    delta: &DeviceSettings,
    caps: &Capabilities,
    plan: &mut Plan,
) -> Result<(), DeviceError> {
    if let Some(antenna) = &delta.antenna {
        if !caps.antennas.contains(antenna) {
            return Err(DeviceError::Unsupported(format!("antenna {antenna}")));
        }
        plan.applied.antenna = Some(antenna.clone());
    }
    if let Some(ppm) = delta.ppm {
        if !ppm.is_finite() || !(-PPM_MAX..=PPM_MAX).contains(&ppm) {
            return Err(DeviceError::Unsupported(format!(
                "ppm {ppm} outside ±{PPM_MAX}"
            )));
        }
        plan.ppm = Some(ppm.round() as i32);
    }
    Ok(())
}

fn requested_gain(
    delta: &DeviceSettings,
    caps: &Capabilities,
    table: &[i32],
) -> Result<Option<i32>, DeviceError> {
    let mut requested = None;
    for gain in &delta.gains {
        let stage = caps
            .stage(&gain.stage)
            .ok_or_else(|| DeviceError::Unsupported(format!("gain stage {}", gain.stage)))?;
        if !(stage.range.min..=stage.range.max).contains(&gain.value_db) {
            return Err(DeviceError::Unsupported(format!(
                "gain {} {} dB outside {}..{} dB",
                gain.stage, gain.value_db, stage.range.min, stage.range.max
            )));
        }
        requested = nearest_gain(table, (gain.value_db * 10.0).round() as i32);
    }
    Ok(requested)
}

fn plan_gain(
    asked: &Requested,
    current: &DeviceSettings,
    table: &[i32],
    mode: DirectSampling,
    plan: &mut Plan,
) -> Result<(), DeviceError> {
    let gain = match (asked.agc, asked.gain_tenths) {
        (Some(true), _) => Some(GainMode::Auto),
        (_, Some(tenths)) => Some(GainMode::Manual(tenths)),
        (Some(false), None) => Some(GainMode::Manual(restored_manual(current, table)?)),
        (None, None) if plan.direct_sampling == Some(DirectSampling::Off) => {
            Some(match current.agc.as_ref().map(|agc| agc.on) {
                Some(false) => GainMode::Manual(restored_manual(current, table)?),
                _ => GainMode::Auto,
            })
        }
        (None, None) => None,
    };
    if mode == DirectSampling::Off {
        plan.gain = gain;
    }
    match gain {
        Some(GainMode::Auto) => plan.applied.agc = Some(AgcSetting::switched(true)),
        Some(GainMode::Manual(tenths)) => {
            plan.applied.agc = Some(AgcSetting::switched(false));
            plan.applied
                .gains
                .push(GainValue::new(GainKind::Tuner, tenths_to_db(tenths)));
        }
        None => {}
    }
    Ok(())
}

pub(crate) fn validate(
    delta: &DeviceSettings,
    caps: &Capabilities,
    current: &DeviceSettings,
    table: &[i32],
) -> Result<Plan, DeviceError> {
    check_stream_settings(delta, caps)?;
    let mut plan = Plan::default();

    let requested_mode = plan_extras(delta, caps, &mut plan)?;
    let agc = plan_switches(delta, caps, &mut plan)?;
    let current_mode = direct_sampling_of(current);
    let mode = requested_mode.unwrap_or(current_mode);
    if mode != current_mode {
        plan.direct_sampling = Some(mode);
    }

    plan_center(delta, caps, current, mode, &mut plan)?;
    plan_sample_rate(delta, &mut plan)?;
    plan_bandwidth(delta, current, mode, &mut plan)?;
    plan_antenna_and_ppm(delta, caps, &mut plan)?;

    let asked = Requested {
        agc,
        gain_tenths: requested_gain(delta, caps, table)?,
    };
    plan_gain(&asked, current, table, mode, &mut plan)?;

    Ok(plan)
}

fn unreachable_center(caps: &Capabilities, mode: DirectSampling, hz: f64) -> String {
    let offers_direct = caps
        .extra
        .iter()
        .any(|setting| setting.name() == DIRECT_SAMPLING);
    match mode {
        DirectSampling::Off if offers_direct && reaches(&[DIRECT_RANGE], hz) => format!(
            "center_hz {hz} outside tuner range; set {DIRECT_SAMPLING} to i or q to reach it \
             with the tuner bypassed"
        ),
        DirectSampling::Off => format!("center_hz {hz} outside tuner range"),
        _ => format!(
            "center_hz {hz} outside the direct-sampling range {}-{} Hz; set {DIRECT_SAMPLING} \
             to off for the tuner's own range",
            DIRECT_RANGE.min, DIRECT_RANGE.max
        ),
    }
}

fn restored_manual(current: &DeviceSettings, table: &[i32]) -> Result<i32, DeviceError> {
    current_manual_tenths(current)
        .and_then(|tenths| nearest_gain(table, tenths))
        .or_else(|| table.iter().copied().max())
        .ok_or_else(|| DeviceError::Unsupported("tuner exposes no gain table".to_string()))
}

fn extra_direct_sampling(
    setting: &ExtraSetting,
    value: &ExtraValue,
) -> Result<DirectSampling, DeviceError> {
    let ExtraSetting::Enum { options, .. } = setting else {
        return Err(DeviceError::Unsupported(format!(
            "extra setting {}: not an enum",
            value.name
        )));
    };
    value
        .value
        .as_str()
        .filter(|text| options.iter().any(|option| option.value == *text))
        .and_then(DirectSampling::parse)
        .ok_or_else(|| {
            DeviceError::Unsupported(format!(
                "extra setting {}: bad value {}",
                value.name, value.value
            ))
        })
}

pub(crate) fn current_manual_tenths(current: &DeviceSettings) -> Option<i32> {
    current
        .gain(GainKind::Tuner.name())
        .filter(|db| db.is_finite())
        .map(|db| (db * 10.0).round() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::GAIN_VALUES;

    fn descriptor(address: u8, serial: Option<&str>) -> DeviceDescriptor {
        DeviceDescriptor {
            index: usize::from(address),
            bus: "001".to_string(),
            address,
            manufacturer: Some("Realtek".to_string()),
            product: Some("RTL2838UHIDIR".to_string()),
            serial: serial.map(str::to_string),
            port_chain: vec![address],
            board_variant: BoardVariant::Generic,
        }
    }

    #[test]
    fn every_lane_tunes_alone_and_the_bank_carries_the_switches_of_the_whole_unit() {
        let caps = kraken_capabilities(5, GAIN_VALUES);
        assert_eq!(caps.rx_streams, 5);
        assert_eq!(caps.tx_streams, 0);
        assert_eq!(caps.coherence, sdrmm_wire::Coherence::TimeSync);
        assert!(caps.per_stream.tuning, "every lane has its own synthesizer");
        assert!(caps.per_stream.gain);
        assert!(caps.noise_source, "the bank calibrates against its own");
        assert!(caps.extra.is_empty(), "the bank has no oddities of its own");
        assert!(caps.bias_tee);
        assert_eq!(caps.agc, Agc::Switch);
        assert!(
            caps.freq_ranges
                .iter()
                .all(|range| range.min >= TUNER_MIN_HZ),
            "elements are wired to antennas, so the tuner is never bypassed"
        );
    }

    #[test]
    fn unique_serial_becomes_the_key() {
        let infos = device_infos(&[descriptor(4, Some("00000123"))]);
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].driver, "rtlsdr");
        assert_eq!(infos[0].key, "00000123");
        assert_eq!(infos[0].serial.as_deref(), Some("00000123"));
        assert_eq!(infos[0].label, "Realtek RTL2838UHIDIR 00000123");
        assert_eq!(infos[0].id(), "rtlsdr:00000123");
    }

    #[test]
    fn repeated_factory_serial_falls_back_to_bus_address() {
        let infos = device_infos(&[
            descriptor(4, Some("00000001")),
            descriptor(7, Some("00000001")),
        ]);
        assert_eq!(infos.len(), 2);
        for info in &infos {
            assert_eq!(info.serial, None);
        }
        assert_eq!(infos[0].key, "001/4");
        assert_eq!(infos[1].key, "001/7");
        assert_eq!(infos[1].label, "Realtek RTL2838UHIDIR (001/7)");
    }

    #[test]
    fn serialless_device_is_keyed_by_location() {
        let mut d = descriptor(9, None);
        d.manufacturer = None;
        d.product = None;
        let infos = device_infos(&[d]);
        assert_eq!(infos[0].key, "001/9");
        assert_eq!(infos[0].serial, None);
        assert_eq!(infos[0].label, "RTL-SDR (001/9)");
    }

    #[test]
    fn capabilities_expose_the_tuner_envelope() {
        let caps = capabilities(BoardVariant::Generic, GAIN_VALUES);
        assert_eq!(
            caps.freq_ranges,
            vec![
                DIRECT_RANGE,
                Range {
                    min: 24e6,
                    max: 1.766e9,
                    step: None
                }
            ]
        );
        assert_eq!(caps.sample_rates, RATE_MENU.to_vec());
        assert_eq!(caps.sample_rate_ranges.len(), 2);
        assert_eq!(caps.antennas, vec!["RX".to_string()]);
        assert!(caps.bandwidths.is_empty(), "the IF filter is not a menu");
        assert_eq!(
            caps.bandwidth_ranges,
            vec![Range {
                min: BANDWIDTH_MIN_HZ,
                max: BANDWIDTH_MAX_HZ,
                step: None
            }]
        );
        assert!(
            caps.bandwidth_auto,
            "the filter tracks the sample rate by itself"
        );
        assert!(caps.bias_tee);
        assert_eq!(caps.agc, Agc::Switch);
        assert_eq!(caps.duplex, Duplex::RxOnly);
        assert!(caps.ppm);
        assert_eq!(caps.rx_streams, 1);
        assert_eq!(caps.gains.len(), 1);
        assert_eq!(caps.gains[0].name, "TUNER");
        assert_eq!(caps.gains[0].kind, GainKind::Tuner);
        assert_eq!(caps.gains[0].range.min, 0.0);
        assert_eq!(caps.gains[0].range.max, 49.6);
        assert_eq!(caps.gains[0].range.step, None);
    }

    #[test]
    fn blog_v4_swaps_direct_sampling_for_its_upconverted_hf_range() {
        let caps = capabilities(BoardVariant::RtlSdrBlogV4, GAIN_VALUES);
        assert_eq!(caps.freq_ranges.len(), 2);
        assert_eq!(caps.freq_ranges[0].min, 500e3);
        assert_eq!(caps.freq_ranges[0].max, 28.8e6);
        assert_eq!(caps.freq_ranges[1].min, 24e6);
        assert!(caps.extra.is_empty(), "a V4 never bypasses its tuner");
    }

    #[test]
    fn the_v4_lite_advertises_the_same_upconverted_hf_range_as_the_v4() {
        assert_eq!(
            capabilities(BoardVariant::RtlSdrBlogV4Lite, GAIN_VALUES),
            capabilities(BoardVariant::RtlSdrBlogV4, GAIN_VALUES)
        );
    }

    #[test]
    fn a_generic_dongle_advertises_the_direct_sampling_zone_and_the_switch_for_it() {
        let caps = capabilities(BoardVariant::Generic, GAIN_VALUES);
        assert_eq!(caps.freq_ranges[0], DIRECT_RANGE);
        assert_eq!(DIRECT_RANGE.max, 14.4e6, "half the 28.8 MHz crystal");
        assert!(caps.profile().reaches(7.1e6));
        assert_eq!(
            caps.extra.last(),
            Some(&ExtraSetting::choice(
                DIRECT_SAMPLING,
                "Direct sampling",
                ["off", "i", "q"].map(ArgumentOption::plain).to_vec(),
                "off",
            ))
        );
    }

    #[test]
    fn gain_stage_follows_the_reported_table() {
        let caps = capabilities(BoardVariant::Generic, &[0, 87, 213]);
        assert_eq!(caps.gains[0].range.max, 21.3);
        assert!(capabilities(BoardVariant::Generic, &[]).gains.is_empty());
    }

    #[test]
    fn extras_are_only_what_the_driver_can_drive() {
        let extra = capabilities(BoardVariant::Generic, GAIN_VALUES).extra;
        let names: Vec<&str> = extra.iter().map(ExtraSetting::name).collect();
        assert_eq!(names, vec![DIRECT_SAMPLING]);
    }

    #[test]
    fn nearest_gain_snaps_clamps_and_breaks_ties_downward() {
        let table = [0, 100, 200];
        assert_eq!(nearest_gain(&table, -50), Some(0));
        assert_eq!(nearest_gain(&table, 400), Some(200));
        assert_eq!(nearest_gain(&table, 100), Some(100));
        assert_eq!(nearest_gain(&table, 130), Some(100));
        assert_eq!(nearest_gain(&table, 170), Some(200));
        assert_eq!(nearest_gain(&table, 150), Some(100));
        assert_eq!(nearest_gain(&[], 100), None);
    }

    #[test]
    fn the_advertised_table_snaps_the_same_way_the_driver_does() {
        let caps = caps();
        let stage = caps.gains.first().expect("a tuner stage");
        assert_eq!(stage.values.len(), GAIN_VALUES.len());
        assert!(!stage.is_switch(), "29 settings is not a switch");
        for tenths in (-100..=600).step_by(7) {
            let driver = nearest_gain(GAIN_VALUES, tenths).expect("a non-empty table");
            let advertised = stage.snap(f64::from(tenths) / 10.0);
            assert!(
                (advertised - tenths_to_db(driver)).abs() < f64::EPSILON,
                "{tenths} tenths: the client would show {advertised} dB where the driver \
                 programs {} dB",
                tenths_to_db(driver)
            );
        }
    }

    #[test]
    fn every_offered_rate_sits_inside_a_window_the_resampler_holds() {
        let caps = caps();
        assert_eq!(
            caps.sample_rate_ranges.len(),
            2,
            "the RTL2832U has two windows"
        );
        for rate in &caps.sample_rates {
            assert!(
                sdrmm_wire::any_range_holds(&caps.sample_rate_ranges, *rate),
                "{rate} is offered but falls in the aliasing gap"
            );
        }
        assert!(
            !sdrmm_wire::any_range_holds(&caps.sample_rate_ranges, 500_000.0),
            "500 kHz aliases and must not be advertised as reachable"
        );
    }

    fn caps() -> Capabilities {
        capabilities(BoardVariant::Generic, GAIN_VALUES)
    }

    fn plan_for(delta: &DeviceSettings) -> Result<Plan, DeviceError> {
        validate(delta, &caps(), &DeviceSettings::default(), GAIN_VALUES)
    }

    const fn manual(hz: f64) -> BandwidthSetting {
        BandwidthSetting::Manual { hz }
    }

    fn tuner(value_db: f64) -> GainValue {
        GainValue::new(GainKind::Tuner, value_db)
    }

    #[test]
    fn validate_rejects_center_outside_every_range() {
        for bad in [1e6, 2e9, f64::NAN] {
            let delta = DeviceSettings {
                center_hz: Some(bad),
                ..DeviceSettings::default()
            };
            assert!(
                matches!(plan_for(&delta), Err(DeviceError::Unsupported(_))),
                "center {bad} must be rejected"
            );
        }
        let ok = DeviceSettings {
            center_hz: Some(100e6),
            ..DeviceSettings::default()
        };
        assert_eq!(plan_for(&ok).unwrap().center_hz, Some(100_000_000));
    }

    #[test]
    fn validate_accepts_hf_only_on_a_blog_v4() {
        let delta = DeviceSettings {
            center_hz: Some(7_100_000.0),
            ..DeviceSettings::default()
        };
        assert!(matches!(plan_for(&delta), Err(DeviceError::Unsupported(_))));
        let v4 = capabilities(BoardVariant::RtlSdrBlogV4, GAIN_VALUES);
        assert!(validate(&delta, &v4, &DeviceSettings::default(), GAIN_VALUES).is_ok());
    }

    #[test]
    fn validate_rejects_rates_outside_the_hardware_windows() {
        for bad in [200_000.0, 500_000.0, 4_000_000.0, f64::NAN] {
            let delta = DeviceSettings {
                sample_rate: Some(bad),
                ..DeviceSettings::default()
            };
            assert!(
                matches!(plan_for(&delta), Err(DeviceError::Unsupported(_))),
                "rate {bad} must be rejected"
            );
        }
    }

    #[test]
    fn validate_accepts_any_rate_inside_a_window() {
        for good in [250_000.0, 1_800_000.0, 2_048_000.0, 3_200_000.0] {
            let delta = DeviceSettings {
                sample_rate: Some(good),
                ..DeviceSettings::default()
            };
            assert_eq!(
                plan_for(&delta).unwrap().sample_rate,
                Some(good as u32),
                "rate {good} must be accepted"
            );
        }
    }

    #[test]
    fn validate_bounds_bandwidth_and_maps_auto_to_the_tracking_filter() {
        let auto = DeviceSettings {
            bandwidth: Some(BandwidthSetting::Auto),
            ..DeviceSettings::default()
        };
        let plan = plan_for(&auto).unwrap();
        assert_eq!(plan.bandwidth, Some(0));
        assert_eq!(plan.applied.bandwidth, Some(BandwidthSetting::Auto));

        let narrow = DeviceSettings {
            bandwidth: Some(manual(300_000.0)),
            ..DeviceSettings::default()
        };
        let plan = plan_for(&narrow).unwrap();
        assert_eq!(plan.bandwidth, Some(300_000));
        assert_eq!(plan.applied.bandwidth, Some(manual(300_000.0)));

        for bad in [-1.0, 0.0, 100e3, 9e6, f64::NAN] {
            let delta = DeviceSettings {
                bandwidth: Some(manual(bad)),
                ..DeviceSettings::default()
            };
            assert!(
                matches!(plan_for(&delta), Err(DeviceError::Unsupported(_))),
                "bandwidth {bad} must be rejected"
            );
        }
    }

    #[test]
    fn rate_change_re_applies_the_recorded_bandwidth() {
        let current = DeviceSettings {
            bandwidth: Some(manual(1_500_000.0)),
            ..DeviceSettings::default()
        };
        let delta = DeviceSettings {
            sample_rate: Some(2_400_000.0),
            ..DeviceSettings::default()
        };
        let plan = validate(&delta, &caps(), &current, GAIN_VALUES).unwrap();
        assert_eq!(plan.bandwidth, Some(1_500_000));

        let plan = validate(
            &DeviceSettings {
                center_hz: Some(100e6),
                ..DeviceSettings::default()
            },
            &caps(),
            &current,
            GAIN_VALUES,
        )
        .unwrap();
        assert_eq!(plan.bandwidth, None);
    }

    #[test]
    fn validate_rejects_unknown_gain_stage_and_out_of_range_values() {
        let unknown = DeviceSettings {
            gains: vec![GainValue {
                stage: "LNA".to_string(),
                value_db: 10.0,
            }],
            ..DeviceSettings::default()
        };
        assert!(matches!(
            plan_for(&unknown),
            Err(DeviceError::Unsupported(_))
        ));

        for bad in [-1.0, 60.0, f64::NAN] {
            let delta = DeviceSettings {
                gains: vec![tuner(bad)],
                ..DeviceSettings::default()
            };
            assert!(
                matches!(plan_for(&delta), Err(DeviceError::Unsupported(_))),
                "gain {bad} must be rejected"
            );
        }
    }

    #[test]
    fn manual_gain_snaps_to_the_table_and_reports_what_landed() {
        let delta = DeviceSettings {
            gains: vec![tuner(30.0)],
            ..DeviceSettings::default()
        };
        let plan = plan_for(&delta).unwrap();
        assert_eq!(plan.gain, Some(GainMode::Manual(297)));
        assert_eq!(plan.applied.gains[0].value_db, 29.7);
        assert_eq!(plan.applied.agc, Some(AgcSetting::switched(false)));
        assert!(plan.applied.extra.is_empty());
    }

    #[test]
    fn agc_on_wins_over_a_manual_value_in_the_same_delta() {
        let delta = DeviceSettings {
            gains: vec![tuner(30.0)],
            agc: Some(AgcSetting::switched(true)),
            ..DeviceSettings::default()
        };
        let plan = plan_for(&delta).unwrap();
        assert_eq!(plan.gain, Some(GainMode::Auto));
        assert!(plan.applied.gains.is_empty());
        assert_eq!(plan.applied.agc, Some(AgcSetting::switched(true)));
    }

    #[test]
    fn leaving_agc_restores_the_last_manual_gain() {
        let delta = DeviceSettings {
            agc: Some(AgcSetting::switched(false)),
            ..DeviceSettings::default()
        };
        let current = DeviceSettings {
            gains: vec![tuner(20.7)],
            ..DeviceSettings::default()
        };
        let plan = validate(&delta, &caps(), &current, GAIN_VALUES).unwrap();
        assert_eq!(plan.gain, Some(GainMode::Manual(207)));

        let plan = plan_for(&delta).unwrap();
        assert_eq!(plan.gain, Some(GainMode::Manual(496)));
    }

    #[test]
    fn bias_tee_is_planned_and_echoed() {
        let delta = DeviceSettings {
            bias_tee: Some(true),
            ..DeviceSettings::default()
        };
        let plan = plan_for(&delta).unwrap();
        assert_eq!(plan.bias_tee, Some(true));
        assert_eq!(plan.applied.bias_tee, Some(true));
        assert!(plan.applied.extra.is_empty());
    }

    #[test]
    fn an_agc_mode_is_refused_on_a_plain_switch() {
        let delta = DeviceSettings {
            agc: Some(AgcSetting::in_mode(true, "fast")),
            ..DeviceSettings::default()
        };
        match plan_for(&delta) {
            Err(DeviceError::Unsupported(message)) => assert!(message.contains("agc"), "{message}"),
            other => panic!("a mode on a switch must be refused, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_unknown_and_mistyped_extras() {
        for value in [
            ExtraValue {
                name: "direct_samp".to_string(),
                value: "1".into(),
            },
            ExtraValue {
                name: "bias_tee".to_string(),
                value: true.into(),
            },
            ExtraValue {
                name: "agc".to_string(),
                value: true.into(),
            },
        ] {
            let delta = DeviceSettings {
                extra: vec![value.clone()],
                ..DeviceSettings::default()
            };
            assert!(
                matches!(plan_for(&delta), Err(DeviceError::Unsupported(_))),
                "extra {value:?} must be rejected"
            );
        }
    }

    #[test]
    fn the_advertised_ppm_range_fits_the_correction_registers() {
        assert!(PPM_MAX <= f64::from(crate::driver::MAX_PPM));
    }

    #[test]
    fn ppm_is_rounded_to_the_registers_granularity() {
        for (requested, expected) in [
            (0.0, 0),
            (1.5, 2),
            (-1.5, -2),
            (12.4, 12),
            (-200.0, -200),
            (200.0, 200),
        ] {
            let delta = DeviceSettings {
                ppm: Some(requested),
                ..DeviceSettings::default()
            };
            assert_eq!(plan_for(&delta).unwrap().ppm, Some(expected), "{requested}");
        }
    }

    #[test]
    fn validate_rejects_ppm_outside_the_advertised_range() {
        for requested in [200.001, -200.001, 1e9, f64::NAN, f64::INFINITY] {
            let delta = DeviceSettings {
                ppm: Some(requested),
                ..DeviceSettings::default()
            };
            assert!(
                matches!(plan_for(&delta), Err(DeviceError::Unsupported(_))),
                "ppm {requested} must be rejected"
            );
        }
    }

    #[test]
    fn validate_rejects_unknown_antennas() {
        let antenna = DeviceSettings {
            antenna: Some("TX/RX".to_string()),
            ..DeviceSettings::default()
        };
        assert!(matches!(
            plan_for(&antenna),
            Err(DeviceError::Unsupported(_))
        ));

        let ok = DeviceSettings {
            antenna: Some("RX".to_string()),
            ..DeviceSettings::default()
        };
        assert_eq!(
            plan_for(&ok).unwrap().applied.antenna.as_deref(),
            Some("RX")
        );
    }

    #[test]
    fn a_full_delta_plans_every_write() {
        let delta = DeviceSettings {
            center_hz: Some(433_920_000.0),
            sample_rate: Some(2_048_000.0),
            bandwidth: Some(manual(1_000_000.0)),
            antenna: Some("RX".to_string()),
            gains: vec![tuner(49.6)],
            bias_tee: Some(true),
            ..DeviceSettings::default()
        };
        let plan = plan_for(&delta).unwrap();
        assert_eq!(plan.center_hz, Some(433_920_000));
        assert_eq!(plan.sample_rate, Some(2_048_000));
        assert_eq!(plan.bandwidth, Some(1_000_000));
        assert_eq!(plan.gain, Some(GainMode::Manual(496)));
        assert_eq!(plan.bias_tee, Some(true));
    }

    fn mode_value(mode: DirectSampling) -> ExtraValue {
        ExtraValue {
            name: DIRECT_SAMPLING.to_string(),
            value: mode.as_str().into(),
        }
    }

    fn on_hf() -> DeviceSettings {
        DeviceSettings {
            center_hz: Some(7_100_000.0),
            sample_rate: Some(1_024_000.0),
            bandwidth: Some(manual(300_000.0)),
            gains: vec![tuner(20.7)],
            agc: Some(AgcSetting::switched(false)),
            extra: vec![mode_value(DirectSampling::QBranch)],
            ..DeviceSettings::default()
        }
    }

    #[test]
    fn direct_sampling_swaps_which_ranges_are_reachable() {
        let to_hf = DeviceSettings {
            extra: vec![mode_value(DirectSampling::QBranch)],
            center_hz: Some(7_100_000.0),
            ..DeviceSettings::default()
        };
        let plan = plan_for(&to_hf).unwrap();
        assert_eq!(plan.direct_sampling, Some(DirectSampling::QBranch));
        assert_eq!(plan.center_hz, Some(7_100_000));
        assert_eq!(
            plan.applied.extra,
            vec![mode_value(DirectSampling::QBranch)]
        );

        let too_high = DeviceSettings {
            center_hz: Some(145_500_000.0),
            ..DeviceSettings::default()
        };
        match validate(&too_high, &caps(), &on_hf(), GAIN_VALUES) {
            Err(DeviceError::Unsupported(message)) => {
                assert!(message.contains("direct-sampling range"), "{message}");
                assert!(message.contains("set direct_sampling to off"), "{message}");
            }
            other => panic!("a VHF centre while direct sampling must be refused, got {other:?}"),
        }
    }

    #[test]
    fn an_hf_centre_with_the_tuner_in_circuit_points_at_the_setting() {
        let delta = DeviceSettings {
            center_hz: Some(7_100_000.0),
            ..DeviceSettings::default()
        };
        match plan_for(&delta) {
            Err(DeviceError::Unsupported(message)) => {
                assert!(
                    message.contains("set direct_sampling to i or q"),
                    "{message}"
                );
            }
            other => panic!("HF without direct sampling must be refused, got {other:?}"),
        }

        let v4 = capabilities(BoardVariant::RtlSdrBlogV4, GAIN_VALUES);
        let too_low = DeviceSettings {
            center_hz: Some(200_000.0),
            ..DeviceSettings::default()
        };
        match validate(&too_low, &v4, &DeviceSettings::default(), GAIN_VALUES) {
            Err(DeviceError::Unsupported(message)) => {
                assert_eq!(message, "center_hz 200000 outside tuner range");
            }
            other => panic!("a sub-upconverter centre must be refused, got {other:?}"),
        }
    }

    #[test]
    fn a_mode_change_alone_carries_the_dial_into_the_new_range() {
        let current = DeviceSettings {
            center_hz: Some(145_500_000.0),
            ..DeviceSettings::default()
        };
        let plan = validate(
            &DeviceSettings {
                extra: vec![mode_value(DirectSampling::IBranch)],
                ..DeviceSettings::default()
            },
            &caps(),
            &current,
            GAIN_VALUES,
        )
        .unwrap();
        assert_eq!(plan.center_hz, Some(14_400_000), "the top of the HF zone");

        let plan = validate(
            &DeviceSettings {
                extra: vec![mode_value(DirectSampling::Off)],
                ..DeviceSettings::default()
            },
            &caps(),
            &on_hf(),
            GAIN_VALUES,
        )
        .unwrap();
        assert_eq!(plan.direct_sampling, Some(DirectSampling::Off));
        assert_eq!(plan.center_hz, Some(24_000_000), "the bottom of the tuner");

        let contradictory = DeviceSettings {
            center_hz: Some(145_500_000.0),
            extra: vec![mode_value(DirectSampling::QBranch)],
            ..DeviceSettings::default()
        };
        assert!(matches!(
            plan_for(&contradictory),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn only_a_changed_mode_is_written() {
        let plan = validate(
            &DeviceSettings {
                extra: vec![mode_value(DirectSampling::QBranch)],
                ..DeviceSettings::default()
            },
            &caps(),
            &on_hf(),
            GAIN_VALUES,
        )
        .unwrap();
        assert_eq!(plan.direct_sampling, None);
        assert_eq!(plan.center_hz, None);
        assert_eq!(
            plan.applied.extra,
            vec![mode_value(DirectSampling::QBranch)]
        );
    }

    #[test]
    fn a_bypassed_tuner_records_its_settings_instead_of_writing_them() {
        let restore = DeviceSettings {
            center_hz: Some(7_100_000.0),
            sample_rate: Some(1_024_000.0),
            bandwidth: Some(manual(300_000.0)),
            gains: vec![tuner(20.7)],
            agc: Some(AgcSetting::switched(false)),
            extra: vec![mode_value(DirectSampling::QBranch)],
            ..DeviceSettings::default()
        };
        let plan = plan_for(&restore).unwrap();
        assert_eq!(plan.direct_sampling, Some(DirectSampling::QBranch));
        assert_eq!(plan.center_hz, Some(7_100_000));
        assert_eq!(plan.sample_rate, Some(1_024_000));
        assert_eq!(plan.gain, None, "the tuner must not be driven in standby");
        assert_eq!(plan.bandwidth, None);
        assert_eq!(plan.applied.bandwidth, Some(manual(300_000.0)));
        assert_eq!(plan.applied.gains[0].value_db, 20.7);
    }

    #[test]
    fn leaving_direct_sampling_restores_the_tuner_it_re_initializes() {
        let leave = DeviceSettings {
            extra: vec![mode_value(DirectSampling::Off)],
            center_hz: Some(145_500_000.0),
            ..DeviceSettings::default()
        };
        let plan = validate(&leave, &caps(), &on_hf(), GAIN_VALUES).unwrap();
        assert_eq!(plan.gain, Some(GainMode::Manual(207)));
        assert_eq!(plan.bandwidth, Some(300_000));

        let mut current = on_hf();
        current.bandwidth = None;
        current.agc = Some(AgcSetting::switched(true));
        let plan = validate(&leave, &caps(), &current, GAIN_VALUES).unwrap();
        assert_eq!(plan.gain, Some(GainMode::Auto));
        assert_eq!(plan.bandwidth, Some(0));
        assert_eq!(plan.applied.bandwidth, Some(BandwidthSetting::Auto));
    }

    #[test]
    fn validate_rejects_unknown_direct_sampling_values() {
        for value in ["", "1", "on", "Q"] {
            let delta = DeviceSettings {
                extra: vec![ExtraValue {
                    name: DIRECT_SAMPLING.to_string(),
                    value: value.into(),
                }],
                ..DeviceSettings::default()
            };
            assert!(
                matches!(plan_for(&delta), Err(DeviceError::Unsupported(_))),
                "direct_sampling {value:?} must be rejected"
            );
        }
        let mistyped = DeviceSettings {
            extra: vec![ExtraValue {
                name: DIRECT_SAMPLING.to_string(),
                value: true.into(),
            }],
            ..DeviceSettings::default()
        };
        assert!(matches!(
            plan_for(&mistyped),
            Err(DeviceError::Unsupported(_))
        ));
        let v4 = capabilities(BoardVariant::RtlSdrBlogV4, GAIN_VALUES);
        let on_v4 = DeviceSettings {
            extra: vec![mode_value(DirectSampling::QBranch)],
            ..DeviceSettings::default()
        };
        assert!(matches!(
            validate(&on_v4, &v4, &DeviceSettings::default(), GAIN_VALUES),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn an_empty_delta_plans_nothing() {
        assert_eq!(
            plan_for(&DeviceSettings::default()).unwrap(),
            Plan::default()
        );
    }

    #[test]
    fn validate_refuses_per_stream_overrides() {
        let delta = DeviceSettings {
            streams: vec![sdrmm_wire::StreamSettings {
                stream: 0,
                gains: vec![tuner(20.7)],
                ..sdrmm_wire::StreamSettings::default()
            }],
            ..DeviceSettings::default()
        };
        match plan_for(&delta) {
            Err(DeviceError::Unsupported(message)) => {
                assert!(message.contains("streams[0]"), "{message}");
            }
            other => panic!("a streams entry must be Unsupported, got {other:?}"),
        }
    }
}
