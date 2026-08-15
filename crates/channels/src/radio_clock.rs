use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, RadioClockFrame,
    RadioClockParams, RadioClockStandard,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const RATE: f64 = 2_000.0;
const CHANNEL_TAPS: usize = 257;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "radio_clock".to_owned(),
    name: "Radio clock (DCF77 / WWVB / MSF / JJY)".to_owned(),
    bandwidth_hz: 200.0,
    input_rate_hz: RATE,
    has_audio: false,
    decoder_kind: Some("radio_clock".to_owned()),
    ..ChannelDescriptor::default()
});

#[derive(Clone, Copy)]
struct Second {
    spacing_ms: f32,
    active_ms: f32,
    a: bool,
    b: bool,
}

pub struct RadioClockChannel {
    standard: RadioClockStandard,
    invert: bool,
    low: f32,
    high: f32,
    last_active: bool,
    since_boundary: u32,
    have_boundary: bool,
    active_samples: u32,
    bins: [u32; 10],
    decoder: Decoder,
}

fn params(settings: &ChannelSettings) -> Result<&RadioClockParams, ChannelError> {
    match &settings.params {
        ChannelParams::RadioClock(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "radio-clock channel got {} params",
            other.type_id()
        ))),
    }
}

pub(crate) fn occupied_band() -> (f64, f64) {
    (-100.0, 100.0)
}

pub(crate) fn channel_filter() -> ChannelFilter {
    ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(CHANNEL_TAPS, 100.0 / RATE),
        1,
    ))
}

impl ChannelRx for RadioClockChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = *params(&settings)?;
        Ok(Self {
            standard: p.standard,
            invert: p.invert,
            low: f32::INFINITY,
            high: 0.0,
            last_active: false,
            since_boundary: 0,
            have_boundary: false,
            active_samples: 0,
            bins: [0; 10],
            decoder: Decoder::new(p.standard),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = *params(&settings)?;
        if p.standard != self.standard || p.invert != self.invert {
            self.standard = p.standard;
            self.invert = p.invert;
            self.reset();
        }
        Ok(())
    }

    fn retuned(&mut self) {
        self.reset();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        for sample in iq {
            self.push(sample.norm(), out);
        }
    }
}

impl RadioClockChannel {
    fn reset(&mut self) {
        self.low = f32::INFINITY;
        self.high = 0.0;
        self.last_active = false;
        self.since_boundary = 0;
        self.have_boundary = false;
        self.active_samples = 0;
        self.bins = [0; 10];
        self.decoder = Decoder::new(self.standard);
    }

    fn push(&mut self, magnitude: f32, out: &mut ChannelOutputs) {
        if !magnitude.is_finite() {
            return;
        }
        self.low = if magnitude < self.low {
            magnitude
        } else {
            self.low + (magnitude - self.low) * 0.000_001
        };
        self.high = if magnitude > self.high {
            magnitude
        } else {
            self.high + (magnitude - self.high) * 0.000_001
        };
        let threshold = self.low + (self.high - self.low) * 0.5;
        let normally_active = match self.standard {
            RadioClockStandard::Jjy => magnitude > threshold,
            _ => magnitude < threshold,
        };
        let active = normally_active ^ self.invert;

        if self.have_boundary {
            let bin = ((self.since_boundary as usize) / 200).min(9);
            if active {
                self.active_samples = self.active_samples.saturating_add(1);
                self.bins[bin] = self.bins[bin].saturating_add(1);
            }
            self.since_boundary = self.since_boundary.saturating_add(1);
        }

        let new_pulse = active && !self.last_active;
        if new_pulse && (!self.have_boundary || self.since_boundary > 1_400) {
            self.boundary(out);
        }
        self.last_active = active;
    }

