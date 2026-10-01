use sdrmm_modem::ppm::PpmDemod;

use super::{PHASE_TABLES, PREAMBLE_PULSES};

pub(super) type Phases = u8;

const _: () = assert!(PHASE_TABLES <= Phases::BITS as usize);

const TILE: usize = 512;
const SCREENED_GAPS: [usize; 4] = [4, 5, 12, 14];

struct Stream {
    weights: Vec<f32>,
    reach: usize,
    values: Vec<f32>,
}

impl Stream {
    fn fill(&mut self, mag: &[f32], from: usize, n: usize) -> Option<()> {
        let len = n + self.reach;
        let samples = mag.get(from..from + len + self.weights.len().saturating_sub(1))?;
        let values = self.values.get_mut(..len)?;
        match *self.weights.as_slice() {
            [w0, w1] => {
                for ((v, &x0), &x1) in values.iter_mut().zip(samples).zip(&samples[1..]) {
                    *v = w0 * x0 + w1 * x1;
                }
            }
            [w0, w1, w2] => {
                for (((v, &x0), &x1), &x2) in values
                    .iter_mut()
                    .zip(samples)
                    .zip(&samples[1..])
                    .zip(&samples[2..])
                {
                    *v = w0 * x0 + w1 * x1 + w2 * x2;
                }
            }
            _ => {
                values.fill(0.0);
                for (k, &w) in self.weights.iter().enumerate() {
                    for (v, &x) in values.iter_mut().zip(&samples[k..]) {
                        *v += w * x;
                    }
                }
            }
        }
        Some(())
    }
}

#[derive(Clone, Copy)]
struct ChipRef {
    stream: usize,
    start: usize,
}

struct PhaseChips {
    pulses: [ChipRef; PREAMBLE_PULSES.len()],
    gaps: [ChipRef; SCREENED_GAPS.len()],
}

impl PhaseChips {
    fn mark(&self, streams: &[Stream], phase: usize, hits: &mut [Phases]) {
        let (Some(p), Some(g)) = (tiles(&self.pulses, streams), tiles(&self.gaps, streams)) else {
            hits.iter_mut().for_each(|hit| *hit |= 1 << phase);
            return;
        };
        let mut passed = [0; TILE];
        for (i, pass) in passed.iter_mut().enumerate() {
            let level = |chip: &[f32; TILE]| chip[i].to_bits();
            let weakest = level(p[0])
                .min(level(p[1]))
                .min(level(p[2]))
                .min(level(p[3]));
            let loudest_gap = level(g[0])
                .max(level(g[1]))
                .max(level(g[2]))
                .max(level(g[3]));
            let [weakest, loudest_gap] = [weakest, loudest_gap].map(f32::from_bits);
            let threshold = (p[0][i] + p[1][i] + p[2][i] + p[3][i]) * 0.25 * 0.5;
            let preamble = weakest > threshold && loudest_gap < threshold;
            *pass = Phases::from(preamble) << phase;
        }
        for (hit, pass) in hits.iter_mut().zip(passed) {
            *hit |= pass;
        }
    }
}

fn tiles<'a, const N: usize>(
    chips: &[ChipRef; N],
    streams: &'a [Stream],
) -> Option<[&'a [f32; TILE]; N]> {
    let mut out = [&[0.0; TILE]; N];
    for (tile, chip) in out.iter_mut().zip(chips) {
        *tile = streams
            .get(chip.stream)?
            .values
            .get(chip.start..chip.start + TILE)?
            .try_into()
            .ok()?;
    }
    Some(out)
}

pub(super) struct PreambleScreen {
    streams: Vec<Stream>,
    phases: Vec<PhaseChips>,
    hits: Vec<Phases>,
}

impl PreambleScreen {
    pub(super) fn new(receivers: &[PpmDemod]) -> Self {
        let mut streams: Vec<Stream> = Vec::new();
        let phases = receivers
            .iter()
            .map(|receiver| {
                let mut chip = |slot| share(&mut streams, receiver, slot);
                PhaseChips {
                    pulses: PREAMBLE_PULSES.map(&mut chip),
                    gaps: SCREENED_GAPS.map(&mut chip),
                }
            })
            .collect();
        for stream in &mut streams {
            stream.values = vec![0.0; TILE + stream.reach];
        }
        Self {
            streams,
            phases,
            hits: Vec::new(),
        }
    }

