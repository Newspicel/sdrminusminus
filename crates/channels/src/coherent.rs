use num_complex::Complex;
use sdrmm_wire::{CoherentParams, DecoderEvent, DfReading, RadarDetection};

use crate::ChannelError;

/// What a coherent processor is, before one exists: enough for the patch graph and the engine to
/// decide whether a radio can run it at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoherentDescriptor {
    pub type_id: &'static str,
    pub name: &'static str,
    pub min_lanes: u32,
    /// Whether inter-lane phase has to be trusted. A time-synced array can run everything that
    /// does not, and must refuse everything that does.
    pub needs_phase: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct CoherentCtx {
    pub lanes: usize,
    pub sample_rate: f64,
    pub center_hz: f64,
}

/// A range–Doppler surface already quantised for the wire, because nothing between here and the
/// browser has any use for the floating-point original.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RangeDopplerSurface {
    pub ranges: usize,
    pub dopplers: usize,
    pub range_step_s: f32,
    pub doppler_step_hz: f32,
    pub db_min: f32,
    pub db_max: f32,
    pub cells: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CoherentOutputs {
    pub bearing: Option<DfReading>,
    pub surface: Option<RangeDopplerSurface>,
    pub detections: Vec<RadarDetection>,
    pub events: Vec<DecoderEvent>,
    /// What to multiply each lane by before summing them into the beam lane. A processor that
    /// knows where the signal is can point the array at it; everything else leaves this alone.
    pub weights: Option<Vec<Complex<f32>>>,
}

impl CoherentOutputs {
    pub fn reset(&mut self) {
        self.bearing = None;
        self.surface = None;
        self.detections.clear();
        self.events.clear();
        self.weights = None;
    }
}

/// A processor that reads several lanes at once.
///
/// Deliberately its own trait rather than a `ChannelRx` with extra arguments: everything here
/// takes every lane together, and nothing here has audio, a filter or a squelch.
pub trait CoherentRx: Send {
    fn descriptor() -> &'static CoherentDescriptor
    where
        Self: Sized;

    fn new(ctx: CoherentCtx, params: &CoherentParams) -> Result<Self, ChannelError>
    where
        Self: Sized;

    fn apply(&mut self, params: &CoherentParams) -> Result<(), ChannelError>;

    fn retuned(&mut self, _center_hz: f64) {}

    fn process(&mut self, lanes: &[&[Complex<f32>]], out: &mut CoherentOutputs);
}

type CreateCoherent = fn(CoherentCtx, &CoherentParams) -> Result<Box<dyn CoherentRx>, ChannelError>;

struct Registration {
    descriptor: fn() -> &'static CoherentDescriptor,
    create: CreateCoherent,
}

fn boxed<C: CoherentRx + 'static>(
    ctx: CoherentCtx,
    params: &CoherentParams,
) -> Result<Box<dyn CoherentRx>, ChannelError> {
    Ok(Box::new(C::new(ctx, params)?))
}

const REGISTRY: &[Registration] = &[
    Registration {
        descriptor: crate::df::DfProcessor::descriptor,
        create: boxed::<crate::df::DfProcessor>,
    },
    Registration {
        descriptor: crate::passive_radar::PassiveRadarProcessor::descriptor,
        create: boxed::<crate::passive_radar::PassiveRadarProcessor>,
    },
];

#[must_use]
pub fn coherent_descriptors() -> Vec<&'static CoherentDescriptor> {
    REGISTRY.iter().map(|entry| (entry.descriptor)()).collect()
}

#[must_use]
pub fn coherent_descriptor(type_id: &str) -> Option<&'static CoherentDescriptor> {
    coherent_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.type_id == type_id)
}

pub fn create_coherent(
    ctx: CoherentCtx,
    params: &CoherentParams,
) -> Result<Box<dyn CoherentRx>, ChannelError> {
    let type_id = params.type_id();
    let entry = REGISTRY
        .iter()
        .find(|entry| (entry.descriptor)().type_id == type_id)
        .ok_or_else(|| ChannelError::UnknownType(type_id.to_owned()))?;
    let descriptor = (entry.descriptor)();
    if (ctx.lanes as u32) < descriptor.min_lanes {
        return Err(ChannelError::InvalidSettings(format!(
            "{} needs at least {} lanes, this radio has {}",
            descriptor.name, descriptor.min_lanes, ctx.lanes
        )));
    }
    (entry.create)(ctx, params)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn coherent_descriptors_are_unique_and_complete() {
        let all = coherent_descriptors();
        assert_eq!(all.len(), 2);
        let ids: HashSet<&str> = all.iter().map(|d| d.type_id).collect();
        assert_eq!(ids, HashSet::from(["df", "passive_radar"]));
        for descriptor in &all {
            assert!(
                !descriptor.name.is_empty(),
                "{} has no name",
                descriptor.type_id
            );
            assert!(
                descriptor.min_lanes >= 2,
                "{} runs on one lane",
                descriptor.type_id
            );
        }
    }

    #[test]
    fn every_descriptor_matches_the_settings_that_name_it() {
        for params in [
            CoherentParams::Df(sdrmm_wire::DfParams::default()),
            CoherentParams::PassiveRadar(sdrmm_wire::PassiveRadarParams::default()),
        ] {
            let descriptor =
                coherent_descriptor(params.type_id()).expect("every params names a processor");
            let lanes = match &params {
                CoherentParams::Df(df) => df.geometry.count() as usize,
                CoherentParams::PassiveRadar(_) => descriptor.min_lanes as usize,
            };
            let ctx = CoherentCtx {
                lanes,
                sample_rate: 2_048_000.0,
                center_hz: 100e6,
            };
            create_coherent(ctx, &params).expect("builds at its own minimum");
        }
    }

    #[test]
    fn a_radio_with_too_few_lanes_is_refused_by_name() {
        let params = CoherentParams::Df(sdrmm_wire::DfParams::default());
        let ctx = CoherentCtx {
            lanes: 1,
            sample_rate: 2_048_000.0,
            center_hz: 100e6,
        };
        let Err(ChannelError::InvalidSettings(message)) = create_coherent(ctx, &params) else {
            panic!("one lane must be refused");
        };
        assert!(message.contains("lanes"), "{message}");
    }
}
