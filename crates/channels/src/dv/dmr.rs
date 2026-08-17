use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Bptc128, Bptc196, CyclicCode, ParityCode, crc16_msb, rs129_parity};
use sdrmm_modem::cpm::CpmDemod;
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, DmrParams, DmrSlots,
    DvChannelDefinition, DvFrame, DvFrameKind, DvMode, DvSlotActivity, DvTrunkProtocol, Vendor,
};

use super::{
    INPUT_RATE_HZ, SymbolWindow, bits_to_u32, c4fm_demod, c4fm_params, pack_bytes,
    vocoder::{HALF_RATE_SOFT_BITS, MbeDecoder},
};
use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

pub(crate) const BAUD: f64 = 4_800.0;
pub(crate) const DEVIATION_HZ: f64 = 1_944.0;
pub(crate) const RRC_ALPHA: f64 = 0.2;
pub(crate) const BANDWIDTH_HZ: f64 = 12_500.0;

const BURST_BITS: usize = 264;
const BURST_SYMBOLS: usize = BURST_BITS / 2;
pub(crate) const SYNC_BITS: u32 = 48;
const HALF_PAYLOAD_BITS: usize = 108;
const TRAILING_SYMBOLS: usize = HALF_PAYLOAD_BITS / 2;

const SLOT_SYMBOLS: usize = 144;

const SUPERFRAME_STRIDE: usize = SLOT_SYMBOLS * 2;
const SPEAKER_HOLD_SYMBOLS: usize = SUPERFRAME_STRIDE * 3;

pub(crate) const SYNC_TOLERANCE: u32 = 4;

const DT_PI_HEADER: u8 = 0x0;
const DT_VOICE_LC_HEADER: u8 = 0x1;
const DT_TERMINATOR_WITH_LC: u8 = 0x2;
const DT_CSBK: u8 = 0x3;
const DT_MBC_HEADER: u8 = 0x4;
const DT_MBC_CONTINUATION: u8 = 0x5;
const DT_DATA_HEADER: u8 = 0x6;
const DT_RATE_HALF_DATA: u8 = 0x7;
const DT_RATE_THREE_QUARTER_DATA: u8 = 0x8;
const DT_RATE_ONE_DATA: u8 = 0xA;
const DT_UNIFIED_SINGLE_BLOCK_DATA: u8 = 0xB;

const VOICE_LC_HEADER_MASK: [u8; 3] = [0x96, 0x96, 0x96];
const TERMINATOR_LC_MASK: [u8; 3] = [0x99, 0x99, 0x99];
const PI_HEADER_MASK: u16 = 0x6969;
const CSBK_MASK: u16 = 0xA5A5;
const MBC_HEADER_MASK: u16 = 0xAAAA;
const DATA_HEADER_MASK: u16 = 0xCCCC;

const LC_BITS: usize = 72;
const VOCODER_FRAME_BITS: usize = HALF_RATE_SOFT_BITS;
const VOCODER_FRAMES_PER_BURST: usize = 3;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "dmr".to_owned(),
    name: "DMR".to_owned(),
    bandwidth_hz: BANDWIDTH_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: true,
    decoder_kind: Some("dv".to_owned()),
    ..ChannelDescriptor::default()
});

struct Sync {
    bits: u64,
    voice: bool,
    slot: Option<u8>,
}

const SYNCS: [Sync; 8] = [
    Sync {
        bits: 0x755F_D7DF_75F7,
        voice: true,
        slot: None,
    },
    Sync {
        bits: 0xDFF5_7D75_DF5D,
        voice: false,
        slot: None,
    },
    Sync {
        bits: 0x7F7D_5DD5_7DFD,
        voice: true,
        slot: None,
    },
    Sync {
        bits: 0xD5D7_F77F_D757,
        voice: false,
        slot: None,
    },
    Sync {
        bits: 0x5D57_7F77_57FF,
        voice: true,
        slot: Some(1),
    },
    Sync {
        bits: 0xF7FD_D5DD_FD55,
        voice: false,
        slot: Some(1),
    },
    Sync {
        bits: 0x7DFF_D5F5_5D5F,
        voice: true,
        slot: Some(2),
    },
    Sync {
        bits: 0xD755_7F5F_F7F5,
        voice: false,
        slot: Some(2),
    },
];

pub(crate) const SYNC_PATTERNS: [u64; SYNCS.len()] = {
    let mut out = [0; SYNCS.len()];
    let mut i = 0;
    while i < SYNCS.len() {
        out[i] = SYNCS[i].bits;
        i += 1;
    }
    out
};

pub struct DmrChannel {
    demod: CpmDemod,
    symbols: Vec<f32>,
    decoder: Decoder,
}

fn params(settings: &ChannelSettings) -> Result<&DmrParams, ChannelError> {
    match &settings.params {
        ChannelParams::Dmr(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "dmr channel got {} params",
            other.type_id()
        ))),
    }
}

pub(crate) fn occupied_band() -> (f64, f64) {
    (-BANDWIDTH_HZ / 2.0, BANDWIDTH_HZ / 2.0)
}

pub(crate) fn channel_filter() -> ChannelFilter {
    super::channel_filter(BANDWIDTH_HZ)
}

impl ChannelRx for DmrChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let params = params(&settings)?;
        Self::with_options(ctx, params.slots, params.ignore_crc)
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let params = params(&settings)?;
        self.set_options(params.slots, params.ignore_crc);
        Ok(())
    }

    fn retuned(&mut self) {
        self.demod.reset();
        self.decoder.reset();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.symbols.clear();
        self.demod.process(iq, &mut self.symbols);
        for &symbol in &self.symbols {
            self.decoder.push(symbol, out);
        }
    }
}

impl DmrChannel {
    fn with_options(
        ctx: ChannelCtx,
        slots: DmrSlots,
        ignore_crc: bool,
    ) -> Result<Self, ChannelError> {
        Ok(Self {
            demod: c4fm_demod(&c4fm_params(ctx.input_rate, BAUD, DEVIATION_HZ, RRC_ALPHA)),
            symbols: Vec::new(),
            decoder: Decoder::new(DmrParams { slots, ignore_crc }),
        })
    }

    fn set_options(&mut self, slots: DmrSlots, ignore_crc: bool) {
        self.decoder.params = DmrParams { slots, ignore_crc };
    }
}

#[derive(Clone, Copy)]
enum Pending {
    None,
    Burst { voice: bool, slot: Option<u8> },
}

struct Decoder {
    params: DmrParams,
    window: SymbolWindow,
    pending: Pending,
    countdown: usize,
    followers: [u8; 2],
    follower_countdown: [usize; 2],
    bits: Vec<bool>,
    soft: Vec<i8>,
    bytes: Vec<u8>,
    slots: [SlotState; 2],
    short_lc: ShortLc,
    mbc_headers: [Option<MbcHeader>; 2],
    speaking: Option<usize>,
    speaking_hold: usize,
}

#[derive(Clone)]
struct MbcHeader {
    fid: u8,
    opcode: u8,
    frame: DvFrame,
}

struct SlotState {
    embedded: Vec<bool>,
    embedded_errors: u32,
    encrypted: Option<bool>,
    talker_alias: TalkerAlias,
    voice: MbeDecoder,
}

impl SlotState {
    fn new() -> Self {
        Self {
            embedded: Vec::with_capacity(Bptc128::CODED_BITS),
            embedded_errors: 0,
            encrypted: None,
            talker_alias: TalkerAlias::default(),
            voice: MbeDecoder::half_rate(),
        }
    }

    fn reset(&mut self) {
        self.embedded.clear();
        self.embedded_errors = 0;
        self.encrypted = None;
        self.talker_alias.reset();
        self.voice.reset();
    }
}

impl Decoder {
    fn new(params: DmrParams) -> Self {
        Self {
            params,
            window: SymbolWindow::new(SLOT_SYMBOLS),
            pending: Pending::None,
            countdown: 0,
            followers: [0; 2],
            follower_countdown: [0; 2],
            bits: Vec::with_capacity(BURST_BITS),
            soft: Vec::with_capacity(BURST_BITS),
            bytes: Vec::with_capacity(BURST_BITS / 8),
            slots: std::array::from_fn(|_| SlotState::new()),
            short_lc: ShortLc::default(),
            mbc_headers: std::array::from_fn(|_| None),
            speaking: None,
            speaking_hold: 0,
        }
    }

    fn reset(&mut self) {
        self.window.reset();
        self.pending = Pending::None;
        self.countdown = 0;
        self.followers = [0; 2];
        self.follower_countdown = [0; 2];
        self.slots.iter_mut().for_each(SlotState::reset);
        self.short_lc.reset();
        self.mbc_headers.fill(None);
        self.speaking = None;
        self.speaking_hold = 0;
    }

    fn push(&mut self, symbol: f32, out: &mut ChannelOutputs) {
        self.window.push(symbol);
        if self.speaking_hold > 0 {
            self.speaking_hold -= 1;
            if self.speaking_hold == 0 {
                self.speaking = None;
            }
        }
        for index in 0..2 {
            if self.follower_countdown[index] > 0 {
                self.follower_countdown[index] -= 1;
                if self.follower_countdown[index] == 0 {
                    self.voice_burst(index as u8 + 1, out);
                }
            }
        }
        if self.countdown > 0 {
            self.countdown -= 1;
            if self.countdown > 0 {
                return;
            }
            match std::mem::replace(&mut self.pending, Pending::None) {
                Pending::None => {}
                Pending::Burst { voice, slot } => self.burst(voice, slot, out),
            }
            return;
        }
        self.hunt(out);
    }

