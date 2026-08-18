use num_complex::Complex;
use sdrmm_dsp::{Bptc128, Bptc196, CyclicCode, crc16_msb, rs129_parity};

use super::{bits, dibits, filler};

const VOICE_SYNC: u64 = 0x5D57_7F77_57FF;
const DATA_SYNC: u64 = 0xF7FD_D5DD_FD55;
const MS_VOICE_SYNC: u64 = 0x7F7D_5DD5_7DFD;
const MS_DATA_SYNC: u64 = 0xD5D7_F77F_D757;
const BS_VOICE_SYNC: u64 = 0x755F_D7DF_75F7;
const BS_DATA_SYNC: u64 = 0xDFF5_7D75_DF5D;

const DT_VOICE_LC_HEADER: u8 = 0x1;
const DT_TERMINATOR_WITH_LC: u8 = 0x2;
const DT_CSBK: u8 = 0x3;
const DT_MBC_HEADER: u8 = 0x4;
const DT_MBC_CONTINUATION: u8 = 0x5;

const BAUD: f64 = 4_800.0;
const DEVIATION_HZ: f64 = 1_944.0;
const RRC_ALPHA: f64 = 0.2;

const SLOT_SYMBOLS: usize = 144;
const BURST_SYMBOLS: usize = 132;

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
    #[must_use]
    pub fn link_control(&self) -> Vec<bool> {
        let flco = u64::from(!self.group) * 3;
        let mut lc = bits(flco, 8);
        lc.extend(bits(0, 8));
        lc.extend(bits(u64::from(self.encrypted) << 6, 8));
        lc.extend(bits(u64::from(self.destination), 24));
        lc.extend(bits(u64::from(self.source), 24));
        lc
    }
}

#[must_use]
pub fn transmission(call: &Call, rate: f64) -> Vec<Complex<f32>> {
    let voice: [[bool; 216]; 6] = std::array::from_fn(|_| {
        let mut payload = [false; 216];
        payload[..108].copy_from_slice(&filler(108, u32::from(call.color_code) + 17));
        payload[108..].copy_from_slice(&filler(108, u32::from(call.color_code) + 23));
        payload
    });
    transmission_with_voice(call, &voice, rate)
}

#[must_use]
pub fn simplex_transmission(call: &Call, rate: f64) -> Vec<Complex<f32>> {
    let voice: [[bool; 216]; 6] = std::array::from_fn(|_| {
        let mut payload = [false; 216];
        payload[..108].copy_from_slice(&filler(108, u32::from(call.color_code) + 17));
        payload[108..].copy_from_slice(&filler(108, u32::from(call.color_code) + 23));
        payload
    });
    let mut tx = Keyed::default();
    for burst in call_bursts_with_voice(call, &voice) {
        tx.burst(&ms_sourced(burst));
    }
    tx.modulate(rate)
}

fn ms_sourced(mut burst: Vec<bool>) -> Vec<bool> {
    let sync = if burst[108..156] == bits(VOICE_SYNC, 48) {
        MS_VOICE_SYNC
    } else if burst[108..156] == bits(DATA_SYNC, 48) {
        MS_DATA_SYNC
    } else {
        return burst;
    };
    burst[108..156].copy_from_slice(&bits(sync, 48));
    burst
}

#[must_use]
pub fn repeater_transmission(call: &Call, slot: u8, rate: f64) -> Vec<Complex<f32>> {
    repeater_calls(call, None, slot, rate)
}

#[must_use]
pub fn dual_slot_transmission(slot_one: &Call, slot_two: &Call, rate: f64) -> Vec<Complex<f32>> {
    repeater_calls(slot_one, Some(slot_two), 1, rate)
}

fn repeater_calls(
    first: &Call,
    second: Option<&Call>,
    first_slot: u8,
    rate: f64,
) -> Vec<Complex<f32>> {
    let first_bursts = call_bursts(first);
    let second_bursts = second.map(call_bursts);
    let mut symbols = Vec::new();
    symbols.extend(dibits(&filler(800, 73)).into_iter().map(Some));
    for (index, first_burst) in first_bursts.iter().enumerate() {
        append_repeater_burst(&mut symbols, first_burst, first_slot);
        if let Some(second_bursts) = &second_bursts {
            append_repeater_burst(&mut symbols, &second_bursts[index], 2);
        } else {
            symbols.extend(
                dibits(&filler(288, 91 + index as u32))
                    .into_iter()
                    .map(Some),
            );
        }
    }
    symbols.extend(dibits(&filler(400, 101)).into_iter().map(Some));
    Keyed { symbols }.modulate(rate)
}

