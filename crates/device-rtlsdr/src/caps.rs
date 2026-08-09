//! Pure translation from an RTL-SDR's USB identity and tuner tables to the wire capability
//! model (PLAN §6), plus the pre-flight validation `apply` runs before touching hardware. No
//! I/O here, so every mapping is unit-testable against fabricated descriptors and gain tables.

use sdrmm_device::DeviceError;
use sdrmm_rtl_driver::{BoardVariant, DeviceDescriptor};
use sdrmm_wire::{
    Capabilities, DeviceInfo, DeviceSettings, ExtraSetting, ExtraValue, GainStage, GainValue, Range,
};

use crate::DRIVER_ID;

/// The R820T/R828D PLL envelope. Both tuners the driver supports share it, so the tuner type does
/// not change the ranges — only the board variant does (see [`capabilities`]).
const TUNER_MIN_HZ: f64 = 24e6;
const TUNER_MAX_HZ: f64 = 1_766e6;
/// RTL-SDR Blog V4: the tuner's `set_freq` upconverts anything below the 28.8 MHz crystal
/// through the board's built-in HF path, which the vendor specifies from ~500 kHz. Plain dongles
/// have no such path — reaching HF there needs direct sampling, which the driver does not
/// implement yet, so the low end honestly stays at [`TUNER_MIN_HZ`].
const V4_HF_MIN_HZ: f64 = 500e3;
const V4_HF_MAX_HZ: f64 = 28.8e6;

/// The RTL2832U resampler's two valid windows (`RtlSdr::set_sample_rate`, librtlsdr's
/// `rtlsdr_set_sample_rate`): everything between them aliases.
const RATE_WINDOWS: [(f64, f64); 2] = [(225_001.0, 300_000.0), (900_001.0, 3_200_000.0)];

/// The rates offered to the client. `Capabilities` carries one `sample_rate_range`, and the two
/// windows above are disjoint — a single spanning range would advertise the 300 kHz–900 kHz
/// hole as tunable — so the discrete list is the honest wire representation, and it is the
/// conventional RTL-SDR menu every other tool offers. Validation still accepts any rate inside
/// the windows: under-advertising the menu is safe, over-advertising the hardware is not.
/// Anything above 2.4 Msps overruns the USB path on many hosts and drops samples; the values
/// are listed because the hardware accepts them, not because they are reliable everywhere.
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

/// Advertised crystal-correction range. Well inside the demodulator's own ±488 ppm register
/// limit ([`sdrmm_rtl_driver::MAX_PPM`]) — every dongle worth correcting is within ±100, and a
/// range that wide is a slider users can actually aim with.
const PPM_MAX: f64 = 200.0;

/// Widest R82xx IF filter mode (`set_bandwidth`'s 8 MHz branch). The wire capability model has
/// no bandwidth *range* — only a discrete list, which the filter does not have — so this
/// envelope is what validation enforces; 0 means "track the sample rate".
const BANDWIDTH_MAX_HZ: f64 = 8e6;

/// Sole gain stage. Named as SoapyRTLSDR names its gain element so the same dongle presents the
/// same control whichever backend opened it.
pub(crate) const TUNER_STAGE: &str = "TUNER";
/// Phantom power on the antenna port (RTL2832U GPIO0).
pub(crate) const BIAS_TEE: &str = "bias_tee";
/// R82xx tuner AGC (LNA + mixer). Not the RTL2832U's digital AGC, which the driver does not
/// program.
pub(crate) const AGC: &str = "agc";

/// What `apply` will write, resolved and range-checked. Built entirely before the first setter
/// runs so a bad field cannot leave the hardware half-retuned.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Plan {
    pub(crate) sample_rate: Option<u32>,
    /// Crystal correction in whole ppm — the only granularity the correction registers have.
    pub(crate) ppm: Option<i32>,
    pub(crate) center_hz: Option<u32>,
    /// Tuner IF filter in Hz; `Some(0)` selects the automatic width.
    pub(crate) bandwidth: Option<u32>,
    pub(crate) gain: Option<GainMode>,
    pub(crate) bias_tee: Option<bool>,
    /// What `settings()` must report once the writes land — the snapped gain and the resolved
    /// AGC mode, i.e. the fields the hardware cannot be asked to read back.
    pub(crate) applied: DeviceSettings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GainMode {
    Auto,
    /// Tenths of a dB, already snapped to the tuner's table.
    Manual(i32),
}

