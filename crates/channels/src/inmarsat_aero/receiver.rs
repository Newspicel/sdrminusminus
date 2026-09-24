use num_complex::Complex;
use serde_json::{Value, json};

use super::{
    burst::{AeroBurstDecoder, BurstChannel, BurstEvent},
    cchannel::{CChannelDeframer, CChannelEvent, su_type_name},
    decoder::{INPUT_RATE, front_filter},
    oqpsk::{self, OqpskDemod},
    taps::Fir,
};

const VOICE_GAP_SECONDS: f64 = 1.0;

pub(super) struct BurstReceiver {
    front: Fir,
    channel: Vec<Complex<f32>>,
    decoder: AeroBurstDecoder,
}

impl BurstReceiver {
    pub(super) fn new() -> Self {
        Self {
            front: front_filter(),
            channel: Vec::new(),
            decoder: AeroBurstDecoder::new(),
        }
    }

    pub(super) fn process(&mut self, input: &[Complex<f32>]) -> Vec<BurstEvent> {
        self.channel.clear();
        self.front.process(input, &mut self.channel);
        self.decoder.process(&self.channel)
    }
}

pub(super) fn burst_tag(channel: BurstChannel) -> &'static str {
    match channel {
        BurstChannel::R => "r-channel",
        BurstChannel::T => "t-channel",
    }
}

pub(super) enum CircuitEvent {
    SignalUnit { kind: &'static str, details: Value },
    VoiceStarted,
}

pub(super) struct CircuitReceiver {
    demod: OqpskDemod,
    deframer: CChannelDeframer,
    soft: Vec<(f32, u8)>,
    samples: u64,
    last_voice: Option<u64>,
}

impl CircuitReceiver {
    pub(super) fn new() -> Self {
        Self {
            demod: OqpskDemod::new_c_channel(oqpsk::CHANNEL_RATE_HR),
            deframer: CChannelDeframer::new(),
            soft: Vec::new(),
            samples: 0,
            last_voice: None,
        }
    }

    pub(super) fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<CircuitEvent>) {
        self.samples += input.len() as u64;
        self.soft.clear();
        self.demod.process(input, &mut self.soft);
        for index in 0..self.soft.len() {
            let (soft, _) = self.soft[index];
            for event in self.deframer.push(soft) {
                if let Some(event) = self.translate(event) {
                    out.push(event);
                }
            }
        }
    }

    fn translate(&mut self, event: CChannelEvent) -> Option<CircuitEvent> {
        match event {
            CChannelEvent::SignalUnit(su) => Some(CircuitEvent::SignalUnit {
                kind: su_type_name(su[0]),
                details: json!({ "channel": "c-channel", "su": crate::datalink::hex(&su) }),
            }),
            CChannelEvent::Voice(_) => {
                let gap = (VOICE_GAP_SECONDS * INPUT_RATE) as u64;
                let fresh = self
                    .last_voice
                    .is_none_or(|last| self.samples.saturating_sub(last) > gap);
                self.last_voice = Some(self.samples);
                fresh.then_some(CircuitEvent::VoiceStarted)
            }
        }
    }
}
