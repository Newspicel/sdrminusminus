use super::{Preamble, crc};
use crate::datv::dvbt::t2::{Coding, Constellation, DecodeError, Frame, Rate};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pre {
    pub preamble: Preamble,
    pub extended: bool,
    pub repetition: bool,
    pub guard_code: u8,
    pub papr: u8,
    pub modulation: u8,
    pub post_cells: usize,
    pub post_info: usize,
    pub pilots: u8,
    pub frames: usize,
    pub data_symbols: usize,
    pub rf_count: usize,
    pub rf_index: usize,
    pub version: u8,
    pub scrambled: bool,
    pub extension: bool,
}

impl Pre {
    pub fn parse(bits: &[bool], preamble: Preamble) -> Result<Self, DecodeError> {
        if bits.len() != 200 || crc(bits) != 0 {
            return Err(DecodeError::Signalling);
        }
        let mut r = Reader::new(bits);
        r.take(8)?;
        let extended = r.flag()?;
        if r.take(3)? != usize::from(preamble.s1) || r.take(4)? != usize::from(preamble.s2) {
            return Err(DecodeError::Signalling);
        }
        let repetition = r.flag()?;
        let guard_code = r.take(3)? as u8;
        let papr = r.take(4)? as u8;
        let modulation = r.take(4)? as u8;
        if r.take(2)? != 0 || r.take(2)? != 0 {
            return Err(DecodeError::Parameters);
        }
        let post_cells = r.take(18)?;
        let post_info = r.take(18)?;
        let pilots = r.take(4)? as u8 + 1;
        r.skip(56)?;
        let frames = r.take(8)?;
        let data_symbols = r.take(12)?;
        r.take(3)?;
        let extension = r.flag()?;
        let rf_count = r.take(3)?;
        let rf_index = r.take(3)?;
        let version = r.take(4)? as u8;
        let scrambled = r.flag()? && version >= 2;
        let pre = Self {
            preamble,
            extended,
            repetition,
            guard_code,
            papr,
            modulation,
            post_cells,
            post_info,
            pilots,
            frames,
            data_symbols,
            rf_count,
            rf_index,
            version,
            scrambled,
            extension,
        };
        if frames == 0
            || data_symbols == 0
            || rf_count == 0
            || rf_index >= rf_count
            || pilots > 8
            || papr > 3
            || modulation > 3
            || post_info == 0
        {
            return Err(DecodeError::Parameters);
        }
        pre.guard()?;
        pre.post_shape()?;
        Ok(pre)
    }

    pub const fn bits(self) -> usize {
        match self.modulation {
            0 => 1,
            1 => 2,
            2 => 4,
            _ => 6,
        }
    }

    pub fn guard(self) -> Result<usize, DecodeError> {
        let fft = self.preamble.fft()?;
        let (n, d) = match self.guard_code {
            0 => (1, 32),
            1 => (1, 16),
            2 => (1, 8),
            3 => (1, 4),
            4 => (1, 128),
            5 => (19, 128),
            6 => (19, 256),
            _ => return Err(DecodeError::Parameters),
        };
        Ok(fft * n / d)
    }