    fn hunt(&mut self, out: &mut ChannelOutputs) {
        for sync in &SYNCS {
            if self.window.sync_distance(sync.bits, SYNC_BITS) <= SYNC_TOLERANCE {
                self.window.anchor(sync.bits, SYNC_BITS);
                let slot = sync.slot.or_else(|| self.cach(out));
                self.pending = Pending::Burst {
                    voice: sync.voice,
                    slot,
                };
                self.countdown = TRAILING_SYMBOLS;
                if let Some(index) = slot_index(slot) {
                    self.slots[index].embedded.clear();
                    self.slots[index].embedded_errors = 0;
                    self.followers[index] = 0;
                    self.follower_countdown[index] = 0;
                }
                return;
            }
        }
    }

    fn cach(&mut self, out: &mut ChannelOutputs) -> Option<u8> {
        const CACH_BACK: usize = HALF_PAYLOAD_BITS / 2 + SYNC_BITS as usize / 2;
        self.cach_at(CACH_BACK, out)
    }

    fn cach_at(&mut self, end_back: usize, out: &mut ChannelOutputs) -> Option<u8> {
        const TACT: [usize; 7] = [0, 4, 8, 12, 14, 18, 22];
        const PAYLOAD: [usize; 17] = [1, 2, 3, 5, 6, 7, 9, 10, 11, 13, 15, 16, 17, 19, 20, 21, 23];
        self.window.bits(end_back, 12, &mut self.bits);
        let mut tact = TACT.map(|position| self.bits[position]);
        ParityCode::HAMMING_7_4.decode(&mut tact)?;
        let slot = u8::from(tact[1]) + 1;
        let lcss = u8::from(tact[2]) << 1 | u8::from(tact[3]);
        let payload = PAYLOAD.map(|position| self.bits[position]);
        if let Some(mut frame) = self.short_lc.push(lcss, payload) {
            frame.errors_corrected = 0;
            self.emit(frame, out);
        }
        Some(slot)
    }

    fn burst(&mut self, voice: bool, slot: Option<u8>, out: &mut ChannelOutputs) {
        if voice {
            self.window.bits(0, BURST_SYMBOLS, &mut self.bits);
            let index = slot_index(slot).unwrap_or(0);
            self.voice_payload(index, slot, out);
            self.followers[index] = 5;
            self.schedule_follower(index);
            return;
        }
        if let Some(index) = slot_index(slot) {
            self.followers[index] = 0;
            self.follower_countdown[index] = 0;
        }
        self.window.bits(0, BURST_SYMBOLS, &mut self.bits);
        let index = slot_index(slot).unwrap_or(0);
        let Some(frame) = self.data_burst(index, slot) else {
            return;
        };
        match frame.kind {
            DvFrameKind::Header => {
                if let Some(encrypted) = frame.encrypted {
                    self.slots[index].encrypted = Some(encrypted);
                }
                self.slots[index].voice.reset();
            }
            DvFrameKind::Terminator => {
                self.slots[index].encrypted = None;
                self.slots[index].voice.reset();
                if self.speaking == Some(index) {
                    self.speaking = None;
                    self.speaking_hold = 0;
                }
            }
            _ => {}
        }
        self.emit(frame, out);
    }

    fn schedule_follower(&mut self, index: usize) {
        if self.followers[index] == 0 {
            return;
        }
        self.followers[index] -= 1;
        self.follower_countdown[index] = SUPERFRAME_STRIDE;
    }

    fn voice_burst(&mut self, slot: u8, out: &mut ChannelOutputs) {
        let index = usize::from(slot - 1);
        self.cach_at(BURST_SYMBOLS, out);
        self.window.bits(0, BURST_SYMBOLS, &mut self.bits);
        if let Some(frame) = self.embedded_signalling(index, Some(slot)) {
            if let Some(encrypted) = frame.encrypted {
                self.slots[index].encrypted = Some(encrypted);
            }
            self.emit(frame, out);
        }
        self.voice_payload(index, Some(slot), out);
        self.schedule_follower(index);
    }

    fn take_speaker(&mut self, index: usize) -> bool {
        if self.speaking.is_some_and(|speaking| speaking != index) {
            return false;
        }
        self.speaking = Some(index);
        self.speaking_hold = SPEAKER_HOLD_SYMBOLS;
        true
    }

    fn voice_payload(&mut self, index: usize, slot: Option<u8>, out: &mut ChannelOutputs) {
        if slot.is_some_and(|slot| !self.params.slots.accepts(slot)) {
            return;
        }
        if !self.take_speaker(index) {
            return;
        }
        self.window
            .vocoder_soft_bits(0, BURST_SYMBOLS, &mut self.soft);
        let mut payload = [0i8; VOCODER_FRAME_BITS * VOCODER_FRAMES_PER_BURST];
        payload[..HALF_PAYLOAD_BITS].copy_from_slice(&self.soft[..HALF_PAYLOAD_BITS]);
        payload[HALF_PAYLOAD_BITS..].copy_from_slice(&self.soft[156..]);
        for frame in payload.as_chunks::<VOCODER_FRAME_BITS>().0 {
            let state = &mut self.slots[index];
            state
                .voice
                .decode_half_soft(frame, state.encrypted != Some(false), out);
        }
    }

    fn emit(&self, frame: DvFrame, out: &mut ChannelOutputs) {
        if frame
            .slot
            .is_none_or(|slot| self.params.slots.accepts(slot))
        {
            out.events.push(DecoderEvent::Dv(frame));
        }
    }

    fn data_burst(&mut self, index: usize, slot: Option<u8>) -> Option<DvFrame> {
        let slot_type = u64::from(bits_to_u32(&self.bits, 98, 10)) << 10
            | u64::from(bits_to_u32(&self.bits, 156, 10));
        let (info, slot_errors) = CyclicCode::GOLAY_20_8.decode(slot_type)?;
        let colour = (info >> 4) as u16 & 0x0F;
        let data_type = info as u8 & 0x0F;

        if !matches!(data_type, DT_MBC_HEADER | DT_MBC_CONTINUATION) {
            self.mbc_headers[index] = None;
        }

        let mut coded = [false; Bptc196::CODED_BITS];
        for (slot, &bit) in coded
            .iter_mut()
            .zip(self.bits[0..98].iter().chain(&self.bits[166..264]))
        {
            *slot = bit;
        }
        if data_type == DT_RATE_THREE_QUARTER_DATA || data_type == DT_RATE_ONE_DATA {
            let mut frame = if data_type == DT_RATE_THREE_QUARTER_DATA {
                let decoded = decode_rate_three_quarter(&self.bits)?;
                data_block(&decoded, "rate 3/4 data")
            } else {
                let decoded = decode_rate_one(&coded);
                data_block(&decoded, "rate 1 data")
            };
            frame.slot = slot;
            frame.color_code = Some(colour);
            frame.errors_corrected = slot_errors;
            return Some(frame);
        }
        let (payload, payload_errors) = Bptc196::decode(&coded)?;
        let errors = slot_errors + payload_errors;

        let mut frame = match data_type {
            DT_VOICE_LC_HEADER => self.link_control(index, &payload, VOICE_LC_HEADER_MASK)?,
            DT_TERMINATOR_WITH_LC => {
                let mut frame = self.link_control(index, &payload, TERMINATOR_LC_MASK)?;
                frame.kind = DvFrameKind::Terminator;
                frame
            }
            DT_CSBK => self.csbk(&payload, errors)?,
            DT_MBC_HEADER => self.mbc_header(index, &payload, errors)?,
            DT_MBC_CONTINUATION => self.mbc_continuation(index, &payload, errors)?,
            DT_DATA_HEADER => self.data_header(&payload, errors)?,
            DT_RATE_HALF_DATA => data_block(&payload, "rate 1/2 data"),
            DT_UNIFIED_SINGLE_BLOCK_DATA => {
                let mut frame = self.checked_block(&payload, DATA_HEADER_MASK, errors)?;
                frame.kind = DvFrameKind::Data;
                frame.opcode = Some("unified single block data".to_owned());
                frame.data = Some(hex_bits(&payload[..80]));
                frame
            }
            DT_PI_HEADER => {
                let mut frame = self.checked_block(&payload, PI_HEADER_MASK, errors)?;
                frame.kind = DvFrameKind::Header;
                frame.encrypted = Some(true);
                frame.algorithm_id = Some(bits_to_u32(&payload, 5, 3) as u8);
                let fid = bits_to_u32(&payload, 8, 8) as u8;
                set_dmr_vendor(&mut frame, fid);
                frame.key_id = Some(bits_to_u32(&payload, 16, 8) as u16);
                frame.message_indicator = Some(format!("{:08X}", bits_to_u32(&payload, 24, 32)));
                frame.destination = Some(bits_to_u32(&payload, 56, 24));
                frame
            }
            _ => return None,
        };
        frame.slot = slot;
        frame.color_code = Some(colour);
        frame.errors_corrected = errors;
        Some(frame)
    }

