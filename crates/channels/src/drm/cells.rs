use std::f32::consts::{PI, SQRT_2};

use num_complex::Complex;

use super::mode::Robustness;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cell {
    Unused,
    Pilot { value: Complex<f32>, gain: bool },
    Fac,
    Sdc,
    Msc,
}

#[derive(Clone, Debug)]
pub struct Layout {
    pub mode: Robustness,
    pub occupancy: u8,
    pub low: i32,
    pub high: i32,
    cells: Vec<Cell>,
    pub sdc_cells: usize,
    pub msc_cells: usize,
}

fn unit(phase: u16, amplitude: f32) -> Complex<f32> {
    Complex::from_polar(amplitude, 2.0 * PI * f32::from(phase) / 1024.0)
}

fn pilot(mode: Robustness, occupancy: u8, frame: usize, symbol: usize, k: i32) -> Option<Cell> {
    let fixed = mode
        .frequency_phase(symbol, k)
        .or_else(|| (symbol == 0).then(|| mode.time_phase(k)).flatten());
    if mode.is_gain_ref(symbol, k) {
        let amplitude = if mode.boosted(occupancy).contains(&k) {
            2.0
        } else {
            SQRT_2
        };
        let phase = fixed.unwrap_or_else(|| mode.gain_phase(symbol, k));
        return Some(Cell::Pilot {
            value: unit(phase, amplitude),
            gain: true,
        });
    }
    if let Some(phase) = fixed {
        return Some(Cell::Pilot {
            value: unit(phase, SQRT_2),
            gain: false,
        });
    }
    mode.afs_phase(frame, symbol, k).map(|phase| Cell::Pilot {
        value: unit(phase, 1.0),
        gain: false,
    })
}

impl Layout {
    #[must_use]
    pub fn new(mode: Robustness, occupancy: u8) -> Option<Self> {
        let (low, high) = mode.carriers(occupancy)?;
        let width = (high - low + 1) as usize;
        let mut cells = Vec::with_capacity(mode.frames() * mode.symbols() * width);
        let mut sdc_cells = 0;
        let mut msc_cells = 0;
        for frame in 0..mode.frames() {
            for symbol in 0..mode.symbols() {
                let fac = mode.fac_carriers(symbol);
                for k in low..=high {
                    let cell = if mode.unused(k) {
                        Cell::Unused
                    } else if let Some(pilot) = pilot(mode, occupancy, frame, symbol, k) {
                        pilot
                    } else if frame == 0 && symbol < mode.sdc_symbols() {
                        sdc_cells += 1;
                        Cell::Sdc
                    } else if fac.contains(&k) {
                        Cell::Fac
                    } else {
                        msc_cells += 1;
                        Cell::Msc
                    };
                    cells.push(cell);
                }
            }
        }
        Some(Self {
            mode,
            occupancy,
            low,
            high,
            cells,
            sdc_cells,
            msc_cells,
        })
    }

    #[must_use]
    pub fn width(&self) -> usize {
        (self.high - self.low + 1) as usize
    }

    #[must_use]
    pub fn symbol(&self, frame: usize, symbol: usize) -> &[Cell] {
        let width = self.width();
        let start = (frame * self.mode.symbols() + symbol) * width;
        &self.cells[start..start + width]
    }

    #[must_use]
    pub fn multiplex_cells(&self) -> usize {
        self.msc_cells / self.mode.frames()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_counts_match_the_specification_tables() {
        let expected: [(Robustness, u8, usize, usize, usize); 17] = [
            (Robustness::A, 0, 167, 3778, 1259),
            (Robustness::A, 1, 190, 4268, 1422),
            (Robustness::A, 2, 359, 7897, 2632),
            (Robustness::A, 3, 405, 8877, 2959),
            (Robustness::A, 4, 754, 16394, 5464),
            (Robustness::A, 5, 846, 18354, 6118),
            (Robustness::B, 0, 130, 2900, 966),
            (Robustness::B, 1, 150, 3330, 1110),
            (Robustness::B, 2, 282, 6153, 2051),
            (Robustness::B, 3, 322, 7013, 2337),
            (Robustness::B, 4, 588, 12747, 4249),
            (Robustness::B, 5, 662, 14323, 4774),
            (Robustness::C, 3, 288, 5532, 1844),
            (Robustness::C, 5, 607, 11603, 3867),
            (Robustness::D, 3, 152, 3679, 1226),
            (Robustness::D, 5, 332, 7819, 2606),
            (Robustness::E, 0, 936, 29842, 7460),
        ];
        for (mode, occupancy, sdc, msc, multiplex) in expected {
            let layout = Layout::new(mode, occupancy).expect("a defined occupancy");
            assert_eq!(layout.sdc_cells, sdc, "{mode:?} {occupancy} SDC");
            assert_eq!(layout.msc_cells, msc, "{mode:?} {occupancy} MSC");
            assert_eq!(layout.multiplex_cells(), multiplex, "{mode:?} {occupancy}");
        }
    }

    #[test]
    fn every_frame_carries_all_fac_cells() {
        for mode in Robustness::ALL {
            let layout = Layout::new(mode, mode.widest()).expect("widest occupancy");
            for frame in 0..mode.frames() {
                let count: usize = (0..mode.symbols())
                    .map(|symbol| {
                        layout
                            .symbol(frame, symbol)
                            .iter()
                            .filter(|cell| matches!(cell, Cell::Fac))
                            .count()
                    })
                    .sum();
                assert_eq!(count, mode.fac_cells(), "{mode:?} frame {frame}");
            }
        }
    }
}
