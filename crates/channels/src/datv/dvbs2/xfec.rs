use num_complex::Complex;

#[cfg(any(test, feature = "synth"))]
use super::frame::modulate;
use super::{
    bb::{BaseBandData, BaseBandFrame},
    bch::{Bch, BchScratch},
    frame::{Constellation, Modulation, column_order, demodulate},
    ldpc::{Frame, Ldpc, Rate},
    pl, s2x,
    superframe::coding::Coding,
    vlsnr::Carrier,
};

const NOISE: f32 = 0.25;
const ROBUST_CORRECT: usize = 12;

pub struct Decoded {
    pub data: BaseBandData,
    pub iterations: usize,
    pub corrected: usize,
}

pub struct Xfec {
    coding: Coding,
    carrier: Carrier,
    order: &'static [usize],
    ldpc: Ldpc,
    bch: Bch,
    scratch: BchScratch,
    pub baseband: BaseBandFrame,
    constellation: Constellation,
    llrs: Vec<f32>,
    ordered: Vec<f32>,
    bits: Vec<bool>,
}

fn parts(coding: Coding) -> Option<(Carrier, &'static [usize], Rate, usize, Constellation)> {
    match coding {
        Coding::Legacy { short, .. } => {
            let mode = coding.modcod()?;
            Some((
                Carrier::Qpsk,
                column_order(mode.modulation, mode.rate),
                mode.rate,
                mode.correct(short),
                Constellation::new(mode.modulation, mode.rate),
            ))
        }
        Coding::Extended { code } => {
            let mode = s2x::mode(code)?;
            Some((
                Carrier::Qpsk,
                mode.order,
                mode.rate,
                mode.modcod().correct(mode.short),
                mode.constellation(),
            ))
        }
        Coding::Robust { carrier, rate, .. } => Some((
            carrier,
            &[],
            rate,
            ROBUST_CORRECT,
            Constellation::new(Modulation::Qpsk, rate),
        )),
    }
}

fn place(order: &[usize], length: usize, compact: bool, mut visit: impl FnMut(usize, usize)) {
    if order.is_empty() {
        (0..length).for_each(|bit| visit(bit, bit));
        return;
    }
    let rows = length.div_ceil(order.len());
    let mut next = 0;
    for row in 0..rows {
        for (position, &column) in order.iter().enumerate() {
            let bit = column * rows + row;
            let at = if compact {
                next
            } else {
                row * order.len() + position
            };
            if bit < length {
                visit(at, bit);
                next += 1;
            }
        }
    }
}

impl Xfec {
    #[must_use]
    pub fn new(coding: Coding) -> Option<Self> {
        if !coding.valid() {
            return None;
        }
        let (carrier, order, rate, correct, constellation) = parts(coding)?;
        let frame = coding.frame();
        let ldpc = Ldpc::new(rate, frame)?;
        let message = rate
            .information(frame)
            .checked_sub(correct * frame.correct_bits())?;
        let bch = Bch::new(frame, correct, message);
        Some(Self {
            coding,
            carrier,
            order,
            ldpc,
            scratch: bch.scratch(),
            bch,
            baseband: BaseBandFrame::new(message),
            constellation,
            llrs: Vec::with_capacity(Frame::Normal.length()),
            ordered: vec![0.0; frame.length()],
            bits: Vec::with_capacity(Frame::Normal.length()),
        })
    }

    #[must_use]
    pub const fn coding(&self) -> Coding {
        self.coding
    }

    #[must_use]
    pub const fn carrier(&self) -> Carrier {
        self.carrier
    }

    #[must_use]
    pub const fn constellation(&self) -> &Constellation {
        &self.constellation
    }

    fn length(&self) -> usize {
        self.coding.frame().length()
    }

    pub fn soft(&mut self, symbols: &[Complex<f32>]) {
        self.llrs.clear();
        match self.carrier {
            Carrier::Qpsk => demodulate(symbols, &self.constellation, NOISE, &mut self.llrs),
            Carrier::Bpsk => self.llrs.extend(
                symbols
                    .iter()
                    .enumerate()
                    .map(|(index, &symbol)| (symbol * pl::bpsk(index, false).conj()).re / NOISE),
            ),
            Carrier::BpskSpread => {
                self.llrs
                    .extend(
                        symbols
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .enumerate()
                            .map(|(index, pair)| {
                                let first = (pair[0] * pl::bpsk(2 * index, false).conj()).re;
                                let second = (pair[1] * pl::bpsk(2 * index + 1, false).conj()).re;
                                (first + second) / NOISE
                            }),
                    );
            }
        }
    }

