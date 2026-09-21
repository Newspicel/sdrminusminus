use super::{DecodeError, acquire::Preamble, pilot_tables::*, signalling::Pre};

const PERMUTATIONS: [[&[usize]; 2]; 6] = [
    [&[8, 7, 6, 5, 0, 1, 2, 3, 4], &[6, 8, 7, 4, 1, 0, 5, 2, 3]],
    [
        &[4, 3, 9, 6, 2, 8, 1, 5, 7, 0],
        &[6, 9, 4, 8, 5, 1, 0, 7, 2, 3],
    ],
    [
        &[6, 3, 0, 9, 4, 2, 1, 8, 5, 10, 7],
        &[5, 9, 1, 4, 3, 0, 8, 10, 7, 2, 6],
    ],
    [
        &[7, 1, 4, 2, 9, 6, 8, 10, 0, 3, 11, 5],
        &[11, 4, 9, 3, 1, 2, 5, 0, 6, 7, 10, 8],
    ],
    [
        &[9, 7, 6, 10, 12, 5, 1, 11, 0, 2, 3, 4, 8],
        &[6, 8, 10, 12, 2, 0, 4, 1, 11, 3, 5, 9, 7],
    ],
    [
        &[7, 13, 3, 4, 9, 2, 12, 11, 1, 8, 10, 0, 5, 6],
        &[7, 13, 3, 4, 9, 2, 12, 11, 1, 8, 10, 0, 5, 6],
    ],
];
const TAPS: [&[usize]; 6] = [
    &[0, 4],
    &[0, 3],
    &[0, 2],
    &[0, 1, 4, 6],
    &[0, 1, 4, 5, 9, 11],
    &[0, 1, 2, 12],
];
const SCATTERED: [(usize, usize); 8] = [
    (3, 4),
    (6, 2),
    (6, 4),
    (12, 2),
    (12, 4),
    (24, 2),
    (24, 4),
    (6, 16),
];
const NORMAL: [usize; 6] = [853, 1705, 3409, 6817, 13633, 27265];
const EXTRA: [usize; 6] = [0, 0, 0, 48, 144, 288];
const CLOSING_ACTIVE: [[usize; 8]; 9] = [
    [402, 654, 490, 707, 544, 0, 0, 0],
    [804, 1309, 980, 1415, 1088, 0, 1396, 0],
    [1609, 2619, 1961, 2831, 2177, 0, 2792, 0],
    [3218, 5238, 3922, 5662, 4354, 0, 5585, 0],
    [3264, 5312, 3978, 5742, 4416, 0, 5664, 0],
    [6437, 10476, 7845, 11324, 8709, 11801, 11170, 0],
    [6573, 10697, 8011, 11563, 8893, 12051, 11406, 0],
    [0, 20952, 0, 22649, 0, 23603, 0, 0],
    [0, 21395, 0, 23127, 0, 24102, 0, 0],
];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Carrier {
    #[default]
    Data,
    Reserved,
    Pilot {
        amplitude: f32,
        inverted: bool,
    },
}

pub struct Mapping {
    pub fft: usize,
    pub carriers: usize,
    pub data: usize,
    pub active: usize,
    pub map: Vec<Carrier>,
    pub pilots: Vec<f32>,
    prbs: Vec<bool>,
    permutations: [Vec<usize>; 2],
    mode: usize,
}

impl Mapping {
    pub fn new(fft: usize) -> Result<Self, DecodeError> {
        if !(1024..=32768).contains(&fft) || !fft.is_power_of_two() {
            return Err(DecodeError::Parameters);
        }
        let mode = fft.ilog2() as usize - 10;
        let mut state = 0x7ff_u16;
        let carriers = NORMAL[mode] + 2 * EXTRA[mode];
        Ok(Self {
            fft,
            carriers,
            data: 0,
            active: 0,
            mode,
            map: vec![Carrier::Data; carriers],
            pilots: vec![0.0; carriers],
            prbs: (0..carriers)
                .map(|_| {
                    let bit = state & 1 != 0;
                    let next = (state ^ (state >> 2)) & 1;
                    state = (state >> 1) | (next << 10);
                    bit
                })
                .collect(),
            permutations: std::array::from_fn(|parity| permutation(fft, mode, parity)),
        })
    }

