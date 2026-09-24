use std::f64::consts::TAU;

use num_complex::Complex;

pub const RIDGE: f64 = 1e-6;
pub const TAP_SIGNIFICANCE: f64 = 9.0;
pub const RELATIVE_FLOOR: f64 = 1e-3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Support {
    pub first_tap: i32,
    pub last_tap: i32,
    pub doppler_order: usize,
}

impl Support {
    #[must_use]
    pub fn taps(&self) -> usize {
        (self.last_tap - self.first_tap + 1).max(1) as usize
    }
}

#[derive(Clone, Debug)]
pub struct DelayDopplerFit {
    occupied: usize,
    taps: usize,
    symbols: usize,
    basis: Vec<Complex<f64>>,
    smoother: Vec<f64>,
    normal: Vec<Complex<f64>>,
    rhs: Vec<Complex<f64>>,
    gains: Vec<Complex<f64>>,
    smoothed: Vec<Complex<f64>>,
    tap_noise: Vec<f64>,
    active: Vec<usize>,
    doppler_dof: f64,
}

impl DelayDopplerFit {
    #[must_use]
    pub fn new(offsets: &[i32], fft: usize, symbols: usize, support: Support) -> Self {
        let taps = support.taps();
        let (smoother, doppler_dof) = smoother(symbols, support.doppler_order);
        Self {
            occupied: offsets.len(),
            taps,
            symbols,
            basis: basis(offsets, fft, support),
            smoother,
            normal: vec![Complex::new(0.0, 0.0); taps * taps],
            rhs: vec![Complex::new(0.0, 0.0); taps],
            gains: vec![Complex::new(0.0, 0.0); symbols * taps],
            smoothed: vec![Complex::new(0.0, 0.0); symbols * taps],
            tap_noise: vec![0.0; taps],
            active: Vec::with_capacity(taps),
            doppler_dof,
        }
    }

    #[must_use]
    pub fn active_taps(&self) -> usize {
        self.active.len()
    }

    pub fn fit(
        &mut self,
        received: &[Complex<f32>],
        known: &[Complex<f32>],
        noise_var: f64,
        channel: &mut [Complex<f32>],
    ) -> f64 {
        debug_assert_eq!(received.len(), self.symbols * self.occupied);
        debug_assert_eq!(known.len(), received.len());
        debug_assert_eq!(channel.len(), received.len());
        self.active.clear();
        self.active.extend(0..self.taps);
        self.pass(received, known, noise_var);
        self.select_taps();
        self.pass(received, known, noise_var);
        self.synthesise(channel);
        residual_noise(received, known, channel, self.dof())
    }

    fn dof(&self) -> f64 {
        self.active.len() as f64 * self.doppler_dof
    }

    fn pass(&mut self, received: &[Complex<f32>], known: &[Complex<f32>], noise_var: f64) {
        let o = self.occupied;
        self.tap_noise.fill(0.0);
        self.gains.fill(Complex::new(0.0, 0.0));
        for symbol in 0..self.symbols {
            let rows = symbol * o..(symbol + 1) * o;
            self.fit_symbol(&received[rows.clone()], &known[rows], symbol, noise_var);
        }
        self.smooth();
    }

    fn fit_symbol(
        &mut self,
        received: &[Complex<f32>],
        known: &[Complex<f32>],
        symbol: usize,
        noise_var: f64,
    ) {
        let a = self.accumulate(received, known);
        let scale = (0..a).map(|k| self.normal[k * a + k].re).sum::<f64>() / a.max(1) as f64;
        let ridge = (noise_var * a as f64)
            .max(RIDGE * scale)
            .max(f64::MIN_POSITIVE);
        for k in 0..a {
            self.normal[k * a + k] += ridge;
        }
        if cholesky(&mut self.normal[..a * a], a).is_none() {
            return;
        }
        solve(&self.normal[..a * a], a, &mut self.rhs[..a]);
        let t = self.taps;
        for (k, &tap) in self.active.iter().enumerate() {
            self.gains[symbol * t + tap] = self.rhs[k];
            self.tap_noise[tap] += noise_var * inverse_diagonal(&self.normal[..a * a], a, k);
        }
    }

    fn accumulate(&mut self, received: &[Complex<f32>], known: &[Complex<f32>]) -> usize {
        let (t, a) = (self.taps, self.active.len());
        self.normal[..a * a].fill(Complex::new(0.0, 0.0));
        self.rhs[..a].fill(Complex::new(0.0, 0.0));
        for (row, (&y, &x)) in self.basis.chunks_exact(t).zip(received.iter().zip(known)) {
            let x = widen(x);
            let weight = x.norm_sqr();
            let target = widen(y) * x.conj();
            for (i, &ti) in self.active.iter().enumerate() {
                let fi = row[ti].conj();
                self.rhs[i] += fi * target;
                for (j, &tj) in self.active.iter().enumerate() {
                    self.normal[i * a + j] += fi * row[tj] * weight;
                }
            }
        }
        a
    }

