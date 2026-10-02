use num_complex::Complex;

use super::{
    coding::{Plan, Qam, higher_cells, permutation, rates_for},
    sdc::Multiplex,
};

const CELL_INTERLEAVER: usize = 5;

#[derive(Clone, Copy, Debug, Default)]
pub struct Cell {
    pub value: Complex<f32>,
    pub weight: f32,
}

pub struct CellDeinterleaver {
    cells: usize,
    depth: usize,
    order: Vec<usize>,
    slots: Vec<Cell>,
    received: usize,
}

impl CellDeinterleaver {
    #[must_use]
    pub fn new(cells: usize, depth: usize) -> Self {
        let mut order = Vec::with_capacity(cells);
        permutation(cells, CELL_INTERLEAVER, &mut order);
        Self {
            cells,
            depth: depth.max(1),
            order,
            slots: vec![Cell::default(); cells * depth.max(1)],
            received: 0,
        }
    }

    #[must_use]
    pub const fn matches(&self, cells: usize, depth: usize) -> bool {
        self.cells == cells && self.depth == depth
    }

    pub fn push(&mut self, frame: &[Cell], out: &mut Vec<Cell>) -> bool {
        if frame.len() != self.cells {
            self.received = 0;
            return false;
        }
        let depth = self.depth;
        let current = self.received;
        for (index, &cell) in frame.iter().enumerate() {
            let original = current + depth - 1 - index % depth;
            let slot = original % depth;
            self.slots[slot * self.cells + self.order[index]] = cell;
        }
        self.received += 1;
        if self.received < depth {
            return false;
        }
        let slot = (current + depth - 1 - (depth - 1)) % depth;
        out.clear();
        out.extend_from_slice(&self.slots[slot * self.cells..(slot + 1) * self.cells]);
        true
    }
}

#[cfg(any(test, feature = "synth"))]
pub struct CellInterleaver {
    cells: usize,
    depth: usize,
    order: Vec<usize>,
    history: std::collections::VecDeque<Vec<Complex<f32>>>,
}

#[cfg(any(test, feature = "synth"))]
impl CellInterleaver {
    #[must_use]
    pub fn new(cells: usize, depth: usize) -> Self {
        let mut order = Vec::with_capacity(cells);
        permutation(cells, CELL_INTERLEAVER, &mut order);
        let depth = depth.max(1);
        let mut state = 0x2545_F491u32;
        let mut filler = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let sign = |bit: u32| if state >> bit & 1 == 1 { 1.0 } else { -1.0 };
            Complex::new(sign(3), sign(11)) * std::f32::consts::FRAC_1_SQRT_2
        };
        Self {
            cells,
            depth,
            order,
            history: (0..depth)
                .map(|_| (0..cells).map(|_| filler()).collect())
                .collect(),
        }
    }

    pub fn push(&mut self, frame: Vec<Complex<f32>>) -> Vec<Complex<f32>> {
        self.history.pop_front();
        self.history.push_back(frame);
        (0..self.cells)
            .map(|index| {
                let back = index % self.depth;
                self.history[self.depth - 1 - back][self.order[index]]
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MscConfig {
    pub qam: Qam,
    pub plus: bool,
    pub cells: usize,
}

#[must_use]
pub fn plan(config: MscConfig, multiplex: &Multiplex) -> Option<Plan> {
    let levels = config.qam.levels();
    let lower = rates_for(config.qam, config.plus, multiplex.protection_lower)?;
    let higher_bytes = multiplex.higher_bytes();
    if higher_bytes == 0 {
        return Plan::eep(config.qam, config.cells, &lower[..levels]);
    }
    let higher = rates_for(config.qam, config.plus, multiplex.protection_higher)?;
    let higher_cells = higher_cells(higher_bytes, &higher[..levels]);
    if higher_cells + 20 > config.cells {
        return None;
    }
    let plan = Plan::protected(
        config.qam,
        higher_cells,
        config.cells - higher_cells,
        Some(&higher[..levels]),
        &lower[..levels],
    )?;
    (plan.bits() / 8 >= higher_bytes + multiplex.lower_bytes()).then_some(plan)
}

#[must_use]
pub fn stream<'a>(
    frame: &'a [u8],
    multiplex: &Multiplex,
    index: usize,
    out: &'a mut Vec<u8>,
) -> Option<&'a [u8]> {
    let higher_total = multiplex.higher_bytes();
    let target = multiplex.streams.get(index)?;
    let higher_start: usize = multiplex.streams[..index]
        .iter()
        .map(|stream| usize::from(stream.higher))
        .sum();
    let lower_start: usize = higher_total
        + multiplex.streams[..index]
            .iter()
            .map(|stream| usize::from(stream.lower))
            .sum::<usize>();
    out.clear();
    out.extend_from_slice(frame.get(higher_start..higher_start + usize::from(target.higher))?);
    out.extend_from_slice(frame.get(lower_start..lower_start + usize::from(target.lower))?);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drm::sdc::Stream;

    #[test]
    fn long_interleaving_restores_every_frame_after_its_delay() {
        let cells = 1259;
        for depth in [1, 5, 6] {
            let mut interleaver = CellInterleaver::new(cells, depth);
            let mut deinterleaver = CellDeinterleaver::new(cells, depth);
            let mut out = Vec::new();
            let mut restored = 0;
            for frame in 0..12 {
                let sent: Vec<Complex<f32>> = (0..cells)
                    .map(|i| Complex::new(frame as f32, i as f32))
                    .collect();
                let air: Vec<Cell> = interleaver
                    .push(sent)
                    .into_iter()
                    .map(|value| Cell { value, weight: 1.0 })
                    .collect();
                if deinterleaver.push(&air, &mut out) {
                    let expected = (frame + 1 - depth) as f32;
                    assert!(
                        out.iter()
                            .enumerate()
                            .all(|(i, cell)| { cell.value == Complex::new(expected, i as f32) })
                    );
                    restored += 1;
                }
            }
            assert_eq!(restored, 13 - depth);
        }
    }

    #[test]
    fn streams_join_their_protected_parts() {
        let multiplex = Multiplex {
            protection_higher: 0,
            protection_lower: 1,
            streams: vec![
                Stream {
                    higher: 2,
                    lower: 3,
                },
                Stream {
                    higher: 1,
                    lower: 2,
                },
            ],
        };
        let frame = [10, 11, 20, 12, 13, 14, 21, 22, 0];
        let mut out = Vec::new();
        assert_eq!(
            stream(&frame, &multiplex, 1, &mut out),
            Some(&[20, 21, 22][..])
        );
        assert_eq!(
            stream(&frame, &multiplex, 0, &mut out),
            Some(&[10, 11, 12, 13, 14][..])
        );
    }

    #[test]
    fn unequal_protection_plans_carry_the_multiplex() {
        let multiplex = Multiplex {
            protection_higher: 0,
            protection_lower: 1,
            streams: vec![Stream {
                higher: 40,
                lower: 900,
            }],
        };
        let config = MscConfig {
            qam: Qam::Q64,
            plus: false,
            cells: 2959,
        };
        let plan = plan(config, &multiplex).expect("a plan");
        assert!(plan.higher_bits() >= 320);
    }
}
