//! Pure translation from an RTL-SDR's USB identity and tuner tables to the wire capability
//! model (PLAN §6), plus the pre-flight validation `apply` runs before touching hardware. No
//! I/O here, so every mapping is unit-testable against fabricated descriptors and gain tables.

use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{
    Capabilities, DeviceInfo, DeviceSettings, Duplex, ExtraSetting, ExtraValue, GainStage,
    GainValue, Range, StreamScope,
};

use crate::{
    DEFAULT_CENTER_HZ, DRIVER_ID,
    driver::{BoardVariant, DIRECT_SAMPLING_MAX_HZ, DeviceDescriptor, DirectSampling},
};

/// The R820T/R828D PLL envelope. Both tuners the driver supports share it, so the tuner type does
/// not change the ranges — only the board variant does (see [`capabilities`]).
const TUNER_MIN_HZ: f64 = 24e6;
const TUNER_MAX_HZ: f64 = 1_766e6;
/// RTL-SDR Blog V4: the tuner's `set_freq` upconverts anything below the 28.8 MHz crystal
/// through the board's built-in HF path, which the vendor specifies from ~500 kHz.
const V4_HF_MIN_HZ: f64 = 500e3;
const V4_HF_MAX_HZ: f64 = 28.8e6;

/// What direct sampling reaches on every other board: the ADC's first Nyquist zone, DC to half
/// the crystal. Only reachable with the tuner bypassed, which is why [`reachable_ranges`] and not
/// `freq_ranges` decides whether a centre is valid — the capability set advertises the union of
/// both modes so the picker can see the dongle covers HF at all.
const DIRECT_RANGE: Range = Range {
    min: 0.0,
    max: DIRECT_SAMPLING_MAX_HZ as f64,
    step: None,
};

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
/// limit ([`MAX_PPM`]) — every dongle worth correcting is within ±100, and a range that wide is
/// a slider users can actually aim with.
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
/// Which ADC branch the demodulator receives, with the tuner bypassed — the HF path on every
/// board but the Blog V4, which has an upconverter instead.
pub(crate) const DIRECT_SAMPLING: &str = "direct_sampling";

/// What `apply` will write, resolved and range-checked. Built entirely before the first setter
/// runs so a bad field cannot leave the hardware half-retuned.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Plan {
    pub(crate) sample_rate: Option<u32>,
    /// Crystal correction in whole ppm — the only granularity the correction registers have.
    pub(crate) ppm: Option<i32>,
    pub(crate) center_hz: Option<u32>,
    /// Tuner IF filter in Hz; `Some(0)` selects the automatic width. Never set while direct
    /// sampling: the filter is in the bypassed half of the radio.
    pub(crate) bandwidth: Option<u32>,
    /// Whether `settings()` must stop reporting a filter width, which `merge_from` cannot do —
    /// an automatic width is the absence of one.
    pub(crate) clear_bandwidth: bool,
    /// Set only when the mode actually changes: re-entering the mode the radio is already in
    /// would re-initialize the tuner for nothing.
    pub(crate) direct_sampling: Option<DirectSampling>,
    /// Never set while direct sampling, for the same reason as `bandwidth`. The requested value
    /// is still reported, because leaving direct sampling restores it.
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
/// would make the registry collapse two physical dongles into one entry, and opening by it
/// would always get whichever enumerated first.
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
            // The descriptor already carries the board, and the tuner range and rate menu are
            // the board's — so the picker can tell whether a template fits without claiming the
            // dongle. The gain table is the unit's and is read when it opens, which is exactly
            // what a profile leaves out.
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

/// The capability envelope of an opened dongle. `gains` is the tuner's own table in tenths of a
/// dB (`RtlSdr::gains`), so the advertised stage range is the hardware's, not a guess.
pub(crate) fn capabilities(board: BoardVariant, gains: &[i32]) -> Capabilities {
    let mut freq_ranges = Vec::with_capacity(2);
    // Both boards reach HF, by different halves of the radio: the V4 upconverts into the tuner
    // and needs nothing switched, everything else bypasses the tuner and needs `direct_sampling`
    // set first. Both ranges overlap the tuner's at one end, and validation accepts a value
    // inside any range that the *current* mode can reach.
    if board == BoardVariant::RtlSdrBlogV4 {
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
        extra: extra_settings(board),
        ppm: true,
        duplex: Duplex::RxOnly,
        rx_streams: 1,
        tx_streams: 0,
        per_stream: StreamScope::default(),
    }
}

