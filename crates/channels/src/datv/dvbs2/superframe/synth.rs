use num_complex::Complex;

use super::{
    codes::{self, Gold, Sequence},
    coding::{self, Coding, Mcs, PILOT_SLOTS, Signal},
    layout::{self, Bundles, CU, HEADER, LENGTH, PERIOD, PILOT_START, SOSF, TYPE_A},
    signalling::{self, Header, Plh, Protection},
};
use crate::datv::dvbs2::{frame::Constellation, frame::Modulation, pl, s2x};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Codes {
    pub reference: u32,
    pub payload: u32,
    pub sosf: u8,
    pub pilot: u8,
    pub trailer: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plframe {
    pub mcs: Mcs,
    pub tsn: u8,
    pub symbols: Vec<Complex<f32>>,
    pub cut: Option<usize>,
}

impl Plframe {
    #[must_use]
    pub fn data(coding: Coding, spread: usize, symbols: Vec<Complex<f32>>) -> Self {
        Self {
            mcs: Mcs {
                signal: Signal::Data(coding),
                spread,
                last: false,
            },
            tsn: 0,
            symbols,
            cut: None,
        }
    }

    #[must_use]
    pub fn dummy() -> Self {
        Self {
            mcs: Mcs {
                signal: Signal::Dummy,
                spread: 1,
                last: false,
            },
            tsn: 0,
            symbols: Vec::new(),
            cut: None,
        }
    }

    #[must_use]
    pub fn closing(units: usize) -> Self {
        Self {
            mcs: Mcs {
                signal: Signal::Data(Coding::Legacy {
                    modcod: 4,
                    short: false,
                }),
                spread: 1,
                last: false,
            },
            tsn: coding::ARBITRARY,
            symbols: Vec::new(),
            cut: Some(units),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dwell {
    pub superframes: usize,
    pub cut: usize,
    pub extra: usize,
    pub gap: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub protection: Protection,
    pub pilots: bool,
    pub periods: usize,
    pub frames: usize,
    pub dwell: Option<Dwell>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            protection: Protection::Standard,
            pilots: true,
            periods: 20,
            frames: 4,
            dwell: None,
        }
    }
}

pub struct Transmitter {
    reference: Sequence,
    payload: Sequence,
    codes: Codes,
}

struct Placed {
    start: usize,
    header: usize,
    length: usize,
}

struct Superframe {
    first: usize,
    units: usize,
    trailing: bool,
    postamble: bool,
}

fn known_pattern(index: usize) -> Complex<f32> {
    const PATTERN: [bool; 12] = [
        false, false, false, true, true, true, false, true, true, false, true, false,
    ];
    signalling::bpsk(PATTERN[index % PATTERN.len()])
}

fn constellation(coding: Coding) -> Option<Constellation> {
    match coding {
        Coding::Legacy { .. } => {
            let mode = coding.modcod()?;
            Some(Constellation::new(mode.modulation, mode.rate))
        }
        Coding::Extended { code } => s2x::mode(code).map(s2x::Mode::constellation),
        Coding::Robust { .. } => None,
    }
}

fn known_symbols(signal: Option<Signal>, count: usize, out: &mut Vec<Complex<f32>>) {
    let mapped = match signal {
        Some(Signal::Data(coding)) => coding
            .mode()
            .filter(|(modulation, _)| !matches!(modulation, Modulation::Qpsk | Modulation::Psk8))
            .and_then(|_| constellation(coding)),
        _ => None,
    };
    for index in 0..count {
        out.push(mapped.map_or_else(|| known_pattern(index), |points| points.point(index)));
    }
}

fn deterministic(count: usize, out: &mut Vec<Complex<f32>>) {
    let mut state = 0b100_1010_1000_0000u16;
    for _ in 0..count {
        let bit = (state >> 13 ^ state >> 14) & 1;
        state = (state << 1 | bit) & 0x7FFF;
        out.push(signalling::bpsk(bit == 1));
    }
}

impl Transmitter {
    #[must_use]
    pub fn new(codes: Codes) -> Self {
        let gold = Gold::new();
        Self {
            reference: Sequence::new(&gold, codes.reference),
            payload: Sequence::new(&gold, codes.payload),
            codes,
        }
    }

    fn start(&self, format: u8, out: &mut Vec<Complex<f32>>) {
        for k in 0..SOSF {
            out.push(self.reference.known(k, codes::sosf(self.codes.sosf, k)));
        }
        for k in SOSF..HEADER {
            let column = (k - SOSF) / 30 + 1;
            let negative = (format & column as u8).count_ones() % 2 == 1;
            out.push(self.payload.known(k, negative));
        }
    }

    fn pilot(&self, start: usize, length: usize, short: bool, out: &mut Vec<Complex<f32>>) {
        for k in 0..length {
            let negative = if short {
                codes::short_pilot(self.codes.pilot, k)
            } else {
                codes::pilot(self.codes.pilot, k)
            };
            out.push(self.reference.known(start + k, negative));
        }
    }

    fn data(&self, start: usize, symbols: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        out.extend(
            symbols
                .iter()
                .enumerate()
                .map(|(k, &symbol)| self.payload.scramble(start + k, symbol)),
        );
    }

    #[must_use]
    pub fn legacy(
        &self,
        payload: &[Complex<f32>],
        format: u8,
        pilots: bool,
        count: usize,
    ) -> Vec<Complex<f32>> {
        let mut out = Vec::with_capacity(LENGTH * count);
        let mut cursor = 0;
        for _ in 0..count {
            self.start(format, &mut out);
            let mut position = HEADER;
            while position < LENGTH {
                if pilots && TYPE_A.starts(position) {
                    self.pilot(position, pl::PILOT_LENGTH, false, &mut out);
                    position += pl::PILOT_LENGTH;
                } else {
                    out.push(payload[cursor % payload.len()]);
                    cursor += 1;
                    position += 1;
                }
            }
        }
        out
    }

    #[must_use]
    pub fn bundled(&self, format: u8, bundles: &[(u8, Vec<Complex<f32>>)]) -> Vec<Complex<f32>> {
        let Some(layout) = layout::bundles(format) else {
            return Vec::new();
        };
        let stream = self.bundle_stream(format, layout, bundles);
        let mut out = Vec::with_capacity(LENGTH);
        self.start(format, &mut out);
        let mut cursor = 0;
        let mut position = HEADER;
        while position < LENGTH {
            if position < layout.tail() && layout.grid.starts(position) {
                self.pilot(position, layout.grid.length, layout.short_pilots, &mut out);
                position += layout.grid.length;
                continue;
            }
            let symbol = if position < layout.tail() {
                cursor += 1;
                stream[cursor - 1]
            } else {
                pl::pilot_symbol()
            };
            self.data(position, &[symbol], &mut out);
            position += 1;
        }
        out
    }

    fn bundle_stream(
        &self,
        format: u8,
        layout: Bundles,
        bundles: &[(u8, Vec<Complex<f32>>)],
    ) -> Vec<Complex<f32>> {
        let mut stream = Vec::with_capacity(layout.count * layout.stream());
        for index in 0..layout.count {
            let (code, payload) = bundles.get(index).map_or(
                (if format == 3 { 32 } else { 0 }, None),
                |(code, payload)| (*code, Some(payload)),
            );
            signalling::bundle_header(code, layout.replicas, &mut stream);
            known_symbols(coding::bundle(format, code), layout.known, &mut stream);
            match payload.filter(|payload| payload.len() >= layout.payload) {
                Some(payload) => stream.extend_from_slice(&payload[..layout.payload]),
                None => stream.extend(std::iter::repeat_n(pl::pilot_symbol(), layout.payload)),
            }
        }
        stream
    }

    fn render(
        &self,
        frame: &Plframe,
        format: u8,
        protection: Protection,
        out: &mut Vec<Complex<f32>>,
    ) {
        let value = coding::flexible_value(format, frame.mcs).unwrap_or(0);
        signalling::plh(
            Plh {
                mcs: value,
                tsn: frame.tsn,
            },
            protection,
            out,
        );
        let Signal::Data(coding) = frame.mcs.signal else {
            out.extend(std::iter::repeat_n(
                pl::pilot_symbol(),
                coding::DUMMY_SLOTS * CU,
            ));
            return;
        };
        if coding::is_dummy(frame.tsn) {
            if frame.tsn == coding::DETERMINISTIC {
                deterministic(coding.slots() * CU, out);
            } else {
                out.extend(std::iter::repeat_n(pl::pilot_symbol(), coding.slots() * CU));
            }
            return;
        }
        if frame.mcs.spread == 1 {
            out.extend_from_slice(&frame.symbols);
            return;
        }
        for _ in 0..frame.mcs.spread {
            for segment in frame.symbols.chunks(PILOT_SLOTS * CU) {
                out.extend_from_slice(segment);
                out.extend(std::iter::repeat_n(pl::pilot_symbol(), CU));
            }
        }
    }

    fn header_units(format: u8, protection: Protection) -> usize {
        if format == 7 {
            Protection::Standard.symbols() / CU
        } else {
            protection.symbols() / CU
        }
    }

    fn place(frames: &[Plframe], format: u8, protection: Protection) -> Vec<Placed> {
        let mut start = 0;
        frames
            .iter()
            .map(|frame| {
                let header = Self::header_units(format, protection);
                let length = frame
                    .cut
                    .unwrap_or(header + coding::frame_slots(frame.mcs, frame.tsn));
                let placed = Placed {
                    start,
                    header,
                    length,
                };
                start += length;
                placed
            })
            .collect()
    }

    fn capacity(format: u8, options: &Options) -> usize {
        match format {
            4 if options.pilots => (LENGTH - PILOT_START - 415 * pl::PILOT_LENGTH) / CU,
            4 => (LENGTH - PILOT_START) / CU,
            5 if options.pilots => 16 * (options.periods - 1),
            _ => (options.periods * PERIOD - PILOT_START) / CU,
        }
    }

    fn superframes(format: u8, placed: &[Placed], options: &Options) -> Vec<Superframe> {
        let total: usize = placed.last().map_or(0, |last| last.start + last.length);
        let mut out = Vec::new();
        let mut first = 0;
        let mut index = 0;
        while first < total {
            let in_dwell = options
                .dwell
                .map(|dwell| (index + 1) % dwell.superframes.max(1) == 0);
            let mut units = if layout::fragments(format) {
                Self::capacity(format, options)
            } else {
                placed
                    .iter()
                    .filter(|frame| frame.start >= first)
                    .take(options.frames)
                    .map(|frame| frame.length)
                    .sum()
            };
            if format == 5 {
                while placed.iter().any(|frame| {
                    frame.start < first + units && frame.start + frame.header > first + units
                }) {
                    units += if options.pilots { 16 } else { 82 };
                }
                if let (Some(true), Some(dwell)) = (in_dwell, options.dwell) {
                    let mut cut = dwell.cut.min(units);
                    while let Some(frame) = placed.iter().find(|frame| {
                        frame.start < first + cut && frame.start + frame.header > first + cut
                    }) {
                        cut = frame.start - first;
                    }
                    units = cut;
                }
            }
            let closing = format == 5 && first + units >= total;
            if closing {
                units = total - first;
            }
            let postamble = in_dwell == Some(true) || closing;
            out.push(Superframe {
                first,
                units,
                trailing: format == 4 || (format == 5 && !postamble),

                postamble,
            });
            first += units;
            index += 1;
        }
        out
    }

    #[must_use]
    pub fn flexible(&self, format: u8, frames: &[Plframe], options: Options) -> Vec<Complex<f32>> {
        let protection = if format == 7 {
            Protection::Standard
        } else {
            options.protection
        };
        let mut frames = frames.to_vec();
        if format == 4 {
            let capacity = Self::capacity(format, &options);
            let used: usize = Self::place(&frames, format, protection)
                .iter()
                .map(|frame| frame.length)
                .sum();
            let remaining = used.div_ceil(capacity) * capacity - used;
            let filler = Self::header_units(format, protection) + coding::DUMMY_SLOTS;
            let whole = remaining / filler;
            let spare = remaining % filler;
            let plain = if spare == 0 {
                whole
            } else {
                whole.saturating_sub(1)
            };
            frames.extend(std::iter::repeat_n(Plframe::dummy(), plain));
            if spare > 0 {
                frames.push(Plframe::closing(remaining - plain * filler));
            }
        }
        let placed = Self::place(&frames, format, protection);
        let superframes = Self::superframes(format, &placed, &options);
        for superframe in &superframes {
            if let Some(last) = placed.iter().rposition(|frame| {
                frame.start >= superframe.first && frame.start < superframe.first + superframe.units
            }) && format != 4
            {
                frames[last].mcs.last = true;
            }
        }
        let mut units = Vec::new();
        for (frame, placed) in frames.iter().zip(&placed) {
            let start = units.len();
            self.render(frame, format, protection, &mut units);
            units.truncate(start + placed.length * CU);
        }
        let mut out = Vec::new();
        for superframe in &superframes {
            let pointer = placed
                .iter()
                .find(|frame| {
                    frame.start >= superframe.first
                        && frame.start < superframe.first + superframe.units
                })
                .map_or(0, |frame| {
                    frame.start - superframe.first + layout::first_unit(format) as usize
                });
            let header = Header {
                pointer: pointer as u16,
                pilots: options.pilots || layout::always_pilots(format),
                protection,
                system: 0,
            };
            let extra = options.dwell.map_or(0, |dwell| dwell.extra);
            self.superframe(format, header, superframe, &units, extra, &mut out);
            if superframe.postamble {
                let gap = options.dwell.map_or(0, |dwell| dwell.gap);
                out.extend(std::iter::repeat_n(Complex::new(0.0, 0.0), gap));
            }
        }
        out
    }

    fn superframe(
        &self,
        format: u8,
        header: Header,
        superframe: &Superframe,
        units: &[Complex<f32>],
        extra: usize,
        out: &mut Vec<Complex<f32>>,
    ) {
        self.start(format, out);
        let mut fields = Vec::new();
        match format {
            4 | 5 => signalling::header(header, format, &mut fields),
            6 => {
                fields.extend(
                    (0..504).map(|k| signalling::bpsk(codes::extension(self.codes.sosf, k))),
                );
                fields.extend(
                    (0..216)
                        .map(|k| signalling::bpsk(signalling::indication(header.protection, k))),
                );
            }
            _ => {}
        }
        self.data(HEADER, &fields, out);
        if format == 4 {
            for k in 0..CU {
                out.push(self.reference.known(
                    signalling::TRAILER_START + k,
                    codes::trailer(self.codes.trailer, k),
                ));
            }
        }
        let mut position = layout::payload_start(format);
        let mut remaining = superframe.units;
        let mut cursor = superframe.first * CU;
        loop {
            let due = header.pilots && TYPE_A.starts(position);
            if due && (remaining > 0 || superframe.trailing || superframe.postamble) {
                self.pilot(position, pl::PILOT_LENGTH, false, out);
                position += pl::PILOT_LENGTH;
                if remaining == 0 {
                    break;
                }
                continue;
            }
            if remaining == 0 {
                break;
            }
            let end = (cursor + CU).min(units.len());
            let mut unit = units[cursor.min(end)..end].to_vec();
            unit.resize(CU, pl::pilot_symbol());
            self.data(position, &unit, out);
            position += CU;
            cursor += CU;
            remaining -= 1;
        }
        if superframe.postamble {
            let bits = signalling::postamble();
            let length = header.protection.postamble() + extra;
            let symbols: Vec<Complex<f32>> = (0..length)
                .map(|k| {
                    bits.get(k)
                        .map_or(pl::pilot_symbol(), |&bit| signalling::bpsk(bit))
                })
                .collect();
            self.data(position, &symbols, out);
        }
    }
}
