//! AIS decoder ( P2): 9600 baud GMSK, NRZI + HDLC framing, CRC-16/X-25.
//!
//! Receiver chain: feedforward carrier-offset corrector → `sdrmm_modem::cpm` GMSK front end
//! (quadrature discriminator → Gaussian matched filter → `SymbolSync` → normalised soft
//! symbols; discriminator tier — no coherent carrier recovery, which is what fits the Pi
//! budget at 48 kHz and 5 samples per bit) → NRZI → HDLC deframer. The front end is a catalog
//! entry described by data alone ( §3.3): M = 2, ±2400 Hz at 9600 baud (h = ½),
//! Gaussian frequency pulse at BT 0.4. Of the old chain's two hand-tuned defences, the
//! engine's floor-settled carrier gate takes over the discriminator clamp's job (keying
//! transients kept out of every estimate) and the corrector takes over the DC blocker's
//! (dial error off the eye within the training sequence — see [`AisChannelRx`]'s field docs
//! for why burst acquisition cannot wait for the engine's slower centre loop). What AIS
//! knows that the library does not is only NRZI, HDLC and the ITU-R M.1371 field layout.
//!
//! Two bit orders meet here. HDLC packs the wire LSB-first into octets and computes the FCS
//! over those octets, while ITU-R M.1371 defines every message field big-endian over the wire
//! bit order — so the deframed octets are bit-reversed once, and only then read as fields.

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{
    Decimator, HdlcDeframer, NrziDecoder, bits_be, design_lowpass, hdlc_fcs_ok, reverse_byte,
};
use sdrmm_modem::{
    cpm::{CpmDemod, CpmParams, Mapping, TIMING_BW_BURST},
    pulse::{self, Norm},
};
use sdrmm_wire::{
    AisMessage, AisParams, ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const CHANNEL_TAPS: usize = 129;

/// ITU-R M.1371 Annex 2 §2.2: 9600 bit/s GMSK, ±2400 Hz deviation, BT 0.4.
const BAUD: f64 = 9_600.0;
const DEVIATION_HZ: f64 = 2_400.0;
const BT: f64 = 0.4;
/// Total span of the entry's GMSK frequency pulse in symbol periods: the NRZ rect's own
/// symbol plus a four-symbol truncation of the BT 0.4 Gaussian premod filter — the shaping
/// span the reference transmitter (`testgen::ais`) has always used.
const PULSE_SPAN: usize = 5;
/// Matched-filter truncation: total span in symbol periods (`span·sps` taps). Three keeps the
/// combined transmit+receive Gaussian's inter-symbol interference inside the eye at 5 samples
/// per bit.
const MATCHED_SPAN: usize = 3;

/// Shortest ITU-R M.1371 message (88 bits, type 15) plus the two FCS octets, and the longest
/// (a five-slot 1008-bit binary broadcast) plus the same.
const MIN_FRAME_BYTES: usize = 13;
const MAX_FRAME_BYTES: usize = 128;

/// "Not available" codes: 102.3 kt, 360.0°, 511°.
const SOG_UNAVAILABLE: u64 = 1_023;
const COG_INVALID: u64 = 3_600;
const HEADING_UNAVAILABLE: u16 = 511;
/// Positions travel in 1/10 000 minute units, so a degree is 600 000 of them. The 91°/181°
/// "not available" codes fall out of the coordinate range check rather than being special-cased.
const COORD_UNITS_PER_DEGREE: f64 = 600_000.0;

/// NMEA 0183 caps a sentence at 82 characters and `!AIVDM,n,n,s,X,,f*hh` costs 20 of them.
const MAX_PAYLOAD_CHARS: usize = 60;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "ais".to_owned(),
    name: "AIS".to_owned(),
    bandwidth_hz: 25_000.0,
    input_rate_hz: 48_000.0,
    has_audio: false,
    decoder_kind: Some("ais".to_owned()),
    ..ChannelDescriptor::default()
});

/// Time constant of the carrier-offset corrector, in samples — the old chain's DC blocker
/// corner (~38 Hz at 48 kHz) restated, and the same reasoning holds: ~40 bit periods is far
/// above the six bits HDLC's stuffing lets the line hold one level, so the estimate cannot
/// chase the data, yet a burst's 24-bit training sequence plus its opening flag is enough to
/// take a receiver frequency error off the eye before the payload begins.
const CARRIER_TAU_SAMPLES: f64 = 200.0;

