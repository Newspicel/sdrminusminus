const GOLDEN: f64 = 0.381_966_011_250_105_1;
const SQRT_EPSILON: f64 = 1.490_116_119_384_765_6e-8;

pub fn brent_max(
    mut f: impl FnMut(f64) -> f64,
    lo: f64,
    hi: f64,
    tol: f64,
    max_iter: u32,
) -> (f64, f64) {
    let mut search = Search::new(lo.min(hi), lo.max(hi));
    let first = -f(search.x);
    search.seed(first);
    for _ in 0..max_iter {
        let Some(u) = search.next_point(tol) else {
            break;
        };
        search.accept(u, -f(u));
    }
    (search.x, -search.fx)
}

struct Search {
    a: f64,
    b: f64,
    x: f64,
    w: f64,
    v: f64,
    fx: f64,
    fw: f64,
    fv: f64,
    step: f64,
    previous: f64,
}

impl Search {
    fn new(a: f64, b: f64) -> Self {
        let x = a + GOLDEN * (b - a);
        Self {
            a,
            b,
            x,
            w: x,
            v: x,
            fx: 0.0,
            fw: 0.0,
            fv: 0.0,
            step: 0.0,
            previous: 0.0,
        }
    }

    fn seed(&mut self, value: f64) {
        self.fx = value;
        self.fw = value;
        self.fv = value;
    }

    fn next_point(&mut self, tol: f64) -> Option<f64> {
        let middle = 0.5 * (self.a + self.b);
        let tol1 = SQRT_EPSILON * self.x.abs() + tol / 3.0;
        let tol2 = 2.0 * tol1;
        if (self.x - middle).abs() <= tol2 - 0.5 * (self.b - self.a) {
            return None;
        }
        if !self.parabolic_step(middle, tol1, tol2) {
            self.previous = if self.x < middle {
                self.b - self.x
            } else {
                self.a - self.x
            };
            self.step = GOLDEN * self.previous;
        }
        Some(if self.step.abs() >= tol1 {
            self.x + self.step
        } else {
            self.x + tol1.copysign(self.step)
        })
    }

    fn parabolic_step(&mut self, middle: f64, tol1: f64, tol2: f64) -> bool {
        if self.previous.abs() <= tol1 {
            return false;
        }
        let r = (self.x - self.w) * (self.fx - self.fv);
        let q = (self.x - self.v) * (self.fx - self.fw);
        let p = (self.x - self.v) * q - (self.x - self.w) * r;
        let q = 2.0 * (q - r);
        let (p, q) = if q > 0.0 { (-p, q) } else { (p, -q) };
        let older = self.previous;
        let inside = p > q * (self.a - self.x) && p < q * (self.b - self.x);
        if p.abs() >= (0.5 * q * older).abs() || !inside {
            return false;
        }
        self.previous = self.step;
        self.step = p / q;
        let u = self.x + self.step;
        if u - self.a < tol2 || self.b - u < tol2 {
            self.step = tol1.copysign(middle - self.x);
        }
        true
    }

    fn accept(&mut self, u: f64, fu: f64) {
        if fu <= self.fx {
            if u < self.x {
                self.b = self.x;
            } else {
                self.a = self.x;
            }
            (self.v, self.fv) = (self.w, self.fw);
            (self.w, self.fw) = (self.x, self.fx);
            (self.x, self.fx) = (u, fu);
            return;
        }
        if u < self.x {
            self.a = u;
        } else {
            self.b = u;
        }
        if fu <= self.fw || self.w == self.x {
            (self.v, self.fv) = (self.w, self.fw);
            (self.w, self.fw) = (u, fu);
        } else if fu <= self.fv || self.v == self.x || self.v == self.w {
            (self.v, self.fv) = (u, fu);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brent_finds_a_parabola_peak() {
        let (x, value) = brent_max(|x| -(x - 0.37).powi(2), -2.0, 3.0, 1e-8, 100);
        assert!((x - 0.37).abs() < 1e-6, "{x}");
        assert!(value.abs() < 1e-12);
    }

    #[test]
    fn brent_finds_a_cosine_peak_with_swapped_bounds() {
        let mut calls = 0;
        let (x, value) = brent_max(
            |x| {
                calls += 1;
                (x - 1.2).cos()
            },
            2.5,
            0.0,
            1e-6,
            60,
        );
        assert!((x - 1.2).abs() < 1e-5, "{x}");
        assert!((value - 1.0).abs() < 1e-9);
        assert!(calls < 40, "{calls} calls");
    }

    #[test]
    fn a_peak_at_the_edge_stays_inside_the_bracket() {
        let (x, _) = brent_max(|x| x, 0.0, 1.0, 1e-6, 100);
        assert!((0.0..=1.0).contains(&x));
        assert!(x > 1.0 - 1e-5, "{x}");
    }

    #[test]
    fn the_iteration_budget_is_honoured() {
        let mut calls = 0u32;
        let _ = brent_max(
            |x| {
                calls += 1;
                -(x - 0.1).abs()
            },
            -1.0,
            1.0,
            0.0,
            5,
        );
        assert_eq!(calls, 6);
    }
}
