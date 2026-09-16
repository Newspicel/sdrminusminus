use super::{
    frame::{Constellation, ModCod, Modulation},
    ldpc::{Frame, Rate},
};

#[allow(clippy::approx_constant)]
mod points;
pub mod tables;

pub struct Mode {
    pub code: u8,
    pub short: bool,
    pub modulation: Modulation,
    pub rate: Rate,
    pub order: &'static [usize],
    pub points: &'static [(f32, f32)],
}

pub fn mode(code: u8) -> Option<&'static Mode> {
    points::MODES.iter().find(|mode| mode.code == code)
}

impl Mode {
    pub fn modcod(&self) -> ModCod {
        ModCod {
            index: self.code,
            modulation: self.modulation,
            rate: self.rate,
        }
    }

    pub fn constellation(&self) -> Constellation {
        Constellation::from_points(self.points)
    }

    pub fn deinterleave(&self, symbols: &[f32]) -> Vec<f32> {
        let length = Frame::of(self.short).length();
        if self.modulation == Modulation::Qpsk {
            return symbols[..length].to_vec();
        }
        let rows = length.div_ceil(self.order.len());
        let mut out = vec![0.0; length];
        for row in 0..rows {
            for (position, &column) in self.order.iter().enumerate() {
                let bit = column * rows + row;
                if bit < length {
                    out[bit] = symbols[row * self.order.len() + position];
                }
            }
        }
        out
    }

    #[cfg(any(test, feature = "test-signals"))]
    pub fn interleave(&self, coded: &[bool]) -> Vec<bool> {
        if self.modulation == Modulation::Qpsk {
            return coded.to_vec();
        }
        let rows = coded.len().div_ceil(self.order.len());
        let size = rows.div_ceil(90) * 90 * self.order.len();
        let mut out = vec![true; size];
        for row in 0..rows {
            for (position, &column) in self.order.iter().enumerate() {
                out[row * self.order.len() + position] =
                    coded.get(column * rows + row).copied().unwrap_or(false);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests;

pub fn reserved_symbols(code: u8) -> Option<usize> {
    Some(match code {
        128 => 21_690,
        130 => 16_290,
        176 => 13_050,
        177 | 252 => 13_338,
        188 | 192 | 196 => 10_890,
        189 | 193 | 197 | 253 => 11_142,
        250 => 22_194,
        251 => 16_686,
        254 => 8_370,
        255 => 6_714,
        _ => return None,
    })
}
