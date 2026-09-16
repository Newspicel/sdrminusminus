use num_complex::Complex;

#[derive(Clone, Copy, Debug)]
pub struct Timing {
    pub fft: usize,
    pub guard: usize,
    pub at: usize,
    pub offset: f32,
    pub quality: f32,
}

fn metric(correlation: Complex<f32>, power: f32) -> f32 {
    (2.0 * correlation.norm() / power.max(1e-12)).min(1.0)
}

pub fn correlation(iq: &[Complex<f32>], n: usize, g: usize, at: usize) -> (f32, f32) {
    let mut sum = Complex::new(0.0, 0.0);
    let mut power = 0.0;
    for i in at..at + g {
        sum += iq[i + n] * iq[i].conj();
        power += iq[i].norm_sqr() + iq[i + n].norm_sqr();
    }
    (metric(sum, power), sum.arg() / n as f32)
}

pub fn acquire(iq: &[Complex<f32>]) -> Option<Timing> {
    let mut best: Option<Timing> = None;
    for n in [2048, 8192] {
        for denominator in [4, 8, 16, 32] {
            let g = n / denominator;
            let period = n + g;
            if iq.len() < 4 * period {
                continue;
            }
            let mut sum = Complex::new(0.0, 0.0);
            let mut power = 0.0;
            for i in 0..g {
                sum += iq[i + n] * iq[i].conj();
                power += iq[i].norm_sqr() + iq[i + n].norm_sqr();
            }
            for at in 0..period.min(iq.len() - 4 * period + 1) {
                if at > 0 {
                    let old = at - 1;
                    let new = at + g - 1;
                    sum += iq[new + n] * iq[new].conj() - iq[old + n] * iq[old].conj();
                    power += iq[new].norm_sqr() + iq[new + n].norm_sqr()
                        - iq[old].norm_sqr()
                        - iq[old + n].norm_sqr();
                }
                let quality = metric(sum, power);
                if quality < 0.75 || best.is_some_and(|best| quality < best.quality - 0.02) {
                    continue;
                }
                if (1..4).any(|i| correlation(iq, n, g, at + period * i).0 < 0.7) {
                    continue;
                }
                if best.is_none_or(|b| {
                    quality > b.quality + 0.002
                        || ((quality - b.quality).abs() <= 0.002 && g > b.guard)
                }) {
                    best = Some(Timing {
                        fft: n,
                        guard: g,
                        at,
                        offset: sum.arg() / n as f32,
                        quality,
                    });
                }
            }
        }
    }
    best
}
