use sdrmm_wire::device::{
    AgcSetting, BandwidthSetting, Capabilities, DcArtifact, DeviceSettings, GainKind, GainStage,
    GainUnit, Range,
};

use crate::ui::kit_sources::units::setting_label;

const SLIDER_STEPS: f64 = 100.0;

#[must_use]
pub fn gain_label(stage: &GainStage) -> String {
    let named = match stage.kind {
        GainKind::Lna => "LNA",
        GainKind::Mixer => "Mixer",
        GainKind::Vga => "VGA",
        GainKind::If => "IF",
        GainKind::Rf => "RF",
        GainKind::Tuner => "Tuner",
        GainKind::Amp => "Amp",
        GainKind::Attenuator => "Attenuator",
        GainKind::Tx => "TX",
        GainKind::Other => "",
    };
    if named.is_empty() {
        setting_label(&stage.name)
    } else {
        named.to_owned()
    }
}

#[must_use]
pub fn gain_unit(unit: GainUnit) -> &'static str {
    match unit {
        GainUnit::Index => "",
        GainUnit::Db => "dB",
    }
}

#[must_use]
pub fn format_gain(unit: GainUnit, value: f64) -> String {
    match unit {
        GainUnit::Index => format!("{}", value.round()),
        GainUnit::Db => format!("{value:.1}"),
    }
}

#[must_use]
pub fn stage_settings(stage: &GainStage) -> Vec<f64> {
    if !stage.values.is_empty() {
        let mut sorted = stage.values.clone();
        sorted.sort_by(f64::total_cmp);
        return sorted;
    }
    let Some(step) = stage.range.step.filter(|step| *step > 0.0) else {
        return Vec::new();
    };
    let mut settings = Vec::new();
    let mut value = stage.range.min;
    while value <= stage.range.max + step / 2.0 {
        settings.push(value.min(stage.range.max));
        value += step;
    }
    settings
}

#[must_use]
pub fn snap_to_stage(stage: &GainStage, db: f64) -> f64 {
    let clamped = db.clamp(stage.range.min, stage.range.max.max(stage.range.min));
    stage_settings(stage)
        .into_iter()
        .fold(None, |best: Option<f64>, setting| match best {
            Some(best) if (best - clamped).abs() <= (setting - clamped).abs() => Some(best),
            _ => Some(setting),
        })
        .unwrap_or(clamped)
}

#[must_use]
pub fn setting_index(settings: &[f64], db: f64) -> usize {
    let mut best = 0;
    for (index, candidate) in settings.iter().enumerate() {
        if (candidate - db).abs() < (settings[best] - db).abs() {
            best = index;
        }
    }
    best
}

#[must_use]
pub fn fits_slider(range: &Range) -> bool {
    match range.step {
        Some(step) if step > 0.0 && range.max > range.min => {
            (range.max - range.min) / step <= SLIDER_STEPS
        }
        _ => false,
    }
}

#[must_use]
pub fn span_of(ranges: &[Range]) -> Option<Range> {
    let first = ranges.first()?;
    let steps: Vec<f64> = ranges.iter().filter_map(|range| range.step).collect();
    Some(Range {
        min: ranges.iter().map(|r| r.min).fold(first.min, f64::min),
        max: ranges.iter().map(|r| r.max).fold(first.max, f64::max),
        step: (steps.len() == ranges.len())
            .then(|| steps.iter().copied().fold(f64::INFINITY, f64::min)),
    })
}

#[must_use]
pub fn snap_to_ranges(ranges: &[Range], value: f64) -> f64 {
    let Some(first) = ranges.first() else {
        return value;
    };
    let held = |range: &Range| value.clamp(range.min, range.max.max(range.min));
    ranges.iter().fold(held(first), |best, range| {
        let candidate = held(range);
        if (candidate - value).abs() < (best - value).abs() {
            candidate
        } else {
            best
        }
    })
}

#[must_use]
pub fn has_dc_artifact(caps: &Capabilities) -> bool {
    caps.dc_artifact != DcArtifact::None
}

