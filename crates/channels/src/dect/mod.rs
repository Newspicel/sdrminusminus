pub(crate) mod burst;
mod identity;
pub(crate) mod mac;
mod station;

#[cfg(test)]
mod tests;

use std::sync::LazyLock;

use burst::{Burst, INPUT_RATE_HZ, OCCUPIED_BANDWIDTH_HZ};
use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass};
use sdrmm_wire::{ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, DectParams};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const CHANNEL_TAPS: usize = 63;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "dect".to_owned(),
    name: "DECT".to_owned(),
    bandwidth_hz: OCCUPIED_BANDWIDTH_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    decoder_kind: Some("dect".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct DectChannel {
    detector: burst::Detector,
    tracker: station::Tracker,
    bursts: Vec<Burst>,
    params: DectParams,
}

fn params(settings: &ChannelSettings) -> Result<&DectParams, ChannelError> {
    match &settings.params {
        ChannelParams::Dect(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "dect channel got {} params",
            other.type_id()
        ))),
    }
}

pub(crate) fn occupied_band() -> (f64, f64) {
    (-OCCUPIED_BANDWIDTH_HZ / 2.0, OCCUPIED_BANDWIDTH_HZ / 2.0)
}

pub(crate) fn channel_filter() -> ChannelFilter {
    ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(CHANNEL_TAPS, OCCUPIED_BANDWIDTH_HZ / 2.0 / INPUT_RATE_HZ),
        1,
    ))
}

impl ChannelRx for DectChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let params = *params(&settings)?;
        Ok(Self {
            detector: burst::Detector::new(params.sides.accepts_rfp(), params.sides.accepts_pp()),
            tracker: station::Tracker::new(params.band),
            bursts: Vec::new(),
            params,
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let params = *params(&settings)?;
        if params.sides != self.params.sides {
            self.detector
                .set_sides(params.sides.accepts_rfp(), params.sides.accepts_pp());
            self.tracker.clear();
        }
        if params.band != self.params.band {
            self.tracker.set_band(params.band);
        }
        self.params = params;
        Ok(())
    }

    fn retuned(&mut self) {
        self.tracker.clear();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.bursts.clear();
        self.detector.process(iq, &mut self.bursts);
        for burst in &self.bursts {
            if let Some(frame) = self.tracker.apply(burst) {
                out.events.push(DecoderEvent::Dect(frame));
            }
        }
    }
}
