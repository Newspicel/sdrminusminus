use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const MAX_AUDIO_NOTCHES: usize = 4;

pub const MIN_BLANKER_THRESHOLD: f32 = 1.5;
pub const MAX_BLANKER_THRESHOLD: f32 = 20.0;
pub const MIN_CLICK_THRESHOLD: f32 = 2.0;
pub const MAX_CLICK_THRESHOLD: f32 = 20.0;
pub const MIN_AUDIO_TONE_HZ: f64 = 30.0;
pub const MAX_AUDIO_TONE_HZ: f64 = 20_000.0;
pub const MIN_NOTCH_WIDTH_HZ: f64 = 10.0;
pub const MAX_NOTCH_WIDTH_HZ: f64 = 2_000.0;

fn default_blanker_threshold() -> f32 {
    5.0
}

fn default_denoise_strength() -> f32 {
    0.5
}

fn default_click_threshold() -> f32 {
    6.0
}

fn default_filter_low_hz() -> f64 {
    300.0
}

fn default_filter_high_hz() -> f64 {
    3_000.0
}

fn default_notch_width_hz() -> f64 {
    100.0
}

fn default_notch_freq_hz() -> f64 {
    1_000.0
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct NoiseBlankerSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_blanker_threshold")]
    pub threshold: f32,
}

impl Default for NoiseBlankerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: default_blanker_threshold(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DenoiseSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_denoise_strength")]
    pub strength: f32,
}

impl Default for DenoiseSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            strength: default_denoise_strength(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ClickRemovalSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_click_threshold")]
    pub threshold: f32,
}

