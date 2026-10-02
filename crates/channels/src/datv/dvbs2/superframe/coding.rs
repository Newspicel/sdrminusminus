use super::super::{
    frame::{ModCod, Modulation},
    ldpc::{Frame, Rate},
    pl, s2x,
    vlsnr::Carrier,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coding {
    Legacy {
        modcod: u8,
        short: bool,
    },
    Extended {
        code: u8,
    },
    Robust {
        carrier: Carrier,
        rate: Rate,
        frame: Frame,
    },
}

impl Coding {
    #[must_use]
    pub fn frame(self) -> Frame {
        match self {
            Self::Legacy { short, .. } => Frame::of(short),
            Self::Extended { code } => Frame::of(s2x::mode(code).is_some_and(|mode| mode.short)),
            Self::Robust { frame, .. } => frame,
        }
    }

    #[must_use]
    pub fn modcod(self) -> Option<ModCod> {
        match self {
            Self::Legacy { modcod, .. } => ModCod::from_index(modcod),
            Self::Extended { code } => s2x::mode(code).map(s2x::Mode::modcod),
            Self::Robust { .. } => None,
        }
    }

    #[must_use]
    pub fn symbols(self) -> usize {
        let length = self.frame().length();
        match self {
            Self::Robust { carrier, .. } => carrier.symbols(length),
            _ => self
                .modcod()
                .map_or(0, |modcod| length.div_ceil(modcod.modulation.bits())),
        }
    }

    #[must_use]
    pub fn slots(self) -> usize {
        self.symbols().div_ceil(pl::SLOT)
    }

    #[must_use]
    pub fn bundled(self, symbols: usize) -> Option<usize> {
        let length = self.frame().length();
        let bits = match self {
            Self::Robust { carrier, .. } => {
                let each = carrier.symbols(length);
                return symbols.is_multiple_of(each).then(|| symbols / each);
            }
            _ => symbols * self.modcod()?.modulation.bits(),
        };
        (bits % length == 0).then_some(bits / length)
    }

    #[must_use]
    pub fn valid(self) -> bool {
        match self {
            Self::Legacy { modcod, short } => {
                modcod <= 28
                    && ModCod::from_index(modcod)
                        .is_some_and(|mode| mode.rate.information(Frame::of(short)) > 0)
            }
            Self::Extended { code } => s2x::mode(code).is_some(),
            Self::Robust { rate, frame, .. } => rate.information(frame) > 0,
        }
    }

    #[must_use]
    pub fn mode(self) -> Option<(Modulation, Rate)> {
        self.modcod().map(|modcod| (modcod.modulation, modcod.rate))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    Dummy,
    Data(Coding),
}

const fn robust(carrier: Carrier, rate: Rate, frame: Frame) -> Signal {
    Signal::Data(Coding::Robust {
        carrier,
        rate,
        frame,
    })
}

fn legacy(modcod: u8, short: bool) -> Option<Signal> {
    if modcod == 0 {
        return Some(Signal::Dummy);
    }
    let coding = Coding::Legacy { modcod, short };
    coding.valid().then_some(Signal::Data(coding))
}

fn extended(code: u8) -> Option<Signal> {
    let coding = Coding::Extended { code };
    coding.valid().then_some(Signal::Data(coding))
}

#[must_use]
pub fn bundle(format: u8, code: u8) -> Option<Signal> {
    let code = code & 0x7F;
    match (format, code) {
        (_, 0..=63) if format == 3 && code & 0x20 == 0 => None,
        (_, 0..=63) => legacy(code & 0x1F, code & 0x20 != 0),
        (2, 65) => Some(robust(Carrier::Qpsk, Rate::R2_9, Frame::Normal)),
        (2, 66..=107) => extended(132 + 2 * (code - 66)),
        (2, 108) => Some(robust(Carrier::Bpsk, Rate::R1_5, Frame::Medium)),
        (2, 109) => Some(robust(Carrier::Bpsk, Rate::R11_45, Frame::Medium)),
        (2, 110) => Some(robust(Carrier::Bpsk, Rate::R1_3, Frame::Medium)),
        (2, 111) => Some(robust(Carrier::BpskSpread, Rate::R1_4, Frame::Short)),
        (2, 112) => Some(robust(Carrier::BpskSpread, Rate::R11_45, Frame::Short)),
        (3, 64) => Some(robust(Carrier::Bpsk, Rate::R1_4, Frame::Short)),
        (3, 65) => Some(robust(Carrier::Bpsk, Rate::R4_15, Frame::Short)),
        (3, 66) => Some(robust(Carrier::Bpsk, Rate::R1_3, Frame::Short)),
        (3, 67..=83) => extended(216 + 2 * (code - 67)),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mcs {
    pub signal: Signal,
    pub spread: usize,
    pub last: bool,
}

const fn spread(signal: Signal, spread: usize, last: bool) -> Mcs {
    Mcs {
        signal,
        spread,
        last,
    }
}

const SPREAD_TABLE: [(u8, bool); 8] = [
    (0, false),
    (0, false),
    (1, false),
    (1, true),
    (2, false),
    (2, true),
    (3, false),
    (3, true),
];

fn spread_entry(index: u8, factor: usize, last: bool) -> Option<Mcs> {
    let &(modcod, short) = SPREAD_TABLE.get(usize::from(index))?;
    match index {
        0 => Some(spread(
            robust(Carrier::Qpsk, Rate::R1_5, Frame::Medium),
            factor,
            last,
        )),
        1 => None,
        _ => legacy(modcod, short).map(|signal| spread(signal, factor, last)),
    }
}

fn flexible_four(value: u8) -> Option<Mcs> {
    match value {
        0 => Some(spread(Signal::Dummy, 1, false)),
        1..=63 => match legacy(value >> 1, value & 1 == 1)? {
            Signal::Dummy => None,
            signal => Some(spread(signal, 1, false)),
        },
        64..=71 => spread_entry(value - 64, 5, false),
        72..=79 => spread_entry(value - 72, 2, false),
        132..=248 if value.is_multiple_of(2) => {
            extended(value).map(|signal| spread(signal, 1, false))
        }
        _ => None,
    }
}

fn flexible_later(value: u8) -> Option<Mcs> {
    let last = value & 1 == 1;
    let code = value & 0xFE;
    let entry = |index: u8, factor: usize| spread_entry(index, factor, last);
    match code {
        0 => Some(spread(Signal::Dummy, 1, last)),
        114 => entry(0, 5),
        116 => entry(2, 5),
        118 => entry(3, 5),
        120 => entry(4, 5),
        122 => entry(5, 5),
        124 => entry(6, 5),
        126 => entry(7, 5),
        176 => entry(0, 2),
        128 => entry(2, 2),
        130 => entry(3, 2),
        252 => entry(4, 2),
        254 => entry(5, 2),
        188 => entry(6, 2),
        1..=113 => match legacy(code >> 2, code >> 1 & 1 == 1)? {
            Signal::Dummy => None,
            signal => Some(spread(signal, 1, last)),
        },
        132..=248 => extended(code).map(|signal| spread(signal, 1, last)),
        _ => None,
    }
}

#[must_use]
pub fn flexible(format: u8, value: u8) -> Option<Mcs> {
    match format {
        4 => flexible_four(value),
        5 | 6 => flexible_later(value),
        7 => flexible_later(value).filter(|mcs| mcs.spread == 1),
        _ => None,
    }
}

#[cfg(any(test, feature = "synth"))]
#[must_use]
pub fn flexible_value(format: u8, mcs: Mcs) -> Option<u8> {
    (0..=255u8).find(|&value| flexible(format, value) == Some(mcs))
}

pub const DUMMY_SLOTS: usize = 36;
pub const PILOT_SLOTS: usize = 15;
pub const DETERMINISTIC: u8 = 255;
pub const ARBITRARY: u8 = 254;

#[must_use]
pub const fn is_dummy(tsn: u8) -> bool {
    tsn == DETERMINISTIC || tsn == ARBITRARY
}

#[must_use]
pub fn copy_slots(coding: Coding, spread: usize) -> usize {
    let slots = coding.slots();
    if spread > 1 {
        slots + slots / PILOT_SLOTS
    } else {
        slots
    }
}

#[must_use]
pub fn frame_slots(mcs: Mcs, tsn: u8) -> usize {
    match mcs.signal {
        Signal::Dummy => DUMMY_SLOTS,
        Signal::Data(coding) if is_dummy(tsn) => coding.slots(),
        Signal::Data(coding) => mcs.spread * copy_slots(coding, mcs.spread),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_capacity_unit_table_matches_table_e8() {
        let qpsk = |short: bool| Coding::Legacy { modcod: 1, short };
        let medium = Coding::Robust {
            carrier: Carrier::Qpsk,
            rate: Rate::R1_5,
            frame: Frame::Medium,
        };
        let cases = [
            (qpsk(false), 5, 1920),
            (qpsk(false), 2, 768),
            (qpsk(false), 1, 360),
            (medium, 5, 960),
            (medium, 2, 384),
            (qpsk(true), 5, 480),
            (qpsk(true), 2, 192),
            (qpsk(true), 1, 90),
            (
                Coding::Legacy {
                    modcod: 12,
                    short: false,
                },
                1,
                240,
            ),
            (
                Coding::Legacy {
                    modcod: 18,
                    short: false,
                },
                1,
                180,
            ),
            (
                Coding::Legacy {
                    modcod: 24,
                    short: false,
                },
                1,
                144,
            ),
            (Coding::Extended { code: 184 }, 1, 120),
            (Coding::Extended { code: 200 }, 1, 103),
            (Coding::Extended { code: 214 }, 1, 90),
            (
                Coding::Legacy {
                    modcod: 12,
                    short: true,
                },
                1,
                60,
            ),
            (
                Coding::Legacy {
                    modcod: 18,
                    short: true,
                },
                1,
                45,
            ),
            (
                Coding::Legacy {
                    modcod: 24,
                    short: true,
                },
                1,
                36,
            ),
        ];
        for (coding, factor, cus) in cases {
            let mcs = Mcs {
                signal: Signal::Data(coding),
                spread: factor,
                last: false,
            };
            assert_eq!(frame_slots(mcs, 0), cus, "{coding:?} x{factor}");
        }
    }

    #[test]
    fn bundles_hold_the_number_of_frames_the_tables_promise() {
        let long = [
            (4u8, 2),
            (32 | 4, 8),
            (13, 3),
            (65, 2),
            (69, 3),
            (74, 4),
            (87, 5),
            (92, 6),
            (100, 7),
            (102, 8),
            (108, 2),
            (111, 2),
        ];
        for (code, frames) in long {
            let Some(Signal::Data(coding)) = bundle(2, code) else {
                panic!("format 2 code {code}");
            };
            assert_eq!(coding.bundled(64_800), Some(frames), "format 2 code {code}");
        }
        let short = [
            (32 | 4u8, 2),
            (32 | 12, 3),
            (64, 1),
            (66, 1),
            (67, 2),
            (73, 3),
            (77, 4),
            (82, 5),
        ];
        for (code, frames) in short {
            let Some(Signal::Data(coding)) = bundle(3, code) else {
                panic!("format 3 code {code}");
            };
            assert_eq!(coding.bundled(16_200), Some(frames), "format 3 code {code}");
        }
        assert_eq!(bundle(2, 0), Some(Signal::Dummy));
        assert_eq!(bundle(3, 32), Some(Signal::Dummy));
        assert_eq!(bundle(2, 88), None);
        assert_eq!(bundle(3, 4), None);
        assert_eq!(bundle(3, 84), None);
    }

    #[test]
    fn the_flexible_tables_decode_and_encode() {
        assert_eq!(
            flexible(4, 64),
            Some(Mcs {
                signal: robust(Carrier::Qpsk, Rate::R1_5, Frame::Medium),
                spread: 5,
                last: false
            })
        );
        assert_eq!(flexible(4, 65), None);
        assert_eq!(
            flexible(4, 4 << 1 | 1),
            Some(spread(
                Signal::Data(Coding::Legacy {
                    modcod: 4,
                    short: true
                }),
                1,
                false
            ))
        );
        assert_eq!(
            flexible(5, 189),
            Some(spread(
                Signal::Data(Coding::Legacy {
                    modcod: 3,
                    short: false
                }),
                2,
                true
            ))
        );
        assert_eq!(flexible(5, 115).map(|mcs| mcs.spread), Some(5));
        assert_eq!(flexible(6, 4 << 2 | 3).map(|mcs| mcs.last), Some(true));
        assert_eq!(flexible(7, 114), None);
        assert_eq!(
            flexible(5, 133),
            extended(132).map(|signal| spread(signal, 1, true))
        );
        for format in 4..=7u8 {
            for value in 0..=255u8 {
                if let Some(mcs) = flexible(format, value) {
                    assert_eq!(flexible_value(format, mcs), Some(value), "{format} {value}");
                }
            }
        }
    }
}