    pub fn p2(&mut self, preamble: Preamble, symbol: usize) -> Result<(), DecodeError> {
        if preamble.fft()? != self.fft || symbol >= preamble.p2_symbols()? {
            return Err(DecodeError::Parameters);
        }
        self.carriers = NORMAL[self.mode] + 2 * EXTRA[self.mode];
        self.map.fill(Carrier::Data);
        let extra = EXTRA[self.mode];
        let step = if self.fft == 32768 && !preamble.miso() {
            6
        } else {
            3
        };
        let amplitude = if step == 6 {
            37.0_f32.sqrt() / 5.0
        } else {
            31.0_f32.sqrt() / 5.0
        };
        for k in 0..self.carriers {
            if k % step == 0 || k < extra || k >= self.carriers - extra {
                self.map[k] = Carrier::Pilot {
                    amplitude,
                    inverted: k % 3 == 0 && k / 3 % 2 == 1,
                };
            }
        }
        if preamble.miso() {
            for k in [
                extra + 1,
                extra + 2,
                self.carriers - extra - 2,
                self.carriers - extra - 3,
            ] {
                self.map[k] = Carrier::Pilot {
                    amplitude,
                    inverted: false,
                };
            }
        }
        for &tone in P2_TONES[self.mode] {
            self.map[tone + extra] = Carrier::Reserved;
        }
        if preamble.miso() {
            for &tone in P2_TONES[self.mode] {
                let k = tone + extra;
                let adjacent = match k % 3 {
                    1 => k + 1,
                    2 => k - 1,
                    _ => continue,
                };
                if self.map[adjacent] != Carrier::Reserved {
                    self.map[adjacent] = Carrier::Pilot {
                        amplitude,
                        inverted: false,
                    };
                }
            }
        }
        self.finish(symbol, 0)?;
        self.active = self.data;
        Ok(())
    }

    pub fn data(&mut self, pre: Pre, symbol: usize) -> Result<(), DecodeError> {
        if pre.preamble.fft()? != self.fft || !(1..=8).contains(&pre.pilots) {
            return Err(DecodeError::Parameters);
        }
        let pattern = usize::from(pre.pilots - 1);
        let (dx, dy) = SCATTERED[pattern];
        let extra = if pre.extended { EXTRA[self.mode] } else { 0 };
        self.carriers = NORMAL[self.mode] + 2 * extra;
        self.map.fill(Carrier::Data);
        let closing =
            has_closing(pre) && symbol + 1 == pre.preamble.p2_symbols()? + pre.data_symbols;
        let amplitude = match pre.pilots {
            1 | 2 => 4.0 / 3.0,
            3 | 4 => 7.0 / 4.0,
            _ => 7.0 / 3.0,
        };
        if !closing {
            self.continual(pattern, dx, pre.extended);
        }
        for k in 0..self.carriers {
            let pilot = if closing {
                k % dx == 0
            } else {
                (k + dx * dy - extra % (dx * dy)) % (dx * dy) == dx * (symbol % dy)
            };
            if pilot || k == 0 || k + 1 == self.carriers {
                let inverted = if k == 0 || k + 1 == self.carriers {
                    symbol % 2 == 1
                } else {
                    k / dx % 2 == 1
                };
                self.map[k] = Carrier::Pilot {
                    amplitude,
                    inverted,
                };
            }
        }
        if closing
            && ((self.fft == 1024 && matches!(pre.pilots, 4 | 5))
                || (self.fft == 2048 && pre.pilots == 7))
        {
            let k = self.carriers - 2;
            self.map[k] = Carrier::Pilot {
                amplitude,
                inverted: k / dx % 2 == 1,
            };
        }
        if pre.papr >= 2 {
            let tones = if closing {
                P2_TONES[self.mode]
            } else {
                DATA_TONES[self.mode]
            };
            let shift = if closing {
                extra
            } else {
                dx * ((symbol + extra / dx) % dy)
            };
            for &tone in tones {
                let carrier = self
                    .map
                    .get_mut(tone + shift)
                    .ok_or(DecodeError::Parameters)?;
                *carrier = Carrier::Reserved;
            }
        }
        self.finish(symbol, EXTRA[self.mode] - extra)?;
        self.active = if closing {
            let row = if self.mode < 3 {
                self.mode
            } else {
                3 + (self.mode - 3) * 2 + usize::from(pre.extended)
            };
            CLOSING_ACTIVE[row][pattern]
                .checked_sub(if pre.papr >= 2 {
                    P2_TONES[self.mode].len()
                } else {
                    0
                })
                .ok_or(DecodeError::Parameters)?
        } else {
            self.data
        };
        if self.active == 0 || self.active > self.data {
            return Err(DecodeError::Parameters);
        }
        Ok(())
    }

