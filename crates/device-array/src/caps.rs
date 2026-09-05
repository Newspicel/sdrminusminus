use sdrmm_wire::{
    ArrayDefinition, Capabilities, DeviceProfile, Duplex, GainStage, Range, StreamScope,
};

/// What every member can do, which is all a composite can promise.
///
/// A setting one radio cannot reach is a setting the array cannot reach, because a lane that is
/// not tuned where the others are is not part of the same measurement.
#[must_use]
pub fn intersect(ranges: &[&[Range]]) -> Vec<Range> {
    let Some((first, rest)) = ranges.split_first() else {
        return Vec::new();
    };
    let mut out: Vec<Range> = (*first).to_vec();
    for other in rest {
        let mut next = Vec::new();
        for a in &out {
            for b in *other {
                let min = a.min.max(b.min);
                let max = a.max.min(b.max);
                if min <= max {
                    next.push(Range {
                        min,
                        max,
                        step: a.step.or(b.step),
                    });
                }
            }
        }
        out = next;
    }
    out
}

fn shared_rates(rates: &[&[f64]]) -> Vec<f64> {
    let Some((first, rest)) = rates.split_first() else {
        return Vec::new();
    };
    (*first)
        .iter()
        .copied()
        .filter(|rate| rest.iter().all(|other| other.contains(rate)))
        .collect()
}

fn shared_names(names: &[&[String]]) -> Vec<String> {
    let Some((first, rest)) = names.split_first() else {
        return Vec::new();
    };
    (*first)
        .iter()
        .filter(|name| rest.iter().all(|other| other.contains(name)))
        .cloned()
        .collect()
}

#[must_use]
pub fn composite(members: &[&Capabilities], definition: &ArrayDefinition) -> Capabilities {
    let freq_ranges = intersect(
        &members
            .iter()
            .map(|member| member.freq_ranges.as_slice())
            .collect::<Vec<_>>(),
    );
    let sample_rate_ranges = intersect(
        &members
            .iter()
            .map(|member| member.sample_rate_ranges.as_slice())
            .collect::<Vec<_>>(),
    );
    let bandwidth_ranges = intersect(
        &members
            .iter()
            .map(|member| member.bandwidth_ranges.as_slice())
            .collect::<Vec<_>>(),
    );
    let sample_rates = shared_rates(
        &members
            .iter()
            .map(|member| member.sample_rates.as_slice())
            .collect::<Vec<_>>(),
    );
    let bandwidths = shared_rates(
        &members
            .iter()
            .map(|member| member.bandwidths.as_slice())
            .collect::<Vec<_>>(),
    );
    let antennas = shared_names(
        &members
            .iter()
            .map(|member| member.antennas.as_slice())
            .collect::<Vec<_>>(),
    );
    let gains: Vec<GainStage> = members
        .first()
        .map(|member| member.gains.clone())
        .unwrap_or_default();
    Capabilities {
        freq_ranges,
        sample_rates,
        sample_rate_ranges,
        gains,
        antennas,
        bandwidths,
        bandwidth_ranges,
        extra: Vec::new(),
        ppm: members.iter().all(|member| member.ppm),
        duplex: Duplex::RxOnly,
        rx_streams: members
            .iter()
            .map(|member| member.rx_streams.max(1))
            .sum::<u32>(),
        tx_streams: 0,
        per_stream: per_stream(definition, members),
        directional: None,
        dc_artifact: sdrmm_wire::DcArtifact::None,
        hardware_sweep: false,
        coherence: definition.coherence,
    }
}

fn per_stream(definition: &ArrayDefinition, members: &[&Capabilities]) -> StreamScope {
    let declared = definition.per_stream();
    StreamScope {
        tuning: declared.tuning,
        gain: declared.gain && members.iter().any(|member| !member.gains.is_empty()),
        antenna: declared.antenna && members.iter().any(|member| !member.antennas.is_empty()),
    }
}

#[must_use]
pub fn composite_profile(
    members: &[&DeviceProfile],
    definition: &ArrayDefinition,
) -> DeviceProfile {
    DeviceProfile {
        freq_ranges: intersect(
            &members
                .iter()
                .map(|member| member.freq_ranges.as_slice())
                .collect::<Vec<_>>(),
        ),
        sample_rates: shared_rates(
            &members
                .iter()
                .map(|member| member.sample_rates.as_slice())
                .collect::<Vec<_>>(),
        ),
        sample_rate_ranges: intersect(
            &members
                .iter()
                .map(|member| member.sample_rate_ranges.as_slice())
                .collect::<Vec<_>>(),
        ),
        duplex: Duplex::RxOnly,
        rx_streams: members
            .iter()
            .map(|member| member.rx_streams.max(1))
            .sum::<u32>(),
        tx_streams: 0,
        per_stream: definition.per_stream(),
    }
}
