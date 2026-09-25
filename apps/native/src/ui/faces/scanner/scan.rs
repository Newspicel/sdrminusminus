use sdrmm_wire::{
    scan::{ScanRange, ScannerStatus},
    state::DeviceSet,
};

pub const MIN_STEP_KHZ: f64 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RangeValues {
    pub start_mhz: f64,
    pub stop_mhz: f64,
    pub step_khz: f64,
}

impl RangeValues {
    #[must_use]
    pub fn of(range: &ScanRange) -> Self {
        Self {
            start_mhz: range.start_hz / 1e6,
            stop_mhz: range.stop_hz / 1e6,
            step_khz: range.step_hz / 1e3,
        }
    }

    #[must_use]
    pub fn wire(self) -> ScanRange {
        ScanRange {
            start_hz: (self.start_mhz * 1e6).round(),
            stop_hz: (self.stop_mhz * 1e6).round(),
            step_hz: (self.step_khz * 1e3).round(),
        }
    }
}

pub fn parse_ranges(inputs: &[RangeValues]) -> Result<Vec<ScanRange>, String> {
    let many = inputs.len() > 1;
    let mut ranges = Vec::with_capacity(inputs.len());
    for (index, input) in inputs.iter().enumerate() {
        if input.stop_mhz < input.start_mhz {
            let line = if many {
                format!("range {}: ", index + 1)
            } else {
                String::new()
            };
            return Err(format!("{line}the stop frequency is below the start"));
        }
        ranges.push(input.wire());
    }
    if ranges.is_empty() {
        return Err(String::from("add at least one range"));
    }
    Ok(ranges)
}

pub fn check_ranges(ranges: &[ScanRange]) -> Result<Vec<ScanRange>, String> {
    let values: Vec<RangeValues> = ranges.iter().map(RangeValues::of).collect();
    parse_ranges(&values)
}

#[must_use]
pub fn target_count(ranges: &[ScanRange]) -> u64 {
    ranges
        .iter()
        .map(|range| {
            if range.step_hz <= 0.0 || range.stop_hz < range.start_hz {
                return 0;
            }
            ((range.stop_hz - range.start_hz) / range.step_hz).floor() as u64 + 1
        })
        .sum()
}

#[must_use]
pub fn live_status(
    set: Option<&DeviceSet>,
    channel: Option<u32>,
    pushed: Option<&ScannerStatus>,
) -> Option<ScannerStatus> {
    let listed = set?
        .scanners
        .iter()
        .find(|scanner| Some(scanner.settings.channel) == channel)?;
    Some(pushed.unwrap_or(listed).clone())
}

#[must_use]
pub fn sweep_kind(set: Option<&DeviceSet>, status: Option<&ScannerStatus>) -> &'static str {
    let own = match status {
        Some(status) => status.hardware_sweep,
        None => set.is_some_and(|set| set.capabilities.hardware_sweep),
    };
    if own {
        "the radio's own"
    } else {
        "by retuning"
    }
}

#[must_use]
pub fn format_mhz(hz: Option<f64>) -> String {
    match hz {
        Some(hz) if hz.is_finite() => format!("{:.4} MHz", hz / 1e6),
        _ => String::from("-"),
    }
}