/// Map one probe's descriptors to wire [`DeviceInfo`]s.
///
/// Key: the serial when it identifies the device, because it survives replug and USB port moves
/// (the registry collapses duplicates by serial, and M5 auto-reconnect re-opens by this id).
/// The bus/address pair is the fallback — it is stable only while the dongle stays plugged into
/// the same port, which is the best a serial-less device can offer.
///
/// Most dongles ship with the same factory serial ("00000001"), so a serial is only identifying
/// when it is unique within this probe. A repeated one is reported as *no* serial: keeping it
/// would make the registry collapse two physical dongles into one entry, and `DeviceId::Serial`
/// would always open whichever enumerated first.
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
            match serial {
                Some(serial) => DeviceInfo {
                    driver: DRIVER_ID.to_string(),
                    key: serial.clone(),
                    label: format!("{model} {serial}"),
                    serial: Some(serial.clone()),
                },
                None => DeviceInfo {
                    driver: DRIVER_ID.to_string(),
                    key: location.clone(),
                    label: format!("{model} ({location})"),
                    serial: None,
                },
            }
        })
        .collect()
}

/// The capability envelope of an opened dongle. `gains` is the tuner's own table in tenths of a
/// dB (`RtlSdr::gains`), so the advertised stage range is the hardware's, not a guess.
pub(crate) fn capabilities(board: BoardVariant, gains: &[i32]) -> Capabilities {
    let mut freq_ranges = Vec::with_capacity(2);
    if board == BoardVariant::RtlSdrBlogV4 {
        // Overlaps the tuner range at its top end; validation accepts a value inside any range.
        freq_ranges.push(Range {
            min: V4_HF_MIN_HZ,
            max: V4_HF_MAX_HZ,
            step: None,
        });
    }
    freq_ranges.push(Range {
        min: TUNER_MIN_HZ,
        max: TUNER_MAX_HZ,
        step: None,
    });

    // The 29-entry R82xx table is not uniformly spaced, so there is no step to advertise;
    // `apply` snaps to the nearest entry and reports what it snapped to.
    let gain_stages = match (gains.iter().copied().min(), gains.iter().copied().max()) {
        (Some(min), Some(max)) => vec![GainStage {
            name: TUNER_STAGE.to_string(),
            range: Range {
                min: tenths_to_db(min),
                max: tenths_to_db(max),
                step: None,
            },
        }],
        _ => Vec::new(),
    };

    Capabilities {
        freq_ranges,
        sample_rates: RATE_MENU.to_vec(),
        sample_rate_range: None,
        gains: gain_stages,
        antennas: vec!["RX".to_string()],
        // The R82xx filter is continuous from the caller's side (it snaps internally to its own
        // cutoff steps, which the tuner does not report back), and the wire model can only carry a
        // discrete list — so none is advertised, exactly as the Soapy path reports for the same
        // dongle. `BANDWIDTH_MAX_HZ` still bounds what `apply` will write.
        bandwidths: Vec::new(),
        extra: extra_settings(),
        tx_capable: false,
    }
}

/// The device-specific knobs the driver can actually drive. Direct sampling, offset tuning and
/// the RTL2832U digital AGC are deliberately absent: nothing programs them yet, and advertising
/// a control that silently does nothing is worse than not offering it. Crystal correction is
/// *not* here — `DeviceSettings` carries `ppm` as a first-class field, so it needs no extra.
fn extra_settings() -> Vec<ExtraSetting> {
    vec![
        ExtraSetting::Bool {
            name: BIAS_TEE.to_string(),
            default: false,
        },
        ExtraSetting::Bool {
            name: AGC.to_string(),
            // Matches what `open` programs: an untouched dongle shows a usable spectrum.
            default: true,
        },
    ]
}

fn extra_name(setting: &ExtraSetting) -> &str {
    match setting {
        ExtraSetting::Bool { name, .. }
        | ExtraSetting::Range { name, .. }
        | ExtraSetting::Enum { name, .. } => name,
    }
}

fn tenths_to_db(tenths: i32) -> f64 {
    f64::from(tenths) / 10.0
}

/// Nearest entry in the tuner's discrete gain table, in tenths of a dB. Requests below the
/// first or above the last entry clamp; a request exactly between two steps takes the lower one
/// so a snap never raises gain beyond what was asked for. `None` only for an empty table.
pub(crate) fn nearest_gain(table: &[i32], tenths: i32) -> Option<i32> {
    table
        .iter()
        .copied()
        .min_by_key(|g| ((i64::from(*g) - i64::from(tenths)).abs(), i64::from(*g)))
}

