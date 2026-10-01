use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{
    Agc, AgcSetting, ArgumentOption, BandwidthSetting, Capabilities, DcArtifact, DeviceSettings,
    Duplex, ExtraSetting, ExtraValue, GainKind, GainStage, GainUnit, GainValue, Range, StreamScope,
};

use crate::proto::{Bits, Features, Identity, Limits, MIN_SAMPLES};

pub(crate) const BITS: &str = "bits";
pub(crate) const BURST: &str = "burst";
const GAIN_STAGE: &str = "RX";
const MHZ: f64 = 1e6;
const DEFAULT_CENTER_MHZ: u32 = 2437;
const DEFAULT_MANUAL_GAIN: u32 = 40;

#[derive(Clone, Debug)]
pub(crate) struct Profile {
    pub(crate) identity: Identity,
    pub(crate) features: Features,
    pub(crate) limits: Limits,
    pub(crate) range_mhz: (u32, u32),
}

impl Profile {
    fn has_gain(&self) -> bool {
        self.features.has("GAIN")
    }

    fn has_agc(&self) -> bool {
        self.features.has("HWAGC")
    }

    pub(crate) fn capabilities(&self) -> Capabilities {
        let limits = &self.limits;
        let (low, high) = self.range_mhz;
        Capabilities {
            freq_ranges: vec![Range {
                min: f64::from(low) * MHZ,
                max: f64::from(high) * MHZ,
                step: Some(MHZ),
            }],
            sample_rates: limits.rates.iter().copied().map(f64::from).collect(),
            sample_rate_ranges: Vec::new(),
            gains: self.gain_stages(),
            antennas: Vec::new(),
            bandwidths: Vec::new(),
            bandwidth_ranges: limits
                .bandwidth_mhz
                .map(|(min, max)| Range {
                    min: f64::from(min) * MHZ,
                    max: f64::from(max) * MHZ,
                    step: Some(MHZ),
                })
                .into_iter()
                .collect(),
            bandwidth_auto: limits.bandwidth_mhz.is_some(),
            bias_tee: false,
            agc: if self.has_agc() {
                Agc::Switch
            } else {
                Agc::None
            },
            extra: self.extras(),
            ppm: false,
            duplex: Duplex::RxOnly,
            rx_streams: 1,
            tx_streams: 0,
            per_stream: StreamScope::default(),
            directional: None,
            dc_artifact: DcArtifact::Managed,
            hardware_sweep: false,
            coherence: sdrmm_wire::Coherence::None,
            noise_source: sdrmm_wire::NoiseSource::None,
            retune_keeps_phase: false,
            rx_stream_choices: Vec::new(),
        }
    }

    fn gain_stages(&self) -> Vec<GainStage> {
        if !self.has_gain() {
            return Vec::new();
        }
        let range = Range {
            min: 0.0,
            max: f64::from(self.limits.gain_max),
            step: Some(1.0),
        };
        vec![GainStage::named(GAIN_STAGE, GainKind::Tuner, range).with_unit(GainUnit::Index)]
    }

    fn extras(&self) -> Vec<ExtraSetting> {
        let mut extras = Vec::new();
        if self.limits.bits.len() > 1 {
            extras.push(ExtraSetting::choice(
                BITS,
                "Bits",
                self.limits
                    .bits
                    .iter()
                    .map(|bits| ArgumentOption::plain(bits.width().to_string()))
                    .collect(),
                self.limits.bits[0].width().to_string(),
            ));
        }
        extras.push(ExtraSetting::range(
            BURST,
            "Burst",
            Range {
                min: f64::from(MIN_SAMPLES),
                max: f64::from(self.identity.max_samples),
                step: Some(1.0),
            },
            "samples",
        ));
        extras
    }

    pub(crate) fn defaults(&self) -> Remote {
        let (low, high) = self.range_mhz;
        Remote {
            center_mhz: DEFAULT_CENTER_MHZ.clamp(low, high),
            rate: self.limits.rates[0],
            bandwidth_mhz: 0,
            agc: self.has_agc(),
            gain: DEFAULT_MANUAL_GAIN.min(self.limits.gain_max),
            bits: self.limits.bits[0],
            burst: self.identity.max_samples,
        }
    }

