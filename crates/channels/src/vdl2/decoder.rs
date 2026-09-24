use std::f64::consts::PI;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, ReedSolomon};
use serde_json::{Value, json};

use crate::acars::block::{AcarsBlock, parse as parse_acars};

use super::atn::{self, ClnpReassembler, CotpReassembler, X25Reassembler};
use super::avlc::{AvlcFrame, Control, Payload};
use super::demod::{Burst, SYMBOL_RATE, Vdl2Demod};
use super::interleave;

const SELECTIVITY_TAPS: usize = 101;
const SELECTIVITY_CUTOFF: f64 = 0.7 * SYMBOL_RATE;
const ACARS_FILL: u8 = 0xFF;

pub struct Vdl2Frame {
    pub avlc: AvlcFrame,
    pub acars: Option<AcarsBlock>,
    pub atn: Option<Value>,
    pub rs_corrected: usize,
    pub freq_skew_hz: f32,
    pub snr_db: f32,
}

struct Reassembly {
    x25: X25Reassembler,
    clnp: ClnpReassembler,
    cotp: CotpReassembler,
}

pub struct Vdl2Decoder {
    selectivity: Decimator,
    filtered: Vec<Complex<f32>>,
    demod: Vdl2Demod,
    bursts: Vec<Burst>,
    rs: ReedSolomon,
    reassembly: Reassembly,
    samples_seen: u64,
    input_rate: f64,
}

impl Vdl2Decoder {
    pub fn new(input_rate: f64) -> Self {
        Self {
            selectivity: selectivity(SELECTIVITY_CUTOFF / input_rate),
            filtered: Vec::new(),
            demod: Vdl2Demod::new(input_rate),
            bursts: Vec::new(),
            rs: interleave::vdl2_rs(),
            reassembly: Reassembly {
                x25: X25Reassembler::new(),
                clnp: ClnpReassembler::new(),
                cotp: CotpReassembler::new(),
            },
            samples_seen: 0,
            input_rate,
        }
    }

    #[cfg(test)]
    pub fn differential(input_rate: f64) -> Self {
        Self {
            selectivity: selectivity(SYMBOL_RATE / input_rate),
            demod: Vdl2Demod::differential(input_rate),
            ..Self::new(input_rate)
        }
    }

    pub fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<Vdl2Frame>) {
        self.samples_seen += input.len() as u64;
        let now = self.samples_seen as f64 / self.input_rate;
        self.selectivity.process(input, &mut self.filtered);
        self.bursts.clear();
        self.demod
            .process(&self.filtered, &self.rs, &mut self.bursts);
        for burst in &mut self.bursts {
            for frame in std::mem::take(&mut burst.frames) {
                out.push(self.reassembly.frame(frame, burst, now));
            }
        }
    }
}

impl Reassembly {
    fn frame(&mut self, frame: AvlcFrame, burst: &Burst, now: f64) -> Vdl2Frame {
        let acars = match frame.payload {
            Payload::Acars => {
                let start = frame
                    .info
                    .iter()
                    .position(|&b| b != ACARS_FILL)
                    .unwrap_or(0);
                parse_acars(&frame.info[start..])
            }
            _ => None,
        };
        let atn = if acars.is_none() && matches!(frame.control, Control::Info { .. }) {
            self.decode_atn(&frame.info, now)
        } else {
            None
        };
        Vdl2Frame {
            avlc: frame,
            acars,
            atn,
            rs_corrected: burst.rs_corrected,
            freq_skew_hz: burst.freq_skew_hz,
            snr_db: burst.snr_db,
        }
    }

    fn decode_atn(&mut self, info: &[u8], now: f64) -> Option<Value> {
        let Some(pkt) = atn::parse_x25(info) else {
            return decode_network(info, &mut self.clnp, &mut self.cotp, now);
        };
        let mut v = serde_json::to_value(&pkt).unwrap_or_default();
        v["layer"] = json!("x25");
        if pkt.kind == "data" {
            if let Some(full) = self.x25.push(&pkt, now) {
                if let Some(net) = decode_network(&full, &mut self.clnp, &mut self.cotp, now) {
                    v["network"] = net;
                }
            } else {
                v["reassembling"] = json!(true);
            }
        } else if !pkt.payload.is_empty()
            && let Some(net) = decode_network(&pkt.payload, &mut self.clnp, &mut self.cotp, now)
        {
            v["network"] = net;
        }
        Some(v)
    }
}

pub(super) fn decode_network(
    b: &[u8],
    clnp: &mut ClnpReassembler,
    cotp: &mut CotpReassembler,
    now: f64,
) -> Option<Value> {
    if b.first() != Some(&0x81) {
        return atn::parse_network(b);
    }
    match clnp.push(b, now) {
        Some(full) => {
            let mut v = atn::parse_network(&full)?;
            cotp_reassemble(&full, &mut v, cotp, now);
            Some(v)
        }
        None => {
            let mut v = atn::parse_network(b)?;
            v["reassembling"] = json!(true);
            Some(v)
        }
    }
}

