use num_complex::Complex;
use serde_json::Value;

use super::{
    acars_block::AcarsBlock,
    demod::MskDemod,
    frame::{FRAME_BITS, FrameHeader},
    framer::{FrameSink, Framer, MergeTiming},
    msk::CoherentMsk,
    oqpsk::{self, HrFramer, OqpskDemod},
    su::{self, AeroUserData},
    taps::{Fir, lowpass_taps},
};

pub(crate) const INPUT_RATE: f64 = 48_000.0;
pub(super) const CHANNEL_RATE: f64 = 24_000.0;
const DECIMATION: usize = 2;
const PASSBAND_HZ: f64 = 2_500.0;
const FRONT_TAPS: usize = 15;
const LOW_RATES: [u32; 2] = [600, 1200];
const MERGE_WINDOW_BITS: u64 = FRAME_BITS as u64 / 2;
const MERGE_SETTLE_BITS: u64 = 4;
const DISCRIMINATOR: usize = 0;
const COHERENT: usize = 1;

pub(super) struct AeroEvent {
    pub user: AeroUserData,
    pub acars: Option<AcarsBlock>,
    pub bit_rate: u32,
    pub su_event: Option<Value>,
    pub frame_header: Option<FrameHeader>,
    pub satellite: Option<Value>,
    pub fec_corrected: Option<u32>,
    pub lock: Option<Value>,
}

struct Latched {
    bit_rate: u32,
    frame_header: Option<FrameHeader>,
    satellite: Option<Value>,
    fec_corrected: Option<u32>,
    lock: Option<Value>,
}

impl Latched {
    fn event(
        &self,
        user: AeroUserData,
        acars: Option<AcarsBlock>,
        su_event: Option<Value>,
    ) -> AeroEvent {
        AeroEvent {
            user,
            acars,
            bit_rate: self.bit_rate,
            su_event,
            frame_header: self.frame_header,
            satellite: self.satellite.clone(),
            fec_corrected: self.fec_corrected,
            lock: self.lock.clone(),
        }
    }
}

fn emit(sink: &mut FrameSink, bit_rate: u32, with_lock: bool, out: &mut Vec<AeroEvent>) {
    if !sink.has_output() {
        return;
    }
    let latched = Latched {
        bit_rate,
        frame_header: sink.last_header,
        satellite: sink.resolver.details(),
        fec_corrected: sink.last_fec_corrected,
        lock: with_lock.then(|| sink.lock.details_json()),
    };
    for user in sink.users.drain(..) {
        let acars = su::parse_acars(&user.data);
        out.push(latched.event(user, acars, None));
    }
    for event in sink.su_events.drain(..) {
        let user = AeroUserData {
            aes_id: event["aes_id"].as_str().unwrap_or("").to_owned(),
            ges_id: event["ges_id"].as_u64().unwrap_or(0) as u8,
            qno: 0,
            refno: 0,
            data: Vec::new(),
        };
        out.push(latched.event(user, None, Some(event)));
    }
}

struct Detector<D> {
    demod: D,
    framer: Framer,
}

struct RateChain {
    rate: u32,
    sink: FrameSink,
    samples: u64,
    discriminator: Detector<MskDemod>,
    coherent: Option<Detector<CoherentMsk>>,
}

impl RateChain {
    fn new(rate: u32, combined: bool) -> Self {
        let samples_per_bit = (CHANNEL_RATE / f64::from(rate)) as u64;
        let merge = MergeTiming {
            window: MERGE_WINDOW_BITS * samples_per_bit,
            settle: MERGE_SETTLE_BITS * samples_per_bit,
        };
        Self {
            rate,
            sink: FrameSink::new(combined.then_some(merge)),
            samples: 0,
            discriminator: Detector {
                demod: MskDemod::new(CHANNEL_RATE, f64::from(rate)),
                framer: Framer::new(rate, combined),
            },
            coherent: combined.then(|| Detector {
                demod: CoherentMsk::new(CHANNEL_RATE, f64::from(rate)),
                framer: Framer::new(rate, true),
            }),
        }
    }

    fn process(&mut self, channel: &[Complex<f32>], bits: &mut Vec<(f32, u8)>) {
        self.samples += channel.len() as u64;
        bits.clear();
        self.discriminator.demod.process(channel, bits);
        for &(soft, hard) in bits.iter() {
            if let Some(frame) = self.discriminator.framer.push(soft, hard) {
                self.sink.offer(DISCRIMINATOR, self.samples, frame);
            }
        }
        if let Some(coherent) = &mut self.coherent {
            bits.clear();
            coherent.demod.process(channel, bits);
            for &(soft, hard) in bits.iter() {
                if let Some(frame) = coherent.framer.push(soft, hard) {
                    self.sink.offer(COHERENT, self.samples, frame);
                }
            }
        }
        self.sink.expire(self.samples);
    }
}

struct HighRateChain {
    demod: OqpskDemod,
    framer: HrFramer,
    sink: FrameSink,
}

pub(super) struct AeroChannelDecoder {
    front: Fir,
    channel: Vec<Complex<f32>>,
    chains: [RateChain; 2],
    high_rate: HighRateChain,
    bits: Vec<(f32, u8)>,
}

pub(super) fn front_filter() -> Fir {
    Fir::new(
        lowpass_taps(PASSBAND_HZ / INPUT_RATE, FRONT_TAPS),
        DECIMATION,
    )
}

impl AeroChannelDecoder {
    pub(super) fn new() -> Self {
        Self::with_detection(true)
    }

    #[cfg(test)]
    pub(super) fn discriminator_only() -> Self {
        Self::with_detection(false)
    }

    fn with_detection(combined: bool) -> Self {
        Self {
            front: front_filter(),
            channel: Vec::new(),
            chains: LOW_RATES.map(|rate| RateChain::new(rate, combined)),
            high_rate: HighRateChain {
                demod: OqpskDemod::new(oqpsk::CHANNEL_RATE_HR),
                framer: HrFramer::new(),
                sink: FrameSink::new(None),
            },
            bits: Vec::new(),
        }
    }

    pub(super) fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<AeroEvent>) {
        self.channel.clear();
        self.front.process(input, &mut self.channel);
        for chain in &mut self.chains {
            chain.process(&self.channel, &mut self.bits);
            emit(&mut chain.sink, chain.rate, true, out);
        }
        let chain = &mut self.high_rate;
        self.bits.clear();
        chain.demod.process(input, &mut self.bits);
        for &(soft, hard) in &self.bits {
            if let Some(frame) = chain.framer.push(soft, hard) {
                chain.sink.offer(DISCRIMINATOR, 0, frame);
            }
        }
        emit(&mut chain.sink, oqpsk::BIT_RATE, false, out);
    }
}
