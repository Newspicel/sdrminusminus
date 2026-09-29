use sdrmm_wire::{
    Agc, AgcReach, ArgumentOption, Capabilities, Coherence, DcArtifact, Duplex, ExtraSetting,
    GainKind, GainStage, Range, StreamScope,
};

use crate::{
    ad9361::{
        GainMode, MAX_BANDWIDTH, MAX_FREQUENCY, MAX_RX_GAIN, MAX_TX_ATTENUATION, MIN_BANDWIDTH,
        MIN_FREQUENCY,
    },
    rate::{MIN_SAMPLE_RATE, max_sample_rate},
};

pub(crate) const QUADRATURE: &str = "quadrature_tracking";
pub(crate) const BB_DC: &str = "bb_dc_tracking";
pub(crate) const SLOW_ATTACK: &str = "slow_attack";
pub(crate) const FAST_ATTACK: &str = "fast_attack";

const fn span(min: f64, max: f64) -> Range {
    Range {
        min,
        max,
        step: None,
    }
}

pub(crate) const RX_GAIN: Range = Range {
    min: 0.0,
    max: MAX_RX_GAIN,
    step: Some(1.0),
};

pub(crate) const TX_GAIN: Range = Range {
    min: -MAX_TX_ATTENUATION,
    max: 0.0,
    step: Some(0.25),
};

pub(crate) fn capabilities(radios: usize, lanes: usize) -> Capabilities {
    let mut capabilities = Capabilities {
        freq_ranges: vec![span(MIN_FREQUENCY, MAX_FREQUENCY)],
        sample_rates: Vec::new(),
        sample_rate_ranges: Vec::new(),
        gains: vec![
            GainStage::new(GainKind::Tuner, RX_GAIN),
            GainStage::new(GainKind::Tx, TX_GAIN).with_agc(AgcReach::Never),
        ],
        antennas: Vec::new(),
        bandwidths: Vec::new(),
        bandwidth_ranges: vec![span(MIN_BANDWIDTH, MAX_BANDWIDTH)],
        bandwidth_auto: true,
        bias_tee: false,
        agc: Agc::Modes {
            options: vec![
                ArgumentOption {
                    value: SLOW_ATTACK.to_string(),
                    label: Some("Slow attack".to_string()),
                },
                ArgumentOption {
                    value: FAST_ATTACK.to_string(),
                    label: Some("Fast attack".to_string()),
                },
            ],
        },
        extra: vec![
            ExtraSetting::bool(QUADRATURE, "Quadrature tracking", true),
            ExtraSetting::bool(BB_DC, "Baseband DC tracking", true),
        ],
        ppm: true,
        duplex: Duplex::Full,
        rx_streams: 1,
        tx_streams: radios as u32,
        per_stream: StreamScope::default(),
        directional: None,
        dc_artifact: DcArtifact::Managed,
        hardware_sweep: false,
        coherence: Coherence::None,
        noise_source: false,
        rx_stream_choices: if radios > 1 {
            (1..=radios as u32).collect()
        } else {
            Vec::new()
        },
    };
    set_lanes(&mut capabilities, lanes.clamp(1, radios.max(1)));
    capabilities
}

pub(crate) fn set_lanes(capabilities: &mut Capabilities, lanes: usize) {
    let lanes = lanes.max(1);
    capabilities.rx_streams = lanes as u32;
    capabilities.sample_rate_ranges = vec![span(MIN_SAMPLE_RATE, max_sample_rate(lanes))];
    if lanes > 1 {
        capabilities.per_stream = StreamScope {
            tuning: false,
            gain: true,
            antenna: false,
            agc: true,
        };
        capabilities.coherence = Coherence::PhaseCoherent;
    } else {
        capabilities.per_stream = StreamScope::default();
        capabilities.coherence = Coherence::None;
    }
}

pub(crate) fn gain_mode(on: bool, mode: Option<&str>) -> GainMode {
    match (on, mode) {
        (false, _) => GainMode::Manual,
        (true, Some(FAST_ATTACK)) => GainMode::FastAttack,
        (true, _) => GainMode::SlowAttack,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_lane_halves_the_rate_and_splits_the_gain() {
        let one = capabilities(2, 1);
        assert_eq!(one.rx_streams, 1);
        assert_eq!(one.rx_stream_choices, vec![1, 2]);
        assert_eq!(one.per_stream, StreamScope::default());
        let two = capabilities(2, 2);
        assert_eq!(two.rx_streams, 2);
        assert!(two.per_stream.gain && two.per_stream.agc && !two.per_stream.tuning);
        assert_eq!(two.coherence, Coherence::PhaseCoherent);
        assert!(
            two.sample_rate_ranges[0].max * 2.0 <= one.sample_rate_ranges[0].max + 1.0,
            "the link is shared"
        );
    }

    #[test]
    fn a_single_chain_board_offers_no_lane_choice() {
        let caps = capabilities(1, 2);
        assert!(caps.rx_stream_choices.is_empty());
        assert_eq!(caps.tx_streams, 1);
    }

    #[test]
    fn agc_modes_map_onto_the_transceiver() {
        assert_eq!(gain_mode(false, Some(FAST_ATTACK)), GainMode::Manual);
        assert_eq!(gain_mode(true, Some(FAST_ATTACK)), GainMode::FastAttack);
        assert_eq!(gain_mode(true, None), GainMode::SlowAttack);
    }
}
