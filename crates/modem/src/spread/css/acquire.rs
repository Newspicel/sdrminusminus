use std::f64::consts::TAU;

use num_complex::Complex;

use super::{CssDemod, MAX_CLOCK_PPM, argmax_bin, wrap};

const INLIER_BINS: f64 = 1.5;

const VOTE_BINS: f64 = 1.0;

const FINE_REACHES: [f64; 3] = [0.0, 0.125, 0.125];

const SIGNIFICANT_SIGMAS: f64 = 3.0;

const SLOPE_REACH: i32 = 3;

const HALF_SAMPLE_GRID: [f64; 2] = [0.0, 0.5];

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Point {
    index: f64,
    value: f64,
    weight: f64,
    residual: f64,
}

#[derive(Clone, Copy, Debug)]
struct Line {
    intercept: f64,
    slope: f64,
}

impl Line {
    fn at(self, index: f64) -> f64 {
        self.intercept + self.slope * index
    }
}

impl CssDemod {
    pub fn estimate_origin(&mut self, iq: &[Complex<f32>], preamble: &[u32]) -> usize {
        self.restart();
        let n = self.params.chips() as f64;
        let first = self.coarse_line(iq, preamble, 0.0);
        let bins = self.coarse_line(iq, preamble, first.slope);
        let (start, drift) = self.coarse_start(iq, preamble, bins);
        let mut lines = (
            Line {
                intercept: start,
                slope: n - drift,
            },
            Line {
                intercept: wrap(bins.intercept + start, n).round(),
                slope: 0.0,
            },
        );
        for reach in FINE_REACHES {
            lines = self.fine_lines(iq, preamble, lines.0, lines.1, reach);
        }
        let slip = self.whole_sample_slip(iq, preamble, lines.0, lines.1);
        lines.0.intercept += slip;
        lines.1.intercept += slip;
        self.settle(preamble.len(), lines.0, lines.1)
    }

    fn coarse_line(&mut self, iq: &[Complex<f32>], preamble: &[u32], drift: f64) -> Line {
        let n = self.params.chips() as f64;
        self.points.clear();
        for (k, &known) in preamble.iter().enumerate() {
            let index = k as f64;
            self.load(iq, index * (n - drift), 0.0, n - drift);
            self.dechirp();
            let bin = argmax_bin(&self.energies);
            let tone = f64::from(bin) + self.fraction(bin as usize);
            self.points.push(Point {
                index,
                value: (tone - f64::from(known) * self.stretch).rem_euclid(n),
                weight: f64::from(self.energies[bin as usize]),
                residual: 0.0,
            });
        }
        let seed = self.hough(n);
        let line = inlier_line(&self.points, seed, n);
        Line {
            intercept: line.intercept,
            slope: line.slope + drift,
        }
    }

    fn hough(&self, n: f64) -> Line {
        let count = self.points.len().max(1) as f64;
        let step = 0.5 / count;
        let reach = (n * MAX_CLOCK_PPM * 1e-6 / step).ceil() as i64;
        let mut best = (
            0usize,
            f64::INFINITY,
            Line {
                intercept: 0.0,
                slope: 0.0,
            },
        );
        for i in -reach..=reach {
            let slope = i as f64 * step;
            for p in &self.points {
                let key = p.value - slope * p.index;
                let votes = self
                    .points
                    .iter()
                    .filter(|q| wrap(q.value - slope * q.index - key, n).abs() <= VOTE_BINS)
                    .count();
                if votes > best.0 || (votes == best.0 && slope.abs() < best.1) {
                    best = (
                        votes,
                        slope.abs(),
                        Line {
                            intercept: key,
                            slope,
                        },
                    );
                }
            }
        }
        best.2
    }