    fn smooth(&mut self) {
        let (s, t) = (self.symbols, self.taps);
        for n in 0..s {
            for k in 0..t {
                let mut acc = Complex::new(0.0, 0.0);
                for m in 0..s {
                    acc += self.gains[m * t + k] * self.smoother[n * s + m];
                }
                self.smoothed[n * t + k] = acc;
            }
        }
    }

    fn select_taps(&mut self) {
        let (s, t) = (self.symbols, self.taps);
        let share = self.doppler_dof / (s as f64 * s as f64);
        let energy = |k: usize| {
            (0..s)
                .map(|n| self.smoothed[n * t + k].norm_sqr())
                .sum::<f64>()
                / s as f64
        };
        let peak = (0..t).map(energy).fold(0.0f64, f64::max);
        self.active.clear();
        for k in 0..t {
            let floor = (TAP_SIGNIFICANCE * self.tap_noise[k] * share).max(RELATIVE_FLOOR * peak);
            if energy(k) > floor {
                self.active.push(k);
            }
        }
    }

    fn synthesise(&self, channel: &mut [Complex<f32>]) {
        let (o, t) = (self.occupied, self.taps);
        for n in 0..self.symbols {
            let gains = &self.smoothed[n * t..(n + 1) * t];
            for (slot, row) in channel[n * o..(n + 1) * o]
                .iter_mut()
                .zip(self.basis.chunks_exact(t))
            {
                let h: Complex<f64> = row.iter().zip(gains).map(|(f, g)| f * g).sum();
                *slot = Complex::new(h.re as f32, h.im as f32);
            }
        }
    }
}

fn widen(v: Complex<f32>) -> Complex<f64> {
    Complex::new(f64::from(v.re), f64::from(v.im))
}

fn basis(offsets: &[i32], fft: usize, support: Support) -> Vec<Complex<f64>> {
    let mut basis = Vec::with_capacity(offsets.len() * support.taps());
    for &offset in offsets {
        for tap in support.first_tap..=support.last_tap {
            let phase = -TAU * f64::from(offset) * f64::from(tap) / fft as f64;
            basis.push(Complex::from_polar(1.0, phase));
        }
    }
    basis
}

fn smoother(symbols: usize, order: usize) -> (Vec<f64>, f64) {
    let columns = (order + 1).min(symbols).max(1);
    let mut q: Vec<Vec<f64>> = Vec::with_capacity(columns);
    for power in 0..columns {
        let mut v: Vec<f64> = (0..symbols)
            .map(|n| {
                let t = if symbols > 1 {
                    2.0 * n as f64 / (symbols - 1) as f64 - 1.0
                } else {
                    0.0
                };
                t.powi(power as i32)
            })
            .collect();
        for u in &q {
            let dot: f64 = v.iter().zip(u).map(|(a, b)| a * b).sum();
            for (a, b) in v.iter_mut().zip(u) {
                *a -= dot * b;
            }
        }
        let norm = v.iter().map(|a| a * a).sum::<f64>().sqrt();
        if norm > 1e-9 {
            q.push(v.into_iter().map(|a| a / norm).collect());
        }
    }
    let mut p = vec![0.0; symbols * symbols];
    for u in &q {
        for n in 0..symbols {
            for m in 0..symbols {
                p[n * symbols + m] += u[n] * u[m];
            }
        }
    }
    (p, q.len() as f64)
}

fn residual_noise(
    received: &[Complex<f32>],
    known: &[Complex<f32>],
    channel: &[Complex<f32>],
    dof: f64,
) -> f64 {
    let count = received.len() as f64;
    let sum: f64 = received
        .iter()
        .zip(known)
        .zip(channel)
        .map(|((&y, &x), &h)| f64::from((y - h * x).norm_sqr()))
        .sum();
    sum / (count - dof).max(1.0)
}

fn cholesky(a: &mut [Complex<f64>], n: usize) -> Option<()> {
    for j in 0..n {
        let mut diag = a[j * n + j].re;
        for k in 0..j {
            diag -= a[j * n + k].norm_sqr();
        }
        if diag <= 0.0 {
            return None;
        }
        let root = diag.sqrt();
        a[j * n + j] = Complex::new(root, 0.0);
        for i in j + 1..n {
            let mut v = a[i * n + j];
            for k in 0..j {
                v -= a[i * n + k] * a[j * n + k].conj();
            }
            a[i * n + j] = v / root;
        }
    }
    Some(())
}