    pub(super) fn scan(&mut self, mag: &[f32], positions: usize) {
        self.hits.clear();
        self.hits.resize(positions, 0);
        for (tile, hits) in self.hits.chunks_mut(TILE).enumerate() {
            let from = tile * TILE;
            if self
                .streams
                .iter_mut()
                .any(|stream| stream.fill(mag, from, hits.len()).is_none())
            {
                hits.fill(Phases::MAX);
                continue;
            }
            for (phase, chips) in self.phases.iter().enumerate() {
                chips.mark(&self.streams, phase, hits);
            }
        }
    }

    pub(super) fn next(&self, from: usize) -> Option<(usize, Phases)> {
        let hits = self.hits.get(from..)?;
        let skip = hits.iter().position(|&hit| hit != 0)?;
        Some((from + skip, hits[skip]))
    }
}

fn share(streams: &mut Vec<Stream>, receiver: &PpmDemod, slot: usize) -> ChipRef {
    let (start, weights) = receiver.grid().slot_weights(slot);
    let index = streams
        .iter()
        .position(|stream| stream.weights == weights)
        .unwrap_or_else(|| {
            streams.push(Stream {
                weights: weights.to_vec(),
                reach: 0,
                values: Vec::new(),
            });
            streams.len() - 1
        });
    if let Some(stream) = streams.get_mut(index) {
        stream.reach = stream.reach.max(start);
    }
    ChipRef {
        stream: index,
        start,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        adsb::{INPUT_RATE_HZ, phase_tables, preamble_ok},
        synth::{
            add_noise,
            adsb::{me_identification, squitter, transmission_at_phase},
        },
    };
    use sdrmm_modem::ppm::magnitudes;

    #[test]
    fn every_position_a_phase_accepts_survives_the_screen() {
        let receivers = phase_tables(INPUT_RATE_HZ);
        let span = receivers.iter().map(|r| r.grid().span()).max().unwrap();
        let mut screen = PreambleScreen::new(&receivers);
        let frames: Vec<Vec<u8>> = (0..40)
            .map(|k| squitter(0xA0_0000 + k, me_identification("SCREEN")))
            .collect();
        for (seed, amp) in [(1, 0.02), (2, 0.15), (3, 0.3)] {
            let phase = f64::from(seed) / 7.0;
            let mut iq = transmission_at_phase(&frames, 40.0, 0.5, INPUT_RATE_HZ, phase);
            add_noise(&mut iq, seed, amp);
            let mut mag = Vec::new();
            magnitudes(&iq, &mut mag);
            let positions = mag.len() + 1 - span;
            screen.scan(&mag, positions);
            let mut accepted = 0;
            let mut screened = 0;
            for at in 0..positions {
                let kept = match screen.next(at) {
                    Some((hit, phases)) if hit == at => phases,
                    _ => 0,
                };
                for (phase, r) in receivers.iter().enumerate() {
                    let exact = preamble_ok(r, &mag[at..at + r.grid().span()]);
                    assert!(
                        !exact || kept >> phase & 1 == 1,
                        "seed {seed}: {at} lost phase {phase}"
                    );
                    accepted += usize::from(exact);
                }
                screened += usize::from(kept != 0);
            }
            assert!(accepted > 0, "seed {seed}: no preamble");
            assert!(
                screened * 50 < positions,
                "seed {seed}: {screened} of {positions} kept"
            );
        }
    }

    #[test]
    fn a_tile_without_samples_is_kept_whole() {
        let receivers = phase_tables(INPUT_RATE_HZ);
        let mut screen = PreambleScreen::new(&receivers);
        screen.scan(&[0.0; 4], 3);
        assert_eq!(screen.next(0), Some((0, Phases::MAX)));
        assert_eq!(screen.next(2), Some((2, Phases::MAX)));
        assert_eq!(screen.next(3), None);
    }
}
