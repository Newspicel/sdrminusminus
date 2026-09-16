use sdrmm_wire::DatvCodeRate;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parameters {
    pub fft: usize,
    pub guard: usize,
    pub bits: usize,
    pub alpha: usize,
    pub hierarchical: bool,
    pub high_rate: DatvCodeRate,
    pub low_rate: DatvCodeRate,
    pub frame: u8,
    pub cell: u8,
}

impl Parameters {
    pub fn same_modulation(self, other: Self) -> bool {
        Self {
            frame: 0,
            cell: 0,
            ..self
        } == Self {
            frame: 0,
            cell: 0,
            ..other
        }
    }
}

pub fn remainder(bits: u128, length: usize) -> u16 {
    let mut register = 0u16;
    for i in (0..length).rev() {
        register = (register << 1) | ((bits >> i) & 1) as u16;
        if register & 0x4000 != 0 {
            register ^= 0x4377;
        }
    }
    register
}

fn repair(mut word: u128) -> Option<u128> {
    let syndrome = remainder(word, 67);
    if syndrome == 0 {
        return Some(word);
    }
    let syndromes: [u16; 67] = std::array::from_fn(|i| remainder(1u128 << i, 67));
    for i in 0..67 {
        if syndromes[i] == syndrome {
            return Some(word ^ (1u128 << i));
        }
        for j in 0..i {
            if syndromes[i] ^ syndromes[j] == syndrome {
                word ^= (1u128 << i) | (1u128 << j);
                return Some(word);
            }
        }
    }
    None
}

#[derive(Default)]
pub struct Tps {
    word: u128,
    count: usize,
    pub bad: u32,
}

impl Tps {
    pub fn push(&mut self, bit: bool) -> Option<Parameters> {
        self.word = ((self.word << 1) | u128::from(bit)) & ((1u128 << 67) - 1);
        self.count = (self.count + 1).min(67);
        let sync = (self.word >> 51) as u16;
        if self.count < 67
            || (sync ^ 0x35ee)
                .count_ones()
                .min((sync ^ 0xca11).count_ones())
                > 2
        {
            return None;
        }
        let Some(word) = repair(self.word) else {
            self.bad = self.bad.saturating_add(1);
            return None;
        };
        let value = |start: usize, length: usize| {
            ((word >> (68 - start - length)) & ((1 << length) - 1)) as usize
        };
        let frame = value(23, 2) as u8;
        if value(1, 16) != if frame & 1 == 0 { 0x35ee } else { 0xca11 }
            || !matches!(value(17, 6), 23 | 31)
        {
            return None;
        }
        let rates = [
            DatvCodeRate::Half,
            DatvCodeRate::TwoThirds,
            DatvCodeRate::ThreeQuarters,
            DatvCodeRate::FiveSixths,
            DatvCodeRate::SevenEighths,
        ];
        let fft = match value(38, 2) {
            0 => 2048,
            1 => 8192,
            _ => return None,
        };
        let hierarchy = value(27, 3);
        if hierarchy > 3 {
            return None;
        }
        Some(Parameters {
            fft,
            guard: fft / (32 >> value(36, 2)),
            bits: *[2, 4, 6].get(value(25, 2))?,
            alpha: if hierarchy == 0 {
                1
            } else {
                1 << (hierarchy - 1)
            },
            hierarchical: hierarchy != 0,
            high_rate: *rates.get(value(30, 3))?,
            low_rate: *rates.get(value(33, 3))?,
            frame,
            cell: value(40, 8) as u8,
        })
    }
}

#[cfg(any(test, feature = "test-signals"))]
pub fn encode(params: Parameters) -> [bool; 68] {
    let mut word = 0u128;
    let mut put = |start: usize, length: usize, value: usize| {
        word |= (value as u128) << (68 - start - length);
    };
    put(
        1,
        16,
        if params.frame & 1 == 0 {
            0x35ee
        } else {
            0xca11
        },
    );
    put(17, 6, 31);
    put(23, 2, usize::from(params.frame));
    put(25, 2, params.bits / 2 - 1);
    put(
        27,
        3,
        if params.hierarchical {
            params.alpha.ilog2() as usize + 1
        } else {
            0
        },
    );
    let rate = |rate| match rate {
        DatvCodeRate::Half | DatvCodeRate::Auto => 0,
        DatvCodeRate::TwoThirds => 1,
        DatvCodeRate::ThreeQuarters => 2,
        DatvCodeRate::FiveSixths => 3,
        DatvCodeRate::SevenEighths => 4,
    };
    put(30, 3, rate(params.high_rate));
    put(33, 3, rate(params.low_rate));
    put(36, 2, (32 * params.guard / params.fft).ilog2() as usize);
    put(38, 2, usize::from(params.fft == 8192));
    put(40, 8, usize::from(params.cell));
    word |= u128::from(remainder(word, 67));
    std::array::from_fn(|i| word & (1u128 << (67 - i)) != 0)
}