    pub(crate) fn commands(&self, applied: Option<&Remote>, desired: &Remote) -> Vec<String> {
        let mut batch = Vec::new();
        if applied.is_none_or(|a| a.center_mhz != desired.center_mhz) {
            batch.push(format!("FREQ {}", desired.center_mhz));
        }
        if self.limits.bandwidth_mhz.is_some()
            && applied.is_none_or(|a| a.bandwidth_mhz != desired.bandwidth_mhz)
        {
            batch.push(format!("BANDWIDTH {}", desired.bandwidth_mhz));
        }
        let gain_moved = applied.is_none_or(|a| a.agc != desired.agc || a.gain != desired.gain);
        if self.has_gain() && gain_moved {
            batch.push(if desired.agc {
                "GAIN HARDWARE".to_string()
            } else {
                format!("GAIN MANUAL {}", desired.gain)
            });
        }
        batch
    }

    pub(crate) fn wire(&self, remote: &Remote) -> DeviceSettings {
        let mut settings = DeviceSettings {
            center_hz: Some(f64::from(remote.center_mhz) * MHZ),
            sample_rate: Some(f64::from(remote.rate)),
            extra: vec![ExtraValue {
                name: BURST.to_string(),
                value: remote.burst.into(),
            }],
            ..DeviceSettings::default()
        };
        if self.limits.bits.len() > 1 {
            settings.extra.insert(
                0,
                ExtraValue {
                    name: BITS.to_string(),
                    value: remote.bits.width().to_string().into(),
                },
            );
        }
        if self.limits.bandwidth_mhz.is_some() {
            settings.bandwidth = Some(match remote.bandwidth_mhz {
                0 => BandwidthSetting::Auto,
                mhz => BandwidthSetting::Manual {
                    hz: f64::from(mhz) * MHZ,
                },
            });
        }
        if self.has_agc() {
            settings.agc = Some(AgcSetting::switched(remote.agc));
        }
        if self.has_gain() && !remote.agc {
            settings.gains.push(GainValue {
                stage: GAIN_STAGE.to_string(),
                value_db: f64::from(remote.gain),
            });
        }
        settings
    }

    pub(crate) fn validate(
        &self,
        delta: &DeviceSettings,
        caps: &Capabilities,
        current: Remote,
    ) -> Result<Remote, DeviceError> {
        check_stream_settings(delta, caps)?;
        refuse_absent(delta)?;
        let mut next = current;
        if let Some(rate) = delta.sample_rate {
            next.rate = self.rate(rate)?;
        }
        if let Some(hz) = delta.center_hz {
            next.center_mhz = self.center(hz)?;
        }
        if let Some(bandwidth) = delta.bandwidth {
            next.bandwidth_mhz = self.bandwidth(bandwidth)?;
        }
        self.gain(delta, &mut next)?;
        for value in &delta.extra {
            self.extra(value, &mut next)?;
        }
        Ok(next)
    }

    fn rate(&self, rate: f64) -> Result<u32, DeviceError> {
        self.limits
            .rates
            .iter()
            .copied()
            .find(|known| (f64::from(*known) - rate).abs() < 0.5)
            .ok_or_else(|| {
                DeviceError::Unsupported(format!(
                    "sample_rate {rate}: the firmware offers {:?}",
                    self.limits.rates
                ))
            })
    }

    fn center(&self, hz: f64) -> Result<u32, DeviceError> {
        let (low, high) = self.range_mhz;
        let mhz = (hz / MHZ).round();
        if !mhz.is_finite() || mhz < f64::from(low) || mhz > f64::from(high) {
            return Err(DeviceError::Unsupported(format!(
                "center_hz {hz} outside {low}..{high} MHz"
            )));
        }
        Ok(mhz as u32)
    }

    fn bandwidth(&self, bandwidth: BandwidthSetting) -> Result<u32, DeviceError> {
        let Some((min, max)) = self.limits.bandwidth_mhz else {
            return Err(DeviceError::Unsupported(
                "bandwidth: this chip's filter is not characterized".to_string(),
            ));
        };
        let Some(hz) = bandwidth.hz() else {
            return Ok(0);
        };
        let mhz = (hz / MHZ).round();
        if !mhz.is_finite() || mhz < f64::from(min) || mhz > f64::from(max) {
            return Err(DeviceError::Unsupported(format!(
                "bandwidth {hz} Hz outside {min}..{max} MHz"
            )));
        }
        Ok(mhz as u32)
    }