    fn checked_block(&mut self, payload: &[bool; 96], mask: u16, errors: u32) -> Option<DvFrame> {
        pack_bytes(&payload[..80], &mut self.bytes);
        let expected = dmr_crc16(&self.bytes) ^ mask;
        let verified = expected == bits_to_u32(payload, 80, 16) as u16;
        if !verified && !(self.params.ignore_crc && errors == 0) {
            return None;
        }
        let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
        frame.crc_verified = Some(verified);
        Some(frame)
    }

    fn link_control(
        &mut self,
        index: usize,
        payload: &[bool; 96],
        mask: [u8; 3],
    ) -> Option<DvFrame> {
        pack_bytes(payload, &mut self.bytes);
        let (lc, parity) = self.bytes.split_at(LC_BITS / 8);
        let mut received = [parity[0], parity[1], parity[2]];
        for (byte, m) in received.iter_mut().zip(mask) {
            *byte ^= m;
        }
        if rs129_parity(lc) != received {
            return None;
        }
        Some(self.decode_lc(index, payload))
    }

    fn csbk(&mut self, payload: &[bool; 96], errors: u32) -> Option<DvFrame> {
        let mut frame = self.checked_block(payload, CSBK_MASK, errors)?;
        let opcode = bits_to_u32(payload, 2, 6) as u8;
        let fid = bits_to_u32(payload, 8, 8) as u8;
        set_dmr_vendor(&mut frame, fid);
        frame.opcode = Some(csbk_opcode_name(fid, opcode));
        if fid != 0 {
            frame.data = Some(hex_bits(&payload[16..80]));
        }
        if fid == 0
            && matches!(
                opcode,
                0b000100
                    | 0b000101
                    | 0b101110
                    | 0b101111
                    | 0b110000
                    | 0b110001
                    | 0b110010
                    | 0b110101
                    | 0b111000
                    | 0b111101
            )
        {
            frame.destination = Some(bits_to_u32(payload, 32, 24));
            frame.source = Some(bits_to_u32(payload, 56, 24));
        } else if fid == 0 && opcode == 0b100110 {
            frame.source = Some(bits_to_u32(payload, 32, 24));
            frame.destination = Some(bits_to_u32(payload, 56, 24));
        }
        match (fid, opcode) {
            (0, 0b110001 | 0b110010) => frame.group_call = Some(true),
            (0, 0b000100 | 0b000101 | 0b110000 | 0b110101) => {
                frame.group_call = Some(false);
            }
            _ => {}
        }
        decode_vendor_csbk(&mut frame, fid, opcode, payload);
        decode_tier_three_csbk(&mut frame, fid, opcode, payload);
        Some(frame)
    }

    fn mbc_header(&mut self, index: usize, payload: &[bool; 96], errors: u32) -> Option<DvFrame> {
        let mut frame = self.checked_block(payload, MBC_HEADER_MASK, errors)?;
        let opcode = bits_to_u32(payload, 2, 6) as u8;
        let fid = bits_to_u32(payload, 8, 8) as u8;
        set_dmr_vendor(&mut frame, fid);
        frame.opcode = Some(format!("{} MBC header", csbk_opcode_name(fid, opcode)));
        decode_tier_three_csbk(&mut frame, fid, opcode, payload);
        if fid == 0 && opcode == 0b101000 {
            let announcement = bits_to_u32(payload, 16, 5) as u8;
            if announcement == 0b00101 {
                frame.channel = Some(bits_to_u32(payload, 68, 12) as u16);
                frame.opcode = Some("broadcast channel frequency MBC header".to_owned());
            }
        }
        self.mbc_headers[index] = Some(MbcHeader {
            fid,
            opcode,
            frame: frame.clone(),
        });
        Some(frame)
    }

    fn mbc_continuation(
        &mut self,
        index: usize,
        payload: &[bool; 96],
        errors: u32,
    ) -> Option<DvFrame> {
        let header = self.mbc_headers[index].take()?;
        let opcode = bits_to_u32(payload, 2, 6) as u8;
        let verified = valid_mbc_crc(payload);
        if opcode != header.opcode
            || !payload[0]
            || (!verified && !(self.params.ignore_crc && errors == 0))
        {
            return None;
        }
        let mut frame = header.frame;
        frame.crc_verified = Some(verified && frame.crc_verified == Some(true));
        let color_code = is_tier_three_grant(opcode).then(|| bits_to_u32(payload, 12, 4) as u8);
        frame.channel_definition = decode_channel_definition(payload, color_code);
        if let Some(definition) = &frame.channel_definition {
            frame.channel = Some(definition.channel);
            frame.trunk_protocol = Some(DvTrunkProtocol::TierThree);
            frame.data = Some(format!(
                "TX {} Hz, RX {} Hz",
                definition.tx_hz, definition.rx_hz
            ));
        }
        frame.opcode = Some(format!(
            "{} absolute parameters",
            csbk_opcode_name(header.fid, opcode)
        ));
        Some(frame)
    }

    fn data_header(&mut self, payload: &[bool; 96], errors: u32) -> Option<DvFrame> {
        let mut frame = self.checked_block(payload, DATA_HEADER_MASK, errors)?;
        let format = bits_to_u32(payload, 4, 4) as u8;
        frame.kind = DvFrameKind::Data;
        frame.group_call = Some(payload[0]);
        frame.destination = Some(bits_to_u32(payload, 16, 24));
        frame.source = Some(bits_to_u32(payload, 40, 24));
        frame.opcode = Some(data_format_name(format).to_owned());
        frame.data = Some(format!(
            "SAP {:X}, {} block(s)",
            bits_to_u32(payload, 8, 4),
            bits_to_u32(payload, 65, 7)
        ));
        Some(frame)
    }

    fn decode_lc(&mut self, index: usize, lc: &[bool]) -> DvFrame {
        let flco = bits_to_u32(lc, 2, 6) as u8;
        let fid = bits_to_u32(lc, 8, 8) as u8;
        let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Header);
        set_dmr_vendor(&mut frame, fid);
        match (fid, flco) {
            (0, 0 | 3) | (0x10, 0) | (0x06, 0 | 3) => {
                frame.group_call = Some(flco == 0);
                frame.destination = Some(bits_to_u32(lc, 24, 24));
                frame.source = Some(bits_to_u32(lc, 48, 24));
                frame.encrypted = Some(lc[17]);
                frame.emergency = Some(lc[16]);
                if fid != 0 {
                    frame.opcode = Some(format!(
                        "{} voice channel user",
                        frame.vendor.map_or("vendor", Vendor::label)
                    ));
                }
            }
            (0, 4..=7) => {
                frame.opcode = Some(talker_alias_opcode(flco).to_owned());
                frame.talker_alias = self.slots[index].talker_alias.update(flco, lc);
            }
            (0x68, 4..=7) => {
                frame.opcode = Some(format!("Hytera XPT {}", talker_alias_opcode(flco)));
                frame.talker_alias = self.slots[index].talker_alias.update(flco, lc);
            }
            (0x10, 0x14..=0x17) => {
                let alias_flco = flco - 0x10;
                frame.opcode = Some(format!("Motorola {}", talker_alias_opcode(alias_flco)));
                frame.talker_alias = self.slots[index].talker_alias.update(alias_flco, lc);
            }
            (0, 8) => {
                frame.opcode = Some("GPS Info".to_owned());
                decode_gps_info(&mut frame, lc);
            }
            (0x10, 4 | 7) => {
                frame.opcode = Some("Capacity Plus voice channel user".to_owned());
                frame.trunk_protocol = Some(DvTrunkProtocol::CapacityPlus);
                frame.group_call = Some(flco == 4);
                frame.destination = Some(bits_to_u32(lc, 24, 24));
                frame.source = Some(bits_to_u32(lc, 56, 16));
                frame.rest_channel = Some(bits_to_u32(lc, 52, 4) as u16);
            }
            (0x68, 0 | 3 | 9) => {
                frame.trunk_protocol = Some(DvTrunkProtocol::HyteraXpt);
                frame.opcode = Some(
                    if flco == 9 {
                        "Hytera XPT call alert"
                    } else {
                        "Hytera XPT voice channel user"
                    }
                    .to_owned(),
                );
                frame.group_call = Some(lc[1]);
                frame.destination = Some(bits_to_u32(lc, 32, 16));
                frame.source = Some(bits_to_u32(lc, 56, 16));
                frame.channel = Some(bits_to_u32(lc, 16, 4) as u16);
                if flco == 9 {
                    frame.data = Some(format!(
                        "free LCN {}, handshake {}",
                        bits_to_u32(lc, 24, 4),
                        bits_to_u32(lc, 28, 4)
                    ));
                }
            }
            _ => {
                let vendor = frame.vendor.map_or("vendor", Vendor::label);
                frame.opcode = Some(format!("{vendor} FLCO {flco}, unparsed"));
                frame.data = Some(hex_bits(&lc[16..72]));
            }
        }
        frame
    }

    fn embedded_signalling(&mut self, index: usize, slot: Option<u8>) -> Option<DvFrame> {
        let emb = u64::from(bits_to_u32(&self.bits, 108, 8)) << 8
            | u64::from(bits_to_u32(&self.bits, 148, 8));
        let (info, errors) = CyclicCode::QR_16_7.decode(emb)?;
        let colour = (info >> 3) as u16 & 0x0F;
        let lcss = info & 0b11;

        match lcss {
            0b01 => {
                self.slots[index].embedded.clear();
                self.slots[index].embedded_errors = 0;
            }
            0b10 | 0b11 if !self.slots[index].embedded.is_empty() => {}
            _ => return None,
        }
        self.slots[index]
            .embedded
            .extend(self.bits[116..148].iter().copied());
        self.slots[index].embedded_errors += errors;

        if lcss != 0b10 {
            return None;
        }
        let coded: [bool; Bptc128::CODED_BITS] =
            self.slots[index].embedded.as_slice().try_into().ok()?;
        let (data, bptc_errors) = Bptc128::decode(&coded)?;
        self.slots[index].embedded.clear();

        let mut lc = [false; LC_BITS];
        let mut checksum = 0u32;
        let mut written = 0;
        for (i, &bit) in data.iter().enumerate() {
            if i >= 22 && i % 11 == 10 {
                checksum = checksum << 1 | u32::from(bit);
            } else {
                lc[written] = bit;
                written += 1;
            }
        }
        pack_bytes(&lc, &mut self.bytes);
        let sum = self.bytes.iter().map(|&b| u32::from(b)).sum::<u32>() % 31;
        if sum != checksum {
            return None;
        }
        let mut frame = self.decode_lc(index, &lc);
        frame.kind = DvFrameKind::Voice;
        frame.slot = slot;
        frame.color_code = Some(colour);
        frame.errors_corrected = self.slots[index].embedded_errors + bptc_errors;
        Some(frame)
    }
}

