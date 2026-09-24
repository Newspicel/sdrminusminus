mod rsu;

use num_complex::Complex;
use sdrmm_dsp::crc16_x25;
use serde_json::Value;

use super::{
    coherent::CoherentMskDemod,
    decoder::CHANNEL_RATE,
    demod::MskDemod,
    frame::{Scrambler, UW, deinterleave, pack_lsb_first},
    su::{self, AeroUserData, Reassembler},
};
use sdrmm_dsp::SoftViterbi;

pub(super) use rsu::R_SU_LEN;
#[cfg(test)]
pub(super) use rsu::build_r_sus;

const UW_TOLERANCE: u32 = 4;
const UW_SEARCH_BITS: usize = 300;
const SECTION1_CODED: usize = 64 * 5;
const GROUP_CODED: usize = 64 * 3;
const QUIET_SAMPLES: u32 = 256;
const MAX_BURST_SECONDS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BurstChannel {
    R,
    T,
}

pub(super) struct BurstResult {
    pub users: Vec<AeroUserData>,
    pub su_events: Vec<Value>,
    pub is_t: bool,
    pub fec_corrected: u32,
}

pub(super) struct BurstPacketizer {
    viterbi: SoftViterbi,
    t_reassembler: Reassembler,
    r_reassembler: rsu::RIsuReassembler,
}

fn find_uw(bits: &[(f32, u8)]) -> Option<usize> {
    let mut shift = 0u32;
    for (index, &(_, hard)) in bits.iter().enumerate().take(UW_SEARCH_BITS + 32) {
        shift = (shift << 1) | u32::from(hard);
        if index >= 31 && (shift ^ UW).count_ones() <= UW_TOLERANCE {
            return Some(index + 1);
        }
    }
    None
}

impl BurstPacketizer {
    pub(super) fn new() -> Self {
        Self {
            viterbi: SoftViterbi::k7(),
            t_reassembler: Reassembler::default(),
            r_reassembler: rsu::RIsuReassembler::default(),
        }
    }

    pub(super) fn process(&mut self, bits: &[(f32, u8)]) -> Option<BurstResult> {
        let start = find_uw(bits)?;
        let coded: Vec<f32> = bits[start..].iter().map(|&(soft, _)| soft).collect();
        if coded.len() < SECTION1_CODED {
            return None;
        }
        let mut deleaved = Vec::with_capacity(coded.len());
        deinterleave(&coded[..SECTION1_CODED], 5, &mut deleaved);
        let mut offset = SECTION1_CODED;
        while offset + GROUP_CODED <= coded.len() {
            deinterleave(&coded[offset..offset + GROUP_CODED], 3, &mut deleaved);
            offset += GROUP_CODED;
        }
        let mut decoded = self.viterbi.decode(&deleaved);
        let fec_corrected = self
            .viterbi
            .encode(&decoded)
            .iter()
            .zip(&deleaved)
            .filter(|&(&bit, &soft)| bit != u8::from(soft >= 0.0))
            .count() as u32;
        Scrambler::new().apply(&mut decoded);
        let bytes = pack_lsb_first(&decoded);
        if bytes.len() >= 6 && crc16_x25(&bytes[..4]) == u16::from_le_bytes([bytes[4], bytes[5]]) {
            return Some(self.t_burst(&bytes, fec_corrected));
        }
        let unit = bytes.get(..R_SU_LEN)?;
        if !rsu::r_su_crc_ok(unit) {
            return None;
        }
        Some(BurstResult {
            users: self.r_reassembler.push(unit).into_iter().collect(),
            su_events: rsu::parse_r_su(unit).into_iter().collect(),
            is_t: false,
            fec_corrected,
        })
    }

    fn t_burst(&mut self, bytes: &[u8], fec_corrected: u32) -> BurstResult {
        let mut users = Vec::new();
        let mut su_events = Vec::new();
        for unit in bytes[6..].as_chunks::<{ su::SU_LEN }>().0 {
            if !su::su_crc_ok(unit) {
                break;
            }
            su_events.extend(su::parse_p_su(unit));
            users.extend(self.t_reassembler.push(unit));
        }
        BurstResult {
            users,
            su_events,
            is_t: true,
            fec_corrected,
        }
    }
}

pub(super) struct BurstGate {
    noise: f32,
    power: f32,
    burst_power: f32,
    active: Option<Vec<Complex<f32>>>,
    quiet: u32,
    max_samples: usize,
}

