use sdrmm_dsp::{DVB_PRIMITIVE, ReedSolomon, crc16_msb};

pub const FRAMES: usize = 5;
pub const CODEWORD: usize = 120;
pub const DATA: usize = 110;
const FIRE_POLY: u16 = 0x782F;
const HEADER_SPAN: usize = 9;

#[must_use]
pub fn firecode(header: &[u8]) -> u16 {
    crc16_msb(FIRE_POLY, 0, header)
}

#[must_use]
pub fn au_crc_ok(au: &[u8]) -> bool {
    au.len() > 2
        && !crc16_msb(0x1021, 0xFFFF, &au[..au.len() - 2])
            == u16::from_be_bytes([au[au.len() - 2], au[au.len() - 1]])
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioFormat {
    pub sample_rate_hz: u32,
    pub spectral_band_replication: bool,
    pub parametric_stereo: bool,
    pub stereo_core: bool,
    pub surround: u8,
}

impl AudioFormat {
    #[must_use]
    pub const fn from_header(byte: u8) -> Self {
        Self {
            sample_rate_hz: if byte & 0x40 != 0 { 48_000 } else { 32_000 },
            spectral_band_replication: byte & 0x20 != 0,
            stereo_core: byte & 0x10 != 0,
            parametric_stereo: byte & 0x08 != 0,
            surround: byte & 0x07,
        }
    }

    #[must_use]
    pub const fn channels(self) -> u8 {
        if self.stereo_core || self.parametric_stereo {
            2
        } else {
            1
        }
    }

    #[must_use]
    pub const fn output_rate_hz(self) -> u32 {
        if self.spectral_band_replication {
            self.sample_rate_hz * 2
        } else {
            self.sample_rate_hz
        }
    }

    #[must_use]
    pub const fn codec(self) -> &'static str {
        match (self.spectral_band_replication, self.parametric_stereo) {
            (true, true) => "HE-AAC v2",
            (true, false) => "HE-AAC",
            _ => "AAC-LC",
        }
    }

    #[must_use]
    pub const fn access_units(self) -> usize {
        match (self.sample_rate_hz == 48_000, self.spectral_band_replication) {
            (true, true) => 3,
            (true, false) => 6,
            (false, true) => 2,
            (false, false) => 4,
        }
    }

    const fn first_unit(self) -> usize {
        match (self.sample_rate_hz == 48_000, self.spectral_band_replication) {
            (true, true) => 6,
            (true, false) => 11,
            (false, true) => 5,
            (false, false) => 8,
        }
    }
}