    fn gain(&self, delta: &DeviceSettings, next: &mut Remote) -> Result<(), DeviceError> {
        for value in &delta.gains {
            if value.stage != GAIN_STAGE || !self.has_gain() {
                return Err(DeviceError::Unsupported(format!(
                    "gain stage {}",
                    value.stage
                )));
            }
            let index = value.value_db.round();
            if !index.is_finite() || index < 0.0 || index > f64::from(self.limits.gain_max) {
                return Err(DeviceError::Unsupported(format!(
                    "gain index {} outside 0..{}",
                    value.value_db, self.limits.gain_max
                )));
            }
            next.gain = index as u32;
            next.agc = false;
        }
        if let Some(agc) = &delta.agc {
            if agc.mode.is_some() || (agc.on && !self.has_agc()) {
                return Err(DeviceError::Unsupported(
                    "agc: the firmware offers only an on/off hardware AGC".to_string(),
                ));
            }
            next.agc = agc.on;
        }
        Ok(())
    }

    fn extra(&self, value: &ExtraValue, next: &mut Remote) -> Result<(), DeviceError> {
        let bad = || {
            DeviceError::Unsupported(format!(
                "extra setting {}: bad value {}",
                value.name, value.value
            ))
        };
        match value.name.as_str() {
            BITS => {
                let width = match &value.value {
                    serde_json::Value::String(text) => text.parse().ok(),
                    other => other.as_u64().and_then(|n| u32::try_from(n).ok()),
                };
                next.bits = width
                    .and_then(Bits::from_width)
                    .filter(|bits| self.limits.bits.contains(bits))
                    .ok_or_else(bad)?;
            }
            BURST => {
                let samples = value.value.as_f64().ok_or_else(bad)?.round();
                if samples < f64::from(MIN_SAMPLES)
                    || samples > f64::from(self.identity.max_samples)
                {
                    return Err(bad());
                }
                next.burst = samples as u32;
            }
            other => return Err(DeviceError::Unsupported(format!("extra setting {other}"))),
        }
        Ok(())
    }
}

