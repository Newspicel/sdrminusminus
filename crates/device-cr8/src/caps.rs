use sdrmm_wire::{
    ArgumentOption, Capabilities, Coherence, DcArtifact, DeviceProfile, Duplex, ExtraSetting,
    GainStage, Range, StreamScope,
};

use crate::ffi;

/// The tuning range the CR-8 covers. The vendor header carries no range at all, so this is the
/// one number here that comes from the datasheet rather than from the SDK.
pub const MIN_FREQ_HZ: f64 = 24e6;
pub const MAX_FREQ_HZ: f64 = 1_766e6;

pub const CLOCK_SETTING: &str = "clock_source";
pub const CLOCK_INTERNAL: &str = "internal";
pub const CLOCK_EXTERNAL: &str = "external";

/// Named for the stage each one drives, because the three add up to the overall figure the
/// library also offers and an operator setting them by hand wants to know which is which.
#[must_use]
pub fn gains() -> Vec<GainStage> {
    vec![
        GainStage {
            name: "LNA".to_owned(),
            range: Range {
                min: 0.0,
                max: 14.0,
                step: Some(1.0),
            },
            values: Vec::new(),
        },
        GainStage {
            name: "Mixer".to_owned(),
            range: Range {
                min: 0.0,
                max: 15.0,
                step: Some(1.0),
            },
            values: Vec::new(),
        },
        GainStage {
            name: "VGA".to_owned(),
            range: Range {
                min: 0.0,
                max: 15.0,
                step: Some(1.0),
            },
            values: Vec::new(),
        },
    ]
}

#[must_use]
pub fn extra() -> Vec<ExtraSetting> {
    vec![ExtraSetting::Enum {
        name: CLOCK_SETTING.to_owned(),
        options: vec![
            ArgumentOption::plain(CLOCK_INTERNAL),
            ArgumentOption::plain(CLOCK_EXTERNAL),
        ],
        default: CLOCK_INTERNAL.to_owned(),
    }]
}

#[must_use]
pub fn capabilities() -> Capabilities {
    Capabilities {
        freq_ranges: vec![Range {
            min: MIN_FREQ_HZ,
            max: MAX_FREQ_HZ,
            step: Some(1.0),
        }],
        sample_rates: vec![ffi::SAMPLE_RATE_HZ],
        sample_rate_ranges: Vec::new(),
        gains: gains(),
        antennas: Vec::new(),
        bandwidths: Vec::new(),
        bandwidth_ranges: Vec::new(),
        extra: extra(),
        ppm: false,
        duplex: Duplex::RxOnly,
        rx_streams: ffi::CHANNEL_COUNT as u32,
        tx_streams: 0,
        per_stream: StreamScope {
            tuning: false,
            gain: true,
            antenna: false,
        },
        directional: None,
        dc_artifact: DcArtifact::Operator,
        hardware_sweep: false,
        coherence: Coherence::PhaseCoherent,
    }
}

#[must_use]
pub fn profile() -> DeviceProfile {
    let capabilities = capabilities();
    DeviceProfile {
        freq_ranges: capabilities.freq_ranges,
        sample_rates: capabilities.sample_rates,
        sample_rate_ranges: capabilities.sample_rate_ranges,
        duplex: Duplex::RxOnly,
        rx_streams: capabilities.rx_streams,
        tx_streams: 0,
        per_stream: capabilities.per_stream,
    }
}
