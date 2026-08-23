use sdrmm_wire::{DectArc, DectIdentity};

pub(crate) const RFPI_BITS: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rfpi {
    pub bits: u64,
    pub sari_available: bool,
    pub arc: DectArc,
    pub ari_bits: u32,
    pub ari: u64,
    pub rpn: u16,
}

fn field(bits: u64, offset: usize, width: usize) -> u64 {
    (bits >> (RFPI_BITS - offset - width)) & ((1u64 << width) - 1)
}

const fn arc_of(code: u64) -> DectArc {
    match code {
        0 => DectArc::A,
        1 => DectArc::B,
        2 => DectArc::C,
        3 => DectArc::D,
        4 => DectArc::E,
        5 => DectArc::F,
        6 => DectArc::G,
        _ => DectArc::H,
    }
}

const fn ari_bits(arc: DectArc) -> u32 {
    match arc {
        DectArc::A | DectArc::E => 36,
        _ => 31,
    }
}

impl Rfpi {
    pub fn parse(bits: u64) -> Self {
        let bits = bits & ((1u64 << RFPI_BITS) - 1);
        let arc = arc_of(field(bits, 1, 3));
        let width = ari_bits(arc);
        let rpn_bits = RFPI_BITS - 1 - width as usize;
        Self {
            bits,
            sari_available: field(bits, 0, 1) == 1,
            arc,
            ari_bits: width,
            ari: field(bits, 1, width as usize),
            rpn: field(bits, 1 + width as usize, rpn_bits) as u16,
        }
    }

    pub fn hex(self) -> String {
        format!("{:010X}", self.bits)
    }

    fn pari_hex(self) -> String {
        let digits = self.ari_bits.div_ceil(4) as usize;
        format!("{:0width$X}", self.ari, width = digits)
    }

    fn multicell(self) -> Option<bool> {
        match self.arc {
            DectArc::A => Some(self.rpn != 0),
            DectArc::C | DectArc::D => Some(self.rpn & 1 == 1),
            _ => None,
        }
    }

    pub fn describe(self) -> DectIdentity {
        let mut out = DectIdentity {
            rfpi: self.hex(),
            pari: self.pari_hex(),
            arc: self.arc,
            sari_available: self.sari_available,
            rpn: self.rpn,
            multicell: self.multicell(),
            ..DectIdentity::default()
        };
        match self.arc {
            DectArc::A => {
                out.emc = Some(field(self.bits, 4, 16) as u16);
                out.fpn = Some(field(self.bits, 20, 17) as u32);
            }
            DectArc::B => {
                out.eic = Some(field(self.bits, 4, 16) as u16);
                out.fpn = Some(field(self.bits, 20, 8) as u32);
                out.fps = Some(field(self.bits, 28, 4) as u8);
            }
            DectArc::C => {
                out.poc = Some(field(self.bits, 4, 16) as u16);
                out.fpn = Some(field(self.bits, 20, 8) as u32);
                out.fps = Some(field(self.bits, 28, 4) as u8);
            }
            DectArc::D => {
                let gop = field(self.bits, 4, 20) as u32;
                out.gop = Some(gop);
                out.mcc = Some((gop >> 8) as u16);
                out.mnc = Some((gop & 0xFF) as u16);
                out.fpn = Some(field(self.bits, 24, 8) as u32);
            }
            DectArc::E => {
                out.fil = Some(field(self.bits, 4, 16) as u16);
                out.fpn = Some(field(self.bits, 20, 17) as u32);
            }
            DectArc::F | DectArc::G | DectArc::H => {}
        }
        out
    }
}