fn slot_index(slot: Option<u8>) -> Option<usize> {
    slot.filter(|slot| (1..=2).contains(slot))
        .map(|slot| usize::from(slot - 1))
}

#[derive(Default)]
struct ShortLc {
    payload: Vec<bool>,
}

impl ShortLc {
    fn reset(&mut self) {
        self.payload.clear();
    }

    fn push(&mut self, lcss: u8, payload: [bool; 17]) -> Option<DvFrame> {
        match lcss {
            0b01 => self.payload.clear(),
            0b11 | 0b10 if !self.payload.is_empty() => {}
            _ => return None,
        }
        self.payload.extend(payload);
        if lcss != 0b10 || self.payload.len() != 68 {
            return None;
        }

        let mut matrix = [[false; 17]; 4];
        for column in 0..17 {
            for (row, cells) in matrix.iter_mut().enumerate() {
                cells[column] = self.payload[column * 4 + row];
            }
        }
        self.payload.clear();
        for row in &mut matrix[..3] {
            ParityCode::HAMMING_17_12.decode(row)?;
        }
        for column in 0..17 {
            if matrix
                .iter()
                .fold(false, |parity, row| parity ^ row[column])
            {
                return None;
            }
        }
        let mut message = [false; 36];
        for (target, bit) in message
            .iter_mut()
            .zip(matrix[..3].iter().flat_map(|row| &row[..12]))
        {
            *target = *bit;
        }
        if crc8_dmr(&message[..28]) != bits_to_u32(&message, 28, 8) as u8 {
            return None;
        }
        Some(decode_short_lc(&message[..28]))
    }
}

fn crc8_dmr(bits: &[bool]) -> u8 {
    let mut crc = 0u8;
    for &bit in bits {
        let high = crc & 0x80 != 0;
        crc <<= 1;
        if high != bit {
            crc ^= 0x07;
        }
    }
    crc
}

fn decode_short_lc(bits: &[bool]) -> DvFrame {
    let slco = bits_to_u32(bits, 0, 4) as u8;
    let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
    frame.vendor = Some(Vendor::Etsi);
    frame.manufacturer_id = Some(0);
    match slco {
        0 => frame.opcode = Some("Short LC null message".to_owned()),
        1 => {
            frame.opcode = Some("Short LC activity update".to_owned());
            for (slot, activity_at, hash_at) in [(1, 4, 12), (2, 8, 20)] {
                let activity = bits_to_u32(bits, activity_at, 4) as u8;
                if activity != 0 {
                    frame.slot_activity.push(DvSlotActivity {
                        slot,
                        activity: activity_name(activity).to_owned(),
                        destination_hash: Some(bits_to_u32(bits, hash_at, 8) as u8),
                    });
                }
            }
        }
        2 | 3 => {
            frame.opcode = Some(if slco == 2 {
                "Tier III control-channel system parameters".to_owned()
            } else {
                "Tier III payload-channel system parameters".to_owned()
            });
            frame.system_id = Some(bits_to_u32(bits, 6, 12) as u16);
            frame.data = Some(format!(
                "network model {}, common slot counter {}",
                bits_to_u32(bits, 4, 2),
                bits_to_u32(bits, 19, 9)
            ));
        }
        8 => {
            frame.opcode = Some("Hytera XPT Short LC".to_owned());
            frame.trunk_protocol = Some(DvTrunkProtocol::HyteraXpt);
            frame.vendor = Some(Vendor::Hytera);
            frame.manufacturer_id = Some(0x68);
            frame.channel = Some(bits_to_u32(bits, 12, 4) as u16);
            frame.data = Some(format!(
                "priority LCN {}, priority hash {:02X}",
                bits_to_u32(bits, 16, 4),
                bits_to_u32(bits, 20, 8)
            ));
        }
        9 | 10 => {
            frame.opcode = Some(if slco == 9 {
                "Connect Plus traffic-channel Short LC".to_owned()
            } else {
                "Connect Plus control-channel Short LC".to_owned()
            });
            frame.vendor = Some(Vendor::Motorola);
            frame.manufacturer_id = Some(0x06);
            frame.network_id = Some(bits_to_u32(bits, 8, 8));
            frame.site_id = Some(bits_to_u32(bits, 16, 8) as u16);
        }
        15 => {
            frame.opcode = Some("Capacity Plus site Short LC".to_owned());
            frame.vendor = Some(Vendor::Motorola);
            frame.manufacturer_id = Some(0x10);
            frame.site_id = Some(bits_to_u32(bits, 22, 3) as u16);
            frame.rest_channel = Some(bits_to_u32(bits, 16, 4) as u16);
        }
        _ => frame.opcode = Some(format!("Short LC {slco:X}, unparsed")),
    }
    frame
}

fn activity_name(activity: u8) -> &'static str {
    match activity {
        2 => "group CSBK",
        3 => "individual CSBK",
        8 => "group voice",
        9 => "individual voice",
        10 => "individual data",
        11 => "group data",
        12 => "emergency group voice",
        13 => "emergency individual voice",
        _ => "reserved activity",
    }
}

fn csbk_opcode_name(fid: u8, opcode: u8) -> String {
    let name = match (fid, opcode) {
        (0, 0b000100) => "unit-to-unit voice service request",
        (0, 0b000101) => "unit-to-unit voice service answer response",
        (0, 0b000111) => "channel timing",
        (0, 0b100110) => "negative acknowledge response",
        (0, 0b111000) => "BS outbound activation",
        (0, 0b111101) => "preamble",
        (0, 0b011001) => "ALOHA",
        (0, 0b011100) => "AHOY",
        (0, 0b101000) => "broadcast",
        (0, 0b101110) => "clear",
        (0, 0b101111) => "protect",
        (0, 0b110000) => "private voice channel grant",
        (0, 0b110001) => "talkgroup voice channel grant",
        (0, 0b110010) => "broadcast talkgroup voice channel grant",
        (0, 0b110101) => "duplex private voice channel grant",
        (0x10, 0x3A) => "Capacity Plus system CSBK",
        (0x10, 0x3B) => "Capacity Plus adjacent sites",
        (0x10, 0x3E) => "Capacity Plus channel status",
        (0x06, 0x01) => "Connect Plus adjacent sites",
        (0x06, 0x03) => "Connect Plus voice channel grant",
        (0x06, 0x06) => "Connect Plus data channel grant",
        (0x06, 0x0C) => "Connect Plus slot termination",
        (0x06, _) => "Connect Plus CSBK, unparsed",
        (0x68, 0x0A) => "Hytera XPT site status",
        (0x68, 0x0B) => "Hytera XPT adjacent sites",
        (0x08 | 0x68, _) => "Hytera CSBK, unparsed",
        _ => return format!("{} CSBK {opcode:02X}, unparsed", dmr_vendor(fid).label()),
    };
    name.to_owned()
}

fn dmr_vendor(fid: u8) -> Vendor {
    match fid {
        0 => Vendor::Etsi,
        0x06 | 0x10 => Vendor::Motorola,
        0x04 => Vendor::FlydeMicro,
        0x05 => Vendor::ProdEl,
        0x08 | 0x68 | 0x88 => Vendor::Hytera,
        0x0D | 0x13 | 0x1C | 0x20 => Vendor::Emc,
        0x33 => Vendor::JvcKenwood,
        0x3C => Vendor::RadioActivity,
        0x58 => Vendor::Tait,
        _ => Vendor::Unknown,
    }
}

fn set_dmr_vendor(frame: &mut DvFrame, fid: u8) {
    frame.vendor = Some(dmr_vendor(fid));
    frame.manufacturer_id = Some(fid);
}