    pub fn decode(&mut self, index: usize, compact: bool) -> Option<Decoded> {
        let length = self.length();
        let stride = if compact { length } else { self.llrs.len() };
        let source = self.llrs.get(index * stride..(index + 1) * stride)?;
        let ordered = &mut self.ordered;
        place(self.order, length, compact, |at, bit| {
            ordered[bit] = source.get(at).copied().unwrap_or(0.0);
        });
        self.bits.clear();
        let iterations = self.ldpc.decode(&self.ordered, &mut self.bits)?;
        let corrected = self
            .bch
            .decode_with_scratch(&mut self.bits, &mut self.scratch)?;
        self.bits.truncate(self.bch.message());
        let data = self.baseband.read(&self.bits)?;
        Some(Decoded {
            data,
            iterations,
            corrected,
        })
    }

    #[cfg(any(test, feature = "synth"))]
    fn interleaved(&self, baseband: &[bool], compact: bool) -> Vec<bool> {
        let mut protected = Vec::new();
        self.bch.encode(baseband, &mut protected);
        let mut coded = Vec::new();
        self.ldpc.encode(&protected, &mut coded);
        if self.carrier != Carrier::Qpsk {
            return coded;
        }
        let length = self.length();
        let size = if compact {
            length
        } else {
            self.coding.slots() * pl::SLOT * self.constellation.bits()
        };
        let mut interleaved = vec![true; size.max(length)];
        place(self.order, length, compact, |at, bit| {
            interleaved[at] = coded[bit];
        });
        interleaved.truncate(size);
        interleaved
    }

    #[cfg(any(test, feature = "synth"))]
    fn map(&self, bits: &[bool], out: &mut Vec<Complex<f32>>) {
        match self.carrier {
            Carrier::Qpsk => modulate(bits, &self.constellation, out),
            Carrier::Bpsk => out.extend(
                bits.iter()
                    .enumerate()
                    .map(|(index, &bit)| pl::bpsk(index, bit)),
            ),
            Carrier::BpskSpread => {
                for (index, &bit) in bits.iter().enumerate() {
                    out.push(pl::bpsk(2 * index, bit));
                    out.push(pl::bpsk(2 * index + 1, bit));
                }
            }
        }
    }

    #[cfg(any(test, feature = "synth"))]
    pub fn encode(&self, baseband: &[bool], compact: bool, out: &mut Vec<Complex<f32>>) {
        self.map(&self.interleaved(baseband, compact), out);
    }

    #[cfg(any(test, feature = "synth"))]
    pub fn encode_bundle(&self, basebands: &[Vec<bool>], out: &mut Vec<Complex<f32>>) {
        let bits: Vec<bool> = basebands
            .iter()
            .flat_map(|baseband| self.interleaved(baseband, true))
            .collect();
        self.map(&bits, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datv::{dvbs::PACKET, ts::null_packet};

    fn packets(count: usize) -> Vec<[u8; PACKET]> {
        (0..count)
            .map(|index| {
                let mut packet = null_packet();
                packet[4] = index as u8;
                packet
            })
            .collect()
    }

    #[test]
    fn every_coding_round_trips_alone_and_bundled() {
        let codings = [
            Coding::Legacy {
                modcod: 12,
                short: false,
            },
            Coding::Legacy {
                modcod: 18,
                short: true,
            },
            Coding::Extended { code: 200 },
            Coding::Extended { code: 246 },
            Coding::Extended { code: 202 },
            Coding::Robust {
                carrier: Carrier::Qpsk,
                rate: Rate::R1_5,
                frame: Frame::Medium,
            },
            Coding::Robust {
                carrier: Carrier::BpskSpread,
                rate: Rate::R1_4,
                frame: Frame::Short,
            },
        ];
        for coding in codings {
            let mut codec = Xfec::new(coding).unwrap_or_else(|| panic!("{coding:?}"));
            let mut carry = 0x47;
            let baseband = codec
                .baseband
                .build(&packets(codec.baseband.capacity()), &mut carry)
                .expect("a frame");
            let mut symbols = Vec::new();
            codec.encode(&baseband, false, &mut symbols);
            assert_eq!(symbols.len(), coding.slots() * pl::SLOT, "{coding:?}");
            codec.soft(&symbols);
            let decoded = codec
                .decode(0, false)
                .unwrap_or_else(|| panic!("{coding:?}"));
            assert_eq!(decoded.data.transport().len(), codec.baseband.capacity());
            let Some(frames) = coding.bundled(64_800) else {
                continue;
            };
            let basebands = vec![baseband.clone(); frames];
            let mut symbols = Vec::new();
            codec.encode_bundle(&basebands, &mut symbols);
            assert_eq!(symbols.len(), 64_800, "{coding:?}");
            codec.soft(&symbols);
            for index in 0..frames {
                assert!(codec.decode(index, true).is_some(), "{coding:?} {index}");
            }
        }
    }
}