impl Default for ClickRemovalSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: default_click_threshold(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct NotchSettings {
    #[serde(default = "default_notch_freq_hz")]
    pub freq_hz: f64,
    #[serde(default = "default_notch_width_hz")]
    pub width_hz: f64,
}

impl Default for NotchSettings {
    fn default() -> Self {
        Self {
            freq_hz: default_notch_freq_hz(),
            width_hz: default_notch_width_hz(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AudioFilterSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_filter_low_hz")]
    pub low_hz: f64,
    #[serde(default = "default_filter_high_hz")]
    pub high_hz: f64,
}

impl Default for AudioFilterSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            low_hz: default_filter_low_hz(),
            high_hz: default_filter_high_hz(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AudioAgcMode {
    #[default]
    Off,
    Slow,
    Medium,
    Fast,
}

impl AudioAgcMode {
    #[must_use]
    pub fn time_constants_s(self) -> Option<(f32, f32)> {
        match self {
            Self::Off => None,
            Self::Slow => Some((0.01, 2.0)),
            Self::Medium => Some((0.005, 0.5)),
            Self::Fast => Some((0.002, 0.1)),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AudioProcessing {
    #[serde(default)]
    pub blanker: NoiseBlankerSettings,
    #[serde(default)]
    pub click_removal: ClickRemovalSettings,
    #[serde(default)]
    pub filter: AudioFilterSettings,
    #[serde(default)]
    pub notches: Vec<NotchSettings>,
    #[serde(default)]
    pub auto_notch: bool,
    #[serde(default)]
    pub denoise: DenoiseSettings,
    #[serde(default)]
    pub agc: AudioAgcMode,
}

impl AudioProcessing {
    #[must_use]
    pub fn default_for(type_id: &str) -> Self {
        Self {
            agc: match type_id {
                "am" | "ssb" => AudioAgcMode::Medium,
                _ => AudioAgcMode::Off,
            },
            ..Self::default()
        }
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.blanker.enabled
            || self.click_removal.enabled
            || self.filter.enabled
            || !self.notches.is_empty()
            || self.auto_notch
            || self.denoise.enabled
            || self.agc != AudioAgcMode::Off
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.blanker.threshold.is_finite()
            || !(MIN_BLANKER_THRESHOLD..=MAX_BLANKER_THRESHOLD).contains(&self.blanker.threshold)
        {
            return Err(format!(
                "noise blanker threshold must be in {MIN_BLANKER_THRESHOLD}..={MAX_BLANKER_THRESHOLD}, got {}",
                self.blanker.threshold
            ));
        }
        if !self.click_removal.threshold.is_finite()
            || !(MIN_CLICK_THRESHOLD..=MAX_CLICK_THRESHOLD).contains(&self.click_removal.threshold)
        {
            return Err(format!(
                "click removal threshold must be in {MIN_CLICK_THRESHOLD}..={MAX_CLICK_THRESHOLD}, got {}",
                self.click_removal.threshold
            ));
        }
        if !self.denoise.strength.is_finite() || !(0.0..=1.0).contains(&self.denoise.strength) {
            return Err(format!(
                "noise reduction strength must be in 0.0..=1.0, got {}",
                self.denoise.strength
            ));
        }
        check_tone_hz("audio filter low cut", self.filter.low_hz)?;
        check_tone_hz("audio filter high cut", self.filter.high_hz)?;
        if self.filter.low_hz >= self.filter.high_hz {
            return Err(format!(
                "audio filter low cut {} Hz must sit below its high cut {} Hz",
                self.filter.low_hz, self.filter.high_hz
            ));
        }
        if self.notches.len() > MAX_AUDIO_NOTCHES {
            return Err(format!(
                "a channel carries at most {MAX_AUDIO_NOTCHES} notches, got {}",
                self.notches.len()
            ));
        }
        for notch in &self.notches {
            check_tone_hz("notch frequency", notch.freq_hz)?;
            if !notch.width_hz.is_finite()
                || !(MIN_NOTCH_WIDTH_HZ..=MAX_NOTCH_WIDTH_HZ).contains(&notch.width_hz)
            {
                return Err(format!(
                    "notch width must be in {MIN_NOTCH_WIDTH_HZ}..={MAX_NOTCH_WIDTH_HZ} Hz, got {}",
                    notch.width_hz
                ));
            }
        }
        Ok(())
    }
}

fn check_tone_hz(what: &str, hz: f64) -> Result<(), String> {
    if hz.is_finite() && (MIN_AUDIO_TONE_HZ..=MAX_AUDIO_TONE_HZ).contains(&hz) {
        Ok(())
    } else {
        Err(format!(
            "{what} must be in {MIN_AUDIO_TONE_HZ}..={MAX_AUDIO_TONE_HZ} Hz, got {hz}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_object_is_every_stage_off() {
        let parsed: AudioProcessing = serde_json::from_str("{}").expect("defaults parse");
        assert_eq!(parsed, AudioProcessing::default());
        assert!(!parsed.is_active());
        parsed.validate().expect("the default chain is valid");
    }

    #[test]
    fn only_the_modes_with_no_level_control_of_their_own_start_with_agc() {
        assert_eq!(AudioProcessing::default_for("am").agc, AudioAgcMode::Medium);
        assert_eq!(
            AudioProcessing::default_for("ssb").agc,
            AudioAgcMode::Medium
        );
        assert_eq!(AudioProcessing::default_for("nfm").agc, AudioAgcMode::Off);
        assert_eq!(AudioProcessing::default_for("wfm").agc, AudioAgcMode::Off);
        assert!(AudioProcessing::default_for("am").is_active());
        assert!(!AudioProcessing::default_for("nfm").is_active());
    }

    #[test]
    fn every_stage_counts_as_active_on_its_own() {
        let stages: [fn(&mut AudioProcessing); 7] = [
            |a| a.blanker.enabled = true,
            |a| a.click_removal.enabled = true,
            |a| a.filter.enabled = true,
            |a| a.notches.push(NotchSettings::default()),
            |a| a.auto_notch = true,
            |a| a.denoise.enabled = true,
            |a| a.agc = AudioAgcMode::Slow,
        ];
        for stage in stages {
            let mut chain = AudioProcessing::default();
            stage(&mut chain);
            assert!(chain.is_active(), "{chain:?}");
            chain.validate().expect("a stage's own default is valid");
        }
    }

    type Break = (fn(&mut AudioProcessing), &'static str);

    #[test]
    fn out_of_range_settings_are_named_rather_than_clamped() {
        let bad: [Break; 9] = [
            (|a| a.blanker.threshold = 0.5, "blanker"),
            (|a| a.blanker.threshold = f32::NAN, "blanker nan"),
            (|a| a.click_removal.threshold = 1.0, "click threshold"),
            (|a| a.click_removal.threshold = f32::NAN, "click nan"),
            (|a| a.denoise.strength = 1.5, "strength"),
            (|a| a.filter.low_hz = 5.0, "low cut"),
            (|a| a.filter.high_hz = 200.0, "crossed cuts"),
            (
                |a| a.notches = vec![NotchSettings::default(); MAX_AUDIO_NOTCHES + 1],
                "too many notches",
            ),
            (
                |a| {
                    a.notches = vec![NotchSettings {
                        freq_hz: 1_000.0,
                        width_hz: 5_000.0,
                    }];
                },
                "notch width",
            ),
        ];
        for (break_it, what) in bad {
            let mut chain = AudioProcessing::default();
            break_it(&mut chain);
            assert!(chain.validate().is_err(), "{what} was accepted");
        }
    }

    #[test]
    fn agc_speeds_are_ordered_and_off_has_none() {
        assert_eq!(AudioAgcMode::Off.time_constants_s(), None);
        let releases: Vec<f32> = [AudioAgcMode::Slow, AudioAgcMode::Medium, AudioAgcMode::Fast]
            .iter()
            .map(|m| m.time_constants_s().expect("a speed has constants").1)
            .collect();
        assert!(
            releases[0] > releases[1] && releases[1] > releases[2],
            "{releases:?}"
        );
    }
}