pub struct AisChannelRx {
    letter: char,
    demod: CpmDemod,
    /// The M = 2 level table the soft symbols are sliced against.
    slicer: Mapping,
    nrzi: NrziDecoder,
    deframer: HdlcDeframer,
    /// Amplitude-weighted circular mean of `x[n]·conj(x[n−1])`: its argument is the mean
    /// per-sample phase advance — the receiver frequency error, since the modulation's own
    /// mean is held near zero by the NRZI line balance the stuffing enforces.
    ///
    /// Why the channel corrects the carrier *before* the cpm front end instead of leaving
    /// the offset to the engine's centre estimate: a 1200 Hz dial error (7 ppm at 162 MHz,
    /// which the old chain decoded through) is half the eye on the discriminator, and the
    /// engine's centre loop is deliberately slower than one AIS burst. Worse, until the bias
    /// is gone the timing detector's symmetry gate rejects the very transitions the 24-bit
    /// training sequence supplies, so the clock never acquires — measured here: 0/5 burst
    /// phases decode at ±1200 Hz without this corrector, 3/5 at ±400 Hz.
    ///
    /// Why *amplitude-weighted* (the one thing the old chain's post-discriminator DC blocker
    /// could not be): each sample votes with its power, so the selection filter's band-edge
    /// precursor ripple (~60 dB down) and the dead channel's noise cannot move an estimate a
    /// burst's body has set, while a burst arriving over a cold estimate re-converges within
    /// samples because its power outweighs the accumulated noise by the same margin.
    carrier_acc: Complex<f32>,
    carrier_alpha: f32,
    carrier_prev: Complex<f32>,
    /// Derotation phase in radians, advanced by `arg(carrier_acc)` each sample; f64 so a
    /// long watch cannot spend the mantissa on accumulated turns.
    carrier_phase: f64,
    mixed: Vec<Complex<f32>>,
    soft: Vec<f32>,
    /// Deframed payload octets, each bit-reversed into the wire bit order the fields use.
    msg: Vec<u8>,
    /// Sequential message identifier stamped on the next multi-sentence AIVDM group (0–9).
    seq: u8,
}

/// The AIS waveform as `cpm/` entry data. `Mapping::natural(2)` puts the mark tone
/// (+2400 Hz, the high NRZI line state) at index 1, level +1.
fn cpm_params(sps: f64) -> CpmParams {
    CpmParams::from_deviation(
        Mapping::natural(2),
        DEVIATION_HZ,
        BAUD,
        pulse::gaussian_freq(sps, BT, PULSE_SPAN, Norm::Area),
        sps,
    )
}

/// The Gaussian matched filter, zero-padded to declare the *upstream* filtering span to the
/// demodulator's gate. The burst has been through the channel's own [`CHANNEL_TAPS`]-tap
/// selection filter before the demodulator sees it, and that filter's leading half is
/// precursor ripple at its band edge — ~12.5 kHz, which a discriminator reads as five times
/// the outermost level however small its amplitude (measured +5.0 for the filter's whole
/// group delay on a keyed burst). The gate keeps a keying edge out of the level, centre and
/// clock estimates for exactly the receive filter's declared span, so the pad (identical
/// convolution, longer declared span) extends that guard over the upstream group delay;
/// without it the estimates learn the ripple and the training sequence arrives scaled by a
/// third and biased past its own amplitude.
fn receive_filter(sps: f64) -> Vec<f32> {
    let mut taps = pulse::gaussian(sps, BT, MATCHED_SPAN, Norm::Area);
    taps.resize(CHANNEL_TAPS / 2 + taps.len(), 0.0);
    taps
}

fn params(settings: &ChannelSettings) -> Result<&AisParams, ChannelError> {
    match &settings.params {
        ChannelParams::Ais(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "ais channel got {} params",
            other.type_id()
        ))),
    }
}

fn check_params(_p: &AisParams) -> Result<(), ChannelError> {
    Ok(())
}

/// Occupied RF band relative to the channel offset, in Hz.
pub(crate) fn occupied_band(_p: &AisParams) -> (f64, f64) {
    let half = DESCRIPTOR.bandwidth_hz / 2.0;
    (-half, half)
}

pub(crate) fn channel_filter(p: &AisParams) -> Result<ChannelFilter, ChannelError> {
    check_params(p)?;
    let (_, half) = occupied_band(p);
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(CHANNEL_TAPS, half / DESCRIPTOR.input_rate_hz),
        1,
    )))
}