impl BurstGate {
    pub(super) fn new(max_samples: usize) -> Self {
        Self {
            noise: 1e-6,
            power: 0.0,
            burst_power: 0.0,
            active: None,
            quiet: 0,
            max_samples,
        }
    }

    pub(super) fn process(&mut self, input: &[Complex<f32>]) -> Vec<Vec<Complex<f32>>> {
        let mut out = Vec::new();
        for &sample in input {
            let power = sample.norm_sqr();
            self.power += 0.2 * (power - self.power);
            let Some(buffer) = &mut self.active else {
                self.noise += 1e-4 * (power - self.noise);
                if self.power > self.noise * 8.0 {
                    self.active = Some(Vec::with_capacity(4096));
                    self.burst_power = self.power;
                    self.quiet = 0;
                }
                continue;
            };
            buffer.push(sample);
            let length = buffer.len();
            self.burst_power += 0.01 * (self.power - self.burst_power).max(0.0);
            if self.power < self.burst_power * 0.1 {
                self.quiet += 1;
            } else {
                self.quiet = 0;
            }
            if self.quiet > QUIET_SAMPLES || length > self.max_samples {
                out.extend(self.active.take());
            }
        }
        out
    }
}

fn cfo_remove(samples: &[Complex<f32>], bit_rate: f64) -> Option<Vec<Complex<f32>>> {
    let samples_per_bit = CHANNEL_RATE / bit_rate;
    let window = (30.0 * samples_per_bit) as usize;
    if samples.len() < window + 16 {
        return None;
    }
    let mut average = 0.0f32;
    let smoothed: Vec<f32> = samples
        .iter()
        .map(|sample| {
            average += 0.1 * (sample.norm_sqr() - average);
            average
        })
        .collect();
    let peak = smoothed.iter().copied().fold(0.0f32, f32::max);
    let start = smoothed
        .iter()
        .position(|&power| power > 0.5 * peak)
        .unwrap_or(0);
    let lead = (2.0 * samples_per_bit) as usize;
    let skip = (start + lead).min(samples.len().saturating_sub(window));
    let cfo = samples[skip..skip + window]
        .windows(2)
        .fold(Complex::new(0.0f32, 0.0), |sum, pair| {
            sum + pair[1] * pair[0].conj()
        })
        .arg();
    let from = start.saturating_sub(lead);
    let mut phase = 0.0f32;
    let mut shifted: Vec<Complex<f32>> = samples[from..]
        .iter()
        .map(|&sample| {
            let mixed = sample * Complex::from_polar(1.0, -phase);
            phase += cfo;
            mixed
        })
        .collect();
    shifted.extend(std::iter::repeat_n(Complex::new(0.0, 0.0), 256));
    Some(shifted)
}

fn demod_burst(samples: &[Complex<f32>], bit_rate: f64, coherent: bool) -> Vec<(f32, u8)> {
    let Some(shifted) = cfo_remove(samples, bit_rate) else {
        return Vec::new();
    };
    let mut bits = Vec::new();
    if coherent {
        CoherentMskDemod::new(CHANNEL_RATE, bit_rate).process(&shifted, &mut bits);
    } else {
        MskDemod::new(CHANNEL_RATE, bit_rate).process(&shifted, &mut bits);
    }
    bits
}

pub(super) struct BurstEvent {
    pub user: Option<AeroUserData>,
    pub su_event: Option<Value>,
    pub bit_rate: u32,
    pub channel: BurstChannel,
    pub fec_corrected: u32,
}

pub(super) struct AeroBurstDecoder {
    gate: BurstGate,
    packetizers: [BurstPacketizer; 2],
}

impl AeroBurstDecoder {
    pub(super) fn new() -> Self {
        Self {
            gate: BurstGate::new(MAX_BURST_SECONDS * CHANNEL_RATE as usize),
            packetizers: [BurstPacketizer::new(), BurstPacketizer::new()],
        }
    }

    pub(super) fn process(&mut self, input: &[Complex<f32>]) -> Vec<BurstEvent> {
        let mut out = Vec::new();
        for burst in self.gate.process(input) {
            for (packetizer, rate) in self.packetizers.iter_mut().zip([600u32, 1200]) {
                let bits = demod_burst(&burst, f64::from(rate), false);
                let result = packetizer
                    .process(&bits)
                    .or_else(|| packetizer.process(&demod_burst(&burst, f64::from(rate), true)));
                if let Some(result) = result {
                    out.extend(events(result, rate));
                    break;
                }
            }
        }
        out
    }
}

