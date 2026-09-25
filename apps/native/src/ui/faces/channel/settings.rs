use sdrmm_wire::{
    audio::{AudioProcessing, MAX_AUDIO_NOTCHES, NotchSettings},
    channel::{
        ChannelDescriptor, ChannelParams, MAX_SQUELCH_AUTO_MARGIN_DB, MIN_SQUELCH_AUTO_MARGIN_DB,
        ParamLimit, Squelch,
    },
};

pub const SQUELCH_MIN_DB: f32 = -120.0;
pub const SQUELCH_MAX_DB: f32 = 0.0;
pub const DEFAULT_SQUELCH_DB: f32 = -60.0;
pub const DEFAULT_SQUELCH_MARGIN_DB: f32 = 8.0;
pub const DEFAULT_NOTCH_FREQ_HZ: f64 = 1_000.0;
pub const DEFAULT_NOTCH_WIDTH_HZ: f64 = 100.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SquelchMode {
    Off,
    Manual,
    Auto,
}

#[must_use]
pub fn squelch_mode(squelch: &Squelch) -> SquelchMode {
    match squelch {
        Squelch::Off => SquelchMode::Off,
        Squelch::Manual { .. } => SquelchMode::Manual,
        Squelch::Auto { .. } => SquelchMode::Auto,
    }
}

#[must_use]
pub fn squelch_at(mode: SquelchMode, level_db: f32, margin_db: f32) -> Squelch {
    match mode {
        SquelchMode::Off => Squelch::Off,
        SquelchMode::Manual => Squelch::Manual { level_db },
        SquelchMode::Auto => Squelch::Auto { margin_db },
    }
}

