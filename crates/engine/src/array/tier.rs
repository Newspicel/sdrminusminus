use std::collections::BTreeSet;

use sdrmm_wire::{Capabilities, Coherence, NoiseSource};

use super::LaneRef;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TierDecision {
    pub(crate) tier: Coherence,
    pub(crate) devices: usize,
    pub(crate) keeps_phase: bool,
    pub(crate) structural_zero_delay: bool,
}

pub(crate) fn decide(
    lanes: &[(LaneRef, &Capabilities)],
    declared: Coherence,
    drift_failed: bool,
) -> TierDecision {
    let devices: BTreeSet<u32> = lanes.iter().map(|(lane, _)| lane.device_set).collect();
    let keeps_phase = !lanes.is_empty() && lanes.iter().all(|(_, caps)| caps.retune_keeps_phase);
    let replayed = !lanes.is_empty()
        && lanes
            .iter()
            .all(|(_, caps)| caps.noise_source == NoiseSource::Replayed);
    let tier = if devices.len() == 1 {
        lanes
            .first()
            .map_or(Coherence::None, |(_, caps)| caps.coherence)
    } else {
        devices
            .iter()
            .filter_map(|device| {
                let mut members = lanes.iter().filter(|(lane, _)| lane.device_set == *device);
                let (_, caps) = members.next()?;
                members.next().map(|_| caps.coherence)
            })
            .fold(declared, Coherence::min)
    };
    let tier = if drift_failed { Coherence::None } else { tier };
    TierDecision {
        tier,
        devices: devices.len(),
        keeps_phase,
        structural_zero_delay: devices.len() == 1
            && (tier == Coherence::PhaseCoherent || (replayed && tier != Coherence::None)),
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{Agc, DcArtifact, Duplex, StreamScope};

    use super::*;

    fn caps(coherence: Coherence, retune_keeps_phase: bool) -> Capabilities {
        Capabilities {
            freq_ranges: Vec::new(),
            sample_rates: Vec::new(),
            sample_rate_ranges: Vec::new(),
            gains: Vec::new(),
            antennas: Vec::new(),
            bandwidths: Vec::new(),
            bandwidth_ranges: Vec::new(),
            bandwidth_auto: false,
            bias_tee: false,
            agc: Agc::None,
            extra: Vec::new(),
            ppm: false,
            duplex: Duplex::RxOnly,
            rx_streams: 5,
            tx_streams: 0,
            per_stream: StreamScope::default(),
            directional: None,
            dc_artifact: DcArtifact::Managed,
            hardware_sweep: false,
            coherence,
            noise_source: NoiseSource::Isolated,
            retune_keeps_phase,
        }
    }

    const fn lane(device_set: u32, stream: u32) -> LaneRef {
        LaneRef { device_set, stream }
    }

    #[test]
    fn one_device_takes_the_driver_tier() {
        let kraken = caps(Coherence::TimeSync, false);
        let lanes: Vec<_> = (0..5).map(|stream| (lane(1, stream), &kraken)).collect();
        let decision = decide(&lanes, Coherence::PhaseCoherent, false);
        assert_eq!(decision.tier, Coherence::TimeSync);
        assert_eq!(decision.devices, 1);
        assert!(!decision.structural_zero_delay);
        let coherent = caps(Coherence::PhaseCoherent, true);
        let lanes = [(lane(2, 0), &coherent), (lane(2, 1), &coherent)];
        let decision = decide(&lanes, Coherence::None, false);
        assert_eq!(decision.tier, Coherence::PhaseCoherent);
        assert!(decision.structural_zero_delay);
        assert!(decision.keeps_phase);
    }

    #[test]
    fn several_devices_take_the_declared_tier_capped_by_multi_lane_members() {
        let dongle = caps(Coherence::None, false);
        let pair = caps(Coherence::TimeSync, false);
        let singles = [(lane(1, 0), &dongle), (lane(2, 0), &dongle)];
        let decision = decide(&singles, Coherence::PhaseCoherent, false);
        assert_eq!(decision.tier, Coherence::PhaseCoherent);
        assert_eq!(decision.devices, 2);
        assert!(!decision.structural_zero_delay);
        let mixed = [
            (lane(1, 0), &dongle),
            (lane(3, 0), &pair),
            (lane(3, 1), &pair),
        ];
        assert_eq!(
            decide(&mixed, Coherence::PhaseCoherent, false).tier,
            Coherence::TimeSync
        );
        assert_eq!(decide(&mixed, Coherence::None, false).tier, Coherence::None);
    }

    #[test]
    fn a_replayed_collection_starts_lined_up() {
        let replayed = Capabilities {
            noise_source: NoiseSource::Replayed,
            ..caps(Coherence::TimeSync, false)
        };
        let lanes: Vec<_> = (0..5).map(|stream| (lane(4, stream), &replayed)).collect();
        let decision = decide(&lanes, Coherence::None, false);
        assert_eq!(decision.tier, Coherence::TimeSync);
        assert!(decision.structural_zero_delay);
        assert!(!decide(&lanes, Coherence::None, true).structural_zero_delay);
        let live = caps(Coherence::TimeSync, false);
        let mixed = [(lane(4, 0), &replayed), (lane(4, 1), &live)];
        assert!(!decide(&mixed, Coherence::None, false).structural_zero_delay);
    }

    #[test]
    fn measured_drift_caps_the_tier_to_none() {
        let coherent = caps(Coherence::PhaseCoherent, true);
        let lanes = [(lane(1, 0), &coherent), (lane(1, 1), &coherent)];
        let decision = decide(&lanes, Coherence::PhaseCoherent, true);
        assert_eq!(decision.tier, Coherence::None);
        assert!(!decision.structural_zero_delay);
    }
}