fn decode_vendor_csbk(frame: &mut DvFrame, fid: u8, opcode: u8, payload: &[bool]) {
    if fid == 0x10 && matches!(opcode, 0x3A | 0x3B | 0x3E) {
        frame.trunk_protocol = Some(DvTrunkProtocol::CapacityPlus);
    } else if fid == 0x68 && matches!(opcode, 0x0A | 0x0B) {
        frame.trunk_protocol = Some(DvTrunkProtocol::HyteraXpt);
    }
    match (fid, opcode) {
        (0x10, 0x3A | 0x3E) => {
            frame.rest_channel = Some(bits_to_u32(payload, 20, 4) as u16);
            frame.data = Some(format!(
                "fragment {}, transmitted TS {}, reserved {}",
                bits_to_u32(payload, 16, 2),
                u8::from(payload[18]) + 1,
                u8::from(payload[19])
            ));
        }
        (0x10, 0x3B) => {
            let sites = (0..6)
                .filter_map(|index| {
                    let site = bits_to_u32(payload, 32 + index * 8, 4);
                    (site != 0).then(|| {
                        format!(
                            "site {site} rest {}",
                            bits_to_u32(payload, 36 + index * 8, 4)
                        )
                    })
                })
                .collect::<Vec<_>>();
            frame.data = Some(if sites.is_empty() {
                "no adjacent sites".to_owned()
            } else {
                sites.join(", ")
            });
        }
        (0x06, 0x01) => {
            let sites = (0..5)
                .filter_map(|index| {
                    let site = bits_to_u32(payload, 16 + index * 8, 8) & 0x3F;
                    (site != 0).then_some(site.to_string())
                })
                .collect::<Vec<_>>();
            frame.data = Some(format!("adjacent sites {}", sites.join(", ")));
        }
        (0x06, 0x03) => {
            let option = bits_to_u32(payload, 72, 8) as u8;
            frame.source = Some(bits_to_u32(payload, 16, 24));
            frame.destination = Some(bits_to_u32(payload, 40, 24));
            frame.channel = Some(bits_to_u32(payload, 64, 4) as u16);
            frame.group_call = match option {
                2 => Some(true),
                3 => Some(false),
                _ => None,
            };
            frame.data = Some(format!(
                "granted TS {}, option {option:02X}",
                u8::from(payload[68]) + 1
            ));
        }
        (0x06, 0x06) => {
            frame.destination = Some(bits_to_u32(payload, 16, 24));
            frame.channel = Some(bits_to_u32(payload, 40, 4) as u16);
            frame.data = Some(format!("granted TS {}", u8::from(payload[44]) + 1));
        }
        (0x68, 0x0A) => {
            frame.channel = Some(bits_to_u32(payload, 16, 4) as u16);
            frame.data = Some(format!(
                "sequence {}, slot states {:03X}",
                bits_to_u32(payload, 0, 2),
                bits_to_u32(payload, 20, 12)
            ));
        }
        _ => {}
    }
}

fn is_tier_three_grant(opcode: u8) -> bool {
    matches!(opcode, 0b110000..=0b110101)
}

fn decode_tier_three_csbk(frame: &mut DvFrame, fid: u8, opcode: u8, payload: &[bool]) {
    if fid != 0 {
        return;
    }
    if is_tier_three_grant(opcode) || matches!(opcode, 0b011001 | 0b011100 | 0b101000) {
        frame.trunk_protocol = Some(DvTrunkProtocol::TierThree);
    }
    if is_tier_three_grant(opcode) {
        frame.channel = Some(bits_to_u32(payload, 16, 12) as u16);
        frame.slot = Some(if payload[28] { 2 } else { 1 });
        frame.late_entry = Some(payload[29]);
        frame.emergency = Some(payload[30]);
    } else if opcode == 0b011001 {
        frame.system_id = Some(bits_to_u32(payload, 40, 16) as u16);
        frame.destination = Some(bits_to_u32(payload, 56, 24));
        frame.data = Some(format!(
            "mask {}, service {}, wait {}, backoff {}",
            bits_to_u32(payload, 25, 5),
            bits_to_u32(payload, 30, 2),
            bits_to_u32(payload, 32, 4),
            bits_to_u32(payload, 37, 4)
        ));
    } else if opcode == 0b011100 {
        frame.group_call = Some(payload[25]);
        frame.destination = Some(bits_to_u32(payload, 32, 24));
        frame.source = Some(bits_to_u32(payload, 56, 24));
    }
}

fn valid_mbc_crc(payload: &[bool; 96]) -> bool {
    let mut bytes = Vec::with_capacity(10);
    pack_bytes(&payload[..80], &mut bytes);
    dmr_crc16(&bytes) == bits_to_u32(payload, 80, 16) as u16
}

fn decode_channel_definition(
    payload: &[bool; 96],
    color_code: Option<u8>,
) -> Option<DvChannelDefinition> {
    if bits_to_u32(payload, 16, 4) != 0 || payload[20] || payload[21] {
        return None;
    }
    let channel = bits_to_u32(payload, 22, 12) as u16;
    let tx_mhz = u64::from(bits_to_u32(payload, 34, 10));
    let tx_fraction = u64::from(bits_to_u32(payload, 44, 13));
    let rx_mhz = u64::from(bits_to_u32(payload, 57, 10));
    let rx_fraction = u64::from(bits_to_u32(payload, 67, 13));
    Some(DvChannelDefinition {
        channel,
        tx_hz: tx_mhz * 1_000_000 + tx_fraction * 125,
        rx_hz: rx_mhz * 1_000_000 + rx_fraction * 125,
        color_code,
    })
}

fn data_format_name(format: u8) -> &'static str {
    match format {
        0 => "unified data transport header",
        1 => "response header",
        2 => "unconfirmed data header",
        3 => "confirmed data header",
        13 => "defined data header",
        14 => "raw data header",
        15 => "proprietary data header",
        _ => "data header",
    }
}

fn data_block(payload: &[bool], name: &str) -> DvFrame {
    let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Data);
    frame.opcode = Some(name.to_owned());
    frame.data = Some(hex_bits(payload));
    frame
}

fn hex_bits(bits: &[bool]) -> String {
    let mut out = String::with_capacity(bits.len().div_ceil(4));
    for nibble in bits.chunks(4) {
        let value = nibble
            .iter()
            .fold(0u8, |acc, bit| acc << 1 | u8::from(*bit));
        out.push(char::from_digit(u32::from(value), 16).unwrap_or('0'));
    }
    out
}

fn decode_rate_three_quarter(burst: &[bool]) -> Option<[bool; 144]> {
    const NEXT: [[u8; 8]; 8] = [
        [0, 8, 4, 12, 2, 10, 6, 14],
        [4, 12, 2, 10, 6, 14, 0, 8],
        [1, 9, 5, 13, 3, 11, 7, 15],
        [5, 13, 3, 11, 7, 15, 1, 9],
        [3, 11, 7, 15, 1, 9, 5, 13],
        [7, 15, 1, 9, 5, 13, 3, 11],
        [2, 10, 6, 14, 0, 8, 4, 12],
        [6, 14, 0, 8, 4, 12, 2, 10],
    ];
    const MAP: [[u8; 2]; 16] = [
        [0, 2],
        [2, 2],
        [1, 3],
        [3, 3],
        [3, 2],
        [1, 2],
        [2, 3],
        [0, 3],
        [3, 1],
        [1, 1],
        [2, 0],
        [0, 0],
        [0, 1],
        [2, 1],
        [1, 0],
        [3, 0],
    ];
    let mut air = Vec::with_capacity(98);
    for pair in burst[..98]
        .as_chunks::<2>()
        .0
        .iter()
        .chain(burst[166..].as_chunks::<2>().0.iter())
    {
        air.push(u8::from(pair[0]) << 1 | u8::from(pair[1]));
    }
    if air.len() != 98 {
        return None;
    }
    let mut encoded = [0u8; 98];
    for (i, value) in encoded.iter_mut().enumerate() {
        let source = if i < 50 {
            (i % 26) * 8 + i / 26
        } else {
            let j = i - 50;
            (j % 24) * 8 + 4 + j / 24
        };
        *value = air[97 - source];
    }
    let mut metric = [u16::MAX; 8];
    metric[0] = 0;
    let mut history = [[(0u8, 0u8); 8]; 49];
    for step in 0..49 {
        let observed = [encoded[step * 2], encoded[step * 2 + 1]];
        let mut next_metric = [u16::MAX; 8];
        for (state, &cost) in metric.iter().enumerate() {
            if cost == u16::MAX {
                continue;
            }
            for input in 0..8 {
                let point = NEXT[state][input];
                let distance = u16::from(MAP[usize::from(point)][0] != observed[0])
                    + u16::from(MAP[usize::from(point)][1] != observed[1]);
                let candidate = cost + distance;
                if candidate < next_metric[input] {
                    next_metric[input] = candidate;
                    history[step][input] = (state as u8, input as u8);
                }
            }
        }
        metric = next_metric;
    }
    let mut state = 0usize;
    let mut tribits = [0u8; 49];
    for step in (0..49).rev() {
        let (previous, input) = history[step][state];
        tribits[step] = input;
        state = usize::from(previous);
    }
    let mut out = [false; 144];
    for (chunk, &tribit) in out.as_chunks_mut::<3>().0.iter_mut().zip(&tribits[..48]) {
        chunk[0] = tribit & 4 != 0;
        chunk[1] = tribit & 2 != 0;
        chunk[2] = tribit & 1 != 0;
    }
    Some(out)
}