fn solve(l: &[Complex<f64>], n: usize, b: &mut [Complex<f64>]) {
    for i in 0..n {
        let mut v = b[i];
        for k in 0..i {
            v -= l[i * n + k] * b[k];
        }
        b[i] = v / l[i * n + i].re;
    }
    for i in (0..n).rev() {
        let mut v = b[i];
        for k in i + 1..n {
            v -= l[k * n + i].conj() * b[k];
        }
        b[i] = v / l[i * n + i].re;
    }
}

fn inverse_diagonal(l: &[Complex<f64>], n: usize, k: usize) -> f64 {
    let mut total = 0.0;
    let mut column = [Complex::new(0.0f64, 0.0); 64];
    if n > column.len() {
        return 1.0 / l[k * n + k].re.powi(2);
    }
    column[k] = Complex::new(1.0 / l[k * n + k].re, 0.0);
    total += column[k].norm_sqr();
    for i in k + 1..n {
        let mut v = Complex::new(0.0, 0.0);
        for j in k..i {
            v -= l[i * n + j] * column[j];
        }
        column[i] = v / l[i * n + i].re;
        total += column[i].norm_sqr();
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offsets() -> Vec<i32> {
        (-26..=26).filter(|&k| k != 0).collect()
    }

    fn support() -> Support {
        Support {
            first_tap: -2,
            last_tap: 16,
            doppler_order: 3,
        }
    }

    fn symbols_of(n: usize) -> Vec<Complex<f32>> {
        let mut state = 0x3c1u32;
        (0..n)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                Complex::from_polar(1.0, (state % 628) as f32 / 100.0)
            })
            .collect()
    }

    fn response(offset: i32, symbol: usize) -> Complex<f32> {
        let paths = [(0.0f64, 0.9f64, 0.0f64, 0.002f64), (5.0, 0.4, 1.3, -0.004)];
        paths
            .iter()
            .map(|&(delay, gain, phase, doppler)| {
                let angle = phase - TAU * f64::from(offset) * delay / 64.0
                    + TAU * doppler * symbol as f64 * 5.0;
                Complex::from_polar(gain as f32, angle as f32)
            })
            .sum()
    }

    #[test]
    fn a_noiseless_doubly_selective_channel_is_recovered() {
        let offsets = offsets();
        let symbols = 16;
        let mut fit = DelayDopplerFit::new(&offsets, 64, symbols, support());
        let known = symbols_of(symbols * offsets.len());
        let truth: Vec<Complex<f32>> = (0..symbols)
            .flat_map(|n| offsets.iter().map(move |&k| response(k, n)))
            .collect();
        let received: Vec<Complex<f32>> = truth.iter().zip(&known).map(|(h, x)| h * x).collect();
        let mut channel = vec![Complex::new(0.0, 0.0); truth.len()];
        let noise = fit.fit(&received, &known, 1e-6, &mut channel);
        let worst = truth
            .iter()
            .zip(&channel)
            .map(|(a, b)| (a - b).norm())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.02, "worst channel error {worst}");
        assert!(noise < 1e-3, "residual {noise}");
    }

    #[test]
    fn cholesky_solves_a_hermitian_system() {
        let n = 3;
        let mut a = vec![
            Complex::new(4.0, 0.0),
            Complex::new(1.0, 1.0),
            Complex::new(0.0, 0.5),
            Complex::new(1.0, -1.0),
            Complex::new(3.0, 0.0),
            Complex::new(0.2, 0.0),
            Complex::new(0.0, -0.5),
            Complex::new(0.2, 0.0),
            Complex::new(2.0, 0.0),
        ];
        let original = a.clone();
        let want = [
            Complex::new(1.0, 0.0),
            Complex::new(-1.0, 2.0),
            Complex::new(0.5, 0.5),
        ];
        let mut b: Vec<Complex<f64>> = (0..n)
            .map(|i| (0..n).map(|j| original[i * n + j] * want[j]).sum())
            .collect();
        cholesky(&mut a, n).unwrap();
        solve(&a, n, &mut b);
        for (got, want) in b.iter().zip(want) {
            assert!((got - want).norm() < 1e-12);
        }
    }

    #[test]
    fn the_smoother_keeps_a_polynomial_and_rejects_a_fast_tone() {
        let (p, dof) = smoother(16, 3);
        assert!((dof - 4.0).abs() < 1e-12);
        let cubic: Vec<f64> = (0..16).map(|n| (n as f64 / 15.0).powi(3) - 0.2).collect();
        for n in 0..16 {
            let v: f64 = (0..16).map(|m| p[n * 16 + m] * cubic[m]).sum();
            assert!((v - cubic[n]).abs() < 1e-9);
        }
        let fast: Vec<f64> = (0..16)
            .map(|n| if n % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let kept: f64 = (0..16)
            .map(|n| {
                (0..16)
                    .map(|m| p[n * 16 + m] * fast[m])
                    .sum::<f64>()
                    .powi(2)
            })
            .sum();
        assert!(kept < 1.0, "a Nyquist tone kept {kept} of 16");
    }
}