/// Pre-flight for `apply`: reject everything the hardware cannot take *before* any setter runs,
/// and resolve the rest into a [`Plan`]. `current` supplies the values a partial delta implies
/// (turning AGC off restores the last manual gain), `table` is the tuner's gain table.
pub(crate) fn validate(
    delta: &DeviceSettings,
    caps: &Capabilities,
    current: &DeviceSettings,
    table: &[i32],
) -> Result<Plan, DeviceError> {
    let mut plan = Plan::default();

    if let Some(f) = delta.center_hz {
        if !caps.freq_ranges.iter().any(|r| r.min <= f && f <= r.max) {
            return Err(DeviceError::Unsupported(format!(
                "center_hz {f} outside tuner range"
            )));
        }
        plan.center_hz = Some(f.round() as u32);
    }

    if let Some(rate) = delta.sample_rate {
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
    }

    // `RtlSdr::set_sample_rate` re-runs the tuner's bandwidth calculation against the new rate
    // (as librtlsdr does), silently reverting an explicit filter width. Carry the recorded one
    // forward so the reported bandwidth stays true after a rate change.
    let bandwidth = delta.bandwidth.or_else(|| {
        plan.sample_rate
            .is_some()
            .then_some(current.bandwidth)
            .flatten()
    });
    if let Some(bw) = bandwidth {
        if !(0.0..=BANDWIDTH_MAX_HZ).contains(&bw) {
            return Err(DeviceError::Unsupported(format!(
                "bandwidth {bw} outside 0..{BANDWIDTH_MAX_HZ} Hz (0 = automatic)"
            )));
        }
        plan.bandwidth = Some(bw.round() as u32);
        plan.applied.bandwidth = (bw > 0.0).then_some(bw);
    }

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
        // The correction registers count in whole ppm, so a fractional request has no
        // representation; it is rounded, and `settings()` reports the rounded value back so the
        // choice is visible rather than silent (the gain table is snapped for the same reason).
        plan.ppm = Some(ppm.round() as i32);
    }

    let mut requested_gain = None;
    for gain in &delta.gains {
        let stage = caps
            .gains
            .iter()
            .find(|s| s.name == gain.stage)
            .ok_or_else(|| DeviceError::Unsupported(format!("gain stage {}", gain.stage)))?;
        if !(stage.range.min..=stage.range.max).contains(&gain.value_db) {
            return Err(DeviceError::Unsupported(format!(
                "gain {} {} dB outside {}..{} dB",
                gain.stage, gain.value_db, stage.range.min, stage.range.max
            )));
        }
        requested_gain = nearest_gain(table, (gain.value_db * 10.0).round() as i32);
    }

    let mut agc = None;
    for value in &delta.extra {
        let setting = caps
            .extra
            .iter()
            .find(|s| extra_name(s) == value.name)
            .ok_or_else(|| DeviceError::Unsupported(format!("extra setting {}", value.name)))?;
        let on = extra_bool(setting, value)?;
        match value.name.as_str() {
            BIAS_TEE => {
                plan.bias_tee = Some(on);
                plan.applied.extra.push(ExtraValue {
                    name: BIAS_TEE.to_string(),
                    value: on.into(),
                });
            }
            AGC => agc = Some(on),
            other => return Err(DeviceError::Unsupported(format!("extra setting {other}"))),
        }
    }

    // The R82xx has one gain control: `set_gain_manual` turns the AGC off as a side effect, so
    // mode and value are the same knob. An explicit `agc` in the delta decides the mode; on its
    // own, a TUNER value means manual, which is what the hardware would do anyway.
    plan.gain = match (agc, requested_gain) {
        (Some(true), _) => Some(GainMode::Auto),
        (_, Some(tenths)) => Some(GainMode::Manual(tenths)),
        (Some(false), None) => {
            // Leaving AGC needs a value: the last manual one, else full sensitivity.
            let tenths = current_manual_tenths(current)
                .and_then(|t| nearest_gain(table, t))
                .or_else(|| table.iter().copied().max())
                .ok_or_else(|| {
                    DeviceError::Unsupported("agc off: tuner exposes no gain table".to_string())
                })?;
            Some(GainMode::Manual(tenths))
        }
        (None, None) => None,
    };
    match plan.gain {
        // In auto the tuner ignores the manual value, so the recorded one is only what a later
        // `agc: false` would restore — the mode is what `settings()` reports as truth.
        Some(GainMode::Auto) => plan.applied.extra.push(ExtraValue {
            name: AGC.to_string(),
            value: true.into(),
        }),
        Some(GainMode::Manual(tenths)) => {
            plan.applied.extra.push(ExtraValue {
                name: AGC.to_string(),
                value: false.into(),
            });
            plan.applied.gains.push(GainValue {
                stage: TUNER_STAGE.to_string(),
                value_db: tenths_to_db(tenths),
            });
        }
        None => {}
    }

    Ok(plan)
}