    fn coarse_start(&mut self, iq: &[Complex<f32>], preamble: &[u32], bins: Line) -> (f64, f64) {
        let n = self.params.chips() as f64;
        let step = 0.5 / preamble.len().max(1) as f64;
        let spin = Line {
            intercept: bins.intercept,
            slope: 0.0,
        };
        let mut best = (f64::NEG_INFINITY, 0.0, bins.slope);
        for nudge in -SLOPE_REACH..=SLOPE_REACH {
            let slope = bins.slope + f64::from(nudge) * step;
            for shift in HALF_SAMPLE_GRID {
                let grid = Line {
                    intercept: shift,
                    slope: n - slope,
                };
                let (strength, index) = self.phase_index(iq, preamble, grid, spin);
                if strength > best.0 {
                    best = (strength, centred(index, n) + shift, slope);
                }
            }
        }
        (best.1, best.2)
    }

    fn whole_sample_slip(
        &mut self,
        iq: &[Complex<f32>],
        preamble: &[u32],
        timing: Line,
        frequency: Line,
    ) -> f64 {
        let (_, index) = self.phase_index(iq, preamble, timing, frequency);
        centred(index, self.params.chips() as f64)
    }

    fn phase_index(
        &mut self,
        iq: &[Complex<f32>],
        preamble: &[u32],
        timing: Line,
        frequency: Line,
    ) -> (f64, usize) {
        let n = self.params.chips();
        self.hypotheses.fill(Complex::new(0.0, 0.0));
        let mut previous: Option<(Complex<f64>, u32)> = None;
        for (k, &known) in preamble.iter().enumerate() {
            let index = k as f64;
            self.load(iq, timing.at(index), frequency.at(index), timing.slope);
            self.dechirp();
            let bin = self.bins[(f64::from(known) * self.stretch).round() as usize % n];
            let peak = Complex::new(f64::from(bin.re), f64::from(bin.im));
            if let Some((before, value)) = previous {
                self.accumulate_hypotheses(peak * before.conj(), known, value);
            }
            previous = Some((peak, known));
        }
        let index = argmax_norm(&self.hypotheses);
        (self.hypotheses[index].norm(), index)
    }

    fn accumulate_hypotheses(&mut self, product: Complex<f64>, value: u32, before: u32) {
        let n = self.hypotheses.len();
        let change = f64::from(value) - f64::from(before);
        let step = Complex::from_polar(1.0, TAU * change / n as f64);
        let mut turn = product;
        for slot in &mut self.hypotheses {
            *slot += turn;
            turn *= step;
        }
    }

    fn fine_lines(
        &mut self,
        iq: &[Complex<f32>],
        preamble: &[u32],
        timing: Line,
        frequency: Line,
        reach: f64,
    ) -> (Line, Line) {
        let n = self.params.chips() as f64;
        self.points.clear();
        for (k, &known) in preamble.iter().enumerate() {
            self.load(
                iq,
                timing.at(k as f64),
                frequency.at(k as f64),
                timing.slope,
            );
            self.dechirp();
            let split = self.wrap_point(known);
            if split == 0 || split >= self.params.chips() {
                continue;
            }
            let resolved = self.split_tone(f64::from(known) * self.stretch, split);
            self.points.push(Point {
                index: k as f64,
                value: resolved.late,
                weight: resolved.weight,
                residual: wrap(resolved.tone - f64::from(known) * self.stretch, n),
            });
        }
        let late = circular_line(&self.points, reach);
        let unwrapped = |p: &Point| late.at(p.index) + wrap(p.value - late.at(p.index), 1.0);
        let n = self.params.chips() as f64;
        let points = &self.points;
        let timing_fit = fit(|| {
            points
                .iter()
                .map(|p| (p.index, timing.at(p.index) + unwrapped(p), p.weight))
        });
        let frequency_fit = fit(|| {
            points.iter().map(|p| {
                (
                    p.index,
                    frequency.at(p.index) + unwrapped(p) + p.residual,
                    p.weight,
                )
            })
        });
        (
            timing_fit.map_or(timing, |f| f.settled(n)),
            frequency_fit.map_or(frequency, |f| f.settled(0.0)),
        )
    }

