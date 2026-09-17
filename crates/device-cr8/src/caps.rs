use sdrmm_wire::{
    Agc, ArgumentOption, Capabilities, Coherence, DcArtifact, DeviceProfile, Duplex, ExtraSetting,
    GainKind, GainStage, GainUnit, Range, StreamScope,
};

use crate::ffi;

/// The tuning range the CR-8 covers. The vendor header carries no range at all, so this is the
/// one number here that comes from the datasheet rather than from the SDK.
pub const MIN_FREQ_HZ: f64 = 24e6;
pub const MAX_FREQ_HZ: f64 = 1_766e6;

pub const CLOCK_SETTING: &str = "clock_source";
pub const CLOCK_INTERNAL: &str = "internal";
pub const CLOCK_EXTERNAL: &str = "external";

fn stage(kind: GainKind, max: f64) -> GainStage {
    GainStage::new(
        kind,
        Range {
            min: 0.0,
            max,
            step: Some(1.0),
        },
    )
    .with_unit(GainUnit::Index)
}

#[must_use]
pub fn gains() -> Vec<GainStage> {
    vec![
        stage(GainKind::Lna, 14.0),
        stage(GainKind::Mixer, 15.0),
        stage(GainKind::Vga, 15.0),
    ]
}

#[must_use]
pub fn extra() -> Vec<ExtraSetting> {
    vec![ExtraSetting::choice(
        CLOCK_SETTING,
        "Clock source",
        vec![
            ArgumentOption::plain(CLOCK_INTERNAL),
            ArgumentOption::plain(CLOCK_EXTERNAL),
        ],
        CLOCK_INTERNAL,
    )]
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
        bandwidth_auto: false,
        bias_tee: false,
        agc: Agc::None,
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
        noise_source: false,
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
