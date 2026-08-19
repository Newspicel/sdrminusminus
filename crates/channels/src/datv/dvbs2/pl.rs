use num_complex::Complex;

pub const HEADER: usize = 90;
pub const SLOT: usize = 90;
pub const PILOT_PERIOD: usize = 16;
pub const PILOT_LENGTH: usize = 36;

const SOF: [bool; 26] = [
    false, true, true, false, false, false, true, true, false, true, false, false, true, false,
    true, true, true, false, true, false, false, false, false, false, true, false,
];

const GENERATORS: [u32; 7] = [
    0x90AC_2DDD,
    0x5555_5555,
    0x3333_3333,
    0x0F0F_0F0F,
    0x00FF_00FF,
    0x0000_FFFF,
    0xFFFF_FFFF,
];

const SCRAMBLE: [bool; 64] = {
    let raw: [u8; 64] = [
        0, 1, 1, 1, 0, 0, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1, 1, 0, 0, 0, 0, 0, 1, 1, 1, 1, 0, 0, 1, 0,
        0, 1, 0, 1, 0, 1, 0, 0, 1, 1, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 1, 1, 0, 1, 1, 1, 1, 1,
        1, 0, 1, 0,
    ];
    let mut out = [false; 64];
    let mut index = 0;
    while index < 64 {
        out[index] = raw[index] == 1;
        index += 1;
    }
    out
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signalling {
    pub modcod: u8,
    pub short: bool,
    pub pilots: bool,
}

impl Signalling {
    #[must_use]
    pub const fn code(self) -> u8 {
        self.modcod << 2 | (self.short as u8) << 1 | self.pilots as u8
    }

    #[must_use]
    pub const fn from_code(code: u8) -> Self {
        Self {
            modcod: code >> 2,
            short: code >> 1 & 1 == 1,
            pilots: code & 1 == 1,
        }
    }
}

#[must_use]
pub fn signalling_bits(signalling: Signalling) -> [bool; 64] {
    let code = signalling.code();
    let mut word = 0u32;
    for (index, &generator) in GENERATORS.iter().take(7).enumerate() {
        if code >> (7 - index) & 1 == 1 {
            word ^= generator;
        }
    }
    let mut out = [false; 64];
    for step in 0..32 {
        let bit = word >> (31 - step) & 1 == 1;
        out[2 * step] = bit ^ SCRAMBLE[2 * step];
        out[2 * step + 1] = (bit ^ (code & 1 == 1)) ^ SCRAMBLE[2 * step + 1];
    }
    out
}

#[must_use]
pub fn bpsk(index: usize, bit: bool) -> Complex<f32> {
    let amplitude = std::f32::consts::FRAC_1_SQRT_2;
    let (real, imaginary) = match (index.is_multiple_of(2), bit) {
        (true, false) => (amplitude, amplitude),
        (true, true) => (-amplitude, -amplitude),
        (false, false) => (-amplitude, amplitude),
        (false, true) => (amplitude, -amplitude),
    };
    Complex::new(real, imaginary)
}

pub fn header(signalling: Signalling, out: &mut Vec<Complex<f32>>) {
    let coded = signalling_bits(signalling);
    for (index, &bit) in SOF.iter().enumerate() {
        out.push(bpsk(index, bit));
    }
    for (step, &bit) in coded.iter().enumerate() {
        out.push(bpsk(SOF.len() + step, bit));
    }
}

#[must_use]
pub fn correlate_header(symbols: &[Complex<f32>]) -> Option<(f32, Signalling, Complex<f32>)> {
    if symbols.len() < HEADER {
        return None;
    }
    let mut sum = Complex::new(0.0f32, 0.0);
    let mut energy = 0.0f32;
    for (index, &bit) in SOF.iter().enumerate() {
        sum += symbols[index] * bpsk(index, bit).conj();
        energy += symbols[index].norm_sqr();
    }
    let coherence = sum.norm() / energy.max(1e-12).sqrt() / (SOF.len() as f32).sqrt();
    let reference = sum / sum.norm().max(1e-12);
    let mut best = (f32::NEG_INFINITY, 0u8);
    for code in 0..128u8 {
        let candidate = signalling_bits(Signalling::from_code(code));
        let score: f32 = candidate
            .iter()
            .enumerate()
            .map(|(step, &bit)| {
                let expected = bpsk(SOF.len() + step, bit) * reference;
                (symbols[SOF.len() + step] * expected.conj()).re
            })
            .sum();
        if score > best.0 {
            best = (score, code);
        }
    }
    Some((coherence, Signalling::from_code(best.1), reference))
}

pub struct Scrambler {
    x: u32,
    y: u32,
}

impl Scrambler {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            x: 0x0_0001,
            y: 0x3_FFFF,
        }
    }

    pub fn reset(&mut self) {
        self.x = 0x0_0001;
        self.y = 0x3_FFFF;
    }

    fn parity(value: u32, mask: u32) -> bool {
        (value & mask).count_ones() % 2 == 1
    }

    pub fn next(&mut self) -> u8 {
        let xa = Self::parity(self.x, 0x8050);
        let xb = Self::parity(self.x, 0x0081);
        let xc = self.x & 1 == 1;
        self.x >>= 1;
        if xb {
            self.x |= 0x2_0000;
        }
        let ya = Self::parity(self.y, 0x04A1);
        let yb = Self::parity(self.y, 0xFF60);
        let yc = self.y & 1 == 1;
        self.y >>= 1;
        if ya {
            self.y |= 0x2_0000;
        }
        u8::from(xa ^ yb) << 1 | u8::from(xc ^ yc)
    }

    pub fn scramble(&mut self, symbols: &mut [Complex<f32>]) {
        for symbol in symbols {
            *symbol = rotate(*symbol, self.next());
        }
    }

    pub fn descramble(&mut self, symbols: &mut [Complex<f32>]) {
        for symbol in symbols {
            *symbol = rotate(*symbol, 4 - self.next());
        }
    }
}