    fn settle(&mut self, preamble: usize, timing: Line, frequency: Line) -> usize {
        let n = self.params.chips() as f64;
        let count = preamble as f64;
        self.timing.reset(timing.slope - n);
        self.tracker.reset(frequency.slope);
        self.offset_bins = frequency.at(count);
        self.anchor = Some(timing.at(count));
        timing.intercept.round().max(0.0) as usize
    }
}

fn inlier_line(points: &[Point], seed: Line, n: f64) -> Line {
    let unwrapped = points.iter().filter_map(|p| {
        let residual = wrap(p.value - seed.at(p.index), n);
        (residual.abs() <= INLIER_BINS).then_some((p.index, seed.at(p.index) + residual, p.weight))
    });
    let line = fit_line(unwrapped).unwrap_or(seed);
    Line {
        intercept: wrap(line.intercept, n),
        slope: line.slope,
    }
}

fn circular_line(points: &[Point], reach: f64) -> Line {
    let count = points.len().max(1) as f64;
    let step = 1.0 / (4.0 * count);
    let reach = (reach / step).ceil() as i32;
    let mut best = (
        f64::NEG_INFINITY,
        Line {
            intercept: 0.0,
            slope: 0.0,
        },
    );
    for i in -reach..=reach {
        let slope = f64::from(i) * step;
        let level: Complex<f64> = points
            .iter()
            .map(|p| Complex::from_polar(p.weight, TAU * (p.value - slope * p.index)))
            .sum();
        if level.norm() > best.0 {
            best = (
                level.norm(),
                Line {
                    intercept: level.arg() / TAU,
                    slope,
                },
            );
        }
    }
    best.1
}

#[derive(Clone, Copy, Debug)]
struct Fit {
    line: Line,
    slope_sigma: f64,
    mean_x: f64,
    mean_y: f64,
}

impl Fit {
    fn settled(self, expected_slope: f64) -> Line {
        if (self.line.slope - expected_slope).abs() > SIGNIFICANT_SIGMAS * self.slope_sigma {
            return self.line;
        }
        Line {
            intercept: self.mean_y - expected_slope * self.mean_x,
            slope: expected_slope,
        }
    }
}

fn fit_line(points: impl Iterator<Item = (f64, f64, f64)> + Clone) -> Option<Line> {
    fit(|| points.clone()).map(|f| f.line)
}

fn fit<I: Iterator<Item = (f64, f64, f64)>>(points: impl Fn() -> I) -> Option<Fit> {
    let (mut count, mut sw, mut sx, mut sy) = (0.0, 0.0, 0.0, 0.0);
    for (x, y, w) in points() {
        count += 1.0;
        sw += w;
        sx += w * x;
        sy += w * y;
    }
    if sw <= 0.0 || count < 3.0 {
        return None;
    }
    let (mean_x, mean_y) = (sx / sw, sy / sw);
    let (mut sxx, mut sxy) = (0.0, 0.0);
    for (x, y, w) in points() {
        sxx += w * (x - mean_x) * (x - mean_x);
        sxy += w * (x - mean_x) * (y - mean_y);
    }
    if sxx <= 0.0 {
        return None;
    }
    let slope = sxy / sxx;
    let line = Line {
        intercept: mean_y - slope * mean_x,
        slope,
    };
    let residual: f64 = points()
        .map(|(x, y, w)| w * (y - line.at(x)).powi(2))
        .sum::<f64>()
        / sw;
    let slope_sigma = (residual * count / (count - 2.0) / (sxx * count / sw)).sqrt();
    Some(Fit {
        line,
        slope_sigma,
        mean_x,
        mean_y,
    })
}

fn centred(index: usize, n: f64) -> f64 {
    wrap(index as f64, n)
}

fn argmax_norm(values: &[Complex<f64>]) -> usize {
    let mut best = 0usize;
    for (k, v) in values.iter().enumerate() {
        if v.norm_sqr() > values[best].norm_sqr() {
            best = k;
        }
    }
    best
}