#[must_use]
pub fn dc_block_on(caps: &Capabilities, settings: &DeviceSettings) -> bool {
    settings
        .dc_block
        .unwrap_or(caps.dc_artifact == DcArtifact::Managed)
}

#[must_use]
pub fn agc_state(caps: &Capabilities, settings: &DeviceSettings) -> AgcSetting {
    let reported = settings.agc.as_ref();
    AgcSetting {
        on: caps.agc.offered() && reported.is_some_and(|agc| agc.on),
        mode: reported
            .and_then(|agc| agc.mode.clone())
            .or_else(|| caps.agc.first_mode().map(str::to_owned)),
    }
}

#[must_use]
pub fn automatic_gain_is_on(caps: &Capabilities, settings: &DeviceSettings) -> bool {
    agc_state(caps, settings).on
}

#[must_use]
pub fn has_filter(caps: &Capabilities) -> bool {
    caps.bandwidth_auto || !caps.bandwidths.is_empty() || !caps.bandwidth_ranges.is_empty()
}

#[must_use]
pub fn filter_is_auto(settings: &DeviceSettings) -> bool {
    settings.bandwidth.is_some_and(BandwidthSetting::is_auto)
}

#[must_use]
pub fn filter_hz(caps: &Capabilities, settings: &DeviceSettings) -> f64 {
    if let Some(BandwidthSetting::Manual { hz }) = settings.bandwidth {
        return hz;
    }
    let rate = settings.sample_rate.unwrap_or(0.0);
    let mut menu = caps.bandwidths.clone();
    menu.sort_by(f64::total_cmp);
    if let Some(wide_enough) = menu.iter().copied().find(|hz| *hz >= rate) {
        return wide_enough;
    }
    if let Some(widest) = menu.last() {
        return *widest;
    }
    snap_to_ranges(&caps.bandwidth_ranges, rate)
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::device::{Agc, ArgumentOption};

    use super::*;

    fn range(min: f64, max: f64, step: Option<f64>) -> Range {
        Range { min, max, step }
    }

    fn stage(range: Range, values: Vec<f64>, kind: GainKind) -> GainStage {
        GainStage::named("TEST", kind, range).with_values(values)
    }

    fn tuner() -> GainStage {
        stage(
            range(0.0, 49.6, None),
            vec![
                0.0, 0.9, 1.4, 2.7, 3.7, 7.7, 8.7, 12.5, 14.4, 15.7, 16.6, 19.7, 20.7, 22.9, 25.4,
                28.0, 29.7, 32.8, 33.8, 36.4, 37.2, 38.6, 40.2, 42.1, 43.4, 43.9, 44.5, 48.0, 49.6,
            ],
            GainKind::Lna,
        )
    }

    pub(crate) fn caps() -> Capabilities {
        serde_json::from_value(serde_json::json!({
            "freq_ranges": [], "sample_rates": [], "gains": [], "antennas": [], "bandwidths": []
        }))
        .expect("bare capabilities")
    }

    #[test]
    fn an_amp_is_a_switch_and_nothing_else_is() {
        assert!(stage(range(0.0, 14.0, Some(14.0)), vec![], GainKind::Amp).is_switch());
        assert!(!stage(range(0.0, 14.0, Some(14.0)), vec![], GainKind::Lna).is_switch());
        assert!(!tuner().is_switch());
    }

    #[test]
    fn a_stage_is_named_by_its_kind_and_falls_back_to_the_hardware_name() {
        assert_eq!(
            gain_label(&GainStage::named(
                "LNA",
                GainKind::Lna,
                range(0.0, 1.0, None)
            )),
            "LNA"
        );
        assert_eq!(
            gain_label(&GainStage::named(
                "MIX",
                GainKind::Mixer,
                range(0.0, 1.0, None)
            )),
            "Mixer"
        );
        assert_eq!(
            gain_label(&GainStage::named(
                "rxvga1",
                GainKind::Other,
                range(0.0, 1.0, None)
            )),
            "rxvga1"
        );
    }

    #[test]
    fn a_firmware_step_reads_without_a_unit() {
        assert_eq!(gain_unit(GainUnit::Index), "");
        assert_eq!(gain_unit(GainUnit::Db), "dB");
        assert_eq!(format_gain(GainUnit::Index, 7.4), "7");
        assert_eq!(format_gain(GainUnit::Db, 7.4), "7.4");
    }

    #[test]
    fn stage_settings_walk_an_even_step_or_a_table_and_skip_a_continuous_stage() {
        assert_eq!(
            stage_settings(&stage(range(0.0, 40.0, Some(8.0)), vec![], GainKind::Lna)),
            vec![0.0, 8.0, 16.0, 24.0, 32.0, 40.0]
        );
        assert_eq!(
            stage_settings(&stage(
                range(0.0, 14.0, None),
                vec![14.0, 0.0],
                GainKind::Lna
            )),
            vec![0.0, 14.0]
        );
        assert!(stage_settings(&stage(range(0.0, 10.0, None), vec![], GainKind::Lna)).is_empty());
    }

    #[test]
    fn a_snap_lands_on_a_setting_the_radio_holds_and_never_raises_gain_on_a_tie() {
        let tuner = tuner();
        assert_eq!(snap_to_stage(&tuner, 20.0), 19.7);
        assert_eq!(snap_to_stage(&tuner, 19.7), 19.7);
        assert_eq!(snap_to_stage(&tuner, 21.0), 20.7);
        assert_eq!(snap_to_stage(&tuner, -5.0), 0.0);
        assert_eq!(snap_to_stage(&tuner, 1000.0), 49.6);
        let even = stage(range(0.0, 20.0, None), vec![0.0, 10.0, 20.0], GainKind::Lna);
        assert_eq!(snap_to_stage(&even, 15.0), 10.0);
        let free = stage(range(0.0, 10.0, None), vec![], GainKind::Lna);
        assert_eq!(snap_to_stage(&free, 3.7), 3.7);
        assert_eq!(snap_to_stage(&free, 11.0), 10.0);
    }

    #[test]
    fn a_setting_index_addresses_the_nearest_setting() {
        let settings = stage_settings(&tuner());
        assert_eq!(settings[setting_index(&settings, 19.7)], 19.7);
        assert_eq!(settings[setting_index(&settings, 20.0)], 19.7);
        assert_eq!(settings[setting_index(&settings, -99.0)], 0.0);
        assert_eq!(settings[setting_index(&settings, 99.0)], 49.6);
    }

    #[test]
    fn a_span_covers_every_window_and_keeps_a_step_only_when_all_agree() {
        let windows = [
            range(225_001.0, 300_000.0, None),
            range(900_001.0, 3_200_000.0, None),
        ];
        assert_eq!(span_of(&windows), Some(range(225_001.0, 3_200_000.0, None)));
        assert_eq!(
            span_of(&[range(2e6, 20e6, Some(1000.0))]),
            Some(range(2e6, 20e6, Some(1000.0)))
        );
        assert_eq!(
            span_of(&[range(0.0, 1.0, Some(1.0)), range(2.0, 3.0, None)]).and_then(|r| r.step),
            None
        );
        assert_eq!(span_of(&[]), None);
    }

    #[test]
    fn a_value_in_a_gap_moves_to_the_nearest_edge() {
        let windows = [
            range(225_001.0, 300_000.0, None),
            range(900_001.0, 3_200_000.0, None),
        ];
        assert_eq!(snap_to_ranges(&windows, 250_000.0), 250_000.0);
        assert_eq!(snap_to_ranges(&windows, 2_048_000.0), 2_048_000.0);
        assert_eq!(snap_to_ranges(&windows, 400_000.0), 300_000.0);
        assert_eq!(snap_to_ranges(&windows, 800_000.0), 900_001.0);
        assert_eq!(snap_to_ranges(&windows, 1000.0), 225_001.0);
        assert_eq!(snap_to_ranges(&windows, 9e9), 3_200_000.0);
        assert_eq!(snap_to_ranges(&[], 500.0), 500.0);
    }

    #[test]
    fn the_dc_blocker_is_offered_to_a_front_end_and_starts_on_where_managed() {
        let mut caps = caps();
        let settings = DeviceSettings::default();
        caps.dc_artifact = DcArtifact::Operator;
        assert!(has_dc_artifact(&caps));
        assert!(!dc_block_on(&caps, &settings));
        caps.dc_artifact = DcArtifact::Managed;
        assert!(has_dc_artifact(&caps));
        assert!(dc_block_on(&caps, &settings));
        assert!(!dc_block_on(
            &caps,
            &DeviceSettings {
                dc_block: Some(false),
                ..DeviceSettings::default()
            }
        ));
        caps.dc_artifact = DcArtifact::None;
        assert!(!has_dc_artifact(&caps));
    }

    #[test]
    fn automatic_gain_reads_the_radio_and_offers_the_first_mode() {
        let mut switch = caps();
        switch.agc = Agc::Switch;
        let on = DeviceSettings {
            agc: Some(AgcSetting::switched(true)),
            ..DeviceSettings::default()
        };
        let off = DeviceSettings {
            agc: Some(AgcSetting::switched(false)),
            ..DeviceSettings::default()
        };
        assert!(automatic_gain_is_on(&switch, &on));
        assert!(!automatic_gain_is_on(&switch, &off));
        assert!(!automatic_gain_is_on(&caps(), &on));
        let mut modes = caps();
        modes.agc = Agc::Modes {
            options: vec![ArgumentOption::plain("fast"), ArgumentOption::plain("slow")],
        };
        assert!(!automatic_gain_is_on(&modes, &DeviceSettings::default()));
        assert_eq!(
            agc_state(&modes, &DeviceSettings::default()),
            AgcSetting::in_mode(false, "fast")
        );
        let slow = DeviceSettings {
            agc: Some(AgcSetting::in_mode(true, "slow")),
            ..DeviceSettings::default()
        };
        assert_eq!(agc_state(&modes, &slow), AgcSetting::in_mode(true, "slow"));
        assert_eq!(agc_state(&switch, &on), AgcSetting::switched(true));
    }

    #[test]
    fn a_filter_shows_the_manual_width_or_the_narrowest_that_covers_the_rate() {
        let mut menu = caps();
        menu.bandwidths = vec![1.75e6, 2.5e6, 5e6];
        assert!(has_filter(&menu));
        let mut auto = caps();
        auto.bandwidth_auto = true;
        assert!(has_filter(&auto));
        let mut ranged = caps();
        ranged.bandwidth_ranges = vec![range(290e3, 8e6, None)];
        assert!(has_filter(&ranged));
        assert!(!has_filter(&caps()));
        let manual = DeviceSettings {
            bandwidth: Some(BandwidthSetting::Manual { hz: 5e6 }),
            ..DeviceSettings::default()
        };
        assert_eq!(filter_hz(&menu, &manual), 5e6);
        let under_auto = DeviceSettings {
            bandwidth: Some(BandwidthSetting::Auto),
            sample_rate: Some(2.4e6),
            ..DeviceSettings::default()
        };
        assert_eq!(filter_hz(&menu, &under_auto), 2.5e6);
        let fast = DeviceSettings {
            sample_rate: Some(20e6),
            ..DeviceSettings::default()
        };
        assert_eq!(filter_hz(&menu, &fast), 5e6);
        let rate = DeviceSettings {
            sample_rate: Some(2.4e6),
            ..DeviceSettings::default()
        };
        assert_eq!(filter_hz(&ranged, &rate), 2.4e6);
    }

    #[test]
    fn a_short_stepped_range_fits_a_slider_and_a_wide_one_does_not() {
        assert!(fits_slider(&range(0.0, 9.0, Some(1.0))));
        assert!(fits_slider(&range(0.0, 100.0, Some(1.0))));
        assert!(!fits_slider(&range(1e3, 2e9, Some(1.0))));
        assert!(!fits_slider(&range(0.0, 200e3, Some(1.0))));
        assert!(!fits_slider(&range(0.0, 10.0, None)));
        assert!(!fits_slider(&range(0.0, 10.0, Some(0.0))));
        assert!(!fits_slider(&range(5.0, 5.0, Some(1.0))));
    }
}