fn unit_offsets(superframe: &[u8], format: AudioFormat) -> Option<Vec<usize>> {
    let units = format.access_units();
    let mut starts = Vec::with_capacity(units + 1);
    starts.push(format.first_unit());
    if units >= 2 {
        starts.push(usize::from(superframe[3]) << 4 | usize::from(superframe[4]) >> 4);
    }
    if units >= 3 {
        starts.push(usize::from(superframe[4] & 0x0F) << 8 | usize::from(superframe[5]));
    }
    if units >= 4 {
        starts.push(usize::from(superframe[6]) << 4 | usize::from(superframe[7]) >> 4);
    }
    if units == 6 {
        starts.push(usize::from(superframe[7] & 0x0F) << 8 | usize::from(superframe[8]));
        starts.push(usize::from(superframe[9]) << 4 | usize::from(superframe[10]) >> 4);
    }
    starts.push(superframe.len() / CODEWORD * DATA);
    starts
        .windows(2)
        .all(|pair| pair[0] < pair[1])
        .then_some(starts)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccessUnits {
    pub format: AudioFormat,
    pub units: Vec<Vec<u8>>,
    pub corrected: u32,
    pub dropped: u32,
}

pub struct SuperframeAssembler {
    reed_solomon: ReedSolomon,
    frame_bytes: usize,
    raw: Vec<u8>,
    frames: usize,
    synced: bool,
    packet: Vec<u8>,
}

impl SuperframeAssembler {
    #[must_use]
    pub fn new(frame_bytes: usize) -> Option<Self> {
        (frame_bytes > 0 && (FRAMES * frame_bytes).is_multiple_of(CODEWORD)).then(|| Self {
            reed_solomon: ReedSolomon::new(DVB_PRIMITIVE, 0, CODEWORD - DATA),
            frame_bytes,
            raw: Vec::with_capacity(FRAMES * frame_bytes),
            frames: 0,
            synced: false,
            packet: vec![0; CODEWORD],
        })
    }

    #[must_use]
    pub const fn bitrate_kbps(&self) -> u32 {
        (FRAMES * self.frame_bytes / CODEWORD * 8) as u32
    }

    pub fn reset(&mut self) {
        self.raw.clear();
        self.frames = 0;
        self.synced = false;
    }

    pub fn frame(&mut self, logical: &[u8]) -> Option<AccessUnits> {
        if logical.len() != self.frame_bytes {
            return None;
        }
        if !self.synced {
            if firecode(logical.get(2..2 + HEADER_SPAN)?)
                != u16::from_be_bytes([logical[0], logical[1]])
            {
                return None;
            }
            self.synced = true;
            self.raw.clear();
            self.frames = 0;
        }
        self.raw.extend_from_slice(logical);
        self.frames += 1;
        if self.frames < FRAMES {
            return None;
        }
        let result = self.finish();
        self.raw.clear();
        self.frames = 0;
        if result.is_none() {
            self.synced = false;
        }
        result
    }

    fn finish(&mut self) -> Option<AccessUnits> {
        let mut superframe = std::mem::take(&mut self.raw);
        let corrected = self.repair(&mut superframe);
        let ok = firecode(superframe.get(2..2 + HEADER_SPAN)?)
            == u16::from_be_bytes([superframe[0], superframe[1]]);
        let result = ok.then(|| self.split(&superframe, corrected)).flatten();
        superframe.clear();
        self.raw = superframe;
        result
    }

    fn repair(&mut self, superframe: &mut [u8]) -> u32 {
        let codewords = superframe.len() / CODEWORD;
        let mut corrected = 0u32;
        for lane in 0..codewords {
            for position in 0..CODEWORD {
                self.packet[position] = superframe[position * codewords + lane];
            }
            match self.reed_solomon.decode(&mut self.packet) {
                Some(count) => {
                    corrected += count;
                    for position in 0..CODEWORD {
                        superframe[position * codewords + lane] = self.packet[position];
                    }
                }
                None => corrected += u32::from(u8::MAX),
            }
        }
        corrected
    }

    fn split(&self, superframe: &[u8], corrected: u32) -> Option<AccessUnits> {
        let format = AudioFormat::from_header(*superframe.get(2)?);
        let starts = unit_offsets(superframe, format)?;
        let mut units = Vec::with_capacity(format.access_units());
        let mut dropped = 0u32;
        for pair in starts.windows(2) {
            let Some(unit) = superframe.get(pair[0]..pair[1]) else {
                dropped += 1;
                continue;
            };
            if au_crc_ok(unit) {
                units.push(unit[..unit.len() - 2].to_vec());
            } else {
                dropped += 1;
            }
        }
        Some(AccessUnits {
            format,
            units,
            corrected,
            dropped,
        })
    }
}

pub struct SuperframeBuilder {
    reed_solomon: ReedSolomon,
    frame_bytes: usize,
}

impl SuperframeBuilder {
    #[must_use]
    pub fn new(frame_bytes: usize) -> Option<Self> {
        (frame_bytes > 0 && (FRAMES * frame_bytes).is_multiple_of(CODEWORD)).then(|| Self {
            reed_solomon: ReedSolomon::new(DVB_PRIMITIVE, 0, CODEWORD - DATA),
            frame_bytes,
        })
    }

    #[must_use]
    pub fn build(&self, format: AudioFormat, payloads: &[Vec<u8>]) -> Option<Vec<Vec<u8>>> {
        let total = FRAMES * self.frame_bytes;
        let codewords = total / CODEWORD;
        let mut data = vec![0u8; codewords * DATA];
        let mut starts = vec![format.first_unit()];
        let mut at = format.first_unit();
        let (last, leading) = payloads.split_last()?;
        for payload in leading {
            let end = at + payload.len() + 2;
            if end > data.len() {
                return None;
            }
            write_unit(&mut data[at..end], payload);
            at = end;
            starts.push(at);
        }
        if at + last.len() + 2 > data.len() {
            return None;
        }
        let mut padded = last.clone();
        padded.resize(data.len() - at - 2, 0);
        write_unit(&mut data[at..], &padded);
        data[2] = header_byte(format);
        write_offsets(&mut data, format, &starts);
        let head = firecode(&data[2..2 + HEADER_SPAN]);
        data[..2].copy_from_slice(&head.to_be_bytes());
        let mut superframe = vec![0u8; total];
        for lane in 0..codewords {
            let mut message = Vec::with_capacity(CODEWORD);
            for position in 0..DATA {
                message.push(data[position * codewords + lane]);
            }
            let mut codeword = Vec::with_capacity(CODEWORD);
            self.reed_solomon.encode(&message, &mut codeword);
            for position in 0..CODEWORD {
                superframe[position * codewords + lane] = codeword[position];
            }
        }
        Some(
            superframe
                .chunks_exact(self.frame_bytes)
                .map(<[u8]>::to_vec)
                .collect(),
        )
    }
}

fn write_unit(slot: &mut [u8], payload: &[u8]) {
    let end = slot.len();
    slot[..payload.len()].copy_from_slice(payload);
    let crc = !crc16_msb(0x1021, 0xFFFF, &slot[..end - 2]);
    slot[end - 2..].copy_from_slice(&crc.to_be_bytes());
}

const fn header_byte(format: AudioFormat) -> u8 {
    let mut byte = format.surround;
    if format.sample_rate_hz == 48_000 {
        byte |= 0x40;
    }
    if format.spectral_band_replication {
        byte |= 0x20;
    }
    if format.stereo_core {
        byte |= 0x10;
    }
    if format.parametric_stereo {
        byte |= 0x08;
    }
    byte
}

fn write_offsets(data: &mut [u8], format: AudioFormat, starts: &[usize]) {
    let units = format.access_units();
    if units >= 2 && let Some(&start) = starts.get(1) {
        data[3] = (start >> 4) as u8;
        data[4] = ((start & 0x0F) << 4) as u8;
    }
    if units >= 3 && let Some(&start) = starts.get(2) {
        data[4] |= (start >> 8) as u8 & 0x0F;
        data[5] = start as u8;
    }
    if units >= 4 && let Some(&start) = starts.get(3) {
        data[6] = (start >> 4) as u8;
        data[7] = ((start & 0x0F) << 4) as u8;
    }
    if units == 6 {
        if let Some(&start) = starts.get(4) {
            data[7] |= (start >> 8) as u8 & 0x0F;
            data[8] = start as u8;
        }
        if let Some(&start) = starts.get(5) {
            data[9] = (start >> 4) as u8;
            data[10] = ((start & 0x0F) << 4) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format() -> AudioFormat {
        AudioFormat {
            sample_rate_hz: 48_000,
            spectral_band_replication: true,
            parametric_stereo: false,
            stereo_core: true,
            surround: 0,
        }
    }

    fn payloads(count: usize, len: usize) -> Vec<Vec<u8>> {
        let mut state = 0x1234_9876u32;
        (0..count)
            .map(|_| {
                (0..len)
                    .map(|_| {
                        state ^= state << 13;
                        state ^= state >> 17;
                        state ^= state << 5;
                        state as u8
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn the_header_round_trips_through_its_flag_byte() {
        for byte in 0..=u8::MAX {
            let format = AudioFormat::from_header(byte);
            assert_eq!(header_byte(format), byte & 0x7F);
        }
        assert_eq!(format().codec(), "HE-AAC");
        assert_eq!(format().output_rate_hz(), 96_000);
        assert_eq!(format().access_units(), 3);
    }

    #[test]
    fn a_superframe_round_trips_its_access_units() {
        let frame_bytes = 24 * 96 / 8;
        let builder = SuperframeBuilder::new(frame_bytes).expect("96 kbps builds superframes");
        let sent = payloads(3, 200);
        let frames = builder.build(format(), &sent).expect("the units fit");
        assert_eq!(frames.len(), FRAMES);
        let mut assembler = SuperframeAssembler::new(frame_bytes).expect("96 kbps assembles");
        assert_eq!(assembler.bitrate_kbps(), 96);
        let mut decoded = None;
        for frame in &frames {
            decoded = assembler.frame(frame).or(decoded);
        }
        let units = decoded.expect("a superframe");
        assert_eq!(units.format, format());
        assert_eq!(units.units.len(), sent.len());
        assert_eq!(units.units[..2], sent[..2]);
        assert!(units.units[2].starts_with(&sent[2]));
        assert_eq!(units.corrected, 0);
        assert_eq!(units.dropped, 0);
    }

    #[test]
    fn the_reed_solomon_lanes_repair_a_burst_of_damage() {
        let frame_bytes = 24 * 96 / 8;
        let builder = SuperframeBuilder::new(frame_bytes).expect("96 kbps builds superframes");
        let sent = payloads(3, 200);
        let mut frames = builder.build(format(), &sent).expect("the units fit");
        for byte in &mut frames[2][40..80] {
            *byte ^= 0xA5;
        }
        let mut assembler = SuperframeAssembler::new(frame_bytes).expect("96 kbps assembles");
        let mut decoded = None;
        for frame in &frames {
            decoded = assembler.frame(frame).or(decoded);
        }
        let units = decoded.expect("a repaired superframe");
        assert_eq!(units.units[..2], sent[..2]);
        assert!(units.units[2].starts_with(&sent[2]));
        assert!(units.corrected > 0);
    }

    #[test]
    fn a_frame_that_is_not_a_superframe_head_does_not_start_one() {
        let frame_bytes = 24 * 96 / 8;
        let mut assembler = SuperframeAssembler::new(frame_bytes).expect("96 kbps assembles");
        assert!(assembler.frame(&vec![0u8; frame_bytes]).is_none());
        assert!(assembler.frame(&vec![0x5Au8; frame_bytes]).is_none());
    }

    #[test]
    fn a_bit_rate_that_is_not_a_whole_number_of_codewords_is_refused() {
        assert!(SuperframeAssembler::new(0).is_none());
        assert!(SuperframeAssembler::new(23).is_none());
        assert!(SuperframeAssembler::new(24).is_some());
    }

    #[test]
    fn an_access_unit_beyond_the_reed_solomon_reach_is_dropped_rather_than_played() {
        let frame_bytes = 24 * 96 / 8;
        let builder = SuperframeBuilder::new(frame_bytes).expect("96 kbps builds superframes");
        let frames = builder
            .build(format(), &payloads(3, 200))
            .expect("the units fit");
        let mut superframe: Vec<u8> = frames.concat();
        let lanes = superframe.len() / CODEWORD;
        for position in 20..30 {
            superframe[position * lanes + 3] ^= 0x5A;
        }
        let mut assembler = SuperframeAssembler::new(frame_bytes).expect("96 kbps assembles");
        let mut decoded = None;
        for frame in superframe.chunks_exact(frame_bytes) {
            decoded = assembler.frame(frame).or(decoded);
        }
        let units = decoded.expect("a superframe with a damaged lane");
        assert!(units.dropped > 0, "{units:?}");
        assert!(units.units.len() < 3);
    }
}