    fn boundary(&mut self, out: &mut ChannelOutputs) {
        if self.have_boundary {
            let spacing_ms = self.since_boundary as f32 * 1_000.0 / RATE as f32;
            let active_ms = self.active_samples as f32 * 1_000.0 / RATE as f32;
            let half_bin = (RATE / 20.0) as u32;
            let second = Second {
                spacing_ms,
                active_ms,
                a: self.bins[1] >= half_bin,
                b: self.bins[2] >= half_bin,
            };
            if let Some(frame) = self.decoder.feed(second) {
                out.events.push(DecoderEvent::RadioClock(frame));
            }
        }
        self.have_boundary = true;
        self.since_boundary = 0;
        self.active_samples = 0;
        self.bins = [0; 10];
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Symbol {
    Zero,
    One,
    Marker,
    Invalid,
}

impl Symbol {
    fn bit(self) -> Option<bool> {
        match self {
            Self::Zero => Some(false),
            Self::One => Some(true),
            Self::Marker | Self::Invalid => None,
        }
    }

    fn char(self) -> char {
        match self {
            Self::Zero => '0',
            Self::One => '1',
            Self::Marker => 'M',
            Self::Invalid => '?',
        }
    }
}

struct Decoder {
    standard: RadioClockStandard,
    symbols: Vec<Symbol>,
    msf_b: Vec<bool>,
}

impl Decoder {
    fn new(standard: RadioClockStandard) -> Self {
        Self {
            standard,
            symbols: Vec::with_capacity(61),
            msf_b: Vec::with_capacity(61),
        }
    }

    fn feed(&mut self, second: Second) -> Option<RadioClockFrame> {
        let symbol = classify(self.standard, second);
        if self.standard == RadioClockStandard::Dcf77 && second.spacing_ms > 1_500.0 {
            if self.symbols.len() < 59 {
                self.symbols.push(symbol);
            }
            if self.symbols.len() == 59 {
                self.symbols.push(Symbol::Marker);
            }
            let completed = self.decode();
            self.symbols.clear();
            self.msf_b.clear();
            return completed;
        }
        let start = match self.standard {
            RadioClockStandard::Dcf77 => false,
            RadioClockStandard::Msf => second.active_ms > 400.0,
            RadioClockStandard::Wwvb | RadioClockStandard::Jjy => {
                symbol == Symbol::Marker
                    && self.symbols.last() == Some(&Symbol::Marker)
                    && second.spacing_ms < 1_200.0
            }
        };
        let completed = start.then(|| self.decode()).flatten();
        if start {
            self.symbols.clear();
            self.msf_b.clear();
            self.symbols.push(Symbol::Marker);
            self.msf_b.push(false);
        } else if self.symbols.len() < 61 {
            self.symbols.push(symbol);
            self.msf_b.push(second.b);
        }
        completed
    }

    fn decode(&self) -> Option<RadioClockFrame> {
        if self.symbols.len() != 60 {
            return None;
        }
        match self.standard {
            RadioClockStandard::Dcf77 => decode_dcf77(&self.symbols),
            RadioClockStandard::Wwvb => decode_wwvb(&self.symbols),
            RadioClockStandard::Msf => decode_msf(&self.symbols, &self.msf_b),
            RadioClockStandard::Jjy => decode_jjy(&self.symbols),
        }
    }
}

fn classify(standard: RadioClockStandard, second: Second) -> Symbol {
    let ms = second.active_ms;
    match standard {
        RadioClockStandard::Dcf77 => nearest(ms, &[(100.0, Symbol::Zero), (200.0, Symbol::One)]),
        RadioClockStandard::Wwvb => nearest(
            ms,
            &[
                (200.0, Symbol::Zero),
                (500.0, Symbol::One),
                (800.0, Symbol::Marker),
            ],
        ),
        RadioClockStandard::Jjy => nearest(
            ms,
            &[
                (800.0, Symbol::Zero),
                (500.0, Symbol::One),
                (200.0, Symbol::Marker),
            ],
        ),
        RadioClockStandard::Msf => {
            if second.a {
                Symbol::One
            } else {
                Symbol::Zero
            }
        }
    }
}

fn nearest(ms: f32, choices: &[(f32, Symbol)]) -> Symbol {
    choices
        .iter()
        .min_by(|a, b| (a.0 - ms).abs().total_cmp(&(b.0 - ms).abs()))
        .filter(|(wanted, _)| (*wanted - ms).abs() <= 80.0)
        .map_or(Symbol::Invalid, |(_, symbol)| *symbol)
}

fn weighted(bits: &[Symbol], fields: &[(usize, u16)]) -> Option<u16> {
    fields.iter().try_fold(0, |value, &(index, weight)| {
        bits.get(index)?
            .bit()
            .map(|set| value + u16::from(set) * weight)
    })
}

fn parity(bits: &[Symbol], range: std::ops::RangeInclusive<usize>, at: usize) -> bool {
    let Some(expected) = bits.get(at).and_then(|s| s.bit()) else {
        return false;
    };
    let ones = range.filter(|&i| bits[i] == Symbol::One).count();
    expected == (ones % 2 == 1)
}

fn symbols(bits: &[Symbol]) -> String {
    bits.iter().map(|s| s.char()).collect()
}

fn frame(
    standard: RadioClockStandard,
    datetime: String,
    utc_offset_minutes: Option<i16>,
    dst: bool,
    leap_warning: bool,
    dut1_seconds: Option<f32>,
    bits: &[Symbol],
) -> RadioClockFrame {
    RadioClockFrame {
        standard,
        datetime,
        utc_offset_minutes,
        dst,
        leap_warning,
        dut1_seconds,
        symbols: symbols(bits),
    }
}

fn decode_dcf77(bits: &[Symbol]) -> Option<RadioClockFrame> {
    if bits[20] != Symbol::One
        || !parity(bits, 21..=27, 28)
        || !parity(bits, 29..=34, 35)
        || !parity(bits, 36..=57, 58)
    {
        return None;
    }
    let minute = weighted(
        bits,
        &[
            (21, 1),
            (22, 2),
            (23, 4),
            (24, 8),
            (25, 10),
            (26, 20),
            (27, 40),
        ],
    )?;
    let hour = weighted(
        bits,
        &[(29, 1), (30, 2), (31, 4), (32, 8), (33, 10), (34, 20)],
    )?;
    let day = weighted(
        bits,
        &[(36, 1), (37, 2), (38, 4), (39, 8), (40, 10), (41, 20)],
    )?;
    let month = weighted(bits, &[(45, 1), (46, 2), (47, 4), (48, 8), (49, 10)])?;
    let year = weighted(
        bits,
        &[
            (50, 1),
            (51, 2),
            (52, 4),
            (53, 8),
            (54, 10),
            (55, 20),
            (56, 40),
            (57, 80),
        ],
    )?;
    valid_clock(2000 + year, hour, minute, month, day)?;
    let dst = bits[17] == Symbol::One && bits[18] == Symbol::Zero;
    let offset = if dst { 120 } else { 60 };
    Some(frame(
        RadioClockStandard::Dcf77,
        iso(2000 + year, month, day, hour, minute, offset),
        Some(offset),
        dst,
        bits[19] == Symbol::One,
        None,
        bits,
    ))
}

fn decode_wwvb(bits: &[Symbol]) -> Option<RadioClockFrame> {
    let minute = weighted(
        bits,
        &[(1, 40), (2, 20), (3, 10), (5, 8), (6, 4), (7, 2), (8, 1)],
    )?;
    let hour = weighted(
        bits,
        &[(12, 20), (13, 10), (15, 8), (16, 4), (17, 2), (18, 1)],
    )?;
    let day = weighted(
        bits,
        &[
            (22, 200),
            (23, 100),
            (25, 80),
            (26, 40),
            (27, 20),
            (28, 10),
            (30, 8),
            (31, 4),
            (32, 2),
            (33, 1),
        ],
    )?;
    let year = weighted(
        bits,
        &[
            (45, 80),
            (46, 40),
            (47, 20),
            (48, 10),
            (50, 8),
            (51, 4),
            (52, 2),
            (53, 1),
        ],
    )?;
    let (month, month_day) = ordinal_date(2000 + year, day)?;
    valid_clock(2000 + year, hour, minute, month, month_day)?;
    let dst = bits[58] == Symbol::One;
    Some(frame(
        RadioClockStandard::Wwvb,
        iso(2000 + year, month, month_day, hour, minute, 0),
        Some(0),
        dst,
        bits[56] == Symbol::One,
        None,
        bits,
    ))
}

fn decode_jjy(bits: &[Symbol]) -> Option<RadioClockFrame> {
    if !parity(bits, 12..=18, 36) || !parity(bits, 1..=8, 37) {
        return None;
    }
    let minute = weighted(
        bits,
        &[(1, 40), (2, 20), (3, 10), (5, 8), (6, 4), (7, 2), (8, 1)],
    )?;
    let hour = weighted(
        bits,
        &[(12, 20), (13, 10), (15, 8), (16, 4), (17, 2), (18, 1)],
    )?;
    let day = weighted(
        bits,
        &[
            (22, 200),
            (23, 100),
            (25, 80),
            (26, 40),
            (27, 20),
            (28, 10),
            (30, 8),
            (31, 4),
            (32, 2),
            (33, 1),
        ],
    )?;
    let year = weighted(
        bits,
        &[
            (41, 80),
            (42, 40),
            (43, 20),
            (44, 10),
            (45, 8),
            (46, 4),
            (47, 2),
            (48, 1),
        ],
    )?;
    let (month, month_day) = ordinal_date(2000 + year, day)?;
    valid_clock(2000 + year, hour, minute, month, month_day)?;
    Some(frame(
        RadioClockStandard::Jjy,
        iso(2000 + year, month, month_day, hour, minute, 540),
        Some(540),
        false,
        bits[53] == Symbol::One,
        None,
        bits,
    ))
}

fn decode_msf(bits: &[Symbol], b: &[bool]) -> Option<RadioClockFrame> {
    let odd = |range: std::ops::RangeInclusive<usize>, at: usize| {
        (range.filter(|&i| bits[i] == Symbol::One).count() + usize::from(b[at])) % 2 == 1
    };
    if !odd(17..=24, 54) || !odd(25..=35, 55) || !odd(36..=38, 56) || !odd(39..=51, 57) {
        return None;
    }
    let year = weighted(
        bits,
        &[
            (17, 80),
            (18, 40),
            (19, 20),
            (20, 10),
            (21, 8),
            (22, 4),
            (23, 2),
            (24, 1),
        ],
    )?;
    let month = weighted(bits, &[(25, 10), (26, 8), (27, 4), (28, 2), (29, 1)])?;
    let day = weighted(
        bits,
        &[(30, 20), (31, 10), (32, 8), (33, 4), (34, 2), (35, 1)],
    )?;
    let hour = weighted(
        bits,
        &[(39, 20), (40, 10), (41, 8), (42, 4), (43, 2), (44, 1)],
    )?;
    let minute = weighted(
        bits,
        &[
            (45, 40),
            (46, 20),
            (47, 10),
            (48, 8),
            (49, 4),
            (50, 2),
            (51, 1),
        ],
    )?;
    valid_clock(2000 + year, hour, minute, month, day)?;
    let dst = b[58];
    let offset = if dst { 60 } else { 0 };
    let positive = b[1..=8].iter().take_while(|&&v| v).count();
    let negative = b[9..=16].iter().take_while(|&&v| v).count();
    let dut1 = match (positive, negative) {
        (0, 0) => Some(0.0),
        (n, 0) => Some(n as f32 / 10.0),
        (0, n) => Some(-(n as f32) / 10.0),
        _ => None,
    };
    Some(frame(
        RadioClockStandard::Msf,
        iso(2000 + year, month, day, hour, minute, offset),
        Some(offset),
        dst,
        false,
        dut1,
        bits,
    ))
}

fn valid_clock(year: u16, hour: u16, minute: u16, month: u16, day: u16) -> Option<()> {
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let max_day = *days.get(usize::from(month.checked_sub(1)?))?;
    (hour < 24 && minute < 60 && day > 0 && day <= max_day).then_some(())
}

fn iso(year: u16, month: u16, day: u16, hour: u16, minute: u16, offset: i16) -> String {
    if offset == 0 {
        return format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:00Z");
    }
    let sign = if offset < 0 { '-' } else { '+' };
    let offset = offset.unsigned_abs();
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:00{sign}{:02}:{:02}",
        offset / 60,
        offset % 60
    )
}

fn ordinal_date(year: u16, ordinal: u16) -> Option<(u16, u16)> {
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let mut left = ordinal;
    for (index, &days) in [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ]
    .iter()
    .enumerate()
    {
        if left <= days {
            return (left > 0).then_some((index as u16 + 1, left));
        }
        left -= days;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(bits: &mut [Symbol], fields: &[(usize, u16)], value: u16) {
        let ones = value % 10;
        let tens = value / 10 % 10;
        let hundreds = value / 100;
        for &(index, weight) in fields {
            let set = match weight {
                1 | 2 | 4 | 8 => ones & weight != 0,
                10 | 20 | 40 | 80 => tens & (weight / 10) != 0,
                100 | 200 => hundreds & (weight / 100) != 0,
                _ => false,
            };
            bits[index] = if set { Symbol::One } else { Symbol::Zero };
        }
    }

    fn even_parity(bits: &mut [Symbol], range: std::ops::RangeInclusive<usize>, at: usize) {
        bits[at] = if range.filter(|&i| bits[i] == Symbol::One).count() % 2 == 1 {
            Symbol::One
        } else {
            Symbol::Zero
        };
    }

    #[test]
    fn dcf77_golden_minute_decodes_and_checks_parity() {
        let mut bits = vec![Symbol::Zero; 60];
        bits[59] = Symbol::Marker;
        bits[18] = Symbol::One;
        bits[20] = Symbol::One;
        set(
            &mut bits,
            &[
                (21, 1),
                (22, 2),
                (23, 4),
                (24, 8),
                (25, 10),
                (26, 20),
                (27, 40),
            ],
            34,
        );
        set(
            &mut bits,
            &[(29, 1), (30, 2), (31, 4), (32, 8), (33, 10), (34, 20)],
            12,
        );
        set(
            &mut bits,
            &[(36, 1), (37, 2), (38, 4), (39, 8), (40, 10), (41, 20)],
            15,
        );
        set(
            &mut bits,
            &[(45, 1), (46, 2), (47, 4), (48, 8), (49, 10)],
            8,
        );
        set(
            &mut bits,
            &[
                (50, 1),
                (51, 2),
                (52, 4),
                (53, 8),
                (54, 10),
                (55, 20),
                (56, 40),
                (57, 80),
            ],
            26,
        );
        even_parity(&mut bits, 21..=27, 28);
        even_parity(&mut bits, 29..=34, 35);
        even_parity(&mut bits, 36..=57, 58);
        let decoded = decode_dcf77(&bits).expect("valid minute");
        assert_eq!(decoded.datetime, "2026-08-15T12:34:00+01:00");
        bits[21] = if bits[21] == Symbol::One {
            Symbol::Zero
        } else {
            Symbol::One
        };
        assert!(decode_dcf77(&bits).is_none());
    }

    #[test]
    fn jjy_golden_minute_decodes_day_of_year() {
        let mut bits = vec![Symbol::Zero; 60];
        for i in [0, 9, 19, 29, 39, 49, 59] {
            bits[i] = Symbol::Marker;
        }
        set(
            &mut bits,
            &[(1, 40), (2, 20), (3, 10), (5, 8), (6, 4), (7, 2), (8, 1)],
            15,
        );
        set(
            &mut bits,
            &[(12, 20), (13, 10), (15, 8), (16, 4), (17, 2), (18, 1)],
            17,
        );
        set(
            &mut bits,
            &[
                (22, 200),
                (23, 100),
                (25, 80),
                (26, 40),
                (27, 20),
                (28, 10),
                (30, 8),
                (31, 4),
                (32, 2),
                (33, 1),
            ],
            162,
        );
        set(
            &mut bits,
            &[
                (41, 80),
                (42, 40),
                (43, 20),
                (44, 10),
                (45, 8),
                (46, 4),
                (47, 2),
                (48, 1),
            ],
            16,
        );
        even_parity(&mut bits, 12..=18, 36);
        even_parity(&mut bits, 1..=8, 37);
        let decoded = decode_jjy(&bits).expect("valid minute");
        assert_eq!(decoded.datetime, "2016-06-10T17:15:00+09:00");
    }

    #[test]
    fn wwvb_golden_minute_decodes_utc_day_of_year() {
        let mut bits = vec![Symbol::Zero; 60];
        for i in [0, 9, 19, 29, 39, 49, 59] {
            bits[i] = Symbol::Marker;
        }
        set(
            &mut bits,
            &[(1, 40), (2, 20), (3, 10), (5, 8), (6, 4), (7, 2), (8, 1)],
            34,
        );
        set(
            &mut bits,
            &[(12, 20), (13, 10), (15, 8), (16, 4), (17, 2), (18, 1)],
            12,
        );
        set(
            &mut bits,
            &[
                (22, 200),
                (23, 100),
                (25, 80),
                (26, 40),
                (27, 20),
                (28, 10),
                (30, 8),
                (31, 4),
                (32, 2),
                (33, 1),
            ],
            227,
        );
        set(
            &mut bits,
            &[
                (45, 80),
                (46, 40),
                (47, 20),
                (48, 10),
                (50, 8),
                (51, 4),
                (52, 2),
                (53, 1),
            ],
            26,
        );
        let decoded = decode_wwvb(&bits).expect("valid minute");
        assert_eq!(decoded.datetime, "2026-08-15T12:34:00Z");
    }

    #[test]
    fn msf_golden_minute_checks_four_odd_parities() {
        let mut bits = vec![Symbol::Zero; 60];
        let mut b = vec![false; 60];
        bits[0] = Symbol::Marker;
        set(
            &mut bits,
            &[
                (17, 80),
                (18, 40),
                (19, 20),
                (20, 10),
                (21, 8),
                (22, 4),
                (23, 2),
                (24, 1),
            ],
            26,
        );
        set(
            &mut bits,
            &[(25, 10), (26, 8), (27, 4), (28, 2), (29, 1)],
            8,
        );
        set(
            &mut bits,
            &[(30, 20), (31, 10), (32, 8), (33, 4), (34, 2), (35, 1)],
            15,
        );
        set(&mut bits, &[(36, 4), (37, 2), (38, 1)], 6);
        set(
            &mut bits,
            &[(39, 20), (40, 10), (41, 8), (42, 4), (43, 2), (44, 1)],
            12,
        );
        set(
            &mut bits,
            &[
                (45, 40),
                (46, 20),
                (47, 10),
                (48, 8),
                (49, 4),
                (50, 2),
                (51, 1),
            ],
            34,
        );
        for (range, at) in [(17..=24, 54), (25..=35, 55), (36..=38, 56), (39..=51, 57)] {
            b[at] = range.filter(|&i| bits[i] == Symbol::One).count() % 2 == 0;
        }
        let decoded = decode_msf(&bits, &b).expect("valid minute");
        assert_eq!(decoded.datetime, "2026-08-15T12:34:00Z");
        b[55] = !b[55];
        assert!(decode_msf(&bits, &b).is_none());
    }

    #[test]
    fn pulse_widths_are_standard_specific() {
        let second = |active_ms| Second {
            spacing_ms: 1_000.0,
            active_ms,
            a: false,
            b: false,
        };
        assert_eq!(
            classify(RadioClockStandard::Dcf77, second(100.0)),
            Symbol::Zero
        );
        assert_eq!(
            classify(RadioClockStandard::Wwvb, second(800.0)),
            Symbol::Marker
        );
        assert_eq!(
            classify(RadioClockStandard::Jjy, second(800.0)),
            Symbol::Zero
        );
    }

    #[test]
    fn synthesized_iq_minute_reaches_the_typed_output() {
        let mut bits = vec![Symbol::Zero; 60];
        bits[18] = Symbol::One;
        bits[20] = Symbol::One;
        set(
            &mut bits,
            &[
                (21, 1),
                (22, 2),
                (23, 4),
                (24, 8),
                (25, 10),
                (26, 20),
                (27, 40),
            ],
            34,
        );
        set(
            &mut bits,
            &[(29, 1), (30, 2), (31, 4), (32, 8), (33, 10), (34, 20)],
            12,
        );
        set(
            &mut bits,
            &[(36, 1), (37, 2), (38, 4), (39, 8), (40, 10), (41, 20)],
            15,
        );
        set(
            &mut bits,
            &[(45, 1), (46, 2), (47, 4), (48, 8), (49, 10)],
            8,
        );
        set(
            &mut bits,
            &[
                (50, 1),
                (51, 2),
                (52, 4),
                (53, 8),
                (54, 10),
                (55, 20),
                (56, 40),
                (57, 80),
            ],
            26,
        );
        even_parity(&mut bits, 21..=27, 28);
        even_parity(&mut bits, 29..=34, 35);
        even_parity(&mut bits, 36..=57, 58);
        bits[59] = Symbol::Marker;

        let mut iq = Vec::with_capacity(121_000);
        iq.extend((0..2_000).map(|_| Complex::new(1.0, 0.0)));
        for _ in 0..2 {
            for symbol in &bits {
                let low = match symbol {
                    Symbol::Zero => 200,
                    Symbol::One => 400,
                    Symbol::Marker | Symbol::Invalid => 0,
                };
                iq.extend((0..2_000).map(|n| Complex::new(if n < low { 0.1 } else { 1.0 }, 0.0)));
            }
        }
        let settings = ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            params: ChannelParams::RadioClock(RadioClockParams::default()),
            audio: Default::default(),
        };
        let mut channel = RadioClockChannel::new(ChannelCtx { input_rate: RATE }, settings)
            .expect("valid fixture channel");
        let mut output = ChannelOutputs::default();
        for chunk in iq.chunks(997) {
            channel.process(chunk, &mut output);
        }
        assert!(output.events.iter().any(|event| {
            matches!(event, DecoderEvent::RadioClock(frame) if frame.datetime == "2026-08-15T12:34:00+01:00")
        }));
    }
}