fn cotp_reassemble(full: &[u8], v: &mut Value, cotp: &mut CotpReassembler, now: f64) {
    let Some(tpdu) = atn::clnp_cotp_tpdu(full) else {
        return;
    };
    let Some((_, eot, seq, _)) = atn::cotp_dt_segment(tpdu) else {
        return;
    };
    if seq == 0 && eot {
        return;
    }
    match cotp.push(tpdu, now) {
        Some(tsdu) => {
            if let Some(app) = atn::parse_cotp_user_app(&tsdu)
                && let Some(c) = v.get_mut("cotp")
            {
                c["app"] = app;
                c["tsdu_reassembled"] = json!(true);
                c["tsdu_len"] = json!(tsdu.len());
            }
        }
        None => {
            if let Some(c) = v.get_mut("cotp") {
                c["tsdu_reassembling"] = json!(true);
            }
        }
    }
}

fn selectivity(cutoff: f64) -> Decimator {
    Decimator::new(&lowpass_taps(cutoff, SELECTIVITY_TAPS), 1)
}

fn lowpass_taps(cutoff: f64, num_taps: usize) -> Vec<f32> {
    const A: [f64; 4] = [0.35875, 0.48829, 0.14128, 0.01168];
    let last = num_taps as f64 - 1.0;
    let center = last / 2.0;
    let taps: Vec<f64> = (0..num_taps)
        .map(|i| {
            let x = 2.0 * PI * i as f64 / last;
            let window = A[0] - A[1] * x.cos() + A[2] * (2.0 * x).cos() - A[3] * (3.0 * x).cos();
            let t = i as f64 - center;
            let sinc = if t.abs() < 1e-12 {
                2.0 * cutoff
            } else {
                (2.0 * PI * cutoff * t).sin() / (PI * t)
            };
            sinc * window
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    taps.into_iter().map(|t| (t / sum) as f32).collect()
}

#[cfg(test)]
mod cotp_pipeline_tests {
    use super::super::cpdlc;
    use super::*;

    fn clnp_dt(cotp: &[u8]) -> Vec<u8> {
        let mut b = vec![0x81, 15, 1, 0x3F, 0x1C, 0x00, 0x00, 0x00, 0x00];
        b.extend_from_slice(&[2, 0x47, 0x01]);
        b.extend_from_slice(&[2, 0x47, 0x02]);
        b.extend_from_slice(cotp);
        b
    }

    fn cotp_dt(dst_ref: u16, eot: bool, seq: u8, user: &[u8]) -> Vec<u8> {
        let mut b = vec![0x04, 0xF0];
        b.extend_from_slice(&dst_ref.to_be_bytes());
        b.push(if eot { 0x80 } else { 0x00 } | (seq & 0x7F));
        b.extend_from_slice(user);
        b
    }

    #[test]
    fn cpdlc_reassembled_across_two_cotp_dt_segments() {
        let apdu = cpdlc::build_downlink_wilco_for_test();
        assert!(apdu.len() >= 2, "need at least two octets to split");
        let split = apdu.len() / 2;
        let s0 = cotp_dt(0x0001, false, 0, &apdu[..split]);
        let s1 = cotp_dt(0x0001, true, 1, &apdu[split..]);
        let mut clnp = ClnpReassembler::new();
        let mut cotp = CotpReassembler::new();
        let v0 = decode_network(&clnp_dt(&s0), &mut clnp, &mut cotp, 0.0).unwrap();
        assert_eq!(v0["cotp"]["tpdu"], "DT");
        assert_eq!(v0["cotp"]["tsdu_segment"], true);
        assert!(v0["cotp"].get("app").is_none());
        assert_eq!(v0["cotp"]["tsdu_reassembling"], true);
        let v1 = decode_network(&clnp_dt(&s1), &mut clnp, &mut cotp, 1.0).unwrap();
        assert_eq!(v1["cotp"]["tsdu_reassembled"], true);
        let app = &v1["cotp"]["app"];
        assert_eq!(app["application"], "CPDLC");
        assert_eq!(app["pdu"], "send");
        assert_eq!(app["message"]["elements"][0]["element"], "dM0NULL");
    }

    #[test]
    fn single_segment_cotp_still_decodes_inline() {
        let apdu = cpdlc::build_downlink_wilco_for_test();
        let dt = cotp_dt(0x0002, true, 0, &apdu);
        let mut clnp = ClnpReassembler::new();
        let mut cotp = CotpReassembler::new();
        let v = decode_network(&clnp_dt(&dt), &mut clnp, &mut cotp, 0.0).unwrap();
        assert_eq!(v["cotp"]["app"]["pdu"], "send");
        assert!(v["cotp"].get("tsdu_reassembled").is_none());
    }
}