impl Default for Scrambler {
    fn default() -> Self {
        Self::new()
    }
}

fn rotate(symbol: Complex<f32>, quarters: u8) -> Complex<f32> {
    match quarters % 4 {
        0 => symbol,
        1 => Complex::new(-symbol.im, symbol.re),
        2 => -symbol,
        _ => Complex::new(symbol.im, -symbol.re),
    }
}

#[must_use]
pub fn pilot_symbol() -> Complex<f32> {
    Complex::new(
        std::f32::consts::FRAC_1_SQRT_2,
        std::f32::consts::FRAC_1_SQRT_2,
    )
}

#[must_use]
pub fn pilot_blocks(slots: usize) -> usize {
    if slots == 0 {
        0
    } else {
        (slots - 1) / PILOT_PERIOD
    }
}

#[must_use]
pub fn frame_symbols(slots: usize, pilots: bool) -> usize {
    HEADER
        + slots * SLOT
        + if pilots {
            pilot_blocks(slots) * PILOT_LENGTH
        } else {
            0
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_start_of_frame_word_matches_the_documented_constant() {
        let value = SOF
            .iter()
            .fold(0u32, |word, &bit| word << 1 | u32::from(bit));
        assert_eq!(value, 0x018D_2E82);
    }

    #[test]
    fn signalling_round_trips_through_its_code() {
        for modcod in 1..32u8 {
            for short in [false, true] {
                for pilots in [false, true] {
                    let signalling = Signalling {
                        modcod,
                        short,
                        pilots,
                    };
                    assert_eq!(Signalling::from_code(signalling.code()), signalling);
                }
            }
        }
    }

    #[test]
    fn a_clean_header_is_recognised_and_read_back() {
        for modcod in [4u8, 7, 11, 13, 17] {
            for short in [false, true] {
                for pilots in [false, true] {
                    let signalling = Signalling {
                        modcod,
                        short,
                        pilots,
                    };
                    let mut symbols = Vec::new();
                    header(signalling, &mut symbols);
                    assert_eq!(symbols.len(), HEADER);
                    let (coherence, read, _) = correlate_header(&symbols).expect("a full header");
                    assert!(coherence > 0.99, "{coherence}");
                    assert_eq!(read, signalling);
                }
            }
        }
    }

    #[test]
    fn a_rotated_header_still_reads_its_signalling() {
        let signalling = Signalling {
            modcod: 7,
            short: false,
            pilots: true,
        };
        let mut symbols = Vec::new();
        header(signalling, &mut symbols);
        for quarters in 0..4u8 {
            let turned: Vec<Complex<f32>> = symbols
                .iter()
                .map(|&value| rotate(value, quarters))
                .collect();
            let (coherence, read, reference) = correlate_header(&turned).expect("a full header");
            assert!((reference.norm() - 1.0).abs() < 1e-3);
            assert!(coherence > 0.99, "turn {quarters}: {coherence}");
            assert_eq!(read, signalling, "turn {quarters}");
        }
    }

    #[test]
    fn noise_does_not_correlate_with_the_start_of_frame() {
        let mut state = 0x2468_ace0u32;
        let noise: Vec<Complex<f32>> = (0..HEADER)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                Complex::new(
                    (state >> 16) as f32 / 32_768.0 - 1.0,
                    (state & 0xFFFF) as f32 / 32_768.0 - 1.0,
                )
            })
            .collect();
        let (coherence, ..) = correlate_header(&noise).expect("a full block");
        assert!(coherence < 0.6, "{coherence}");
    }

    #[test]
    fn scrambling_is_undone_by_descrambling() {
        let original: Vec<Complex<f32>> = (0..1_000)
            .map(|index| Complex::from_polar(1.0, index as f32 * 0.37))
            .collect();
        let mut symbols = original.clone();
        Scrambler::new().scramble(&mut symbols);
        assert!(
            symbols
                .iter()
                .zip(&original)
                .any(|(a, b)| (a - b).norm() > 0.1)
        );
        Scrambler::new().descramble(&mut symbols);
        for (restored, source) in symbols.iter().zip(&original) {
            assert!((restored - source).norm() < 1e-5);
        }
    }

    #[test]
    fn a_pilot_block_follows_every_sixteen_slots() {
        assert_eq!(pilot_blocks(16), 0);
        assert_eq!(pilot_blocks(17), 1);
        assert_eq!(pilot_blocks(32), 1);
        assert_eq!(pilot_blocks(33), 2);
        assert_eq!(frame_symbols(360, false), HEADER + 360 * SLOT);
        assert_eq!(
            frame_symbols(360, true),
            HEADER + 360 * SLOT + 22 * PILOT_LENGTH
        );
    }
}