fn call_bursts(call: &Call) -> Vec<Vec<bool>> {
    let voice: [[bool; 216]; 6] = std::array::from_fn(|_| {
        let mut payload = [false; 216];
        payload[..108].copy_from_slice(&filler(108, u32::from(call.color_code) + 17));
        payload[108..].copy_from_slice(&filler(108, u32::from(call.color_code) + 23));
        payload
    });
    call_bursts_with_voice(call, &voice)
}

fn call_bursts_with_voice(call: &Call, voice: &[[bool; 216]; 6]) -> Vec<Vec<bool>> {
    let mut bursts = vec![data_burst(
        call,
        DT_VOICE_LC_HEADER,
        &lc_with_parity(call, [0x96, 0x96, 0x96]),
    )];
    let embedded = Bptc128::encode(&embedded_block(call));
    bursts.push(voice_burst(call, None, &voice[0]));
    for (i, lcss) in [0b01u8, 0b11, 0b11, 0b10].into_iter().enumerate() {
        bursts.push(voice_burst(
            call,
            Some((lcss, embedded[i * 32..(i + 1) * 32].to_vec())),
            &voice[i + 1],
        ));
    }
    bursts.push(voice_burst(call, Some((0, vec![false; 32])), &voice[5]));
    bursts.push(data_burst(
        call,
        DT_TERMINATOR_WITH_LC,
        &lc_with_parity(call, [0x99, 0x99, 0x99]),
    ));
    bursts
}

fn append_repeater_burst(symbols: &mut Vec<Option<u8>>, burst: &[bool], slot: u8) {
    let mut burst = burst.to_vec();
    let sync = if burst[108..156] == bits(VOICE_SYNC, 48) {
        BS_VOICE_SYNC
    } else if burst[108..156] == bits(DATA_SYNC, 48) {
        BS_DATA_SYNC
    } else {
        0
    };
    if sync != 0 {
        burst[108..156].copy_from_slice(&bits(sync, 48));
    }
    symbols.extend(dibits(&cach(slot)).into_iter().map(Some));
    symbols.extend(dibits(&burst).into_iter().map(Some));
}

fn cach(slot: u8) -> Vec<bool> {
    let mut tact = [true, slot == 2, false, false, false, false, false];
    sdrmm_dsp::ParityCode::HAMMING_7_4.encode(&mut tact);
    let payload = [false; 17];
    [
        tact[0],
        payload[0],
        payload[1],
        payload[2],
        tact[1],
        payload[3],
        payload[4],
        payload[5],
        tact[2],
        payload[6],
        payload[7],
        payload[8],
        tact[3],
        payload[9],
        tact[4],
        payload[10],
        payload[11],
        payload[12],
        tact[5],
        payload[13],
        payload[14],
        payload[15],
        tact[6],
        payload[16],
    ]
    .to_vec()
}

