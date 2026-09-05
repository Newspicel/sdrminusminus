use sdrmm_channels::coherent::CoherentOutputs;
use sdrmm_wire::CalState;

use super::Publisher;
use crate::{
    coherent::{CoherentSinks, CoherentUpdate, SurfaceUpdate},
    runtime::DecodedSink,
};

struct Packet {
    outputs: CoherentOutputs,
    cal: CalState,
    frequency: f64,
    report: bool,
}

pub(crate) struct CoherentPublisher {
    queue: Publisher<Packet>,
    decoded: DecodedSink,
}

impl CoherentPublisher {
    pub(crate) fn new(node: u32, sinks: CoherentSinks) -> std::io::Result<Self> {
        let decoded = sinks.decoded.clone();
        let mut sequence = 0u32;
        let queue = Publisher::new(
            "sdrmm-coherent-publish",
            16,
            || Packet {
                outputs: CoherentOutputs {
                    events: Vec::with_capacity(16),
                    detections: Vec::with_capacity(64),
                    ..Default::default()
                },
                cal: CalState {
                    lanes: Vec::with_capacity(sdrmm_wire::MAX_STREAMS as usize),
                    ..Default::default()
                },
                frequency: 0.0,
                report: false,
            },
            move |packet| {
                for event in packet.outputs.events.drain(..) {
                    sinks.decoded.publish(packet.frequency, event);
                }
                if let Some(surface) = packet.outputs.surface.take() {
                    sequence = sequence.wrapping_add(1);
                    let _ = sinks.surfaces.send(SurfaceUpdate {
                        node,
                        seq: sequence,
                        surface: std::sync::Arc::new(surface),
                    });
                }
                if packet.report {
                    let _ = sinks.updates.send(CoherentUpdate {
                        node,
                        reading: packet.outputs.bearing.take(),
                        detections: packet.outputs.detections.clone(),
                        cal: packet.cal.clone(),
                    });
                }
                packet.outputs.reset();
            },
            || {},
        )?;
        Ok(Self { queue, decoded })
    }

    pub(crate) fn publish(
        &mut self,
        outputs: &mut CoherentOutputs,
        cal: &CalState,
        frequency: f64,
        report: bool,
    ) {
        if !self.queue.submit(|packet| {
            std::mem::swap(&mut packet.outputs, outputs);
            packet.cal.tier = cal.tier;
            packet.cal.phase_unknown = cal.phase_unknown;
            packet.cal.solved = cal.solved;
            packet.cal.lanes.clear();
            packet.cal.lanes.extend_from_slice(&cal.lanes);
            packet.frequency = frequency;
            packet.report = report;
        }) {
            self.decoded.note_lost(1);
        }
    }
}
