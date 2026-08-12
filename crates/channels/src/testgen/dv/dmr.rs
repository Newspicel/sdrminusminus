//! DMR reference transmitter (ETSI TS 102 361-1): builds the bursts of a whole transmission —
//! voice LC header, a voice superframe carrying its link control in the embedded field, and a
//! terminator — and modulates them as a direct-mode radio would.

use num_complex::Complex;
use sdrmm_dsp::{Bptc128, Bptc196, CyclicCode, crc16_msb, rs129_parity};

use super::{bits, dibits, filler};

/// Direct-mode slot 1 sync patterns, which name their timeslot on the wire.
const VOICE_SYNC: u64 = 0x5D57_7F77_57FF;
const DATA_SYNC: u64 = 0xF7FD_D5DD_FD55;

const DT_VOICE_LC_HEADER: u8 = 0x1;
const DT_TERMINATOR_WITH_LC: u8 = 0x2;
const DT_CSBK: u8 = 0x3;

const BAUD: f64 = 4_800.0;
const DEVIATION_HZ: f64 = 1_944.0;
const RRC_ALPHA: f64 = 0.2;

/// Symbols in a 30 ms timeslot, of which the 264-bit burst is 132 (27.5 ms). A direct-mode
/// radio keys off for the remaining 2.5 ms of guard time and for the whole of the other
/// slot — so it transmits 132 symbols in every 288, and this generator has to key off for the
/// other 156 or it would not be exercising a TDMA receiver at all.
const SLOT_SYMBOLS: usize = 144;
const BURST_SYMBOLS: usize = 132;

/// One call, as its transmitter would key it.
pub struct Call {
    pub color_code: u8,
    pub group: bool,
    pub encrypted: bool,
    pub destination: u32,
    pub source: u32,
}

impl Default for Call {
    fn default() -> Self {
        Self {
            color_code: 1,
            group: true,
            encrypted: false,
            destination: 505,
            source: 2_621_001,
        }
    }
}

impl Call {
    /// The 72-bit full link control this call is described by.
    #[must_use]
    pub fn link_control(&self) -> Vec<bool> {
        let flco = u64::from(!self.group) * 3;
        let mut lc = bits(flco, 8);
        // Feature set id 0 (ETSI standard), then service options whose second bit is privacy.
        lc.extend(bits(0, 8));
        lc.extend(bits(u64::from(self.encrypted) << 6, 8));
        lc.extend(bits(u64::from(self.destination), 24));
        lc.extend(bits(u64::from(self.source), 24));
        lc
    }
}

/// The voice LC header a radio sends at the head of a call, a voice superframe whose embedded
/// link control repeats the same addressing, and a terminator — each keyed for its own 30 ms
/// slot with the rest of the 60 ms TDMA frame dead, as a direct-mode radio transmits.
#[must_use]
pub fn transmission(call: &Call, rate: f64) -> Vec<Complex<f32>> {
    let voice = std::array::from_fn(|_| {
        let mut payload = [false; 216];
        payload[..108].copy_from_slice(&filler(108, u32::from(call.color_code) + 17));
        payload[108..].copy_from_slice(&filler(108, u32::from(call.color_code) + 23));
        payload
    });
    transmission_with_voice(call, &voice, rate)
}

/// The same independently-framed transmission with caller-provided, already encoded vocoder
/// sockets. This seam lets receive tests generate AMBE frames without putting a production
/// encoder in the channel.
#[must_use]
pub(crate) fn transmission_with_voice(
    call: &Call,
    voice: &[[bool; 216]; 6],
    rate: f64,
) -> Vec<Complex<f32>> {
    let mut tx = Keyed::default();
    // Conventional voice initiation is one LC header followed by voice burst A (§5.1.2.2).
    tx.burst(&data_burst(
        call,
        DT_VOICE_LC_HEADER,
        &lc_with_parity(call, [0x96, 0x96, 0x96]),
    ));

    let embedded = Bptc128::encode(&embedded_block(call));
    // Burst A carries the sync; B to E one quarter each of the embedded link control; F the
    // null fragment that closes the superframe.
    tx.burst(&voice_burst(call, None, &voice[0]));
    for (i, lcss) in [0b01u8, 0b11, 0b11, 0b10].into_iter().enumerate() {
        let fragment: Vec<bool> = embedded[i * 32..(i + 1) * 32].to_vec();
        tx.burst(&voice_burst(call, Some((lcss, fragment)), &voice[i + 1]));
    }
    tx.burst(&voice_burst(call, Some((0b00, vec![false; 32])), &voice[5]));

    tx.burst(&data_burst(
        call,
        DT_TERMINATOR_WITH_LC,
        &lc_with_parity(call, [0x99, 0x99, 0x99]),
    ));
    tx.modulate(rate)
}

