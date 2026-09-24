use num_complex::Complex;
use serde_json::Value;

use super::{
    acars_block::AcarsBlock,
    demod::MskDemod,
    frame::FrameHeader,
    framer::{FrameSink, Framer},
    oqpsk::{self, HrFramer, OqpskDemod},
    su::{self, AeroUserData},
    taps::{Fir, lowpass_taps},
};

pub(super) const INPUT_RATE: f64 = 48_000.0;
pub(super) const CHANNEL_RATE: f64 = 24_000.0;
const DECIMATION: usize = 2;
const PASSBAND_HZ: f64 = 2_500.0;
const FRONT_TAPS: usize = 15;
const LOW_RATES: [u32; 2] = [600, 1200];

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
    fn from_sink(sink: &FrameSink, bit_rate: u32, lock: Option<Value>) -> Self {
        Self {
            bit_rate,
            frame_header: sink.last_header,
            satellite: sink.resolver.details(),
            fec_corrected: sink.last_fec_corrected,
            lock,
        }
    }

    fn event(&self, user: AeroUserData, acars: Option<AcarsBlock>, su_event: Option<Value>) -> AeroEvent {
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

fn emit(
    sink: &mut FrameSink,
    latched: &Latched,
    users: &mut Vec<AeroUserData>,
    out: &mut Vec<AeroEvent>,
) {
    for user in users.drain(..) {
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

struct RateChain {
    rate: u32,
    demod: MskDemod,
    framer: Framer,
}

struct HighRateChain {
    demod: OqpskDemod,
    framer: HrFramer,
}

pub(super) struct AeroChannelDecoder {
    front: Fir,
    channel: Vec<Complex<f32>>,
    chains: [RateChain; 2],
    high_rate: HighRateChain,
    bits: Vec<(f32, u8)>,
    users: Vec<AeroUserData>,
}

impl AeroChannelDecoder {
    pub(super) fn new() -> Self {
        let chain = |rate: u32| RateChain {
            rate,
            demod: MskDemod::new(CHANNEL_RATE, f64::from(rate)),
            framer: Framer::new(rate),
        };
        Self {
            front: Fir::new(lowpass_taps(PASSBAND_HZ / INPUT_RATE, FRONT_TAPS), DECIMATION),
            channel: Vec::new(),
            chains: LOW_RATES.map(chain),
            high_rate: HighRateChain {
                demod: OqpskDemod::new(oqpsk::CHANNEL_RATE_HR),
                framer: HrFramer::new(),
            },
            bits: Vec::new(),
            users: Vec::new(),
        }
    }

    pub(super) fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<AeroEvent>) {
        self.channel.clear();
        self.front.process(input, &mut self.channel);
        for chain in &mut self.chains {
            self.bits.clear();
            chain.demod.process(&self.channel, &mut self.bits);
            for &(soft, hard) in &self.bits {
                chain.framer.push(soft, hard, &mut self.users);
            }
            let sink = &mut chain.framer.sink;
            if self.users.is_empty() && sink.su_events.is_empty() {
                continue;
            }
            let lock = Some(chain.framer.lock.details_json());
            let latched = Latched::from_sink(sink, chain.rate, lock);
            emit(sink, &latched, &mut self.users, out);
        }
        self.process_high_rate(input, out);
    }

    fn process_high_rate(&mut self, input: &[Complex<f32>], out: &mut Vec<AeroEvent>) {
        let chain = &mut self.high_rate;
        self.bits.clear();
        chain.demod.process(input, &mut self.bits);
        for &(soft, hard) in &self.bits {
            chain.framer.push(soft, hard, &mut self.users);
        }
        let sink = &mut chain.framer.sink;
        if self.users.is_empty() && sink.su_events.is_empty() {
            return;
        }
        let latched = Latched::from_sink(sink, oqpsk::BIT_RATE, None);
        emit(sink, &latched, &mut self.users, out);
    }
}
