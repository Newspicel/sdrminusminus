use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{
    Agc, AgcSetting, Capabilities, Coherence, DcArtifact, DeviceSettings, Duplex, GainKind,
    GainStage, GainValue, Range, StreamScope,
};

use crate::{proto::Command, session::Station};

const MAX_GAIN_DB: f64 = 120.0;
const DEFAULT_GAIN_DB: u8 = 50;

pub(crate) fn capabilities(station: &Station) -> Capabilities {
    Capabilities {
        freq_ranges: vec![Range {
            min: station.low_hz,
            max: station.high_hz(),
            step: None,
        }],
        sample_rates: vec![station.sample_rate],
        sample_rate_ranges: Vec::new(),
        gains: vec![GainStage::new(
            GainKind::Rf,
            Range {
                min: 0.0,
                max: MAX_GAIN_DB,
                step: Some(1.0),
            },
        )],
        antennas: Vec::new(),
        bandwidths: Vec::new(),
        bandwidth_ranges: Vec::new(),
        bandwidth_auto: false,
        bias_tee: false,
        agc: Agc::Switch,
        extra: Vec::new(),
        ppm: false,
        duplex: Duplex::RxOnly,
        rx_streams: 1,
        tx_streams: 0,
        per_stream: StreamScope::default(),
        directional: None,
        dc_artifact: DcArtifact::None,
        hardware_sweep: false,
        coherence: Coherence::None,
        noise_source: sdrmm_wire::NoiseSource::None,
        retune_keeps_phase: false,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Tuning {
    pub center_hz: f64,
    pub agc: bool,
    pub gain_db: u8,
}

impl Tuning {
    pub(crate) fn new(station: &Station) -> Self {
        Self {
            center_hz: station.low_hz + station.span_hz / 2.0,
            agc: true,
            gain_db: DEFAULT_GAIN_DB,
        }
    }

    pub(crate) fn wire(&self, station: &Station) -> DeviceSettings {
        DeviceSettings {
            center_hz: Some(self.center_hz),
            sample_rate: Some(station.sample_rate),
            agc: Some(AgcSetting::switched(self.agc)),
            gains: vec![GainValue::new(GainKind::Rf, f64::from(self.gain_db))],
            ..DeviceSettings::default()
        }
    }

    pub(crate) fn tune(&self, station: &Station) -> Command {
        Command::Tune {
            khz: station.khz(self.center_hz),
            half_width_hz: station.half_width_hz(),
        }
    }

    pub(crate) fn gain(&self) -> Command {
        Command::Gain {
            agc: self.agc,
            manual_db: self.gain_db,
        }
    }

    pub(crate) fn replay(&self, station: &Station) -> Vec<Command> {
        vec![self.tune(station), self.gain()]
    }
}

fn unsupported(what: &str) -> DeviceError {
    DeviceError::Unsupported(format!("{what}: a KiwiSDR has no such setting"))
}

pub(crate) fn validate(
    station: &Station,
    caps: &Capabilities,
    current: &Tuning,
    settings: &DeviceSettings,
) -> Result<(Tuning, Vec<Command>), DeviceError> {
    check_stream_settings(settings, caps)?;
    if settings.ppm.is_some_and(|ppm| ppm != 0.0) {
        return Err(unsupported("ppm"));
    }
    if settings.bias_tee == Some(true) {
        return Err(unsupported("bias_tee"));
    }
    if settings.bandwidth.is_some() {
        return Err(unsupported("bandwidth"));
    }
    if settings.antenna.is_some() {
        return Err(unsupported("antenna"));
    }
    if let Some(extra) = settings.extra.first() {
        return Err(unsupported(&extra.name));
    }
    if let Some(rate) = settings.sample_rate
        && rate != station.sample_rate
    {
        return Err(DeviceError::Unsupported(format!(
            "sample_rate {rate}: this KiwiSDR streams {}",
            station.sample_rate
        )));
    }

    let mut next = current.clone();
    if let Some(hz) = settings.center_hz {
        if !(station.low_hz..=station.high_hz()).contains(&hz) {
            return Err(DeviceError::Unsupported(format!(
                "center_hz {hz}: this KiwiSDR tunes {}..{} Hz",
                station.low_hz,
                station.high_hz()
            )));
        }
        next.center_hz = hz;
    }
    if let Some(agc) = &settings.agc {
        if !caps.agc.admits(agc) {
            return Err(unsupported("agc mode"));
        }
        next.agc = agc.on;
    }
    for gain in &settings.gains {
        let stage = caps
            .stage(&gain.stage)
            .ok_or_else(|| unsupported(&format!("gain stage {}", gain.stage)))?;
        if !stage.range.holds(gain.value_db) {
            return Err(DeviceError::Unsupported(format!(
                "gain {}: 0..{MAX_GAIN_DB} dB",
                gain.value_db
            )));
        }
        next.gain_db = gain.value_db.round() as u8;
    }

    let mut batch = Vec::new();
    if next.center_hz != current.center_hz {
        batch.push(next.tune(station));
    }
    if (next.agc, next.gain_db) != (current.agc, current.gain_db) {
        batch.push(next.gain());
    }
    Ok((next, batch))
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::BandwidthSetting;

    use super::*;

    fn station() -> Station {
        Station {
            sample_rate: 12_000.0,
            low_hz: 0.0,
            span_hz: 30e6,
            channels: Some(8),
        }
    }

    fn apply(settings: DeviceSettings) -> Result<(Tuning, Vec<Command>), DeviceError> {
        let station = station();
        validate(
            &station,
            &capabilities(&station),
            &Tuning::new(&station),
            &settings,
        )
    }

    #[test]
    fn a_fresh_device_sits_mid_band_with_agc() {
        let tuning = Tuning::new(&station());
        assert_eq!(tuning.center_hz, 15e6);
        assert!(tuning.agc);
        let wire = tuning.wire(&station());
        assert_eq!(wire.sample_rate, Some(12_000.0));
        assert!(apply(wire).expect("own settings are valid").1.is_empty());
    }

    #[test]
    fn retuning_sends_only_the_tune() {
        let (tuning, batch) = apply(DeviceSettings {
            center_hz: Some(7_074_000.0),
            ..DeviceSettings::default()
        })
        .expect("in band");
        assert_eq!(tuning.center_hz, 7_074_000.0);
        assert_eq!(
            batch,
            vec![Command::Tune {
                khz: 7_074.0,
                half_width_hz: 6_000
            }]
        );
    }

    #[test]
    fn manual_gain_sends_only_the_gain() {
        let (tuning, batch) = apply(DeviceSettings {
            agc: Some(AgcSetting::switched(false)),
            gains: vec![GainValue::new(GainKind::Rf, 72.4)],
            ..DeviceSettings::default()
        })
        .expect("in range");
        assert_eq!(tuning.gain_db, 72);
        assert_eq!(
            batch,
            vec![Command::Gain {
                agc: false,
                manual_db: 72
            }]
        );
    }

    #[test]
    fn out_of_band_and_foreign_settings_are_refused() {
        let refused =
            |settings: DeviceSettings| matches!(apply(settings), Err(DeviceError::Unsupported(_)));
        assert!(refused(DeviceSettings {
            center_hz: Some(31e6),
            ..DeviceSettings::default()
        }));
        assert!(refused(DeviceSettings {
            sample_rate: Some(48_000.0),
            ..DeviceSettings::default()
        }));
        assert!(refused(DeviceSettings {
            gains: vec![GainValue::new(GainKind::Rf, 121.0)],
            ..DeviceSettings::default()
        }));
        assert!(refused(DeviceSettings {
            gains: vec![GainValue::new(GainKind::Lna, 10.0)],
            ..DeviceSettings::default()
        }));
        assert!(refused(DeviceSettings {
            bandwidth: Some(BandwidthSetting::Auto),
            ..DeviceSettings::default()
        }));
        assert!(refused(DeviceSettings {
            antenna: Some("A".to_string()),
            ..DeviceSettings::default()
        }));
        assert!(refused(DeviceSettings {
            ppm: Some(1.0),
            ..DeviceSettings::default()
        }));
        assert!(refused(DeviceSettings {
            bias_tee: Some(true),
            ..DeviceSettings::default()
        }));
    }

    #[test]
    fn capabilities_follow_the_station() {
        let caps = capabilities(&station());
        assert_eq!(caps.freq_ranges[0].max, 30e6);
        assert_eq!(caps.sample_rates, vec![12_000.0]);
        assert!(caps.stage(GainKind::Rf.name()).is_some());
        assert_eq!(caps.duplex, Duplex::RxOnly);
    }
}