fn decode_rate_one(coded: &[bool; 196]) -> [bool; 192] {
    let mut out = [false; 192];
    out[..96].copy_from_slice(&coded[..96]);
    out[96..].copy_from_slice(&coded[100..]);
    out
}

fn talker_alias_opcode(flco: u8) -> &'static str {
    match flco {
        4 => "talker alias header",
        5 => "talker alias block 1",
        6 => "talker alias block 2",
        _ => "talker alias block 3",
    }
}

#[derive(Default)]
struct TalkerAlias {
    format: u8,
    length: usize,
    bits: Vec<bool>,
    next_block: u8,
}

impl TalkerAlias {
    fn reset(&mut self) {
        self.bits.clear();
        self.length = 0;
        self.next_block = 0;
    }

    fn update(&mut self, flco: u8, lc: &[bool]) -> Option<String> {
        if flco == 4 {
            self.format = bits_to_u32(lc, 16, 2) as u8;
            self.length = bits_to_u32(lc, 18, 5) as usize;
            self.bits.clear();
            let start = if self.format == 0 { 23 } else { 24 };
            self.bits.extend_from_slice(&lc[start..72]);
            self.next_block = 5;
        } else if flco == self.next_block {
            self.bits.extend_from_slice(&lc[16..72]);
            self.next_block += 1;
        } else {
            return None;
        }
        self.decode()
    }

    fn decode(&self) -> Option<String> {
        let needed = match self.format {
            0 => self.length * 7,
            1 | 2 => self.length * 8,
            3 => self.length * 16,
            _ => return None,
        };
        if self.bits.len() < needed {
            return None;
        }
        let text = match self.format {
            0 => self.bits[..needed]
                .as_chunks::<7>()
                .0
                .iter()
                .map(|bits| bits_to_u32(bits, 0, 7) as u8 as char)
                .collect(),
            1 => self.bits[..needed]
                .as_chunks::<8>()
                .0
                .iter()
                .map(|bits| bits_to_u32(bits, 0, 8) as u8 as char)
                .collect(),
            2 => String::from_utf8(
                self.bits[..needed]
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|bits| bits_to_u32(bits, 0, 8) as u8)
                    .collect(),
            )
            .ok()?,
            3 => String::from_utf16(
                &self.bits[..needed]
                    .as_chunks::<16>()
                    .0
                    .iter()
                    .map(|bits| bits_to_u32(bits, 0, 16) as u16)
                    .collect::<Vec<_>>(),
            )
            .ok()?,
            _ => return None,
        };
        Some(text.trim_end_matches('\0').to_owned())
    }
}

fn decode_gps_info(frame: &mut DvFrame, lc: &[bool]) {
    let error = bits_to_u32(lc, 20, 3);
    let lon = signed_bits(bits_to_u32(lc, 23, 25), 25);
    let lat = signed_bits(bits_to_u32(lc, 48, 24), 24);
    frame.lon = Some(f64::from(lon) * 360.0 / 2f64.powi(25));
    frame.lat = Some(f64::from(lat) * 180.0 / 2f64.powi(24));
    frame.position_error_m = match error {
        0 => Some(2),
        1 => Some(20),
        2 => Some(200),
        3 => Some(2_000),
        4 => Some(20_000),
        5 => Some(200_000),
        _ => None,
    };
}

fn signed_bits(value: u32, width: u32) -> i32 {
    let shift = 32 - width;
    ((value << shift) as i32) >> shift
}