/// Both advertised extras are booleans; anything else in the delta is a client bug, not a value
/// to coerce.
fn extra_bool(setting: &ExtraSetting, value: &ExtraValue) -> Result<bool, DeviceError> {
    match setting {
        ExtraSetting::Bool { .. } => value.value.as_bool().ok_or_else(|| {
            DeviceError::Unsupported(format!(
                "extra setting {}: bad value {}",
                value.name, value.value
            ))
        }),
        _ => Err(DeviceError::Unsupported(format!(
            "extra setting {}: not a boolean",
            value.name
        ))),
    }
}

fn current_manual_tenths(current: &DeviceSettings) -> Option<i32> {
    current
        .gains
        .iter()
        .find(|g| g.stage == TUNER_STAGE)
        .filter(|g| g.value_db.is_finite())
        .map(|g| (g.value_db * 10.0).round() as i32)
}

#[cfg(test)]
mod tests {
    use sdrmm_rtl_driver::GAIN_VALUES;

    use super::*;

    fn descriptor(address: u8, serial: Option<&str>) -> DeviceDescriptor {
        DeviceDescriptor {
            index: usize::from(address),
            bus: "001".to_string(),
            address,
            vendor_id: 0x0bda,
            product_id: 0x2838,
            manufacturer: Some("Realtek".to_string()),
            product: Some("RTL2838UHIDIR".to_string()),
            serial: serial.map(str::to_string),
            board_variant: BoardVariant::Generic,
        }
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
            // Reporting the shared serial would collapse both dongles into one registry entry.
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
            vec![Range {
                min: 24e6,
                max: 1.766e9,
                step: None
            }]
        );
        assert_eq!(caps.sample_rates, RATE_MENU.to_vec());
        assert_eq!(caps.sample_rate_range, None);
        assert_eq!(caps.antennas, vec!["RX".to_string()]);
        assert!(caps.bandwidths.is_empty());
        assert!(!caps.tx_capable);
        assert_eq!(caps.gains.len(), 1);
        assert_eq!(caps.gains[0].name, "TUNER");
        assert_eq!(caps.gains[0].range.min, 0.0);
        assert_eq!(caps.gains[0].range.max, 49.6);
        assert_eq!(caps.gains[0].range.step, None);
    }

    #[test]
    fn blog_v4_adds_the_upconverted_hf_range() {
        let caps = capabilities(BoardVariant::RtlSdrBlogV4, GAIN_VALUES);
        assert_eq!(caps.freq_ranges.len(), 2);
        assert_eq!(caps.freq_ranges[0].min, 500e3);
        assert_eq!(caps.freq_ranges[0].max, 28.8e6);
        assert_eq!(caps.freq_ranges[1].min, 24e6);
    }

    #[test]
    fn gain_stage_follows_the_reported_table() {
        let caps = capabilities(BoardVariant::Generic, &[0, 87, 213]);
        assert_eq!(caps.gains[0].range.max, 21.3);
        // A tuner that reports no gains gets no stage rather than a fabricated one.
        assert!(capabilities(BoardVariant::Generic, &[]).gains.is_empty());
    }

    #[test]
    fn extras_are_only_what_the_driver_can_drive() {
        let extra = capabilities(BoardVariant::Generic, GAIN_VALUES).extra;
        let names: Vec<&str> = extra.iter().map(extra_name).collect();
        assert_eq!(names, vec![BIAS_TEE, AGC]);
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

    fn caps() -> Capabilities {
        capabilities(BoardVariant::Generic, GAIN_VALUES)
    }

    fn plan_for(delta: &DeviceSettings) -> Result<Plan, DeviceError> {
        validate(delta, &caps(), &DeviceSettings::default(), GAIN_VALUES)
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
    fn validate_bounds_bandwidth_and_maps_zero_to_automatic() {
        let auto = DeviceSettings {
            bandwidth: Some(0.0),
            ..DeviceSettings::default()
        };
        let plan = plan_for(&auto).unwrap();
        assert_eq!(plan.bandwidth, Some(0));
        assert_eq!(plan.applied.bandwidth, None);

        let narrow = DeviceSettings {
            bandwidth: Some(300_000.0),
            ..DeviceSettings::default()
        };
        let plan = plan_for(&narrow).unwrap();
        assert_eq!(plan.bandwidth, Some(300_000));
        assert_eq!(plan.applied.bandwidth, Some(300_000.0));

        for bad in [-1.0, 9e6, f64::NAN] {
            let delta = DeviceSettings {
                bandwidth: Some(bad),
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
            bandwidth: Some(1_500_000.0),
            ..DeviceSettings::default()
        };
        let delta = DeviceSettings {
            sample_rate: Some(2_400_000.0),
            ..DeviceSettings::default()
        };
        let plan = validate(&delta, &caps(), &current, GAIN_VALUES).unwrap();
        assert_eq!(plan.bandwidth, Some(1_500_000));

        // Without a rate change there is nothing to restore.
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
                gains: vec![GainValue {
                    stage: TUNER_STAGE.to_string(),
                    value_db: bad,
                }],
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
            gains: vec![GainValue {
                stage: TUNER_STAGE.to_string(),
                value_db: 30.0,
            }],
            ..DeviceSettings::default()
        };
        let plan = plan_for(&delta).unwrap();
        // 29.7 dB and 32.8 dB bracket the request; 29.7 is nearer.
        assert_eq!(plan.gain, Some(GainMode::Manual(297)));
        assert_eq!(plan.applied.gains[0].value_db, 29.7);
        // A manual value implies manual mode — that is what the tuner does.
        assert_eq!(
            plan.applied.extra,
            vec![ExtraValue {
                name: AGC.to_string(),
                value: false.into(),
            }]
        );
    }

    #[test]
    fn agc_on_wins_over_a_manual_value_in_the_same_delta() {
        let delta = DeviceSettings {
            gains: vec![GainValue {
                stage: TUNER_STAGE.to_string(),
                value_db: 30.0,
            }],
            extra: vec![ExtraValue {
                name: AGC.to_string(),
                value: true.into(),
            }],
            ..DeviceSettings::default()
        };
        let plan = plan_for(&delta).unwrap();
        assert_eq!(plan.gain, Some(GainMode::Auto));
        assert!(plan.applied.gains.is_empty());
        assert_eq!(plan.applied.extra[0].value.as_bool(), Some(true));
    }

    #[test]
    fn leaving_agc_restores_the_last_manual_gain() {
        let delta = DeviceSettings {
            extra: vec![ExtraValue {
                name: AGC.to_string(),
                value: false.into(),
            }],
            ..DeviceSettings::default()
        };
        let current = DeviceSettings {
            gains: vec![GainValue {
                stage: TUNER_STAGE.to_string(),
                value_db: 20.7,
            }],
            ..DeviceSettings::default()
        };
        let plan = validate(&delta, &caps(), &current, GAIN_VALUES).unwrap();
        assert_eq!(plan.gain, Some(GainMode::Manual(207)));

        // With nothing to restore, full sensitivity beats leaving the tuner in auto while
        // reporting manual.
        let plan = plan_for(&delta).unwrap();
        assert_eq!(plan.gain, Some(GainMode::Manual(496)));
    }

    #[test]
    fn bias_tee_is_planned_and_echoed() {
        let delta = DeviceSettings {
            extra: vec![ExtraValue {
                name: BIAS_TEE.to_string(),
                value: true.into(),
            }],
            ..DeviceSettings::default()
        };
        let plan = plan_for(&delta).unwrap();
        assert_eq!(plan.bias_tee, Some(true));
        assert_eq!(plan.applied.extra[0].name, BIAS_TEE);
    }

    #[test]
    fn validate_rejects_unknown_and_mistyped_extras() {
        for value in [
            ExtraValue {
                name: "direct_samp".to_string(),
                value: "1".into(),
            },
            ExtraValue {
                name: BIAS_TEE.to_string(),
                value: "yes".into(),
            },
            ExtraValue {
                name: AGC.to_string(),
                value: 1.into(),
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

    /// The correction registers count in whole ppm, so a fractional request is rounded rather
    /// than refused — and the rounding must be visible, which is why `apply` reports the
    /// hardware's own value back afterwards.
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
            bandwidth: Some(1_000_000.0),
            antenna: Some("RX".to_string()),
            gains: vec![GainValue {
                stage: TUNER_STAGE.to_string(),
                value_db: 49.6,
            }],
            extra: vec![ExtraValue {
                name: BIAS_TEE.to_string(),
                value: true.into(),
            }],
            ..DeviceSettings::default()
        };
        let plan = plan_for(&delta).unwrap();
        assert_eq!(plan.center_hz, Some(433_920_000));
        assert_eq!(plan.sample_rate, Some(2_048_000));
        assert_eq!(plan.bandwidth, Some(1_000_000));
        assert_eq!(plan.gain, Some(GainMode::Manual(496)));
        assert_eq!(plan.bias_tee, Some(true));
    }

    #[test]
    fn an_empty_delta_plans_nothing() {
        assert_eq!(
            plan_for(&DeviceSettings::default()).unwrap(),
            Plan::default()
        );
    }
}