    fn continual(&mut self, pattern: usize, dx: usize, extended: bool) {
        let modulus = if self.mode == 0 {
            1632
        } else {
            1632 << (self.mode - 1)
        };
        let amplitude = match self.mode {
            0 | 1 => 4.0 / 3.0,
            2 => 4.0 * 2.0_f32.sqrt() / 3.0,
            _ => 8.0 / 3.0,
        };
        for group in &CONTINUAL[pattern][..self.mode + 1] {
            for &index in *group {
                let k = if self.mode == 5 {
                    index
                } else {
                    index % modulus
                };
                if k < self.carriers {
                    self.map[k] = Carrier::Pilot {
                        amplitude,
                        inverted: k % dx == 0 && k / dx % 2 == 1,
                    };
                }
            }
        }
        if extended && self.mode >= 3 {
            for &k in EXTENDED[pattern][self.mode - 3] {
                self.map[k] = Carrier::Pilot {
                    amplitude,
                    inverted: k % dx == 0 && k / dx % 2 == 1,
                };
            }
        }
    }

    fn finish(&mut self, symbol: usize, offset: usize) -> Result<(), DecodeError> {
        let byte = *PN_SEQUENCE_TABLE
            .get(symbol / 8)
            .ok_or(DecodeError::Parameters)?;
        let pn = byte >> (7 - symbol % 8) & 1 != 0;
        self.data = 0;
        for k in 0..self.carriers {
            self.pilots[k] = match self.map[k] {
                Carrier::Pilot { amplitude, .. } => {
                    if self.prbs[k + offset] ^ pn {
                        -amplitude
                    } else {
                        amplitude
                    }
                }
                Carrier::Data => {
                    self.data += 1;
                    0.0
                }
                Carrier::Reserved => 0.0,
            };
        }
        Ok(())
    }

    pub fn deinterleave<T: Copy>(
        &self,
        input: &[T],
        symbol: usize,
        output: &mut [T],
    ) -> Result<(), DecodeError> {
        if input.len() != self.data || output.len() < self.data {
            return Err(DecodeError::Length);
        }
        let mut index = 0;
        for &address in &self.permutations[symbol % 2] {
            if address < self.data {
                if self.fft == 32768 && symbol.is_multiple_of(2) {
                    output[index] = input[address];
                } else {
                    output[address] = input[index];
                }
                index += 1;
            }
        }
        Ok(())
    }
}

fn permutation(fft: usize, mode: usize, parity: usize) -> Vec<usize> {
    let degree = fft.ilog2() as usize - 1;
    let mut state = 0;
    (0..fft)
        .map(|i| {
            state = match i {
                0 | 1 => 0,
                2 => 1,
                _ => {
                    (state >> 1)
                        | (TAPS[mode].iter().fold(0, |v, &tap| v ^ (state >> tap & 1))
                            << (degree - 1))
                }
            };
            PERMUTATIONS[mode][parity]
                .iter()
                .enumerate()
                .fold((i % 2) * fft / 2, |v, (bit, &position)| {
                    v | ((state >> bit & 1) << position)
                })
        })
        .collect()
}

pub fn has_closing(pre: Pre) -> bool {
    if pre.pilots == 8 {
        return false;
    }
    if pre.preamble.miso() {
        return true;
    }
    match pre.guard_code {
        0 => pre.pilots == 7 || (pre.pilots == 6 && pre.preamble.fft().is_ok_and(|n| n >= 16384)),
        1 | 6 => matches!(pre.pilots, 4 | 5),
        2 | 5 => matches!(pre.pilots, 2 | 3),
        3 => pre.pilots == 1,
        _ => false,
    }
}