/// The device-specific knobs the driver can actually drive. Offset tuning and the RTL2832U
/// digital AGC are deliberately absent: nothing programs them, and advertising a control that
/// silently does nothing is worse than not offering it. Crystal correction is *not* here —
/// `DeviceSettings` carries `ppm` as a first-class field, so it needs no extra.
///
/// Direct sampling is offered on every board but the Blog V4, whose HF path is an upconverter in
/// front of the tuner: bypassing the tuner there would disconnect the antenna from the receiver.
fn extra_settings(board: BoardVariant) -> Vec<ExtraSetting> {
    let mut settings = vec![
        ExtraSetting::Bool {
            name: BIAS_TEE.to_string(),
            default: false,
        },
        ExtraSetting::Bool {
            name: AGC.to_string(),
            // Matches what `open` programs: an untouched dongle shows a usable spectrum.
            default: true,
        },
    ];
    if board != BoardVariant::RtlSdrBlogV4 {
        settings.push(ExtraSetting::Enum {
            name: DIRECT_SAMPLING.to_string(),
            options: DirectSampling::all()
                .iter()
                .map(|mode| mode.as_str().to_string())
                .collect(),
            default: DirectSampling::Off.as_str().to_string(),
        });
    }
    settings
}

/// The ranges the radio can tune *in this mode*, which is the question a centre has to answer —
/// `freq_ranges` advertises the union of both, because a device picker asks what the radio can be
/// made to reach rather than what it reaches right now. The direct-sampling zone is recognized by
/// value: it is one constant, written into the capability set by [`capabilities`].
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

/// The frequency in `ranges` nearest to `hz`, for a mode change to land the dial on.
fn clamp_to_ranges(ranges: &[Range], hz: f64) -> f64 {
    ranges
        .iter()
        .map(|range| hz.clamp(range.min, range.max))
        .min_by(|a, b| (a - hz).abs().total_cmp(&(b - hz).abs()))
        .unwrap_or(hz)
}

/// The mode a settings snapshot is in. An absent or unparsable value is the mode every dongle
/// powers up in, which is also what a board that does not offer the setting is permanently in.
fn direct_sampling_of(settings: &DeviceSettings) -> DirectSampling {
    settings
        .extra
        .iter()
        .find(|value| value.name == DIRECT_SAMPLING)
        .and_then(|value| value.value.as_str())
        .and_then(DirectSampling::parse)
        .unwrap_or_default()
}

