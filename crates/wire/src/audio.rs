//! The audio processing chain every voice channel carries: blanker, notches, filters, noise
//! reduction and AGC as settings on the channel rather than as a channel type of their own.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Manual notches a channel may carry at once. Four covers the carriers a crowded band throws
/// at one passband; past that the answer is a narrower filter, not a longer list.
pub const MAX_AUDIO_NOTCHES: usize = 4;

pub const MIN_BLANKER_THRESHOLD: f32 = 1.5;
pub const MAX_BLANKER_THRESHOLD: f32 = 20.0;
/// Voice-band limits for everything tuned in the audio domain, at the 48 kHz PCM rate every
/// channel produces.
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

/// Impulse blanker, applied to the channel's IQ ahead of its selectivity filter.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct NoiseBlankerSettings {
    #[serde(default)]
    pub enabled: bool,
    /// How far above the channel's average magnitude a sample has to sit to be cut out, as a
    /// multiple of it. Lower blanks more, and eventually blanks the signal.
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

/// Spectral noise reduction.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DenoiseSettings {
    #[serde(default)]
    pub enabled: bool,
    /// How much of the tracked noise floor is subtracted, `0.0..=1.0`. Past the middle of the
    /// range the residue starts to sound processed, which is a taste, not a fault.
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

/// One operator-placed notch in the audio.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct NotchSettings {
    #[serde(default = "default_notch_freq_hz")]
    pub freq_hz: f64,
    /// −3 dB width of the null. Narrow removes a heterodyne and leaves the voice; wide takes a
    /// bite out of both.
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

/// Adjustable audio passband — the "narrow the filter until only the voice is left" control,
/// after demodulation rather than in the RF channel.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AudioFilterSettings {
    #[serde(default)]
    pub enabled: bool,
    /// Low cut. Rumble, hum and the subaudible band live below this.
    #[serde(default = "default_filter_low_hz")]
    pub low_hz: f64,
    /// High cut. Hiss and splatter live above it.
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

/// How hard the audio AGC rides the level. The speeds are the ones a radio's own switch offers,
/// because they are the ones that suit the modes: slow for SSB speech, fast for a band being
/// tuned across, medium for everything else.
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
    /// Attack and release time constants in seconds. Attack is short in every mode — the point
    /// of an AGC is that a strong station never arrives at full scale — and it is the release
    /// that the switch actually chooses between.
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

/// Everything a voice channel does to its audio after the demodulator, plus the one stage that
/// has to run before it.
///
/// Stages run in the order they are declared: the blanker on IQ, then passband, notches,
/// adaptive notch, noise reduction and AGC on the demodulated audio. Filtering first keeps the
/// junk outside the passband out of everything downstream, and the AGC runs last so what it
/// levels is what the listener actually hears.
///
/// Every stage is off by default, which is what a channel had before this existed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AudioProcessing {
    #[serde(default)]
    pub blanker: NoiseBlankerSettings,
    #[serde(default)]
    pub filter: AudioFilterSettings,
    #[serde(default)]
    pub notches: Vec<NotchSettings>,
    /// Remove steady carriers without being told where they are.
    #[serde(default)]
    pub auto_notch: bool,
    #[serde(default)]
    pub denoise: DenoiseSettings,
    #[serde(default)]
    pub agc: AudioAgcMode,
}

impl AudioProcessing {
    /// The chain a newly created channel of `type_id` starts with. AM and SSB carry no level
    /// control of their own — an envelope and a product detector both hand over whatever the
    /// band gave them — so they start with the AGC that a receiver would have had switched on.
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

    /// Whether any stage would do something, which is what decides if the chain is built at all.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.blanker.enabled
            || self.filter.enabled
            || !self.notches.is_empty()
            || self.auto_notch
            || self.denoise.enabled
            || self.agc != AudioAgcMode::Off
    }

    /// # Errors
    /// Describes the first setting outside the range its control offers. A number that reached
    /// the DSP thread unchecked would be a filter with a pole outside the unit circle.
    pub fn validate(&self) -> Result<(), String> {
        if !self.blanker.threshold.is_finite()
            || !(MIN_BLANKER_THRESHOLD..=MAX_BLANKER_THRESHOLD).contains(&self.blanker.threshold)
        {
            return Err(format!(
                "noise blanker threshold must be in {MIN_BLANKER_THRESHOLD}..={MAX_BLANKER_THRESHOLD}, got {}",
                self.blanker.threshold
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

    /// The one place a mode's own habits leak into the shared chain, and the reason it is a
    /// function rather than a `Default`.
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
        let stages: [fn(&mut AudioProcessing); 6] = [
            |a| a.blanker.enabled = true,
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

    /// One way of breaking a chain, and the name it goes wrong under.
    type Break = (fn(&mut AudioProcessing), &'static str);

    #[test]
    fn out_of_range_settings_are_named_rather_than_clamped() {
        let bad: [Break; 7] = [
            (|a| a.blanker.threshold = 0.5, "blanker"),
            (|a| a.blanker.threshold = f32::NAN, "blanker nan"),
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