#[must_use]
pub fn format_db(db: Option<f32>) -> String {
    match db {
        Some(db) if db.is_finite() => format!("{db:.1} dB"),
        _ => String::from("-"),
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::scan::{DEFAULT_SCAN_RANGE, ScanSettings, ScanState};

    use super::*;

    fn values(start_mhz: f64, stop_mhz: f64, step_khz: f64) -> RangeValues {
        RangeValues {
            start_mhz,
            stop_mhz,
            step_khz,
        }
    }

    fn range(start_hz: f64, stop_hz: f64, step_hz: f64) -> ScanRange {
        ScanRange {
            start_hz,
            stop_hz,
            step_hz,
        }
    }

    #[test]
    fn the_editor_units_become_whole_wire_hertz() {
        assert_eq!(
            parse_ranges(&[values(145.6, 145.8, 12.5)]),
            Ok(vec![range(145_600_000.0, 145_800_000.0, 12_500.0)])
        );
        assert_eq!(
            parse_ranges(&[values(433.075, 434.79, 8.33)]),
            Ok(vec![range(433_075_000.0, 434_790_000.0, 8_330.0)])
        );
    }

    #[test]
    fn what_no_single_field_catches_is_refused() {
        let backwards = parse_ranges(&[values(146.0, 145.0, 12.5)]);
        assert!(backwards.is_err_and(|error| error.contains("below the start")));
        let empty = parse_ranges(&[]);
        assert!(empty.is_err_and(|error| error.contains("at least one range")));
    }

    #[test]
    fn the_offending_line_is_named_when_there_are_several() {
        let parsed = parse_ranges(&[values(145.6, 145.8, 12.5), values(2.0, 1.0, 25.0)]);
        assert!(parsed.is_err_and(|error| error.starts_with("range 2: ")));
    }

    #[test]
    fn a_new_scanner_starts_on_a_range_that_parses() {
        assert_eq!(
            check_ranges(&[DEFAULT_SCAN_RANGE]),
            Ok(vec![DEFAULT_SCAN_RANGE])
        );
    }

    #[test]
    fn targets_count_inclusively_like_the_server() {
        assert_eq!(target_count(&[range(100.0, 200.0, 50.0)]), 3);
        assert_eq!(target_count(&[range(100.0, 249.0, 50.0)]), 3);
        assert_eq!(
            target_count(&[range(100.0, 200.0, 50.0), range(0.0, 0.0, 10.0)]),
            4
        );
    }

    fn status() -> ScannerStatus {
        ScannerStatus {
            state: ScanState::Scanning,
            settings: ScanSettings::for_channel(1),
            targets: 10,
            first_hz: 0.0,
            last_hz: 0.0,
            current_hz: 145_500_000.0,
            current_db: None,
            sweeps: 0,
            hits: 0,
            hardware_sweep: false,
            error: None,
        }
    }

    fn set(scanners: Vec<ScannerStatus>, hardware_sweep: bool) -> DeviceSet {
        let mut set: DeviceSet = serde_json::from_value(serde_json::json!({
            "id": 1,
            "device": { "driver": "virtual", "key": "siggen", "label": "Signal Generator" },
            "capabilities": {
                "freq_ranges": [], "sample_rates": [], "gains": [], "antennas": [], "bandwidths": []
            },
            "settings": {},
            "status": "running",
            "channels": []
        }))
        .expect("a device set");
        set.scanners = scanners;
        set.capabilities.hardware_sweep = hardware_sweep;
        set
    }

    #[test]
    fn a_pushed_update_wins_over_the_snapshot() {
        let held = set(vec![status()], false);
        assert_eq!(live_status(Some(&held), Some(1), None), Some(status()));
        let mut pushed = status();
        pushed.current_hz = 146_000_000.0;
        assert_eq!(
            live_status(Some(&held), Some(1), Some(&pushed)),
            Some(pushed)
        );
    }

    #[test]
    fn nothing_is_reported_for_a_decoder_that_is_not_scanning() {
        assert_eq!(
            live_status(Some(&set(vec![status()], false)), Some(2), Some(&status())),
            None
        );
        assert_eq!(
            live_status(Some(&set(vec![], false)), Some(1), Some(&status())),
            None
        );
        assert_eq!(live_status(None, Some(1), Some(&status())), None);
    }

    #[test]
    fn absent_readings_show_a_dash() {
        assert_eq!(format_db(Some(-31.5)), "-31.5 dB");
        assert_eq!(format_db(None), "-");
        assert_eq!(format_db(Some(f32::NEG_INFINITY)), "-");
        assert_eq!(format_mhz(None), "-");
        assert_eq!(format_mhz(Some(f64::NAN)), "-");
        assert_eq!(format_mhz(Some(145_500_000.0)), "145.5000 MHz");
    }

    #[test]
    fn the_sweep_is_named_by_what_actually_sweeps() {
        let radio = set(vec![], true);
        assert_eq!(sweep_kind(Some(&radio), None), "the radio's own");
        assert_eq!(sweep_kind(Some(&radio), Some(&status())), "by retuning");
        assert_eq!(sweep_kind(None, None), "by retuning");
    }
}