#[must_use]
pub fn nudged_squelch(squelch: &Squelch, delta_db: f32) -> Squelch {
    if let Squelch::Auto { margin_db } = squelch {
        return Squelch::Auto {
            margin_db: (margin_db + delta_db)
                .clamp(MIN_SQUELCH_AUTO_MARGIN_DB, MAX_SQUELCH_AUTO_MARGIN_DB),
        };
    }
    let level = squelch.manual_level_db().unwrap_or(DEFAULT_SQUELCH_DB) + delta_db;
    Squelch::Manual {
        level_db: level.clamp(SQUELCH_MIN_DB, SQUELCH_MAX_DB),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NumberLimit {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
}

impl NumberLimit {
    #[must_use]
    pub const fn new(min: f64, max: f64, step: f64) -> Self {
        Self {
            min: Some(min),
            max: Some(max),
            step: Some(step),
        }
    }

    #[must_use]
    pub fn clamp(self, value: f64) -> f64 {
        let low = self.min.map_or(value, |min| value.max(min));
        self.max.map_or(low, |max| low.min(max))
    }
}

#[must_use]
pub fn limit_of(limits: &[ParamLimit], name: &str) -> NumberLimit {
    limits
        .iter()
        .find(|limit| limit.name == name)
        .map_or_else(NumberLimit::default, |limit| NumberLimit {
            min: Some(limit.min),
            max: Some(limit.max),
            step: limit.step,
        })
}

#[must_use]
pub fn scaled_limit(limit: NumberLimit, factor: f64) -> NumberLimit {
    NumberLimit {
        min: limit.min.map(|min| min * factor),
        max: limit.max.map(|max| max * factor),
        step: limit.step.map(|step| step * factor),
    }
}

#[must_use]
pub fn with_notch_added(notches: &[NotchSettings]) -> Option<Vec<NotchSettings>> {
    if notches.len() >= MAX_AUDIO_NOTCHES {
        return None;
    }
    let mut added = notches.to_vec();
    added.push(NotchSettings {
        freq_hz: DEFAULT_NOTCH_FREQ_HZ,
        width_hz: DEFAULT_NOTCH_WIDTH_HZ,
    });
    Some(added)
}

#[must_use]
pub fn with_notch_at(
    notches: &[NotchSettings],
    index: usize,
    edit: impl FnOnce(&mut NotchSettings),
) -> Vec<NotchSettings> {
    let mut edited = notches.to_vec();
    if let Some(notch) = edited.get_mut(index) {
        edit(notch);
    }
    edited
}

#[must_use]
pub fn with_notch_removed(notches: &[NotchSettings], index: usize) -> Vec<NotchSettings> {
    notches
        .iter()
        .enumerate()
        .filter(|(at, _)| *at != index)
        .map(|(_, notch)| *notch)
        .collect()
}

#[must_use]
pub fn audio_chain_active(audio: Option<&AudioProcessing>) -> bool {
    audio.is_some_and(AudioProcessing::is_active)
}

#[must_use]
pub fn channel_has_audio(descriptor: Option<&ChannelDescriptor>) -> bool {
    descriptor.is_none_or(|descriptor| descriptor.has_audio)
}

#[must_use]
pub fn channel_decoder_kind(descriptor: Option<&ChannelDescriptor>) -> Option<&str> {
    descriptor.and_then(|descriptor| descriptor.decoder_kind.as_deref())
}

#[must_use]
pub fn channel_has_video(descriptor: Option<&ChannelDescriptor>) -> bool {
    descriptor.is_some_and(|descriptor| descriptor.has_video)
}

#[must_use]
pub fn keeps_calls(descriptor: Option<&ChannelDescriptor>) -> bool {
    channel_decoder_kind(descriptor) == Some(sdrmm_wire::patch::DV_DECODER_KIND)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RadioWindow {
    pub low_hz: f64,
    pub high_hz: f64,
}

#[must_use]
pub fn radio_window_hz(
    center_hz: Option<f64>,
    span_hz: Option<f64>,
    descriptor: Option<&ChannelDescriptor>,
) -> Option<RadioWindow> {
    let limit = offset_limit_hz(span_hz, descriptor)?;
    let center = center_hz.filter(|center| center.is_finite())?;
    Some(RadioWindow {
        low_hz: center - limit,
        high_hz: center + limit,
    })
}

#[must_use]
pub fn reaches_hz(frequency_hz: f64, window: Option<RadioWindow>) -> bool {
    window.is_none_or(|window| frequency_hz >= window.low_hz && frequency_hz <= window.high_hz)
}

#[must_use]
pub fn param_bandwidth_hz(params: &ChannelParams) -> Option<f64> {
    serde_json::to_value(params)
        .ok()?
        .pointer("/settings/bandwidth_hz")?
        .as_f64()
}

#[must_use]
pub fn channel_width_hz(
    params: Option<&ChannelParams>,
    descriptor: Option<&ChannelDescriptor>,
) -> Option<f64> {
    params
        .and_then(param_bandwidth_hz)
        .or_else(|| descriptor.map(|descriptor| descriptor.bandwidth_hz))
        .filter(|width| width.is_finite() && *width > 0.0)
}

#[must_use]
pub fn offset_limit_hz(
    span_hz: Option<f64>,
    descriptor: Option<&ChannelDescriptor>,
) -> Option<f64> {
    let span = span_hz.filter(|span| span.is_finite() && *span > 0.0)?;
    let width = descriptor.map_or(0.0, |descriptor| descriptor.bandwidth_hz);
    Some(((span - width) / 2.0).max(0.0))
}

#[must_use]
pub fn clamp_offset_hz(hz: f64, limit_hz: Option<f64>) -> f64 {
    limit_hz.map_or(hz, |limit| hz.clamp(-limit, limit))
}

#[must_use]
pub fn offset_for_frequency_hz(
    frequency_hz: f64,
    center_hz: f64,
    limit_hz: Option<f64>,
) -> Option<f64> {
    if !frequency_hz.is_finite() || !center_hz.is_finite() {
        return None;
    }
    let offset = (frequency_hz - center_hz).round();
    match limit_hz {
        Some(limit) if offset.abs() > limit => None,
        _ => Some(offset),
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{
        audio::{AudioAgcMode, AudioFilterSettings, ClickRemovalSettings, DenoiseSettings},
        channel::{AmParams, ChannelParams},
    };

    use super::*;

    fn descriptor(bandwidth_hz: f64) -> ChannelDescriptor {
        ChannelDescriptor {
            type_id: "nfm".into(),
            name: "NFM".into(),
            bandwidth_hz,
            input_rate_hz: 48_000.0,
            ..ChannelDescriptor::default()
        }
    }

    #[test]
    fn audio_controls_follow_the_descriptor_and_assume_audio_when_unknown() {
        let mut data = descriptor(12_500.0);
        data.has_audio = false;
        data.decoder_kind = Some("adsb".into());
        assert!(!channel_has_audio(Some(&data)));
        assert!(channel_has_audio(None));
        assert!(channel_has_audio(Some(&descriptor(12_500.0))));
    }

    #[test]
    fn the_decoder_kind_is_what_the_channel_emits() {
        let mut pager = descriptor(12_500.0);
        pager.decoder_kind = Some("pocsag".into());
        assert_eq!(channel_decoder_kind(Some(&pager)), Some("pocsag"));
        assert_eq!(channel_decoder_kind(Some(&descriptor(0.0))), None);
        assert_eq!(channel_decoder_kind(None), None);
    }

    #[test]
    fn only_a_digital_voice_decoder_records_calls() {
        let mut dmr = descriptor(12_500.0);
        dmr.decoder_kind = Some("dv".into());
        assert!(keeps_calls(Some(&dmr)));
        assert!(!keeps_calls(Some(&descriptor(12_500.0))));
        assert!(!keeps_calls(None));
    }

    #[test]
    fn the_offset_limit_keeps_the_whole_passband_inside_the_span() {
        assert_eq!(
            offset_limit_hz(Some(2_400_000.0), Some(&descriptor(12_500.0))),
            Some(1_193_750.0)
        );
        assert_eq!(
            offset_limit_hz(Some(100_000.0), Some(&descriptor(150_000.0))),
            Some(0.0)
        );
        assert_eq!(offset_limit_hz(None, Some(&descriptor(0.0))), None);
        assert_eq!(offset_limit_hz(Some(0.0), Some(&descriptor(0.0))), None);
        assert_eq!(offset_limit_hz(Some(2_000_000.0), None), Some(1_000_000.0));
    }

    #[test]
    fn a_channel_width_prefers_its_own_setting_over_the_type() {
        let am = ChannelParams::Am(AmParams {
            bandwidth_hz: 8_000.0,
        });
        assert_eq!(
            channel_width_hz(Some(&am), Some(&descriptor(10_000.0))),
            Some(8_000.0)
        );
        assert_eq!(
            channel_width_hz(None, Some(&descriptor(12_500.0))),
            Some(12_500.0)
        );
        assert_eq!(channel_width_hz(None, Some(&descriptor(0.0))), None);
        assert_eq!(channel_width_hz(None, None), None);
    }

    #[test]
    fn the_radio_window_names_the_edges_a_channel_fits_between() {
        assert_eq!(
            radio_window_hz(
                Some(145_000_000.0),
                Some(2_400_000.0),
                Some(&descriptor(12_500.0))
            ),
            Some(RadioWindow {
                low_hz: 143_806_250.0,
                high_hz: 146_193_750.0
            })
        );
        assert_eq!(
            radio_window_hz(None, Some(2_400_000.0), Some(&descriptor(0.0))),
            None
        );
        assert_eq!(
            radio_window_hz(Some(145_000_000.0), None, Some(&descriptor(0.0))),
            None
        );
    }

    #[test]
    fn reach_holds_inside_the_window_and_while_there_is_none() {
        let window = Some(RadioWindow {
            low_hz: 143_806_250.0,
            high_hz: 146_193_750.0,
        });
        assert!(reaches_hz(145_000_000.0, window));
        assert!(reaches_hz(143_806_250.0, window));
        assert!(reaches_hz(146_193_750.0, window));
        assert!(!reaches_hz(143_000_000.0, window));
        assert!(!reaches_hz(147_000_000.0, window));
        assert!(reaches_hz(1_090_000_000.0, None));
    }

    #[test]
    fn an_offset_step_stops_at_the_edge_of_the_span() {
        assert_eq!(clamp_offset_hz(1_200_000.0, Some(1_193_750.0)), 1_193_750.0);
        assert_eq!(
            clamp_offset_hz(-1_200_000.0, Some(1_193_750.0)),
            -1_193_750.0
        );
        assert_eq!(clamp_offset_hz(-25_000.0, Some(1_193_750.0)), -25_000.0);
        assert_eq!(clamp_offset_hz(9_000_000.0, None), 9_000_000.0);
    }

    #[test]
    fn a_frequency_becomes_a_whole_hertz_offset_the_span_can_reach() {
        assert_eq!(
            offset_for_frequency_hz(433_920_000.0, 433_000_000.0, Some(1_000_000.0)),
            Some(920_000.0)
        );
        assert_eq!(
            offset_for_frequency_hz(432_500_000.0, 433_000_000.0, Some(1_000_000.0)),
            Some(-500_000.0)
        );
        assert_eq!(
            offset_for_frequency_hz(433_920_000.4, 433_000_000.0, None),
            Some(920_000.0)
        );
        assert_eq!(
            offset_for_frequency_hz(435_000_000.0, 433_000_000.0, Some(1_000_000.0)),
            None
        );
        assert_eq!(
            offset_for_frequency_hz(435_000_000.0, 433_000_000.0, None),
            Some(2_000_000.0)
        );
        assert_eq!(offset_for_frequency_hz(f64::NAN, 433_000_000.0, None), None);
        assert_eq!(
            offset_for_frequency_hz(433_000_000.0, f64::INFINITY, None),
            None
        );
    }

    #[test]
    fn the_squelch_mode_reads_off_the_tagged_value() {
        assert_eq!(squelch_mode(&Squelch::Off), SquelchMode::Off);
        assert_eq!(
            squelch_mode(&Squelch::Manual { level_db: -60.0 }),
            SquelchMode::Manual
        );
        assert_eq!(
            squelch_mode(&Squelch::Auto { margin_db: 8.0 }),
            SquelchMode::Auto
        );
    }

    #[test]
    fn each_squelch_mode_is_built_from_the_value_held_for_it() {
        assert_eq!(squelch_at(SquelchMode::Off, -55.0, 12.0), Squelch::Off);
        assert_eq!(
            squelch_at(SquelchMode::Manual, -55.0, 12.0),
            Squelch::Manual { level_db: -55.0 }
        );
        assert_eq!(
            squelch_at(SquelchMode::Auto, -55.0, 12.0),
            Squelch::Auto { margin_db: 12.0 }
        );
    }

    #[test]
    fn a_nudge_moves_the_level_or_the_margin_and_stays_in_range() {
        assert_eq!(
            nudged_squelch(&Squelch::Manual { level_db: -60.0 }, 2.0),
            Squelch::Manual { level_db: -58.0 }
        );
        assert_eq!(
            nudged_squelch(&Squelch::Manual { level_db: -1.0 }, 2.0),
            Squelch::Manual { level_db: 0.0 }
        );
        assert_eq!(
            nudged_squelch(&Squelch::Off, -2.0),
            Squelch::Manual { level_db: -62.0 }
        );
        assert_eq!(
            nudged_squelch(&Squelch::Auto { margin_db: 8.0 }, 2.0),
            Squelch::Auto { margin_db: 10.0 }
        );
        assert_eq!(
            nudged_squelch(&Squelch::Auto { margin_db: 39.0 }, 2.0),
            Squelch::Auto { margin_db: 40.0 }
        );
    }

    #[test]
    fn a_number_field_takes_the_range_its_decoder_published() {
        let limits = vec![
            ParamLimit {
                name: "wpm".into(),
                min: 5.0,
                max: 60.0,
                step: Some(1.0),
            },
            ParamLimit {
                name: "threshold".into(),
                min: 1.5,
                max: 100.0,
                step: None,
            },
        ];
        assert_eq!(limit_of(&limits, "wpm"), NumberLimit::new(5.0, 60.0, 1.0));
        assert_eq!(
            limit_of(&limits, "threshold"),
            NumberLimit {
                min: Some(1.5),
                max: Some(100.0),
                step: None
            }
        );
        assert_eq!(limit_of(&limits, "baud"), NumberLimit::default());
        assert_eq!(limit_of(&[], "wpm"), NumberLimit::default());
    }

    #[test]
    fn a_limit_scales_into_the_unit_a_field_shows() {
        let scaled = scaled_limit(NumberLimit::new(500_000.0, 9_000_000.0, 500_000.0), 1e-6);
        assert!((scaled.min.unwrap_or_default() - 0.5).abs() < 1e-9);
        assert!((scaled.max.unwrap_or_default() - 9.0).abs() < 1e-9);
        assert!((scaled.step.unwrap_or_default() - 0.5).abs() < 1e-9);
        assert_eq!(
            scaled_limit(NumberLimit::default(), 1e-6),
            NumberLimit::default()
        );
    }

    #[test]
    fn notches_append_at_the_defaults_until_the_channel_is_full() {
        let one = NotchSettings {
            freq_hz: 1_000.0,
            width_hz: 100.0,
        };
        assert_eq!(with_notch_added(&[]), Some(vec![one]));
        assert_eq!(with_notch_added(&vec![one; MAX_AUDIO_NOTCHES]), None);
        let two = [
            one,
            NotchSettings {
                freq_hz: 2_000.0,
                width_hz: 50.0,
            },
        ];
        assert_eq!(
            with_notch_at(&two, 1, |notch| notch.freq_hz = 2_500.0),
            vec![
                one,
                NotchSettings {
                    freq_hz: 2_500.0,
                    width_hz: 50.0
                }
            ]
        );
        assert_eq!(
            with_notch_removed(&two, 0),
            vec![NotchSettings {
                freq_hz: 2_000.0,
                width_hz: 50.0
            }]
        );
    }

    #[test]
    fn the_audio_chain_is_active_once_any_stage_works() {
        assert!(!audio_chain_active(None));
        assert!(!audio_chain_active(Some(&AudioProcessing::default())));
        let active = [
            AudioProcessing {
                agc: AudioAgcMode::Slow,
                ..AudioProcessing::default()
            },
            AudioProcessing {
                auto_notch: true,
                ..AudioProcessing::default()
            },
            AudioProcessing {
                denoise: DenoiseSettings {
                    enabled: true,
                    ..DenoiseSettings::default()
                },
                ..AudioProcessing::default()
            },
            AudioProcessing {
                filter: AudioFilterSettings {
                    enabled: true,
                    ..AudioFilterSettings::default()
                },
                ..AudioProcessing::default()
            },
            AudioProcessing {
                click_removal: ClickRemovalSettings {
                    enabled: true,
                    ..ClickRemovalSettings::default()
                },
                ..AudioProcessing::default()
            },
            AudioProcessing {
                notches: vec![NotchSettings::default()],
                ..AudioProcessing::default()
            },
        ];
        for audio in &active {
            assert!(audio_chain_active(Some(audio)), "{audio:?}");
        }
    }
}