    pub fn post_shape(self) -> Result<(usize, usize, usize), DecodeError> {
        if self.modulation > 3 || self.post_info == 0 || self.post_info > 262143 {
            return Err(DecodeError::Parameters);
        }
        let blocks = (self.post_info + 32).div_ceil(7032);
        if self.preamble.lite() && blocks > 1 {
            return Err(DecodeError::Parameters);
        }
        let information = (self.post_info + 32).div_ceil(blocks);
        let multiple = self.preamble.p2_symbols()?.max(2) * self.bits();
        let punctured = (multiple - 1).max((7032 - information) * 6 / 5);
        let transmitted = (information + 9168 - punctured).div_ceil(multiple) * multiple;
        if transmitted * blocks / self.bits() != self.post_cells {
            return Err(DecodeError::Signalling);
        }
        Ok((blocks, information, transmitted))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plp {
    pub id: u8,
    pub kind: u8,
    pub payload: u8,
    pub first_frame: usize,
    pub group: u8,
    pub coding: Coding,
    pub max_blocks: usize,
    pub frame_interval: usize,
    pub time_length: usize,
    pub time_across_frames: bool,
    pub inband_a: bool,
    pub inband_b: bool,
    pub mode: u8,
    pub start: usize,
    pub blocks: usize,
}

impl Plp {
    pub const fn interleaving_frames(self) -> usize {
        if self.time_across_frames {
            self.time_length
        } else {
            1
        }
    }

    fn parse(r: &mut Reader<'_>, lite: bool) -> Result<Self, DecodeError> {
        let id = r.take(8)? as u8;
        let kind = r.take(3)? as u8;
        let payload = r.take(5)? as u8;
        r.skip(4)?;
        let first_frame = r.take(8)?;
        let group = r.take(8)? as u8;
        let rate = match r.take(3)? {
            0 => Rate::R1_2,
            1 => Rate::R3_5,
            2 => Rate::R2_3,
            3 => Rate::R3_4,
            4 => Rate::R4_5,
            5 => Rate::R5_6,
            6 => Rate::R1_3,
            _ => Rate::R2_5,
        };
        let constellation = match r.take(3)? {
            0 => Constellation::Qpsk,
            1 => Constellation::Qam16,
            2 => Constellation::Qam64,
            3 => Constellation::Qam256,
            _ => return Err(DecodeError::Parameters),
        };
        let rotated = r.flag()?;
        let frame = match r.take(2)? {
            0 => Frame::Short,
            1 => Frame::Normal,
            _ => return Err(DecodeError::Parameters),
        };
        let coding = Coding {
            frame,
            rate,
            constellation,
            rotated,
            lite,
        }
        .validate()?;
        let max_blocks = r.take(10)?;
        let frame_interval = r.take(8)?;
        let time_length = r.take(8)?;
        let time_across_frames = r.flag()?;
        let inband_a = r.flag()?;
        let inband_b = r.flag()?;
        r.skip(11)?;
        let mode = r.take(2)? as u8;
        r.skip(2)?;
        if kind > 2
            || frame_interval == 0
            || first_frame >= frame_interval
            || (time_across_frames && time_length < 2)
        {
            return Err(DecodeError::Parameters);
        }
        Ok(Self {
            id,
            kind,
            payload,
            first_frame,
            group,
            coding,
            max_blocks,
            frame_interval,
            time_length,
            time_across_frames,
            inband_a,
            inband_b,
            mode,
            start: 0,
            blocks: 0,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Post {
    pub subslices: usize,
    pub frame: usize,
    pub subslice_interval: usize,
    pub type2_start: usize,
    pub fef_length: usize,
    pub fef_interval: usize,
    pub plps: [Option<Plp>; 256],
    pub count: usize,
}

impl Post {
    pub fn parse(bits: &[bool], pre: Pre) -> Result<Self, DecodeError> {
        if bits.len() != pre.post_info + 32 || crc(bits) != 0 {
            return Err(DecodeError::Signalling);
        }
        let mut r = Reader::new(&bits[..pre.post_info]);
        let subslices = r.take(15)?;
        let count = r.take(8)?;
        let aux = r.take(4)?;
        r.skip(8 + 35 * pre.rf_count)?;
        let (mut fef_length, fef_interval) = if pre.preamble.s2 & 1 != 0 {
            r.take(4)?;
            (r.take(22)?, r.take(8)?)
        } else {
            (0, 0)
        };
        let mut plps = [None; 256];
        let mut seen = [false; 256];
        for slot in &mut plps[..count] {
            let plp = Plp::parse(&mut r, pre.preamble.lite())?;
            if seen[usize::from(plp.id)] {
                return Err(DecodeError::Signalling);
            }
            seen[usize::from(plp.id)] = true;
            *slot = Some(plp);
        }
        let fef_msb = r.take(2)?;
        if pre.preamble.lite() {
            fef_length |= fef_msb << 22;
        }
        r.skip(30 + aux * 32)?;
        let frame = r.take(8)?;
        let subslice_interval = r.take(22)?;
        let type2_start = r.take(22)?;
        r.skip(19)?;
        for slot in &mut plps[..count] {
            let plp = slot.as_mut().ok_or(DecodeError::Signalling)?;
            if r.take(8)? != usize::from(plp.id) {
                return Err(DecodeError::Signalling);
            }
            plp.start = r.take(22)?;
            plp.blocks = r.take(10)?;
            r.skip(8)?;
            if plp.blocks > plp.max_blocks {
                return Err(DecodeError::Signalling);
            }
        }
        r.skip(8 + aux * 48)?;
        if count == 0 || subslices == 0 || frame >= pre.frames {
            return Err(DecodeError::Signalling);
        }
        Ok(Self {
            subslices,
            frame,
            subslice_interval,
            type2_start,
            fef_length,
            fef_interval,
            plps,
            count,
        })
    }

    pub fn select(&self, id: Option<u8>) -> Option<Plp> {
        self.plps[..self.count]
            .iter()
            .flatten()
            .find(|p| p.kind != 0 && p.payload == 3 && id.is_none_or(|id| p.id == id))
            .copied()
    }
}

struct Reader<'a> {
    bits: &'a [bool],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bits: &'a [bool]) -> Self {
        Self { bits, at: 0 }
    }
    fn take(&mut self, length: usize) -> Result<usize, DecodeError> {
        if length > usize::BITS as usize {
            return Err(DecodeError::Length);
        }
        let bits = self
            .bits
            .get(self.at..self.at + length)
            .ok_or(DecodeError::Length)?;
        self.at += length;
        Ok(bits.iter().fold(0, |v, &bit| (v << 1) | usize::from(bit)))
    }
    fn flag(&mut self) -> Result<bool, DecodeError> {
        Ok(self.take(1)? != 0)
    }
    fn skip(&mut self, length: usize) -> Result<(), DecodeError> {
        if self.at + length > self.bits.len() {
            return Err(DecodeError::Length);
        }
        self.at += length;
        Ok(())
    }
}