impl ChannelRx for AisChannelRx {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        check_params(p)?;
        let sps = ctx.input_rate / BAUD;
        let cpm = cpm_params(sps);
        Ok(Self {
            letter: p.ais_channel.letter(),
            // Area norm: the level estimates rely on the receive filter's unit DC gain.
            // Burst timing bandwidth: an AIS transmission gives the clock a 24-bit training
            // sequence, not a continuous stream.
            demod: CpmDemod::new(&cpm, &receive_filter(sps), TIMING_BW_BURST),
            slicer: cpm.mapping().clone(),
            nrzi: NrziDecoder::new(),
            deframer: HdlcDeframer::new(MIN_FRAME_BYTES, MAX_FRAME_BYTES),
            carrier_acc: Complex::new(0.0, 0.0),
            carrier_alpha: (1.0 / CARRIER_TAU_SAMPLES) as f32,
            carrier_prev: Complex::new(0.0, 0.0),
            carrier_phase: 0.0,
            mixed: Vec::new(),
            soft: Vec::new(),
            msg: Vec::with_capacity(MAX_FRAME_BYTES),
            seq: 0,
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        check_params(p)?;
        self.letter = p.ais_channel.letter();
        Ok(())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.mixed.clear();
        for &sample in iq {
            let rotation = sample * self.carrier_prev.conj();
            self.carrier_prev = sample;
            self.carrier_acc += self.carrier_alpha * (rotation - self.carrier_acc);
            self.carrier_phase -= f64::from(self.carrier_acc.arg());
            self.carrier_phase = self.carrier_phase.rem_euclid(std::f64::consts::TAU);
            self.mixed
                .push(sample * Complex::from_polar(1.0, self.carrier_phase as f32));
        }
        self.soft.clear();
        self.demod.process(&self.mixed, &mut self.soft);
        for &symbol in &self.soft {
            // Index 1 is the +1 level — the high line state NRZI decodes against.
            let level = self.slicer.slice(symbol) == 1;
            let Some(frame) = self.deframer.push(self.nrzi.decode(level)) else {
                continue;
            };
            if !hdlc_fcs_ok(&frame) {
                continue;
            }
            let Some((payload, _fcs)) = frame.split_last_chunk::<2>() else {
                continue;
            };
            self.msg.clear();
            self.msg.extend(payload.iter().copied().map(reverse_byte));
            let bits = self.msg.len() * 8;
            let fill = (6 - bits % 6) % 6;
            let nmea = sentences(&armour(&self.msg, bits), fill, self.letter, &mut self.seq);
            if let Some(message) = decode(&self.msg, bits, self.letter, nmea) {
                out.events.push(DecoderEvent::Ais(message));
            }
        }
    }
}

/// Bit offsets of the position fields, which the class A and class B reports lay out
/// differently while sharing every encoding.
struct PositionLayout {
    sog: usize,
    lon: usize,
    lat: usize,
    cog: usize,
    heading: usize,
}

/// ITU-R M.1371 Annex 8 Table 45 (message types 1, 2 and 3).
const CLASS_A_POSITION: PositionLayout = PositionLayout {
    sog: 50,
    lon: 61,
    lat: 89,
    cog: 116,
    heading: 128,
};

/// ITU-R M.1371 Annex 8 Table 61 (message type 18).
const CLASS_B_POSITION: PositionLayout = PositionLayout {
    sog: 46,
    lon: 57,
    lat: 85,
    cog: 112,
    heading: 124,
};

/// Message type, repeat indicator and MMSI — the header every AIS message opens with.
const HEADER_BITS: usize = 38;

fn decode(msg: &[u8], bits: usize, letter: char, nmea: String) -> Option<AisMessage> {
    if bits < HEADER_BITS {
        return None;
    }
    let mut out = AisMessage {
        mmsi: bits_be(msg, 8, 30) as u32,
        msg_type: bits_be(msg, 0, 6) as u8,
        ais_channel: letter,
        nmea,
        ..AisMessage::default()
    };
    match out.msg_type {
        1..=3 if bits >= 168 => {
            out.nav_status = Some(bits_be(msg, 38, 4) as u8);
            read_position(msg, &CLASS_A_POSITION, &mut out);
        }
        18 if bits >= 168 => read_position(msg, &CLASS_B_POSITION, &mut out),
        // ITU-R M.1371 Annex 8 Table 50.
        5 if bits >= 424 => {
            out.call_sign = text(msg, 70, 7);
            out.name = text(msg, 112, 20);
            out.destination = text(msg, 302, 20);
        }
        // Table 68: part A carries the name, part B the call sign.
        24 if bits >= 160 => {
            if bits_be(msg, 38, 2) == 0 {
                out.name = text(msg, 40, 20);
            } else {
                out.call_sign = text(msg, 90, 7);
            }
        }
        _ => {}
    }
    Some(out)
}

fn read_position(msg: &[u8], layout: &PositionLayout, out: &mut AisMessage) {
    let sog = bits_be(msg, layout.sog, 10);
    out.sog_kt = (sog != SOG_UNAVAILABLE).then(|| sog as f64 / 10.0);
    out.lon = coord(signed(msg, layout.lon, 28), 180.0);
    out.lat = coord(signed(msg, layout.lat, 27), 90.0);
    let cog = bits_be(msg, layout.cog, 12);
    out.cog_deg = (cog < COG_INVALID).then(|| cog as f64 / 10.0);
    let heading = bits_be(msg, layout.heading, 9) as u16;
    out.heading_deg = (heading != HEADING_UNAVAILABLE).then_some(heading);
}

/// Two's-complement read of a `len`-bit field (`len` in 2..=64).
fn signed(msg: &[u8], offset: usize, len: usize) -> i64 {
    let raw = bits_be(msg, offset, len);
    if raw >> (len - 1) & 1 == 1 {
        raw as i64 - (1i64 << len)
    } else {
        raw as i64
    }
}

fn coord(raw: i64, limit_deg: f64) -> Option<f64> {
    let deg = raw as f64 / COORD_UNITS_PER_DEGREE;
    (deg.abs() <= limit_deg).then_some(deg)
}

/// Six-bit ASCII: 0–31 are `@`–`_`, 32–63 are space–`?`. `None` once the pad is trimmed away.
fn text(msg: &[u8], offset: usize, chars: usize) -> Option<String> {
    let mut s = String::with_capacity(chars);
    for i in 0..chars {
        let v = bits_be(msg, offset + i * 6, 6) as u8;
        s.push(char::from(if v < 32 { v + 64 } else { v }));
    }
    let trimmed = s.trim_end_matches(['@', ' ']);
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// NMEA "armoured ASCII": six payload bits per character, the last group zero-padded.
fn armour(msg: &[u8], bits: usize) -> String {
    (0..bits.div_ceil(6))
        .map(|group| {
            let offset = group * 6;
            let take = 6.min(bits - offset);
            let value = (bits_be(msg, offset, take) << (6 - take)) as u8;
            let c = value + 48;
            char::from(if c > 87 { c + 8 } else { c })
        })
        .collect()
}

/// `!AIVDM` sentences for one payload, newline-separated when it needs more than one. Only the
/// final sentence reports the fill bits; a multi-sentence group carries a sequential id so a
/// parser can reassemble it.
fn sentences(payload: &str, fill: usize, letter: char, seq: &mut u8) -> String {
    let chunks: Vec<&[u8]> = payload.as_bytes().chunks(MAX_PAYLOAD_CHARS).collect();
    let total = chunks.len();
    let id = if total > 1 {
        let id = *seq;
        *seq = (*seq + 1) % 10;
        id.to_string()
    } else {
        String::new()
    };

    let mut out = String::new();
    for (i, chunk) in chunks.iter().enumerate() {
        let text: String = chunk.iter().copied().map(char::from).collect();
        let pad = if i + 1 == total { fill } else { 0 };
        let body = format!("AIVDM,{total},{},{id},{letter},{text},{pad}", i + 1);
        let checksum = body.bytes().fold(0u8, |acc, b| acc ^ b);
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!("!{body}*{checksum:02X}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{AisChannel, AmParams};

    use super::*;
    use crate::{
        testgen::{
            add_noise,
            ais::{
                PositionReport, burst, class_b_payload, corrupted_burst, position_payload,
                static_data_payloads, static_payload,
            },
            shift, silence,
        },
        testutil::settings,
    };

    const RATE: f64 = 48_000.0;

    fn ais_settings(channel: AisChannel) -> ChannelSettings {
        settings(ChannelParams::Ais(AisParams {
            ais_channel: channel,
        }))
    }

    fn channel(ais_channel: AisChannel) -> AisChannelRx {
        AisChannelRx::new(ChannelCtx { input_rate: RATE }, ais_settings(ais_channel)).unwrap()
    }

    fn report() -> PositionReport {
        PositionReport {
            mmsi: 244_670_316,
            lat: 52.372_5,
            lon: 4.893_2,
            sog_kt: 12.4,
            cog_deg: 187.3,
            heading_deg: 188,
            nav_status: 3,
        }
    }

    /// Feed `iq` in the given block sizes and collect the decoded messages, asserting the
    /// channel stays silent (AIS has no audio).
    fn run_blocks(
        chan: &mut AisChannelRx,
        iq: &[Complex<f32>],
        sizes: &[usize],
    ) -> Vec<AisMessage> {
        let mut out = ChannelOutputs::default();
        let mut msgs = Vec::new();
        let mut pos = 0;
        for len in sizes.iter().cycle() {
            if pos >= iq.len() {
                break;
            }
            let end = (pos + len).min(iq.len());
            out.reset();
            chan.process(&iq[pos..end], &mut out);
            assert!(out.audio_pcm.is_empty(), "ais must not produce audio");
            for event in out.events.drain(..) {
                match event {
                    DecoderEvent::Ais(m) => msgs.push(m),
                    other => panic!("unexpected event {other:?}"),
                }
            }
            pos = end;
        }
        msgs
    }

    fn run(iq: &[Complex<f32>]) -> Vec<AisMessage> {
        run_blocks(
            &mut channel(AisChannel::A),
            iq,
            &[997, 1, 4_096, 65, 2_048, 7, 1_024],
        )
    }

    /// Quiet the receiver hears before a transmitter keys, in samples. The cpm gate measures
    /// the channel's noise floor over 384 symbol periods before it may open (its estimates
    /// learn nothing until then, so a burst inside that window meets a frozen clock); a
    /// receiver tuned to a marine channel has heard seconds of quiet before any vessel's
    /// slot comes up, and 480 bit periods — 50 ms — of lead-in models that.
    const LEAD_IN: usize = 2_400;

    /// The receiver's own noise, 40 dB below the carrier — what an antenna delivers when no
    /// one transmits. Digital silence is not that: against a measured floor of exactly zero
    /// the gate reads any nonzero power as a carrier, so the selection filter's band-edge
    /// ripple around each burst counts as keyed signal and a burst's end is never seen.
    const FLOOR: f32 = 0.01;

    /// Frame a transmitter's burst the way the receiver meets it: quiet lead-in, the burst,
    /// quiet tail, all over the receiver's own noise floor.
    fn framed(burst_iq: Vec<Complex<f32>>) -> Vec<Complex<f32>> {
        let mut iq = silence(LEAD_IN);
        iq.extend(burst_iq);
        iq.extend(silence(480));
        add_noise(&mut iq, 0x1157, FLOOR);
        iq
    }

    fn raw(payload: &[bool]) -> Vec<Complex<f32>> {
        framed(burst(payload, RATE))
    }

    /// What the engine actually hands a channel: the DDC output through the mode's own
    /// selection filter (). Every decode test goes through it, so the filter's edge
    /// ringing on the burst is part of what the receiver has to survive.
    fn select(iq: &[Complex<f32>]) -> Vec<Complex<f32>> {
        let mut filter = crate::channel_filter(&ChannelParams::Ais(AisParams::default())).unwrap();
        let mut out = Vec::new();
        filter.process(iq, &mut out);
        out
    }

    fn transmission(payload: &[bool]) -> Vec<Complex<f32>> {
        select(&raw(payload))
    }

    fn only(msgs: Vec<AisMessage>) -> AisMessage {
        assert_eq!(msgs.len(), 1, "expected exactly one message: {msgs:?}");
        msgs.into_iter().next().unwrap()
    }

    /// A vessel's burst arrives at whatever fraction of a bit the receiver's clock happens
    /// to sit at, and AIS grants exactly 24 training bits to acquire in — there is no second
    /// burst to coast into, as TDMA voice modes have. Every sub-bit arrival phase must
    /// decode, at every dial error the frequency-error test covers: without the carrier
    /// corrector the measured surface was 3/5 phases at ±400 Hz and 0/5 at ±1200 Hz.
    #[test]
    fn acquires_from_every_burst_phase_at_every_dial_error() {
        for offset_hz in [0.0, -400.0, 400.0, -1_200.0, 1_200.0] {
            for phase in 0..5usize {
                let mut iq = silence(LEAD_IN + phase);
                iq.extend(burst(&position_payload(&report()), RATE));
                iq.extend(silence(480));
                add_noise(&mut iq, 0x1157, FLOOR);
                shift(&mut iq, offset_hz, RATE);
                let msgs = run_blocks(&mut channel(AisChannel::A), &select(&iq), &[997]);
                assert_eq!(
                    msgs.len(),
                    1,
                    "no decode at burst phase {phase}/5, {offset_hz:+.0} Hz"
                );
                assert_eq!(
                    msgs[0].mmsi, 244_670_316,
                    "phase {phase}, {offset_hz:+.0} Hz"
                );
            }
        }
    }

    #[test]
    fn decodes_a_type_1_position_report() {
        let m = only(run(&transmission(&position_payload(&report()))));
        assert_eq!(m.msg_type, 1);
        assert_eq!(m.mmsi, 244_670_316);
        assert_eq!(m.ais_channel, 'A');
        assert_eq!(m.nav_status, Some(3));
        assert_eq!(m.sog_kt, Some(12.4));
        assert_eq!(m.cog_deg, Some(187.3));
        assert_eq!(m.heading_deg, Some(188));
        assert!((m.lat.unwrap() - 52.372_5).abs() < 1e-4, "lat {:?}", m.lat);
        assert!((m.lon.unwrap() - 4.893_2).abs() < 1e-4, "lon {:?}", m.lon);
    }

    #[test]
    fn decodes_a_type_18_class_b_report() {
        let m = only(run(&transmission(&class_b_payload(&report()))));
        assert_eq!(m.msg_type, 18);
        assert_eq!(m.mmsi, 244_670_316);
        assert_eq!(m.sog_kt, Some(12.4));
        assert_eq!(m.cog_deg, Some(187.3));
        assert_eq!(m.heading_deg, Some(188));
        assert!((m.lat.unwrap() - 52.372_5).abs() < 1e-4, "lat {:?}", m.lat);
        assert!((m.lon.unwrap() - 4.893_2).abs() < 1e-4, "lon {:?}", m.lon);
        // Class B reports carry no navigational status.
        assert_eq!(m.nav_status, None);
    }

    #[test]
    fn decodes_a_type_5_static_report_with_the_padding_trimmed() {
        let payload = static_payload(244_670_316, "NAUTICA", "PBRT", "ROTTERDAM");
        let m = only(run(&transmission(&payload)));
        assert_eq!(m.msg_type, 5);
        assert_eq!(m.mmsi, 244_670_316);
        assert_eq!(m.name.as_deref(), Some("NAUTICA"));
        assert_eq!(m.call_sign.as_deref(), Some("PBRT"));
        assert_eq!(m.destination.as_deref(), Some("ROTTERDAM"));
    }

    #[test]
    fn decodes_both_parts_of_a_type_24_static_data_report() {
        let (part_a, part_b) = static_data_payloads(244_670_316, "SEA WOLF", "PBRT");
        let mut iq = raw(&part_a);
        iq.extend(raw(&part_b));
        let msgs = run(&select(&iq));
        assert_eq!(msgs.len(), 2, "{msgs:?}");
        assert_eq!(msgs[0].msg_type, 24);
        assert_eq!(msgs[0].name.as_deref(), Some("SEA WOLF"));
        assert_eq!(msgs[0].call_sign, None);
        assert_eq!(msgs[1].call_sign.as_deref(), Some("PBRT"));
        assert_eq!(msgs[1].name, None);
    }

    #[test]
    fn an_undecoded_message_type_still_reports_its_sender_and_sentence() {
        // Type 4 (base station report) lays its position out differently, so nothing beyond
        // the header may be guessed at — but the AIVDM sentence still carries everything.
        let mut payload = position_payload(&report());
        payload[..6].copy_from_slice(&[false, false, false, true, false, false]);
        let m = only(run(&transmission(&payload)));
        assert_eq!(m.msg_type, 4);
        assert_eq!(m.mmsi, 244_670_316);
        assert_eq!(
            (m.lat, m.lon, m.sog_kt, m.nav_status),
            (None, None, None, None)
        );
        assert!(checksum_ok(&m.nmea), "{}", m.nmea);
    }

    #[test]
    fn unavailable_sentinels_decode_to_none() {
        let m = only(run(&transmission(&position_payload(&PositionReport {
            mmsi: 1,
            lat: 91.0,
            lon: 181.0,
            sog_kt: 102.3,
            cog_deg: 360.0,
            heading_deg: 511,
            nav_status: 15,
        }))));
        assert_eq!(m.lat, None);
        assert_eq!(m.lon, None);
        assert_eq!(m.sog_kt, None);
        assert_eq!(m.cog_deg, None);
        assert_eq!(m.heading_deg, None);
    }

    #[test]
    fn a_payload_with_a_long_run_of_ones_survives_bit_stuffing() {
        // 28 consecutive ones inside the MMSI field, far past the five that force a stuffed zero.
        let payload = position_payload(&PositionReport {
            mmsi: 268_435_455,
            ..report()
        });
        let longest = payload
            .chunk_by(|a, b| a == b)
            .filter(|run| run[0])
            .map(<[bool]>::len)
            .max()
            .unwrap_or(0);
        assert!(
            longest >= 6,
            "fixture must force stuffing, longest run {longest}"
        );

        let m = only(run(&transmission(&payload)));
        assert_eq!(m.mmsi, 268_435_455);
        assert_eq!(m.heading_deg, Some(188));
    }

    #[test]
    fn a_corrupted_burst_emits_nothing() {
        let payload = position_payload(&report());
        // One payload bit flipped after the FCS was taken: the burst still frames cleanly, so
        // only the CRC can reject it.
        let iq = framed(corrupted_burst(&payload, 100, RATE));
        assert!(run(&select(&iq)).is_empty());
        // The same burst without the flip must decode, or the test proves nothing.
        assert_eq!(only(run(&transmission(&payload))).mmsi, 244_670_316);
    }

    #[test]
    fn two_bursts_back_to_back_both_decode() {
        let first = position_payload(&report());
        let second = position_payload(&PositionReport {
            mmsi: 211_234_567,
            sog_kt: 0.0,
            ..report()
        });
        let mut iq = raw(&first);
        iq.extend(raw(&second));
        let msgs = run(&select(&iq));
        assert_eq!(msgs.len(), 2, "{msgs:?}");
        assert_eq!(msgs[0].mmsi, 244_670_316);
        assert_eq!(msgs[1].mmsi, 211_234_567);
        assert_eq!(msgs[1].sog_kt, Some(0.0));
    }

    #[test]
    fn ragged_block_splits_give_identical_results() {
        let iq = transmission(&static_payload(1_234_567, "SEA WOLF", "ZZ9", "HAMBURG"));
        let whole = run_blocks(&mut channel(AisChannel::A), &iq, &[iq.len()]);
        let ragged = run_blocks(
            &mut channel(AisChannel::A),
            &iq,
            &[1, 3, 512, 17, 4_096, 63],
        );
        assert_eq!(whole.len(), 1);
        assert_eq!(whole, ragged);
    }

    /// Recompute the NMEA checksum the way a parser does: XOR of everything between `!`
    /// and `*`, two uppercase hex digits.
    fn checksum_ok(sentence: &str) -> bool {
        let Some((body, tail)) = sentence.strip_prefix('!').and_then(|s| s.split_once('*')) else {
            return false;
        };
        let want = body.bytes().fold(0u8, |acc, b| acc ^ b);
        tail == format!("{want:02X}")
    }

    #[test]
    fn a_single_sentence_carries_the_channel_letter_and_a_valid_checksum() {
        let mut chan = channel(AisChannel::B);
        let msgs = run_blocks(
            &mut chan,
            &transmission(&position_payload(&report())),
            &[1_000],
        );
        let m = only(msgs);
        assert_eq!(m.ais_channel, 'B');
        assert!(checksum_ok(&m.nmea), "{}", m.nmea);
        assert!(m.nmea.starts_with("!AIVDM,1,1,,B,"), "{}", m.nmea);
        // 168 payload bits is 28 whole six-bit groups, so nothing is padded.
        assert!(m.nmea.contains(",0*"), "{}", m.nmea);
    }

    /// Six-bit value of an armoured character, inverting the `+48 / +8` mapping.
    fn dearmour_char(c: char) -> u8 {
        let v = c as u8 - 48;
        if v > 40 { v - 8 } else { v }
    }

    #[test]
    fn a_type_5_payload_splits_into_two_valid_sentences() {
        let payload = static_payload(244_670_316, "NAUTICA", "PBRT", "ROTTERDAM");
        let m = only(run(&transmission(&payload)));

        let parts: Vec<&str> = m.nmea.split('\n').collect();
        assert_eq!(parts.len(), 2, "{}", m.nmea);
        let mut bits = Vec::new();
        for (i, part) in parts.iter().enumerate() {
            assert!(checksum_ok(part), "{part}");
            let fields: Vec<&str> = part.split([',', '*']).collect();
            assert_eq!(fields[0], "!AIVDM");
            assert_eq!(fields[1], "2");
            assert_eq!(fields[2], (i + 1).to_string());
            assert_eq!(fields[3], "0", "sequential id");
            assert_eq!(fields[4], "A");
            // 424 bits is 70 full groups plus four bits, so the last sentence pads two.
            assert_eq!(fields[6], if i == 0 { "0" } else { "2" });
            for c in fields[5].chars() {
                let v = dearmour_char(c);
                bits.extend((0..6).rev().map(|k| v >> k & 1 == 1));
            }
        }
        assert_eq!(bits.len(), 426);
        assert_eq!(&bits[..424], payload.as_slice());
        assert_eq!(&bits[424..], [false, false]);
    }

    #[test]
    fn decodes_through_a_receiver_frequency_error() {
        // Half the deviation of tuning error is a standing discriminator offset the DC blocker
        // has to remove before the slicer sees the eye — 1200 Hz is 7 ppm at 162 MHz.
        for offset_hz in [-1_200.0, -400.0, 400.0, 1_200.0] {
            let mut iq = raw(&position_payload(&report()));
            shift(&mut iq, offset_hz, RATE);
            assert_eq!(only(run(&select(&iq))).mmsi, 244_670_316, "{offset_hz} Hz");
        }
    }

    #[test]
    fn decodes_through_noise() {
        for seed in 1u32..=4 {
            let mut iq = raw(&static_payload(244_670_316, "NAUTICA", "PBRT", "ROTTERDAM"));
            // Uniform noise at a fifth of the carrier amplitude on each of I and Q.
            add_noise(&mut iq, seed, 0.2);
            let m = only(run(&select(&iq)));
            assert_eq!(m.name.as_deref(), Some("NAUTICA"), "seed {seed}");
        }
    }

    #[test]
    fn mismatched_params_variant_is_rejected() {
        let mut chan = channel(AisChannel::A);
        let err = chan.apply(settings(ChannelParams::Am(AmParams::default())));
        assert!(matches!(err, Err(ChannelError::InvalidSettings(_))));
        let built = AisChannelRx::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Am(AmParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }

    #[test]
    fn apply_switches_the_reported_channel_letter() {
        let mut chan = channel(AisChannel::A);
        chan.apply(ais_settings(AisChannel::B)).unwrap();
        let msgs = run_blocks(
            &mut chan,
            &transmission(&position_payload(&report())),
            &[512],
        );
        assert_eq!(only(msgs).ais_channel, 'B');
    }

    /// The committed fixture (fixtures/ais_position_pre_cpm_240k), rendered by the
    /// pre-migration generator with its stepped burst envelope: decode-equivalence across the
    /// cpm migration is proven against the committed artifact, not only against the migrated
    /// generator. The capture is the raw burst at +25 kHz in a 240 kHz stream, so the test
    /// runs the engine's own front-end shape — mix to baseband, decimate to the channel
    /// rate, selection filter — with the listening lead-in any tuned receiver has.
    ///
    /// Its stem deliberately differs from the playable `ais_position_240k`: that one is
    /// rewritten by `cargo xtask fixtures` from today's generator, so reading it here would
    /// leave the migrated generator proving itself and this test asserting nothing.
    #[test]
    fn decodes_the_committed_fixture() {
        const FIXTURE: &[u8] =
            include_bytes!("../../../fixtures/ais_position_pre_cpm_240k.sigmf-data");
        const FIXTURE_RATE: f64 = 240_000.0;
        let mut wide: Vec<Complex<f32>> = FIXTURE
            .as_chunks::<8>()
            .0
            .iter()
            .map(|s| {
                Complex::new(
                    f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                    f32::from_le_bytes([s[4], s[5], s[6], s[7]]),
                )
            })
            .collect();
        shift(&mut wide, -25_000.0, FIXTURE_RATE);
        let decim = (FIXTURE_RATE / RATE) as usize;
        let mut narrow = Vec::new();
        Decimator::new(
            &design_lowpass(CHANNEL_TAPS, DESCRIPTOR.bandwidth_hz / 2.0 / FIXTURE_RATE),
            decim,
        )
        .process(&wide, &mut narrow);
        let m = only(run(&select(&framed(narrow))));
        assert_eq!(m.msg_type, 1);
        assert_eq!(m.mmsi, 211_234_560);
        assert_eq!(m.nav_status, Some(0));
        assert_eq!(m.sog_kt, Some(12.3));
        assert_eq!(m.cog_deg, Some(178.4));
        assert_eq!(m.heading_deg, Some(179));
        assert!((m.lat.unwrap() - 53.5413).abs() < 1e-4, "lat {:?}", m.lat);
        assert!((m.lon.unwrap() - 9.9846).abs() < 1e-4, "lon {:?}", m.lon);
    }

    /// The migrated generator at the device rate the engine e2e and the fixture renderer
    /// drive it at — 240 kHz, 25 samples per bit — decoded through the same DDC shape as the
    /// committed fixture: the sps axis of the cpm parameterisation is data, and this pins it
    /// at a second operating point.
    #[test]
    fn a_240k_rendering_decodes_through_the_ddc_path() {
        const DEVICE_RATE: f64 = 240_000.0;
        let mut wide = burst(&position_payload(&report()), DEVICE_RATE);
        shift(&mut wide, 25_000.0, DEVICE_RATE);
        shift(&mut wide, -25_000.0, DEVICE_RATE);
        let mut narrow = Vec::new();
        Decimator::new(
            &design_lowpass(CHANNEL_TAPS, DESCRIPTOR.bandwidth_hz / 2.0 / DEVICE_RATE),
            (DEVICE_RATE / RATE) as usize,
        )
        .process(&wide, &mut narrow);
        assert_eq!(only(run(&select(&framed(narrow)))).mmsi, 244_670_316);
    }

    /// The virtual-device replay path: a planted burst padded to a second and looped, over
    /// *digital* silence — the one place mathematically exact zeros reach a channel, since
    /// no antenna produces them. Against a measured floor of exactly zero the gate cannot
    /// tell the selection filter's band-edge ripple from a carrier, so this pins down that
    /// the receive-filter pad still keeps the ripple out of the estimates and later passes
    /// decode; the first pass lands inside the gate's floor-settling window and is the cold
    /// start the engine's own e2e rides out by looping.
    #[test]
    fn a_looped_replay_over_digital_silence_decodes() {
        let one_pass = {
            let mut iq = burst(&position_payload(&report()), RATE);
            let pad = RATE as usize - iq.len();
            iq.extend(silence(pad));
            iq
        };
        let mut iq = one_pass.clone();
        iq.extend(one_pass);
        let msgs = run(&select(&iq));
        assert!(!msgs.is_empty(), "no pass of the loop decoded");
        for m in msgs {
            assert_eq!(m.mmsi, 244_670_316);
        }
    }

    #[test]
    fn wrong_input_rate_is_rejected() {
        let built = AisChannelRx::new(
            ChannelCtx {
                input_rate: 240_000.0,
            },
            ais_settings(AisChannel::A),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }
}