fn events(result: BurstResult, bit_rate: u32) -> impl Iterator<Item = BurstEvent> {
    let channel = if result.is_t {
        BurstChannel::T
    } else {
        BurstChannel::R
    };
    let fec_corrected = result.fec_corrected;
    let event = move |user, su_event| BurstEvent {
        user,
        su_event,
        bit_rate,
        channel,
        fec_corrected,
    };
    let users: Vec<BurstEvent> = result
        .users
        .into_iter()
        .map(|user| event(Some(user), None))
        .collect();
    users.into_iter().chain(
        result
            .su_events
            .into_iter()
            .map(move |su_event| event(None, Some(su_event))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inmarsat_aero::{
        frame::interleave,
        modulate::modulate,
        su::build_isu_chain,
        tests::{Noise, acars_user},
    };

    fn burst_bits(decoded_bytes: &[u8]) -> Vec<u8> {
        let mut bits: Vec<u8> = decoded_bytes
            .iter()
            .flat_map(|&byte| (0..8).map(move |index| (byte >> index) & 1))
            .collect();
        Scrambler::new().apply(&mut bits);
        let coded = SoftViterbi::k7().encode(&bits);
        let mut out: Vec<u8> = (0..74).map(|index| (index % 2) as u8).collect();
        out.extend((0..32).rev().map(|index| ((UW >> index) & 1) as u8));
        interleave(&coded[..SECTION1_CODED], 5, &mut out);
        let mut offset = SECTION1_CODED;
        while offset + GROUP_CODED <= coded.len() {
            interleave(&coded[offset..offset + GROUP_CODED], 3, &mut out);
            offset += GROUP_CODED;
        }
        out
    }

    fn burst_iq(
        bits: &[u8],
        rate: f64,
        cfo: f64,
        lead: usize,
        tone_bits: f64,
    ) -> Vec<Complex<f32>> {
        let mut iq = vec![Complex::new(0.0, 0.0); lead];
        let mut phase = 0.0f64;
        for _ in 0..(tone_bits * CHANNEL_RATE / rate) as usize {
            phase += std::f64::consts::TAU * cfo / CHANNEL_RATE;
            iq.push(Complex::from_polar(0.5, phase as f32));
        }
        iq.extend(modulate(bits, rate, CHANNEL_RATE, cfo, 0.5));
        iq.extend(vec![Complex::new(0.0, 0.0); 3000]);
        iq
    }

    fn t_burst_bytes() -> Vec<u8> {
        let mut bytes = vec![0xA1, 0xB2, 0xC3, 0x44];
        bytes.extend(crc16_x25(&bytes).to_le_bytes());
        for unit in build_isu_chain(0xA1B2C3, 0x44, 1, 7, &acars_user()) {
            bytes.extend(unit);
        }
        while bytes.len() < 20 || !(bytes.len() - 20).is_multiple_of(12) {
            bytes.push(0);
        }
        bytes
    }

    fn decode(iq: &[Complex<f32>], decoder: &mut AeroBurstDecoder) -> Vec<BurstEvent> {
        iq.chunks(4096)
            .flat_map(|chunk| decoder.process(chunk))
            .collect()
    }

    #[test]
    fn decodes_t_burst_with_acars() {
        let mut iq = burst_iq(&burst_bits(&t_burst_bytes()), 1200.0, 180.0, 2000, 126.0);
        Noise(0xfeed_f00d_dead_c0de).add(&mut iq, 0.01);
        let events = decode(&iq, &mut AeroBurstDecoder::new());
        let event = events
            .iter()
            .find(|event| event.user.is_some())
            .expect("user data from T burst");
        assert_eq!(event.bit_rate, 1200);
        assert_eq!(event.channel, BurstChannel::T);
        let block = event
            .user
            .as_ref()
            .and_then(|user| su::parse_acars(&user.data))
            .expect("ACARS");
        assert!(block.crc_ok);
        assert_eq!(block.core.tail.as_deref(), Some("VT-ANB"));
    }

    fn channel(channel: sdrmm_wire::AeroChannel) -> crate::inmarsat_aero::InmarsatAeroChannel {
        use crate::{ChannelCtx, ChannelRx};
        crate::inmarsat_aero::InmarsatAeroChannel::new(
            ChannelCtx {
                input_rate: crate::inmarsat_aero::decoder::INPUT_RATE,
            },
            crate::testutil::settings(sdrmm_wire::ChannelParams::InmarsatAero(
                sdrmm_wire::InmarsatAeroParams { channel },
            )),
        )
        .expect("aero channel")
    }

    #[test]
    fn the_burst_setting_decodes_t_bursts_through_the_channel() {
        let mut iq = burst_iq(&burst_bits(&t_burst_bytes()), 1200.0, 180.0, 2000, 126.0);
        Noise(0xfeed_f00d_dead_c0de).add(&mut iq, 0.01);
        let wide =
            crate::testgen::resample(&iq, CHANNEL_RATE, crate::inmarsat_aero::decoder::INPUT_RATE);
        let mut padded = wide;
        padded.extend(std::iter::repeat_n(Complex::new(0.0, 0.0), 48_000));
        let bursts =
            crate::testutil::run_events(&mut channel(sdrmm_wire::AeroChannel::Burst), &padded);
        let acars = bursts
            .iter()
            .find_map(|event| match event {
                sdrmm_wire::DecoderEvent::InmarsatAero(message)
                    if message.message_type == "acars" =>
                {
                    Some(message)
                }
                _ => None,
            })
            .expect("ACARS from the T burst");
        assert!(acars.crc_ok);
        assert_eq!(acars.station.as_deref(), Some("VT-ANB"));
        let forward =
            crate::testutil::run_events(&mut channel(sdrmm_wire::AeroChannel::P), &padded);
        assert!(
            forward.is_empty(),
            "the P channel must not read bursts: {forward:?}"
        );
    }

    #[test]
    fn decodes_r_burst() {
        let payload: Vec<u8> = (0..25).map(|index| index as u8 ^ 0x33).collect();
        let units = build_r_sus(0x123456, 0x07, 2, 3, &payload);
        assert_eq!(units.len(), 3);
        let mut decoder = AeroBurstDecoder::new();
        let mut events = Vec::new();
        for unit in &units {
            let mut bytes = unit.clone();
            bytes.resize(20, 0);
            let iq = burst_iq(&burst_bits(&bytes), 600.0, -90.0, 1500, 150.0);
            events.extend(decode(&iq, &mut decoder));
        }
        assert_eq!(events.len(), 1);
        let user = events[0].user.as_ref().expect("user");
        assert_eq!(user.data, payload);
        assert_eq!(user.aes_id, "123456");
        assert_eq!(events[0].bit_rate, 600);
        assert_eq!(events[0].channel, BurstChannel::R);
    }

    #[test]
    fn decodes_r_control_su() {
        let mut unit = vec![0u8; R_SU_LEN];
        unit[2] = 0x30;
        let crc = crc16_x25(&unit[..17]);
        unit[17..].copy_from_slice(&crc.to_le_bytes());
        unit.push(0);
        let iq = burst_iq(&burst_bits(&unit), 600.0, -90.0, 1500, 150.0);
        let events = decode(&iq, &mut AeroBurstDecoder::new());
        let event = events
            .iter()
            .find(|event| {
                event
                    .su_event
                    .as_ref()
                    .is_some_and(|value| value["su_type"] == "r-call-progress")
            })
            .expect("R control event");
        assert_eq!(event.bit_rate, 600);
        assert_eq!(event.channel, BurstChannel::R);
    }

    #[test]
    fn matches_xng_on_a_t_burst() {
        let mut iq = burst_iq(&burst_bits(&t_burst_bytes()), 1200.0, 180.0, 2000, 126.0);
        Noise(0xfeed_f00d_dead_c0de).add(&mut iq, 0.01);
        let ours: Vec<(Vec<u8>, Option<u32>)> = decode(&iq, &mut AeroBurstDecoder::new())
            .into_iter()
            .filter_map(|event| {
                let fec = event.fec_corrected;
                event.user.map(|user| (user.data, Some(fec)))
            })
            .collect();
        let mut reference = xng_mode_aero::AeroBurstDecoder::new(CHANNEL_RATE, 0.0).expect("xng");
        let theirs: Vec<(Vec<u8>, Option<u32>)> = iq
            .chunks(4096)
            .flat_map(|chunk| reference.process(chunk))
            .filter(|event| event.su_event.is_none())
            .map(|event| (event.user.data, event.fec_corrected))
            .collect();
        assert!(!ours.is_empty());
        assert_eq!(ours, theirs);
    }
}