fn refuse_absent(delta: &DeviceSettings) -> Result<(), DeviceError> {
    let absent = [
        (delta.ppm.is_some(), "ppm"),
        (delta.antenna.is_some(), "antenna"),
        (delta.bias_tee == Some(true), "bias_tee"),
    ];
    match absent.iter().find(|(asked, _)| *asked) {
        Some((_, name)) => Err(DeviceError::Unsupported(format!(
            "{name}: ESP-SDR has no such control"
        ))),
        None => Ok(()),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Remote {
    pub(crate) center_mhz: u32,
    pub(crate) rate: u32,
    pub(crate) bandwidth_mhz: u32,
    pub(crate) agc: bool,
    pub(crate) gain: u32,
    pub(crate) bits: Bits,
    pub(crate) burst: u32,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn esp32() -> Profile {
        Profile {
            identity: Identity::parse("ESP32SDR 6 burst 16380").expect("valid"),
            features: Features::parse(
                "CAPS SPEC SPECN SPECCAPS UARTBAUD RXLIMITS SERIALLEASE TUNEEXT RX40 RX16 LPFANA GAIN HWAGC IQ8",
            )
            .expect("valid"),
            limits: Limits::parse(
                r#"LIMITS {"gain":[0,72,1],"bandwidth":[12,67,1,0],"rates":[80000000,40000000,16000000],"bits":[8,10]}"#,
            )
            .expect("valid"),
            range_mhz: (100, 6000),
        }
    }

    fn apply(delta: DeviceSettings) -> Result<Remote, DeviceError> {
        let profile = esp32();
        profile.validate(&delta, &profile.capabilities(), profile.defaults())
    }

    #[test]
    fn capabilities_follow_the_firmware_limits() {
        let caps = esp32().capabilities();
        assert_eq!(caps.freq_ranges[0].min, 100e6);
        assert_eq!(caps.freq_ranges[0].max, 6e9);
        assert_eq!(caps.sample_rates, [80e6, 40e6, 16e6]);
        assert_eq!(caps.bandwidth_ranges[0].min, 12e6);
        assert!(caps.bandwidth_auto);
        assert_eq!(caps.agc, Agc::Switch);
        assert_eq!(caps.gains[0].unit, GainUnit::Index);
        assert_eq!(caps.gains[0].range.max, 72.0);
        let names: Vec<&str> = caps.extra.iter().map(ExtraSetting::name).collect();
        assert_eq!(names, [BITS, BURST]);
    }

    #[test]
    fn a_fresh_radio_replays_every_setting_in_order() {
        let profile = esp32();
        let remote = profile.defaults();
        assert_eq!(
            profile.commands(None, &remote),
            ["FREQ 2437", "BANDWIDTH 0", "GAIN HARDWARE"]
        );
        assert!(profile.commands(Some(&remote), &remote).is_empty());
    }

    #[test]
    fn tuning_rounds_to_whole_megahertz() {
        let next = apply(DeviceSettings {
            center_hz: Some(433_920_000.0),
            ..DeviceSettings::default()
        })
        .expect("in range");
        assert_eq!(next.center_mhz, 434);
        assert_eq!(esp32().wire(&next).center_hz, Some(434e6));
        assert!(
            apply(DeviceSettings {
                center_hz: Some(50e6),
                ..DeviceSettings::default()
            })
            .is_err()
        );
    }

    #[test]
    fn a_manual_gain_turns_the_agc_off() {
        let profile = esp32();
        let next = apply(DeviceSettings {
            gains: vec![GainValue {
                stage: GAIN_STAGE.into(),
                value_db: 30.0,
            }],
            ..DeviceSettings::default()
        })
        .expect("valid");
        assert!(!next.agc);
        assert_eq!(
            profile.commands(Some(&profile.defaults()), &next),
            ["GAIN MANUAL 30"]
        );
        let wire = profile.wire(&next);
        assert_eq!(wire.agc, Some(AgcSetting::switched(false)));
        assert_eq!(wire.gains[0].value_db, 30.0);
        assert!(
            apply(DeviceSettings {
                gains: vec![GainValue {
                    stage: GAIN_STAGE.into(),
                    value_db: 73.0,
                }],
                ..DeviceSettings::default()
            })
            .is_err()
        );
    }

    #[test]
    fn only_offered_rates_bandwidths_and_extras_pass() {
        assert_eq!(
            apply(DeviceSettings {
                sample_rate: Some(40e6),
                bandwidth: Some(BandwidthSetting::Manual { hz: 20e6 }),
                extra: vec![
                    ExtraValue {
                        name: BITS.into(),
                        value: "10".into(),
                    },
                    ExtraValue {
                        name: BURST.into(),
                        value: 4096.into(),
                    },
                ],
                ..DeviceSettings::default()
            })
            .map(|r| (r.rate, r.bandwidth_mhz, r.bits, r.burst))
            .expect("valid"),
            (40_000_000, 20, Bits::Ten, 4096)
        );
        for delta in [
            DeviceSettings {
                sample_rate: Some(20e6),
                ..DeviceSettings::default()
            },
            DeviceSettings {
                bandwidth: Some(BandwidthSetting::Manual { hz: 5e6 }),
                ..DeviceSettings::default()
            },
            DeviceSettings {
                extra: vec![ExtraValue {
                    name: BURST.into(),
                    value: 20_000.into(),
                }],
                ..DeviceSettings::default()
            },
            DeviceSettings {
                ppm: Some(1.0),
                ..DeviceSettings::default()
            },
        ] {
            assert!(apply(delta.clone()).is_err(), "{delta:?}");
        }
    }

    #[test]
    fn wire_settings_round_trip_through_validate() {
        let profile = esp32();
        let caps = profile.capabilities();
        let remote = Remote {
            center_mhz: 2412,
            rate: 16_000_000,
            bandwidth_mhz: 30,
            agc: false,
            gain: 12,
            bits: Bits::Ten,
            burst: 1000,
        };
        let back = profile
            .validate(&profile.wire(&remote), &caps, profile.defaults())
            .expect("own settings are valid");
        assert_eq!(back, remote);
    }
}