fn agc_of(settings: &DeviceSettings) -> Option<bool> {
    settings
        .extra
        .iter()
        .find(|value| value.name == AGC)
        .and_then(|value| value.value.as_bool())
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
    check_stream_settings(delta, caps)?;
    let mut plan = Plan::default();

    // The mode is resolved first because it decides what the rest means: which frequencies are
    // reachable, and whether the tuner is in the signal path at all.
    let mut agc = None;
    let mut requested_mode = None;
    for value in &delta.extra {
        let setting = caps
            .extra
            .iter()
            .find(|s| extra_name(s) == value.name)
            .ok_or_else(|| DeviceError::Unsupported(format!("extra setting {}", value.name)))?;
        match value.name.as_str() {
            BIAS_TEE => {
                let on = extra_bool(setting, value)?;
                plan.bias_tee = Some(on);
                plan.applied.extra.push(ExtraValue {
                    name: BIAS_TEE.to_string(),
                    value: on.into(),
                });
            }
            AGC => agc = Some(extra_bool(setting, value)?),
            DIRECT_SAMPLING => {
                let mode = extra_direct_sampling(setting, value)?;
                requested_mode = Some(mode);
                plan.applied.extra.push(ExtraValue {
                    name: DIRECT_SAMPLING.to_string(),
                    value: mode.as_str().into(),
                });
            }
            other => return Err(DeviceError::Unsupported(format!("extra setting {other}"))),
        }
    }
    let current_mode = direct_sampling_of(current);
    let mode = requested_mode.unwrap_or(current_mode);
    if mode != current_mode {
        plan.direct_sampling = Some(mode);
    }

    let ranges = reachable_ranges(caps, mode);
    if let Some(f) = delta.center_hz {
        if !reaches(&ranges, f) {
            return Err(DeviceError::Unsupported(unreachable_center(caps, mode, f)));
        }
        plan.center_hz = Some(f.round() as u32);
    } else if plan.direct_sampling.is_some() {
        // A mode change carries a centre whether the caller sent one or not: the radio is parked
        // on a frequency the mode being entered cannot reach, and `RtlSdr`'s setters retune from
        // that cached centre. Nearest-in-range rather than a fixed default, and reported back —
        // the same contract the gain table's snapping has.
        let hz = current
            .center_hz
            .filter(|hz| hz.is_finite())
            .unwrap_or(f64::from(DEFAULT_CENTER_HZ));
        plan.center_hz = Some(clamp_to_ranges(&ranges, hz).round() as u32);
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
    // (as librtlsdr does), silently reverting an explicit filter width, and leaving direct
    // sampling re-initializes the tuner back to its DVB-T default. Carry the recorded width
    // through both so the reported bandwidth stays true. On the way out of direct sampling an
    // unrecorded width is `0` — automatic — because the tuner's filter has to be told the sample
    // rate again either way.
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
                .then(|| current.bandwidth.unwrap_or(0.0))
        });
    if let Some(bw) = bandwidth {
        if !(0.0..=BANDWIDTH_MAX_HZ).contains(&bw) {
            return Err(DeviceError::Unsupported(format!(
                "bandwidth {bw} outside 0..{BANDWIDTH_MAX_HZ} Hz (0 = automatic)"
            )));
        }
        // The filter belongs to the bypassed half of the radio while direct sampling, so nothing
        // is written — but the width is still reported and stored, because that is what leaving
        // direct sampling restores.
        if mode == DirectSampling::Off {
            plan.bandwidth = Some(bw.round() as u32);
        }
        plan.applied.bandwidth = (bw > 0.0).then_some(bw);
        plan.clear_bandwidth = bw == 0.0;
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

    // The R82xx has one gain control: `set_gain_manual` turns the AGC off as a side effect, so
    // mode and value are the same knob. An explicit `agc` in the delta decides the mode; on its
    // own, a TUNER value means manual, which is what the hardware would do anyway.
    let gain = match (agc, requested_gain) {
        (Some(true), _) => Some(GainMode::Auto),
        (_, Some(tenths)) => Some(GainMode::Manual(tenths)),
        // Leaving AGC needs a value: the last manual one, else full sensitivity.
        (Some(false), None) => Some(GainMode::Manual(restored_manual(current, table)?)),
        // Leaving direct sampling re-initializes the tuner, which resets its gain registers to
        // the R82xx defaults — so the mode the radio was already in has to be written again.
        (None, None) if plan.direct_sampling == Some(DirectSampling::Off) => {
            Some(match agc_of(current) {
                Some(false) => GainMode::Manual(restored_manual(current, table)?),
                _ => GainMode::Auto,
            })
        }
        (None, None) => None,
    };
    // Same rule as the filter width: with the tuner bypassed the gain is not on the signal path,
    // so it is recorded rather than written. Refusing it instead would break the reconnect path,
    // which restores a whole stored configuration — direct sampling and tuner gain together — in
    // one call.
    if mode == DirectSampling::Off {
        plan.gain = gain;
    }
    match gain {
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

/// Why a centre is out of reach, and what would bring it in reach. A board that offers direct
/// sampling can usually reach a rejected HF frequency by switching to it, and saying so is the
/// difference between a dead end and a next step.
fn unreachable_center(caps: &Capabilities, mode: DirectSampling, hz: f64) -> String {
    let offers_direct = caps
        .extra
        .iter()
        .any(|setting| extra_name(setting) == DIRECT_SAMPLING);
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

/// The manual gain to leave the AGC for: the last one recorded, else full sensitivity.
fn restored_manual(current: &DeviceSettings, table: &[i32]) -> Result<i32, DeviceError> {
    current_manual_tenths(current)
        .and_then(|tenths| nearest_gain(table, tenths))
        .or_else(|| table.iter().copied().max())
        .ok_or_else(|| DeviceError::Unsupported("tuner exposes no gain table".to_string()))
}

/// Both boolean extras; anything else in the delta is a client bug, not a value to coerce.
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

/// The mode named by an enum extra, checked against the options the capability set advertised —
/// a board that does not offer a branch must not be switched to it by spelling it correctly.
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
        .filter(|text| options.iter().any(|option| option == text))
        .and_then(DirectSampling::parse)
        .ok_or_else(|| {
            DeviceError::Unsupported(format!(
                "extra setting {}: bad value {}",
                value.name, value.value
            ))
        })
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
        assert_eq!(caps.sample_rate_range, None);
        assert_eq!(caps.antennas, vec!["RX".to_string()]);
        assert!(caps.bandwidths.is_empty());
        assert_eq!(caps.duplex, Duplex::RxOnly);
        // The dongle's crystal is exactly what a ppm correction is for, and this backend
        // programs it (`set_freq_correction`), so the control is offered here.
        assert!(caps.ppm);
        assert_eq!(caps.rx_streams, 1);
        assert_eq!(caps.gains.len(), 1);
        assert_eq!(caps.gains[0].name, "TUNER");
        assert_eq!(caps.gains[0].range.min, 0.0);
        assert_eq!(caps.gains[0].range.max, 49.6);
        assert_eq!(caps.gains[0].range.step, None);
    }

    /// The V4 reaches HF through an upconverter in front of the tuner, so it gets a wider HF
    /// range than direct sampling could and no setting to switch — bypassing the tuner there
    /// would disconnect the antenna from the receiver.
    #[test]
    fn blog_v4_swaps_direct_sampling_for_its_upconverted_hf_range() {
        let caps = capabilities(BoardVariant::RtlSdrBlogV4, GAIN_VALUES);
        assert_eq!(caps.freq_ranges.len(), 2);
        assert_eq!(caps.freq_ranges[0].min, 500e3);
        assert_eq!(caps.freq_ranges[0].max, 28.8e6);
        assert_eq!(caps.freq_ranges[1].min, 24e6);
        let names: Vec<&str> = caps.extra.iter().map(extra_name).collect();
        assert_eq!(names, vec![BIAS_TEE, AGC]);
    }

    /// The dongle can be *made* to reach HF, which is what a device picker asks — and it takes a
    /// setting, which is what `validate` enforces.
    #[test]
    fn a_generic_dongle_advertises_the_direct_sampling_zone_and_the_switch_for_it() {
        let caps = capabilities(BoardVariant::Generic, GAIN_VALUES);
        assert_eq!(caps.freq_ranges[0], DIRECT_RANGE);
        assert_eq!(DIRECT_RANGE.max, 14.4e6, "half the 28.8 MHz crystal");
        assert!(caps.profile().reaches(7.1e6));
        assert_eq!(
            caps.extra.last(),
            Some(&ExtraSetting::Enum {
                name: DIRECT_SAMPLING.to_string(),
                options: vec!["off".to_string(), "i".to_string(), "q".to_string()],
                default: "off".to_string(),
            })
        );
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
        assert_eq!(names, vec![BIAS_TEE, AGC, DIRECT_SAMPLING]);
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
    /// The advertised range is a usability choice; the register width is hardware. A range that
    /// outgrew the registers would wrap a correction into the opposite sign.
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

    fn mode_value(mode: DirectSampling) -> ExtraValue {
        ExtraValue {
            name: DIRECT_SAMPLING.to_string(),
            value: mode.as_str().into(),
        }
    }

    /// A settings snapshot of a dongle already listening on 40 m through the Q branch, with the
    /// tuner settings it had before the switch still recorded.
    fn on_hf() -> DeviceSettings {
        DeviceSettings {
            center_hz: Some(7_100_000.0),
            sample_rate: Some(1_024_000.0),
            bandwidth: Some(300_000.0),
            gains: vec![GainValue {
                stage: TUNER_STAGE.to_string(),
                value_db: 20.7,
            }],
            extra: vec![
                ExtraValue {
                    name: AGC.to_string(),
                    value: false.into(),
                },
                mode_value(DirectSampling::QBranch),
            ],
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

        // The tuner's own range is out of reach while it is bypassed, and the refusal says which
        // way back.
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

    /// The refusal a beginner meets first: an HF frequency on a dongle whose tuner starts at
    /// 24 MHz. It has to name the setting that would reach it.
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

        // The V4 has no such setting, so its refusals must not advertise one — 200 kHz is below
        // even its upconverter.
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

    /// Every setter that retunes reads the driver's cached centre, and after a mode change that
    /// cache holds a frequency the new mode cannot reach — so a mode change always carries a
    /// centre, clamped to the nearest reachable one and reported rather than silently kept.
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

        // A centre the caller asked for explicitly is refused rather than clamped: clamping is
        // for the frequency nobody named.
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

    /// Re-sending the mode the radio is already in must plan no switch: leaving direct sampling
    /// re-initializes the tuner, so a redundant write would drop its gain and filter.
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
        // Still echoed, so the reported settings stay the answer to "what mode is this in".
        assert_eq!(
            plan.applied.extra,
            vec![mode_value(DirectSampling::QBranch)]
        );
    }

    /// With the tuner bypassed its gain and IF filter are not on the signal path. Both are still
    /// accepted — the reconnect path restores a whole stored configuration in one call, and a
    /// refusal there would leave a faulted HF set unable to come back — and both are recorded,
    /// because leaving direct sampling is what puts them back into effect.
    #[test]
    fn a_bypassed_tuner_records_its_settings_instead_of_writing_them() {
        let restore = DeviceSettings {
            center_hz: Some(7_100_000.0),
            sample_rate: Some(1_024_000.0),
            bandwidth: Some(300_000.0),
            gains: vec![GainValue {
                stage: TUNER_STAGE.to_string(),
                value_db: 20.7,
            }],
            extra: vec![
                ExtraValue {
                    name: AGC.to_string(),
                    value: false.into(),
                },
                mode_value(DirectSampling::QBranch),
            ],
            ..DeviceSettings::default()
        };
        let plan = plan_for(&restore).unwrap();
        assert_eq!(plan.direct_sampling, Some(DirectSampling::QBranch));
        assert_eq!(plan.center_hz, Some(7_100_000));
        assert_eq!(plan.sample_rate, Some(1_024_000));
        assert_eq!(plan.gain, None, "the tuner must not be driven in standby");
        assert_eq!(plan.bandwidth, None);
        assert_eq!(plan.applied.bandwidth, Some(300_000.0));
        assert_eq!(plan.applied.gains[0].value_db, 20.7);
    }

    /// Leaving direct sampling re-initializes the tuner, which resets its gain registers and its
    /// filter to the R82xx defaults — so both have to be planned again even though the caller
    /// only asked for a mode.
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

        // A dongle that was in AGC before the switch comes back in AGC, and one with no recorded
        // width comes back on the automatic one rather than the tuner's DVB-T default.
        let mut current = on_hf();
        current.bandwidth = None;
        current.extra[0].value = true.into();
        let plan = validate(&leave, &caps(), &current, GAIN_VALUES).unwrap();
        assert_eq!(plan.gain, Some(GainMode::Auto));
        assert_eq!(plan.bandwidth, Some(0));
        assert_eq!(plan.applied.bandwidth, None);
        assert!(plan.clear_bandwidth);
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
        // Not a string at all, and — on a board that does not offer the setting — not a setting.
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

    /// An RTL-SDR has one stream and declares nothing per-stream, so any `streams` entry is a
    /// refusal naming the entry — never a silent drop into reported settings.
    #[test]
    fn validate_refuses_per_stream_overrides() {
        let delta = DeviceSettings {
            streams: vec![sdrmm_wire::StreamSettings {
                stream: 0,
                gains: vec![GainValue {
                    stage: TUNER_STAGE.to_string(),
                    value_db: 20.7,
                }],
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