/// A single CSBK burst — the signalling that travels outside a call.
#[must_use]
pub fn csbk(
    color_code: u8,
    opcode: u8,
    destination: u32,
    source: u32,
    rate: f64,
) -> Vec<Complex<f32>> {
    let mut payload = bits(u64::from(opcode) & 0x3F, 8);
    // Feature set id, then the sixteen opcode-specific bits every CSBK carries ahead of its
    // two addresses.
    payload.extend(bits(0, 8));
    payload.extend(bits(0, 16));
    payload.extend(bits(u64::from(destination), 24));
    payload.extend(bits(u64::from(source), 24));
    let crc = !crc16_msb(0x1021, 0, &pack(&payload)) ^ 0xA5A5;
    payload.extend(bits(u64::from(crc), 16));

    let call = Call {
        color_code,
        ..Call::default()
    };
    // Repeated, as the preamble CSBKs that precede a call are: one burst is 27.5 ms of carrier
    // and a receiver joining a dead channel has nothing else to acquire on.
    let mut tx = Keyed::default();
    for _ in 0..3 {
        tx.burst(&data_burst(&call, DT_CSBK, &payload));
    }
    tx.modulate(rate)
}

/// The symbol stream a direct-mode radio puts on the air: its own burst, then the guard time
/// and the other radio's slot, which it spends keyed off.
#[derive(Default)]
struct Keyed {
    symbols: Vec<Option<u8>>,
}

impl Keyed {
    fn burst(&mut self, bits: &[bool]) {
        let burst = dibits(bits);
        assert_eq!(burst.len(), BURST_SYMBOLS, "a DMR burst is 264 bits");
        self.symbols.extend(burst.into_iter().map(Some));
        self.symbols
            .extend(std::iter::repeat_n(None, 2 * SLOT_SYMBOLS - BURST_SYMBOLS));
    }

    fn modulate(self, rate: f64) -> Vec<Complex<f32>> {
        let mut noise = Noise(0x5d5d_7f77);
        super::c4fm_keyed(&self.symbols, rate, BAUD, DEVIATION_HZ, RRC_ALPHA)
            .into_iter()
            // The receiver's own noise is on the channel whether the transmitter is keyed or
            // not. A generated signal without it reads as digitally silent between bursts,
            // which is not something any receiver sees — and a carrier detector has nothing
            // to measure a noise floor from.
            .map(|sample| sample + noise.sample())
            .collect()
    }
}

/// A receiver's noise floor, 40 dB below a unit-magnitude carrier.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32 * 2.0 - 1.0) * 0.01
    }

    fn sample(&mut self) -> Complex<f32> {
        Complex::new(self.next(), self.next())
    }
}

/// The 96-bit signalling block of a link control frame: 72 bits of addressing and the
/// Reed-Solomon parity, masked by frame type.
fn lc_with_parity(call: &Call, mask: [u8; 3]) -> Vec<bool> {
    let lc = call.link_control();
    let parity = rs129_parity(&pack(&lc));
    let mut out = lc;
    for (byte, m) in parity.into_iter().zip(mask) {
        out.extend(bits(u64::from(byte ^ m), 8));
    }
    out
}

/// The 77 bits a BPTC(128,77) embedded block carries: the link control with its five-bit
/// checksum threaded through column 10 of rows 2 to 6.
fn embedded_block(call: &Call) -> [bool; Bptc128::DATA_BITS] {
    let lc = call.link_control();
    let checksum = pack(&lc).iter().map(|&b| u32::from(b)).sum::<u32>() % 31;
    let mut out = [false; Bptc128::DATA_BITS];
    let (mut read, mut check_bit) = (0, 0);
    for (i, slot) in out.iter_mut().enumerate() {
        if i >= 22 && i % 11 == 10 {
            *slot = checksum >> (4 - check_bit) & 1 == 1;
            check_bit += 1;
        } else {
            *slot = lc[read];
            read += 1;
        }
    }
    out
}

/// A 264-bit data burst: BPTC payload either side of the sync, with the Golay slot type in
/// between.
fn data_burst(call: &Call, data_type: u8, payload: &[bool]) -> Vec<bool> {
    let mut block = [false; Bptc196::DATA_BITS];
    for (slot, &bit) in block.iter_mut().zip(payload) {
        *slot = bit;
    }
    let coded = Bptc196::encode(&block);
    let slot_type =
        CyclicCode::GOLAY_20_8.encode(u32::from(call.color_code) << 4 | u32::from(data_type));

    let mut burst = coded[..98].to_vec();
    burst.extend(bits(slot_type >> 10, 10));
    burst.extend(bits(DATA_SYNC, 48));
    burst.extend(bits(slot_type & 0x3FF, 10));
    burst.extend(&coded[98..]);
    burst
}

/// A 264-bit voice burst: filler where the vocoder frames go, and either the sync (burst A) or
/// an embedded signalling field with one quarter of the link control (bursts B to F).
fn voice_burst(call: &Call, embedded: Option<(u8, Vec<bool>)>, vocoder: &[bool; 216]) -> Vec<bool> {
    let mut burst = vocoder[..108].to_vec();
    match embedded {
        None => burst.extend(bits(VOICE_SYNC, 48)),
        Some((lcss, fragment)) => {
            let emb = CyclicCode::QR_16_7
                .encode(u32::from(call.color_code) << 3 | u32::from(lcss & 0b11));
            burst.extend(bits(emb >> 8, 8));
            burst.extend(fragment);
            burst.extend(bits(emb & 0xFF, 8));
        }
    }
    burst.extend(&vocoder[108..]);
    burst
}

fn pack(bits: &[bool]) -> Vec<u8> {
    bits.as_chunks::<8>()
        .0
        .iter()
        .map(|chunk| chunk.iter().fold(0u8, |acc, &b| acc << 1 | u8::from(b)))
        .collect()
}