fn dmr_crc16(data: &[u8]) -> u16 {
    !crc16_msb(0x1021, 0, data)
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::DmrSlots;

    use super::*;
    use crate::{
        AUDIO_RATE,
        dv::{testutil::decode, vocoder::testutil::half_rate_frames},
        testgen::dv::dmr as tx,
        testutil::settings,
    };

    fn channel(slots: DmrSlots) -> DmrChannel {
        DmrChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(ChannelParams::Dmr(DmrParams {
                slots,
                ignore_crc: false,
            })),
        )
        .expect("dmr channel")
    }

    fn put(bits: &mut [bool], at: usize, width: usize, value: u64) {
        for index in 0..width {
            bits[at + index] = value >> (width - index - 1) & 1 == 1;
        }
    }

    #[test]
    fn decodes_an_absolute_channel_definition_off_the_air() {
        let iq = tx::channel_definition(3, 811, 451_125_000, 456_250_000, INPUT_RATE_HZ);
        let frames = decode(&mut channel(DmrSlots::Both), &iq);

        let defined = frames
            .iter()
            .find(|frame| frame.channel_definition.is_some())
            .expect("an absolute channel definition");
        assert_eq!(
            defined.channel_definition,
            Some(DvChannelDefinition {
                channel: 811,
                tx_hz: 451_125_000,
                rx_hz: 456_250_000,
                color_code: None,
            })
        );
        assert_eq!(defined.channel, Some(811));
        assert_eq!(defined.color_code, Some(3));
        assert_eq!(defined.crc_verified, Some(true));
        assert_eq!(defined.trunk_protocol, Some(DvTrunkProtocol::TierThree));
    }

    #[test]
    fn an_interrupted_multi_block_is_not_read_as_a_definition() {
        let iq =
            tx::interrupted_channel_definition(3, 811, 451_125_000, 456_250_000, INPUT_RATE_HZ);
        let frames = decode(&mut channel(DmrSlots::Both), &iq);
        assert!(
            frames
                .iter()
                .all(|frame| frame.channel_definition.is_none()),
            "a stale header paired with a later continuation"
        );
    }

    #[test]
    fn a_tier_three_csbk_names_its_protocol() {
        let iq = tx::csbk(3, 0b110001, 505, 2_621_001, INPUT_RATE_HZ);
        let frames = decode(&mut channel(DmrSlots::Both), &iq);
        let grant = frames
            .iter()
            .find(|frame| frame.trunk_protocol.is_some())
            .expect("a tagged grant");
        assert_eq!(grant.trunk_protocol, Some(DvTrunkProtocol::TierThree));
        assert_eq!(grant.crc_verified, Some(true));
    }

    #[test]
    fn tier_three_grant_exposes_channel_slot_and_flags() {
        let mut payload = [false; 96];
        put(&mut payload, 16, 12, 37);
        payload[28] = true;
        payload[29] = true;
        payload[30] = true;
        put(&mut payload, 32, 24, 9_001);
        put(&mut payload, 56, 24, 1_234_567);
        let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
        decode_tier_three_csbk(&mut frame, 0, 0b110001, &payload);
        assert_eq!(frame.channel, Some(37));
        assert_eq!(frame.slot, Some(2));
        assert_eq!(frame.late_entry, Some(true));
        assert_eq!(frame.emergency, Some(true));
    }

    #[test]
    fn absolute_channel_definition_uses_125_hz_steps() {
        let mut payload = [false; 96];
        payload[0] = true;
        put(&mut payload, 2, 6, 0b101000);
        put(&mut payload, 22, 12, 811);
        put(&mut payload, 34, 10, 451);
        put(&mut payload, 44, 13, 1000);
        put(&mut payload, 57, 10, 456);
        put(&mut payload, 67, 13, 2000);
        let mut bytes = Vec::new();
        pack_bytes(&payload[..80], &mut bytes);
        put(&mut payload, 80, 16, u64::from(dmr_crc16(&bytes)));
        assert!(valid_mbc_crc(&payload));
        assert_eq!(
            decode_channel_definition(&payload, None),
            Some(DvChannelDefinition {
                channel: 811,
                tx_hz: 451_125_000,
                rx_hz: 456_250_000,
                color_code: None,
            })
        );
    }

    #[test]
    fn ras_mode_marks_an_unverified_block_and_refuses_a_repaired_one() {
        let payload = [false; 96];
        let mut strict = Decoder::new(DmrParams::default());
        assert!(strict.checked_block(&payload, CSBK_MASK, 0).is_none());

        let mut ras = Decoder::new(DmrParams {
            ignore_crc: true,
            ..DmrParams::default()
        });
        let frame = ras.checked_block(&payload, CSBK_MASK, 0).expect("kept");
        assert_eq!(frame.crc_verified, Some(false));
        assert!(ras.checked_block(&payload, CSBK_MASK, 1).is_none());
    }

    fn encoded_tone_sockets() -> [[bool; 216]; 6] {
        let mut sockets = [[false; 216]; 6];
        for (index, air) in half_rate_frames(18).iter().enumerate() {
            let at = index % 3 * VOCODER_FRAME_BITS;
            sockets[index / 3][at..at + VOCODER_FRAME_BITS].copy_from_slice(air);
        }
        sockets
    }

    fn decode_audio(iq: &[Complex<f32>]) -> (Vec<DvFrame>, Vec<f32>) {
        let mut chan = channel(DmrSlots::Both);
        let mut out = ChannelOutputs::default();
        let mut frames = Vec::new();
        let mut audio = Vec::new();
        let quiet = crate::testutil::complex_noise(0x1157, 0.01, 4 * INPUT_RATE_HZ as usize / 10);
        chan.process(&quiet, &mut out);
        for block in iq.chunks(997) {
            out.reset();
            chan.process(block, &mut out);
            audio.extend_from_slice(&out.audio_pcm);
            for event in out.events.drain(..) {
                let DecoderEvent::Dv(frame) = event else {
                    panic!("unexpected event")
                };
                frames.push(frame);
            }
        }
        (frames, audio)
    }

    #[test]
    fn decodes_a_call_from_header_to_terminator() {
        let call = tx::Call::default();
        let iq = tx::transmission(&call, INPUT_RATE_HZ);
        let frames = decode(&mut channel(DmrSlots::Both), &iq);

        let header = frames
            .iter()
            .find(|f| f.kind == DvFrameKind::Header)
            .expect("voice LC header");
        assert_eq!(header.mode, DvMode::Dmr);
        assert_eq!(header.slot, Some(1));
        assert_eq!(header.color_code, Some(u16::from(call.color_code)));
        assert_eq!(header.group_call, Some(true));
        assert_eq!(header.destination, Some(call.destination));
        assert_eq!(header.source, Some(call.source));

        let headers: Vec<&DvFrame> = frames
            .iter()
            .filter(|f| f.kind == DvFrameKind::Header)
            .collect();
        assert_eq!(headers.len(), 1, "voice LC header decoded more than once");
        for header in headers {
            assert!(
                header.errors_corrected <= 4,
                "header needed {} corrections: {header:?}",
                header.errors_corrected
            );
        }

        let voice = frames
            .iter()
            .find(|f| f.kind == DvFrameKind::Voice)
            .expect("late entry: no embedded link control survived the superframe");
        assert_eq!(voice.destination, Some(call.destination));
        assert_eq!(voice.source, Some(call.source));
        assert_eq!(voice.color_code, Some(u16::from(call.color_code)));

        let terminator = frames
            .iter()
            .find(|f| f.kind == DvFrameKind::Terminator)
            .expect("terminator with link control");
        assert_eq!(terminator.destination, Some(call.destination));
        assert_eq!(terminator.source, Some(call.source));
        assert_eq!(terminator.color_code, Some(u16::from(call.color_code)));
    }

    #[test]
    fn decodes_voice_to_audio() {
        let call = tx::Call::default();
        let iq = tx::transmission_with_voice(&call, &encoded_tone_sockets(), INPUT_RATE_HZ);
        let (_, audio) = decode_audio(&iq);
        assert!(
            (audio.len() as isize - (18 * 160 * 6) as isize).abs() <= 1,
            "not every vocoder frame decoded: {} samples",
            audio.len()
        );
        assert!(audio.iter().all(|sample| sample.is_finite()));
        assert!(
            audio.iter().all(|sample| sample.abs() < 1.0),
            "presentation gain drove the vocoder into full-scale clipping"
        );
        let settled = &audio[3 * 960..];
        let rms = crate::testutil::rms(settled);
        let (frequency, _) = crate::testutil::dominant_tone(settled, f64::from(AUDIO_RATE));
        assert!(rms > 0.01, "decoded tone is silent: rms {rms}");
        assert!(
            (frequency - 440.0).abs() < 40.0,
            "decoded tone shifted to {frequency} Hz"
        );
    }

    #[test]
    fn encrypted_calls_are_reported_and_muted() {
        let call = tx::Call {
            encrypted: true,
            ..tx::Call::default()
        };
        let iq = tx::transmission(&call, INPUT_RATE_HZ);
        let (frames, audio) = decode_audio(&iq);
        assert!(
            frames
                .iter()
                .filter(|frame| matches!(frame.kind, DvFrameKind::Header | DvFrameKind::Voice))
                .all(|frame| frame.encrypted == Some(true)),
            "privacy was lost in late-entry signalling: {frames:?}"
        );
        assert!(!audio.is_empty());
        assert!(audio.iter().all(|&sample| sample == 0.0));
    }

    #[test]
    fn decodes_a_recorded_call() {
        const FIXTURE: &[u8] = include_bytes!("../../../../fixtures/dmr_call_48k.sigmf-data");
        let iq: Vec<Complex<f32>> = FIXTURE
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
        let mut chan = channel(DmrSlots::Both);
        let mut filter = channel_filter();
        let mut out = ChannelOutputs::default();
        let mut filtered = Vec::new();
        let mut frames = Vec::new();
        let mut audio = Vec::new();
        for block in iq.chunks(997) {
            filter.process(block, &mut filtered);
            out.reset();
            chan.process(&filtered, &mut out);
            audio.extend_from_slice(&out.audio_pcm);
            for event in out.events.drain(..) {
                let DecoderEvent::Dv(frame) = event else {
                    panic!("unexpected event")
                };
                frames.push(frame);
            }
        }

        let calls: Vec<&DvFrame> = frames
            .iter()
            .filter(|f| f.kind == DvFrameKind::Header || f.kind == DvFrameKind::Voice)
            .collect();
        assert!(
            frames.iter().any(|f| f.kind == DvFrameKind::Header),
            "no voice LC header: {frames:?}"
        );
        assert!(
            frames
                .iter()
                .filter(|f| f.kind == DvFrameKind::Voice)
                .count()
                >= 3,
            "late entry recovered fewer than three superframes: {frames:?}"
        );
        for frame in calls {
            assert_eq!(frame.color_code, Some(1));
            assert_eq!(frame.group_call, Some(true));
            assert_eq!(frame.source, Some(12_345_678));
            assert_eq!(frame.destination, Some(12_345_678));
        }
        assert!(
            audio.len() >= 18 * 160 * 6,
            "no complete off-air audio superframe"
        );
        assert!(audio.iter().all(|sample| sample.is_finite()));
        let rms = crate::testutil::rms(&audio);
        let peak = audio
            .iter()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        assert!(
            rms > 0.001 && peak > 0.01,
            "off-air voice produced no signal: rms {rms}, peak {peak}, frames {frames:?}"
        );
    }

    #[test]
    fn decodes_a_private_call_header() {
        let call = tx::Call {
            group: false,
            destination: 2_621_002,
            ..tx::Call::default()
        };
        let iq = tx::transmission(&call, INPUT_RATE_HZ);
        let frames = decode(&mut channel(DmrSlots::Both), &iq);
        let header = frames
            .iter()
            .find(|f| f.kind == DvFrameKind::Header)
            .expect("voice LC header");
        assert_eq!(header.group_call, Some(false));
        assert_eq!(header.destination, Some(call.destination));
    }

    #[test]
    fn decodes_a_csbk() {
        let iq = tx::csbk(3, 0b111101, 505, 2_621_001, INPUT_RATE_HZ);
        let frames = decode(&mut channel(DmrSlots::Both), &iq);
        let csbk = frames
            .iter()
            .find(|f| f.kind == DvFrameKind::Control)
            .expect("csbk");
        assert_eq!(csbk.color_code, Some(3));
        assert_eq!(csbk.opcode.as_deref(), Some("preamble"));
        assert_eq!(csbk.destination, Some(505));
        assert_eq!(csbk.source, Some(2_621_001));
    }

    #[test]
    fn vendor_csbk_opcode_collision_does_not_invent_addresses() {
        let iq = tx::csbk_with_fid(3, 0x08, 0b111101, 505, 2_621_001, INPUT_RATE_HZ);
        let frames = decode(&mut channel(DmrSlots::Both), &iq);
        let csbk = frames
            .iter()
            .find(|frame| frame.kind == DvFrameKind::Control)
            .expect("vendor CSBK");
        assert_eq!(csbk.vendor, Some(Vendor::Hytera));
        assert_eq!(csbk.manufacturer_id, Some(0x08));
        assert_eq!(csbk.opcode.as_deref(), Some("Hytera CSBK, unparsed"));
        assert_eq!(csbk.destination, None);
        assert_eq!(csbk.source, None);
    }

    #[test]
    fn a_hytera_xpt_csbk_names_its_protocol() {
        let iq = tx::csbk_with_fid(3, 0x68, 0x0A, 505, 2_621_001, INPUT_RATE_HZ);
        let frames = decode(&mut channel(DmrSlots::Both), &iq);
        let csbk = frames
            .iter()
            .find(|frame| frame.kind == DvFrameKind::Control)
            .expect("Hytera XPT CSBK");
        assert_eq!(csbk.trunk_protocol, Some(DvTrunkProtocol::HyteraXpt));
        assert_eq!(csbk.crc_verified, Some(true));
    }

    #[test]
    fn vendor_dispatch_decodes_connect_plus_and_capacity_plus_fields() {
        let mut connect = [false; 96];
        write_bits(&mut connect, 16, 24, 151_015);
        write_bits(&mut connect, 40, 24, 1_216);
        write_bits(&mut connect, 64, 4, 2);
        connect[68] = true;
        write_bits(&mut connect, 72, 8, 2);
        let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
        decode_vendor_csbk(&mut frame, 0x06, 0x03, &connect);
        assert_eq!(frame.source, Some(151_015));
        assert_eq!(frame.destination, Some(1_216));
        assert_eq!(frame.channel, Some(2));
        assert_eq!(frame.group_call, Some(true));
        assert!(
            frame
                .data
                .as_deref()
                .is_some_and(|data| data.contains("TS 2"))
        );

        let mut capacity = [false; 96];
        write_bits(&mut capacity, 16, 2, 3);
        write_bits(&mut capacity, 20, 4, 7);
        let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
        decode_vendor_csbk(&mut frame, 0x10, 0x3E, &capacity);
        assert_eq!(frame.rest_channel, Some(7));
    }

    #[test]
    fn decodes_gps_info_link_control() {
        let mut decoder = Decoder::new(DmrParams::default());
        let mut lc = [false; 72];
        write_bits(&mut lc, 2, 6, 8);
        write_bits(&mut lc, 8, 8, 0);
        write_bits(&mut lc, 20, 3, 1);
        write_bits(&mut lc, 23, 25, ((12.5 / 360.0) * 2f64.powi(25)) as u32);
        write_bits(
            &mut lc,
            48,
            24,
            ((-33.75 / 180.0) * 2f64.powi(24)) as i32 as u32,
        );
        let frame = decoder.decode_lc(0, &lc);
        assert!((frame.lon.expect("longitude") - 12.5).abs() < 0.000_1);
        assert!((frame.lat.expect("latitude") + 33.75).abs() < 0.000_1);
        assert_eq!(frame.position_error_m, Some(20));
    }

    #[test]
    fn reassembles_utf8_talker_alias() {
        let mut decoder = Decoder::new(DmrParams::default());
        let alias = b"SCANNER-ALIAS";
        let mut stream = Vec::new();
        for byte in alias {
            stream.extend((0..8).rev().map(|bit| byte >> bit & 1 == 1));
        }
        stream.resize(49 + 3 * 56, false);
        let mut completed = None;
        for flco in 4u8..=7 {
            let mut lc = [false; 72];
            write_bits(&mut lc, 2, 6, u32::from(flco));
            if flco == 4 {
                write_bits(&mut lc, 16, 2, 2);
                write_bits(&mut lc, 18, 5, alias.len() as u32);
                lc[24..72].copy_from_slice(&stream[..48]);
            } else {
                let start = 48 + usize::from(flco - 5) * 56;
                lc[16..72].copy_from_slice(&stream[start..start + 56]);
            }
            completed = decoder.decode_lc(0, &lc).talker_alias.or(completed);
        }
        assert_eq!(completed.as_deref(), Some("SCANNER-ALIAS"));
    }

    fn write_bits(target: &mut [bool], offset: usize, len: usize, value: u32) {
        for (index, bit) in target[offset..offset + len].iter_mut().enumerate() {
            *bit = value >> (len - 1 - index) & 1 == 1;
        }
    }

    #[test]
    fn csbk_opcodes_are_the_specs_six_bit_binary_values() {
        for (opcode, name) in [
            (0b000100, "unit-to-unit voice service request"),
            (0b000101, "unit-to-unit voice service answer response"),
            (0b000111, "channel timing"),
            (0b100110, "negative acknowledge response"),
            (0b111000, "BS outbound activation"),
            (0b111101, "preamble"),
            (0b011001, "ALOHA"),
            (0b101111, "protect"),
            (0b110000, "private voice channel grant"),
            (0b110001, "talkgroup voice channel grant"),
        ] {
            assert_eq!(csbk_opcode_name(0, opcode), name, "opcode {opcode:06b}");
        }
    }

    #[test]
    fn the_slot_filter_selects_what_is_reported() {
        let iq = tx::transmission(&tx::Call::default(), INPUT_RATE_HZ);
        assert!(!decode(&mut channel(DmrSlots::One), &iq).is_empty());
        assert!(decode(&mut channel(DmrSlots::Two), &iq).is_empty());
    }

    #[test]
    fn repeater_cach_activates_the_slot_filter() {
        let call = tx::Call::default();
        let iq = tx::repeater_transmission(&call, 2, INPUT_RATE_HZ);
        assert!(decode(&mut channel(DmrSlots::One), &iq).is_empty());
        let frames = decode(&mut channel(DmrSlots::Two), &iq);
        assert!(!frames.is_empty());
        assert!(frames.iter().all(|frame| frame.slot == Some(2)));
    }

    #[test]
    fn concurrent_repeater_slots_keep_independent_call_state() {
        let first = tx::Call {
            destination: 101,
            source: 1_000_001,
            ..tx::Call::default()
        };
        let second = tx::Call {
            destination: 202,
            source: 2_000_002,
            ..tx::Call::default()
        };
        let iq = tx::dual_slot_transmission(&first, &second, INPUT_RATE_HZ);
        let frames = decode(&mut channel(DmrSlots::Both), &iq);
        for (slot, source, destination) in [
            (1, first.source, first.destination),
            (2, second.source, second.destination),
        ] {
            let call = frames
                .iter()
                .find(|frame| frame.slot == Some(slot) && frame.source == Some(source))
                .unwrap_or_else(|| panic!("missing slot {slot} call: {frames:?}"));
            assert_eq!(call.destination, Some(destination));
        }
    }

    #[test]
    fn concurrent_repeater_slots_yield_one_call_worth_of_audio() {
        let first = tx::Call {
            destination: 101,
            source: 1_000_001,
            ..tx::Call::default()
        };
        let second = tx::Call {
            destination: 202,
            source: 2_000_002,
            ..tx::Call::default()
        };
        let iq = tx::dual_slot_transmission(&first, &second, INPUT_RATE_HZ);
        let (frames, audio) = decode_audio(&iq);
        assert!(
            frames.iter().any(|frame| frame.slot == Some(2)),
            "the unheard slot stopped being reported"
        );
        let voice_frames = 18;
        assert!(
            (audio.len() as isize - (voice_frames * 960) as isize).abs() <= 2,
            "audio from both slots ran together: {} samples for {voice_frames} vocoder frames",
            audio.len()
        );
    }

    #[test]
    fn a_slot_that_stops_hands_the_speaker_over() {
        let first = tx::Call::default();
        let second = tx::Call {
            destination: 202,
            source: 2_000_002,
            ..tx::Call::default()
        };
        let mut iq = tx::repeater_transmission(&first, 1, INPUT_RATE_HZ);
        iq.extend(tx::repeater_transmission(&second, 2, INPUT_RATE_HZ));
        let (_, audio) = decode_audio(&iq);
        let voice_frames = 36;
        assert!(
            (audio.len() as isize - (voice_frames * 960) as isize).abs() <= 4,
            "the second call never reached the speaker: {} samples",
            audio.len()
        );
    }

    #[test]
    fn short_lc_reports_activity_on_both_slots() {
        let mut message = [false; 36];
        write_bits(&mut message, 0, 4, 1);
        write_bits(&mut message, 4, 4, 8);
        write_bits(&mut message, 8, 4, 10);
        write_bits(&mut message, 12, 8, 0xA5);
        write_bits(&mut message, 20, 8, 0x5A);
        let crc = crc8_dmr(&message[..28]);
        write_bits(&mut message, 28, 8, u32::from(crc));

        let mut matrix = [[false; 17]; 4];
        for row in 0..3 {
            matrix[row][..12].copy_from_slice(&message[row * 12..(row + 1) * 12]);
            ParityCode::HAMMING_17_12.encode(&mut matrix[row]);
        }
        for column in 0..17 {
            matrix[3][column] = matrix[..3]
                .iter()
                .fold(false, |parity, row| parity ^ row[column]);
        }
        let transmitted: Vec<bool> = (0..17)
            .flat_map(|column| (0..4).map(move |row| matrix[row][column]))
            .collect();
        let mut decoder = ShortLc::default();
        let mut frame = None;
        for (index, lcss) in [1, 3, 3, 2].into_iter().enumerate() {
            let payload: [bool; 17] = transmitted[index * 17..(index + 1) * 17]
                .try_into()
                .expect("CACH row");
            frame = decoder.push(lcss, payload).or(frame);
        }
        let frame = frame.expect("decoded Short LC");
        assert_eq!(frame.slot_activity.len(), 2);
        assert_eq!(frame.slot_activity[0].activity, "group voice");
        assert_eq!(frame.slot_activity[0].destination_hash, Some(0xA5));
        assert_eq!(frame.slot_activity[1].activity, "individual data");
        assert_eq!(frame.slot_activity[1].destination_hash, Some(0x5A));
    }

    #[test]
    fn noise_decodes_to_nothing() {
        let noise = crate::testutil::complex_noise(9, 0.5, 400_000);
        assert!(decode(&mut channel(DmrSlots::Both), &noise).is_empty());
    }

    #[test]
    fn retuning_forgets_the_call_in_progress() {
        let call = tx::Call::default();
        let iq = tx::transmission(&call, INPUT_RATE_HZ);
        let mut chan = channel(DmrSlots::Both);
        let mut out = ChannelOutputs::default();
        chan.process(&iq[..iq.len() / 2], &mut out);
        chan.retuned();
        out.reset();
        chan.process(&iq[iq.len() / 2..], &mut out);
        let frames: Vec<&DvFrame> = out
            .events
            .iter()
            .map(|event| {
                let DecoderEvent::Dv(frame) = event else {
                    panic!("unexpected event")
                };
                frame
            })
            .collect();
        assert!(
            frames.iter().all(|f| f.kind != DvFrameKind::Voice),
            "an embedded link control was assembled across the retune: {frames:?}"
        );
    }
}