#[must_use]
pub(crate) fn transmission_with_voice(
    call: &Call,
    voice: &[[bool; 216]; 6],
    rate: f64,
) -> Vec<Complex<f32>> {
    let mut tx = Keyed::default();
    tx.burst(&data_burst(
        call,
        DT_VOICE_LC_HEADER,
        &lc_with_parity(call, [0x96, 0x96, 0x96]),
    ));

    let embedded = Bptc128::encode(&embedded_block(call));
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

#[must_use]
pub fn csbk(
    color_code: u8,
    opcode: u8,
    destination: u32,
    source: u32,
    rate: f64,
) -> Vec<Complex<f32>> {
    csbk_with_fid(color_code, 0, opcode, destination, source, rate)
}

#[must_use]
pub(crate) fn csbk_with_fid(
    color_code: u8,
    fid: u8,
    opcode: u8,
    destination: u32,
    source: u32,
    rate: f64,
) -> Vec<Complex<f32>> {
    let mut payload = bits(u64::from(opcode) & 0x3F, 8);
    payload.extend(bits(u64::from(fid), 8));
    payload.extend(bits(0, 16));
    payload.extend(bits(u64::from(destination), 24));
    payload.extend(bits(u64::from(source), 24));
    csbk_bits(color_code, &payload, rate)
}

/// A CSBK whose body the caller lays out itself, for the opcodes that carry something other than
/// a pair of addresses. `head` is the 80 bits before the checksum.
#[must_use]
pub(crate) fn csbk_bits(color_code: u8, head: &[bool], rate: f64) -> Vec<Complex<f32>> {
    let mut payload = head.to_vec();
    payload.resize(80, false);
    let crc = !crc16_msb(0x1021, 0, &pack(&payload)) ^ 0xA5A5;
    payload.extend(bits(u64::from(crc), 16));

    let call = Call {
        color_code,
        ..Call::default()
    };
    let mut tx = Keyed::default();
    for _ in 0..3 {
        tx.burst(&data_burst(&call, DT_CSBK, &payload));
    }
    tx.modulate(rate)
}

#[must_use]
pub fn channel_definition(
    color_code: u8,
    channel: u16,
    tx_hz: u64,
    rx_hz: u64,
    rate: f64,
) -> Vec<Complex<f32>> {
    multi_block(color_code, channel, tx_hz, rx_hz, false, rate)
}

#[must_use]
pub fn interrupted_channel_definition(
    color_code: u8,
    channel: u16,
    tx_hz: u64,
    rx_hz: u64,
    rate: f64,
) -> Vec<Complex<f32>> {
    multi_block(color_code, channel, tx_hz, rx_hz, true, rate)
}

fn multi_block(
    color_code: u8,
    channel: u16,
    tx_hz: u64,
    rx_hz: u64,
    interrupt: bool,
    rate: f64,
) -> Vec<Complex<f32>> {
    const OPCODE: u64 = 0b101000;

    let mut header = vec![false; 80];
    write(&mut header, 2, 6, OPCODE);
    write(&mut header, 16, 5, 0b00101);
    write(&mut header, 68, 12, u64::from(channel));
    let header = with_crc(header, 0xAAAA);

    let mut block = vec![false; 80];
    block[0] = true;
    write(&mut block, 2, 6, OPCODE);
    write(&mut block, 22, 12, u64::from(channel));
    write(&mut block, 34, 10, tx_hz / 1_000_000);
    write(&mut block, 44, 13, tx_hz % 1_000_000 / 125);
    write(&mut block, 57, 10, rx_hz / 1_000_000);
    write(&mut block, 67, 13, rx_hz % 1_000_000 / 125);
    let continuation = with_crc(block, 0);

    let mut preamble = vec![false; 80];
    write(&mut preamble, 2, 6, 0b111101);
    let preamble = with_crc(preamble, 0xA5A5);

    let call = Call {
        color_code,
        ..Call::default()
    };
    let mut tx = Keyed::default();
    for _ in 0..2 {
        tx.burst(&data_burst(&call, DT_MBC_HEADER, &header));
        if interrupt {
            tx.burst(&data_burst(&call, DT_CSBK, &preamble));
        }
        tx.burst(&data_burst(&call, DT_MBC_CONTINUATION, &continuation));
    }
    tx.modulate(rate)
}

fn write(bits: &mut [bool], at: usize, width: usize, value: u64) {
    for index in 0..width {
        bits[at + index] = value >> (width - index - 1) & 1 == 1;
    }
}

fn with_crc(mut payload: Vec<bool>, mask: u16) -> Vec<bool> {
    let crc = !crc16_msb(0x1021, 0, &pack(&payload)) ^ mask;
    payload.extend(bits(u64::from(crc), 16));
    payload
}

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
            .map(|sample| sample + noise.sample())
            .collect()
    }
}

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

fn lc_with_parity(call: &Call, mask: [u8; 3]) -> Vec<bool> {
    let lc = call.link_control();
    let parity = rs129_parity(&pack(&lc));
    let mut out = lc;
    for (byte, m) in parity.into_iter().zip(mask) {
        out.extend(bits(u64::from(byte ^ m), 8));
    }
    out
}

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
