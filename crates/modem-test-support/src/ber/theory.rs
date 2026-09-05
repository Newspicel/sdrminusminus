use std::f64::consts::{FRAC_1_SQRT_2, LN_2, PI};

const ERF_NUM: [f64; 5] = [
    3.1611237438705655,
    113.86415415105016,
    377.485237685302,
    3209.3775891384694,
    0.18577770618460315,
];
const ERF_DEN: [f64; 4] = [
    23.601290952344122,
    244.02463793444417,
    1282.6165260773723,
    2844.236833439171,
];

const ERFC_MID_NUM: [f64; 9] = [
    0.5641884969886701,
    8.883149794388377,
    66.11919063714163,
    298.6351381974001,
    881.952221241769,
    1712.0476126340707,
    2051.0783778260716,
    1230.3393547979972,
    2.1531153547440383e-8,
];
const ERFC_MID_DEN: [f64; 8] = [
    15.744926110709835,
    117.6939508913125,
    537.1811018620099,
    1621.3895745666903,
    3290.7992357334597,
    4362.619090143247,
    3439.3676741437216,
    1230.3393548037495,
];

const ERFC_FAR_NUM: [f64; 6] = [
    0.30532663496123236,
    0.36034489994980445,
    0.12578172611122926,
    0.016083785148742275,
    0.0006587491615298378,
    0.016315387137302097,
];
const ERFC_FAR_DEN: [f64; 5] = [
    2.568520192289822,
    1.8729528499234604,
    0.5279051029514285,
    0.06051834131244132,
    0.0023352049762686918,
];

const FRAC_1_SQRT_PI: f64 = 0.5641895835477563;

#[must_use]
pub fn erfc(x: f64) -> f64 {
    let y = x.abs();
    if y <= 0.46875 {
        let ysq = if y > 1.11e-16 { y * y } else { 0.0 };
        let mut num = ERF_NUM[4] * ysq;
        let mut den = ysq;
        for (&n, &d) in ERF_NUM[..3].iter().zip(&ERF_DEN[..3]) {
            num = (num + n) * ysq;
            den = (den + d) * ysq;
        }
        return 1.0 - x * (num + ERF_NUM[3]) / (den + ERF_DEN[3]);
    }
    let tail = if y <= 4.0 {
        let mut num = ERFC_MID_NUM[8] * y;
        let mut den = y;
        for (&n, &d) in ERFC_MID_NUM[..7].iter().zip(&ERFC_MID_DEN[..7]) {
            num = (num + n) * y;
            den = (den + d) * y;
        }
        exp_neg_squared(y) * (num + ERFC_MID_NUM[7]) / (den + ERFC_MID_DEN[7])
    } else if y < 26.543 {
        let inv = 1.0 / (y * y);
        let mut num = ERFC_FAR_NUM[5] * inv;
        let mut den = inv;
        for (&n, &d) in ERFC_FAR_NUM[..4].iter().zip(&ERFC_FAR_DEN[..4]) {
            num = (num + n) * inv;
            den = (den + d) * inv;
        }
        let r = inv * (num + ERFC_FAR_NUM[4]) / (den + ERFC_FAR_DEN[4]);
        exp_neg_squared(y) * (FRAC_1_SQRT_PI - r) / y
    } else {
        0.0
    };
    if x < 0.0 { 2.0 - tail } else { tail }
}

fn exp_neg_squared(y: f64) -> f64 {
    let ysq = (y * 16.0).trunc() / 16.0;
    let del = (y - ysq) * (y + ysq);
    (-ysq * ysq).exp() * (-del).exp()
}

#[must_use]
pub fn q(x: f64) -> f64 {
    0.5 * erfc(x * FRAC_1_SQRT_2)
}

fn ebn0_lin(ebn0_db: f64) -> f64 {
    10f64.powf(ebn0_db / 10.0)
}

fn bits_per_symbol(m: u32) -> f64 {
    debug_assert!(m >= 2 && m.is_power_of_two(), "modulation order {m}");
    f64::from(m.ilog2())
}

#[must_use]
pub fn bpsk_ber(ebn0_db: f64) -> f64 {
    0.5 * erfc(ebn0_lin(ebn0_db).sqrt())
}

#[must_use]
pub fn qpsk_ber(ebn0_db: f64) -> f64 {
    bpsk_ber(ebn0_db)
}

#[must_use]
pub fn mpam_ser(m: u32, ebn0_db: f64) -> f64 {
    let k = bits_per_symbol(m);
    let m_f = f64::from(m);
    let g = ebn0_lin(ebn0_db);
    2.0 * (m_f - 1.0) / m_f * q((6.0 * k / (m_f * m_f - 1.0) * g).sqrt())
}

#[must_use]
pub fn mpam_ber(m: u32, ebn0_db: f64) -> f64 {
    mpam_ser(m, ebn0_db) / bits_per_symbol(m)
}

fn is_square_qam(m: u32) -> bool {
    m >= 4 && m.is_power_of_two() && m.ilog2().is_multiple_of(2)
}

#[must_use]
pub fn mqam_ser(m: u32, ebn0_db: f64) -> f64 {
    debug_assert!(is_square_qam(m), "square QAM order {m}");
    let k = bits_per_symbol(m);
    let m_f = f64::from(m);
    let g = ebn0_lin(ebn0_db);
    let p = 2.0 * (1.0 - 1.0 / m_f.sqrt()) * q((3.0 * k / (m_f - 1.0) * g).sqrt());
    p * (2.0 - p)
}

#[must_use]
pub fn mqam_ber(m: u32, ebn0_db: f64) -> f64 {
    debug_assert!(is_square_qam(m), "square QAM order {m}");
    let k = bits_per_symbol(m);
    let m_f = f64::from(m);
    let g = ebn0_lin(ebn0_db);
    4.0 / k * (1.0 - 1.0 / m_f.sqrt()) * q((3.0 * k / (m_f - 1.0) * g).sqrt())
}

#[must_use]
pub fn mpsk_ser(m: u32, ebn0_db: f64) -> f64 {
    let k = bits_per_symbol(m);
    let g = ebn0_lin(ebn0_db);
    match m {
        2 => q((2.0 * g).sqrt()),
        4 => {
            let p = q((2.0 * g).sqrt());
            p * (2.0 - p)
        }
        _ => 2.0 * q((2.0 * k * g).sqrt() * (PI / f64::from(m)).sin()),
    }
}

#[must_use]
pub fn dbpsk_ber(ebn0_db: f64) -> f64 {
    0.5 * (-ebn0_lin(ebn0_db)).exp()
}

#[must_use]
pub fn dqpsk_ber(ebn0_db: f64) -> f64 {
    let g = ebn0_lin(ebn0_db);
    let a = (2.0 * g * (1.0 - FRAC_1_SQRT_2)).sqrt();
    let b = (2.0 * g * (1.0 + FRAC_1_SQRT_2)).sqrt();
    let r = a / b;
    let x = a * b;
    let scaled = bessel_i_scaled(x, series_len(x, r));
    let mut sum = 0.5 * scaled[0];
    let mut rk = r;
    for &ik in &scaled[1..] {
        sum += rk * ik;
        rk *= r;
    }
    let d = b - a;
    (-0.5 * d * d).exp() * sum
}

#[must_use]
pub fn marcum_q1(a: f64, b: f64) -> f64 {
    debug_assert!(a >= 0.0 && b >= 0.0, "Marcum Q of ({a}, {b})");
    if b <= 0.0 {
        return 1.0;
    }
    if a <= 0.0 {
        return (-0.5 * b * b).exp();
    }
    if a > b {
        let (swapped, i0_scaled) = q1_core(b, a);
        let d = a - b;
        return 1.0 + (-0.5 * d * d).exp() * i0_scaled - swapped;
    }
    q1_core(a, b).0
}

fn q1_core(a: f64, b: f64) -> (f64, f64) {
    let x = a * b;
    let r = a / b;
    let scaled = bessel_i_scaled(x, series_len(x, r));
    let mut sum = 0.0;
    let mut rk = 1.0;
    for &ik in &scaled {
        sum += rk * ik;
        rk *= r;
    }
    let d = b - a;
    ((-0.5 * d * d).exp() * sum, scaled[0])
}

fn bessel_ratio_estimate(j: f64, x: f64) -> f64 {
    x / (j + (j * j + x * x).sqrt())
}

fn series_len(x: f64, r: f64) -> usize {
    let mut k = 1usize;
    let mut bound = 1.0f64;
    while bound > 1e-22 && k < 2000 {
        bound *= r * bessel_ratio_estimate(k as f64, x);
        k += 1;
    }
    k
}

fn bessel_i_scaled(x: f64, k_max: usize) -> Vec<f64> {
    let mut out = vec![0.0; k_max + 1];
    if x <= 0.0 {
        out[0] = 1.0;
        return out;
    }
    let mut start = k_max + 1;
    let mut headroom = bessel_ratio_estimate(start as f64, x);
    while headroom > 1e-24 && start < k_max + 4000 {
        start += 1;
        headroom *= bessel_ratio_estimate(start as f64, x);
    }
    let mut above = 0.0f64;
    let mut cur = 1e-280f64;
    let mut norm = 2.0 * cur;
    let mut j = start;
    while j > 0 {
        let below = above + (2.0 * j as f64 / x) * cur;
        above = cur;
        cur = below;
        j -= 1;
        norm += if j == 0 { cur } else { 2.0 * cur };
        if j <= k_max {
            out[j] = cur;
        }
        if cur > 1e250 {
            above *= 1e-250;
            cur *= 1e-250;
            norm *= 1e-250;
            for v in &mut out {
                *v *= 1e-250;
            }
        }
    }
    for v in &mut out {
        *v /= norm;
    }
    out
}

pub const MAX_ORTHOGONAL_ORDER: u32 = 1 << 12;

const BINOMIAL_LIMIT: u32 = 64;

#[must_use]
pub fn mfsk_noncoherent_ser(m: u32, ebn0_db: f64) -> f64 {
    debug_assert!(
        m >= 2 && m.is_power_of_two() && m <= MAX_ORTHOGONAL_ORDER,
        "orthogonal order {m} is not a power of two in 2..={MAX_ORTHOGONAL_ORDER}"
    );
    let k = bits_per_symbol(m);
    let gamma_b = ebn0_lin(ebn0_db);
    if m <= BINOMIAL_LIMIT {
        orthogonal_ser_binomial(m, k, gamma_b)
    } else {
        orthogonal_ser_quadrature(m, k * gamma_b)
    }
}

fn orthogonal_ser_binomial(m: u32, k: f64, gamma_b: f64) -> f64 {
    let gs = Dd::product(k, gamma_b);
    let mut sum = Dd::ZERO;
    let mut binom: u128 = 1;
    for n in 1..m {
        binom = binom * u128::from(m - n) / u128::from(n);
        let np1 = f64::from(n + 1);
        let term = Dd::from_u64(binom as u64)
            .mul(gs.mul_f64(-f64::from(n)).div_f64(np1).exp())
            .div_f64(np1);
        sum = sum.add(if n % 2 == 1 { term } else { term.neg() });
    }
    sum.to_f64()
}

fn orthogonal_ser_quadrature(m: u32, gamma: f64) -> f64 {
    let root_gamma = gamma.max(0.0).sqrt();
    let hi = root_gamma + 12.0;
    let panels = ((hi / 0.002).ceil() as usize).clamp(2_000, 200_000) & !1;
    let step = hi / panels as f64;
    let exponent = f64::from(m - 1);
    let integrand = |u: f64| {
        let gap = u - root_gamma;
        let tail = -(exponent * (-(-u * u).exp()).ln_1p()).exp_m1();
        2.0 * u * (-gap * gap).exp() * bessel_i0_scaled(2.0 * u * root_gamma) * tail
    };
    let mut sum = integrand(0.0) + integrand(hi);
    for i in 1..panels {
        let weight = if i % 2 == 1 { 4.0 } else { 2.0 };
        sum += weight * integrand(i as f64 * step);
    }
    (sum * step / 3.0).clamp(0.0, 1.0)
}

fn bessel_i0_scaled(x: f64) -> f64 {
    if x < 3.75 {
        let t = x / 3.75;
        let t2 = t * t;
        let series = 1.0
            + t2 * (3.515_622_9
                + t2 * (3.089_942_4
                    + t2 * (1.206_749_2
                        + t2 * (0.265_973_2 + t2 * (0.036_076_8 + t2 * 0.004_581_3)))));
        return series * (-x).exp();
    }
    let t = 3.75 / x;
    let series = 0.398_942_28
        + t * (0.013_285_92
            + t * (0.002_253_19
                + t * (-0.001_575_65
                    + t * (0.009_162_81
                        + t * (-0.020_577_06
                            + t * (0.026_355_37 + t * (-0.016_476_33 + t * 0.003_923_77)))))));
    series / x.sqrt()
}

#[must_use]
pub fn mfsk_noncoherent_ber(m: u32, ebn0_db: f64) -> f64 {
    let m_f = f64::from(m);
    mfsk_noncoherent_ser(m, ebn0_db) * m_f / (2.0 * (m_f - 1.0))
}

#[derive(Clone, Debug, PartialEq)]
pub struct NearestNeighbour {
    pub d_min: f64,
    pub neighbours: f64,
    pub bits_per_error: f64,
    pub bits_per_symbol: f64,
    pairs: Vec<(f64, f64)>,
    points: f64,
}

const SHELL_SLACK: f64 = 1.002;

impl NearestNeighbour {
    #[must_use]
    pub fn of(c: &sdrmm_modem::constellation::Constellation) -> Self {
        let p = c.points();
        let n = p.len();
        let d2 = |a: usize, b: usize| f64::from((p[a] - p[b]).norm_sqr());
        let mut min = f64::INFINITY;
        for i in 0..n {
            for j in (i + 1)..n {
                min = min.min(d2(i, j));
            }
        }
        let limit = min * SHELL_SLACK;
        let (mut shell_pairs, mut shell_bits) = (0u64, 0u64);
        let mut pairs = Vec::with_capacity(n * (n - 1) / 2);
        for i in 0..n {
            for j in (i + 1)..n {
                let d = d2(i, j);
                let bits = f64::from((c.labels()[i] ^ c.labels()[j]).count_ones());
                pairs.push((d.sqrt(), bits));
                if d <= limit {
                    shell_pairs += 1;
                    shell_bits += bits as u64;
                }
            }
        }
        Self {
            d_min: min.sqrt(),
            neighbours: 2.0 * shell_pairs as f64 / n as f64,
            bits_per_error: shell_bits as f64 / shell_pairs as f64,
            bits_per_symbol: f64::from(c.bits_per_symbol() as u32),
            pairs,
            points: n as f64,
        }
    }

    #[must_use]
    pub fn ser(&self, ebn0_db: f64) -> f64 {
        let gs = self.bits_per_symbol * ebn0_lin(ebn0_db);
        (self.neighbours * q(self.d_min * (0.5 * gs).sqrt())).min(1.0)
    }

    #[must_use]
    pub fn ber(&self, ebn0_db: f64) -> f64 {
        let scale = (0.5 * self.bits_per_symbol * ebn0_lin(ebn0_db)).sqrt();
        let sum: f64 = self
            .pairs
            .iter()
            .map(|&(d, bits)| bits * q(d * scale))
            .sum();
        (2.0 * sum / (self.points * self.bits_per_symbol)).min(1.0)
    }
}

const LN2_DD: Dd = Dd {
    hi: LN_2,
    lo: 2.3190468138462996e-17,
};

#[derive(Clone, Copy, Debug)]
struct Dd {
    hi: f64,
    lo: f64,
}

fn two_sum(a: f64, b: f64) -> Dd {
    let s = a + b;
    let bb = s - a;
    Dd {
        hi: s,
        lo: (a - (s - bb)) + (b - bb),
    }
}

fn quick_two_sum(a: f64, b: f64) -> Dd {
    let s = a + b;
    Dd {
        hi: s,
        lo: b - (s - a),
    }
}

fn two_prod(a: f64, b: f64) -> Dd {
    let p = a * b;
    Dd {
        hi: p,
        lo: a.mul_add(b, -p),
    }
}

impl Dd {
    const ZERO: Dd = Dd { hi: 0.0, lo: 0.0 };
    const ONE: Dd = Dd { hi: 1.0, lo: 0.0 };

    fn product(a: f64, b: f64) -> Dd {
        two_prod(a, b)
    }

    fn from_u64(v: u64) -> Dd {
        let hi = v as f64;
        #[allow(clippy::cast_possible_truncation)]
        let lo = (v as i128 - hi as i128) as f64;
        Dd { hi, lo }
    }

    fn neg(self) -> Dd {
        Dd {
            hi: -self.hi,
            lo: -self.lo,
        }
    }

    fn add(self, o: Dd) -> Dd {
        let s = two_sum(self.hi, o.hi);
        let t = two_sum(self.lo, o.lo);
        let u = quick_two_sum(s.hi, s.lo + t.hi);
        quick_two_sum(u.hi, u.lo + t.lo)
    }

    fn mul(self, o: Dd) -> Dd {
        let p = two_prod(self.hi, o.hi);
        quick_two_sum(p.hi, p.lo + (self.hi * o.lo + self.lo * o.hi))
    }

    fn mul_f64(self, m: f64) -> Dd {
        let p = two_prod(self.hi, m);
        quick_two_sum(p.hi, p.lo + self.lo * m)
    }

    fn div_f64(self, d: f64) -> Dd {
        let q1 = self.hi / d;
        let p = two_prod(q1, d);
        let s = two_sum(self.hi, -p.hi);
        let q2 = (s.hi + ((s.lo + self.lo) - p.lo)) / d;
        quick_two_sum(q1, q2)
    }

    fn exp(self) -> Dd {
        debug_assert!(
            self.hi <= 0.0,
            "Dd::exp is written for the e^-x of a decay term"
        );
        if self.hi < -708.0 {
            return Dd::ZERO;
        }
        let k = (self.hi / LN2_DD.hi).round();
        let r = self.add(LN2_DD.mul_f64(-k));
        let mut sum = Dd::ONE;
        let mut term = Dd::ONE;
        let mut i = 1.0f64;
        loop {
            term = term.mul(r).div_f64(i);
            sum = sum.add(term);
            i += 1.0;
            if term.hi.abs() < 1e-35 || i > 40.0 {
                break;
            }
        }
        #[allow(clippy::cast_possible_truncation)]
        let scale = 2f64.powi(k as i32);
        Dd {
            hi: sum.hi * scale,
            lo: sum.lo * scale,
        }
    }

    fn to_f64(self) -> f64 {
        self.hi + self.lo
    }
}

#[must_use]
pub fn am_fom(depth: f64, message_power: f64) -> f64 {
    let modulated = depth * depth * message_power;
    modulated / (1.0 + modulated)
}

#[must_use]
pub fn ssb_fom() -> f64 {
    1.0
}

#[must_use]
pub fn fm_fom(deviation_ratio: f64, message_power: f64) -> f64 {
    3.0 * deviation_ratio * deviation_ratio * message_power
}

#[must_use]
pub fn pm_fom(peak_phase_rad: f64, message_power: f64) -> f64 {
    peak_phase_rad * peak_phase_rad * message_power
}

#[must_use]
pub fn analog_sinad_db(fom: f64, channel_snr_db: f64) -> f64 {
    channel_snr_db + 10.0 * fom.log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_rel(actual: f64, expected: f64, tol: f64, what: &str) {
        let rel = ((actual - expected) / expected).abs();
        assert!(
            rel < tol,
            "{what}: got {actual:e}, want {expected:e}, rel err {rel:e}"
        );
    }

    #[test]
    fn nearest_neighbour_bound_reproduces_the_closed_forms() {
        use sdrmm_modem::constellation::tables;
        let pam4 = NearestNeighbour::of(&tables::pam(4).unwrap());
        assert!((pam4.d_min - 2.0 / 5f64.sqrt()).abs() < 1e-6, "{pam4:?}");
        assert!((pam4.neighbours - 1.5).abs() < 1e-12);
        assert!((pam4.bits_per_error - 1.0).abs() < 1e-12);
        for db in [12.0, 15.0, 18.0] {
            assert_rel(pam4.ser(db), mpam_ser(4, db), 0.02, "4-PAM SER");
            assert!(
                pam4.ber(db) >= mpam_ber(4, db) * 0.999,
                "4-PAM union bound below theory"
            );
            assert_rel(pam4.ber(db), mpam_ber(4, db), 0.05, "4-PAM BER");
        }
        let qam16 = NearestNeighbour::of(&tables::qam_square(16).unwrap());
        assert!((qam16.neighbours - 3.0).abs() < 1e-12, "{qam16:?}");
        for db in [14.0, 17.0, 20.0] {
            assert_rel(qam16.ser(db), mqam_ser(16, db), 0.02, "16-QAM SER");
            assert!(
                qam16.ber(db) >= mqam_ber(16, db) * 0.999,
                "16-QAM union bound below theory"
            );
            assert_rel(qam16.ber(db), mqam_ber(16, db), 0.05, "16-QAM BER");
        }
        let bpsk = NearestNeighbour::of(&tables::pam(2).unwrap());
        for db in [0.0, 5.0, 10.0] {
            assert_rel(bpsk.ser(db), bpsk_ber(db), 1e-9, "BPSK SER");
            assert_rel(bpsk.ber(db), bpsk_ber(db), 1e-9, "BPSK union bound");
        }
    }

    #[test]
    fn exotic_tables_read_back_a_usable_bound() {
        use sdrmm_modem::constellation::tables;
        let cross32 = NearestNeighbour::of(&tables::qam_cross(32).unwrap());
        let qam32_ish = NearestNeighbour::of(&tables::qam_square(64).unwrap());
        assert!(cross32.bits_per_error > 1.0, "{cross32:?}");
        assert!(cross32.d_min > qam32_ish.d_min);
        assert!(cross32.ser(20.0) < qam32_ish.ser(20.0));
        let apsk16 = NearestNeighbour::of(&tables::apsk16_dvbs2(3.15).unwrap());
        let qam16 = NearestNeighbour::of(&tables::qam_square(16).unwrap());
        assert!(apsk16.d_min < qam16.d_min, "{apsk16:?} vs {qam16:?}");
        for nn in [&cross32, &apsk16] {
            assert!(nn.ser(25.0) < 1e-6 && nn.ser(0.0) <= 1.0);
            assert!(nn.ber(25.0) < 1e-5 && nn.ber(0.0) <= 1.0);
        }
        let star = NearestNeighbour::of(&tables::qam_star(&[1.0, 2.0], 8).unwrap());
        let truncated = |db: f64| star.ser(db) * star.bits_per_error / star.bits_per_symbol;
        assert!(
            star.ber(6.0) > 1.3 * truncated(6.0),
            "shoulder: union {} vs truncated {}",
            star.ber(6.0),
            truncated(6.0)
        );
        assert!(
            star.ber(18.0) < 1.02 * truncated(18.0),
            "tail: union {} vs truncated {}",
            star.ber(18.0),
            truncated(18.0)
        );
    }

    #[test]
    fn erfc_matches_independent_values() {
        let table = [
            (0.1, 0.8875370839817152),
            (0.5, 0.4795001221869535),
            (1.0, 0.15729920705028513),
            (1.5, 0.033894853524689274),
            (2.0, 0.004677734981047266),
            (3.0, 2.209049699858544e-5),
            (4.0, 1.541725790028002e-8),
            (5.0, 1.537459794428035e-12),
            (6.0, 2.1519736712498913e-17),
            (-0.5, 1.5204998778130465),
            (-2.0, 1.9953222650189528),
        ];
        for (x, want) in table {
            assert_rel(erfc(x), want, 1e-12, &format!("erfc({x})"));
        }
        assert!((erfc(0.0) - 1.0).abs() < 1e-15, "erfc(0)");
        assert_eq!(erfc(30.0), 0.0, "erfc past the underflow cutoff");
    }

    #[test]
    fn q_is_the_gaussian_tail() {
        assert!((q(0.0) - 0.5).abs() < 1e-15, "q(0)");
        let table = [
            (1.0, 0.15865525393145705),
            (2.0, 0.02275013194817921),
            (3.0, 0.0013498980316300946),
            (4.0, 3.1671241833119924e-5),
        ];
        for (x, want) in table {
            assert_rel(q(x), want, 1e-12, &format!("q({x})"));
        }
    }

    #[test]
    fn bpsk_hits_published_waterfall_points() {
        assert_rel(bpsk_ber(6.789522612404168), 1e-3, 1e-10, "BPSK at 6.79 dB");
        assert_rel(bpsk_ber(9.587858346847607), 1e-5, 1e-10, "BPSK at 9.59 dB");
    }

    #[test]
    fn qpsk_ber_equals_bpsk_ber() {
        for tenth in 0..=140 {
            let db = f64::from(tenth) * 0.1;
            assert_eq!(qpsk_ber(db), bpsk_ber(db), "at {db} dB");
        }
    }

    #[test]
    fn dbpsk_hits_published_waterfall_point() {
        assert_rel(
            dbpsk_ber(7.934137466447398),
            1e-3,
            1e-10,
            "DBPSK at 7.93 dB",
        );
    }

    #[test]
    fn pam_matches_forms_and_reduces_to_bpsk() {
        for tenth in 0..=140 {
            let db = f64::from(tenth) * 0.1;
            assert_rel(
                mpam_ber(2, db),
                bpsk_ber(db),
                1e-12,
                &format!("2-PAM at {db} dB"),
            );
            assert_rel(
                mpam_ser(2, db),
                bpsk_ber(db),
                1e-12,
                &format!("2-PAM SER at {db} dB"),
            );
        }
        assert_rel(
            mpam_ser(4, 6.0),
            0.05574261263932141,
            1e-12,
            "4-PAM SER at 6 dB",
        );
        assert_rel(
            mpam_ser(4, 10.0),
            0.0035083012357854495,
            1e-12,
            "4-PAM SER at 10 dB",
        );
    }

    #[test]
    fn qam16_matches_table_values() {
        assert_rel(
            mqam_ber(16, 10.5),
            0.001025725227946195,
            1e-12,
            "16-QAM BER at 10.5 dB",
        );
        assert_rel(
            mqam_ber(16, 10.522401171856055),
            1e-3,
            1e-10,
            "16-QAM at its 1e-3 point",
        );
        assert_rel(
            mqam_ber(16, 4.0),
            0.058618457419250876,
            1e-12,
            "16-QAM BER at 4 dB",
        );
        assert_rel(
            mqam_ser(16, 12.0),
            0.0005545578503225422,
            1e-12,
            "16-QAM SER at 12 dB",
        );
        assert_rel(
            mqam_ber(64, 18.0),
            6.35114807198656e-6,
            1e-12,
            "64-QAM BER at 18 dB",
        );
    }

    #[test]
    fn qam4_ber_equals_qpsk() {
        for tenth in 0..=140 {
            let db = f64::from(tenth) * 0.1;
            assert_rel(
                mqam_ber(4, db),
                qpsk_ber(db),
                1e-12,
                &format!("4-QAM at {db} dB"),
            );
        }
    }

    #[test]
    fn psk_matches_forms() {
        for tenth in 0..=140 {
            let db = f64::from(tenth) * 0.1;
            assert_rel(
                mpsk_ser(2, db),
                bpsk_ber(db),
                1e-12,
                &format!("2-PSK at {db} dB"),
            );
        }
        assert_rel(
            mpsk_ser(4, 6.79),
            0.001997857657700032,
            1e-12,
            "QPSK SER at 6.79 dB",
        );
        assert_rel(
            mpsk_ser(8, 10.0),
            0.0030341859621386717,
            1e-12,
            "8-PSK SER at 10 dB",
        );
        assert_rel(
            mpsk_ser(8, 14.0),
            2.6268980874931245e-6,
            1e-12,
            "8-PSK SER at 14 dB",
        );
    }

    #[test]
    fn marcum_q1_matches_independent_values() {
        let table = [
            (1.0, 2.0, 0.26901206003591),
            (0.5, 3.0, 0.01784367338648221),
            (2.0, 1.0, 0.918107696369406),
            (3.0, 3.0, 0.5674797622908615),
            (2.42, 5.84, 0.0005012345039334298),
        ];
        for (a, b, want) in table {
            assert_rel(marcum_q1(a, b), want, 1e-12, &format!("Q1({a}, {b})"));
        }
        assert_eq!(marcum_q1(1.5, 0.0), 1.0, "Q1(a, 0)");
        assert_rel(marcum_q1(0.0, 2.0), (-2.0f64).exp(), 1e-14, "Q1(0, b)");
    }

    #[test]
    fn dqpsk_matches_exact_marcum_form() {
        let table = [
            (0.0, 0.1639075303995848),
            (4.0, 0.04874886223803079),
            (6.0, 0.017235900604692805),
            (8.0, 0.0036429431289647296),
            (10.0, 0.0003431845960334517),
            (12.0, 9.052589122173602e-6),
            (14.0, 3.197767175455164e-8),
        ];
        for (db, want) in table {
            assert_rel(dqpsk_ber(db), want, 1e-12, &format!("DQPSK at {db} dB"));
        }
        assert_rel(
            dqpsk_ber(9.197822982008024),
            1e-3,
            1e-10,
            "DQPSK at its 1e-3 point",
        );
    }

    #[test]
    fn noncoherent_2fsk_is_half_exp() {
        for tenth in 0..=140 {
            let db = f64::from(tenth) * 0.1;
            let want = 0.5 * (-0.5 * 10f64.powf(db / 10.0)).exp();
            assert_rel(
                mfsk_noncoherent_ser(2, db),
                want,
                1e-13,
                &format!("2-FSK at {db} dB"),
            );
            assert_eq!(
                mfsk_noncoherent_ber(2, db),
                mfsk_noncoherent_ser(2, db),
                "binary: every symbol error is the bit error"
            );
        }
        assert_rel(
            mfsk_noncoherent_ber(2, 10.94443742308721),
            1e-3,
            1e-10,
            "2-FSK 1e-3 point",
        );
    }

    #[test]
    fn mfsk_alternating_sum_survives_cancellation() {
        assert_rel(
            mfsk_noncoherent_ser(64, 0.0),
            0.29641064049182236,
            1e-13,
            "64-FSK at 0 dB",
        );
        assert_rel(
            mfsk_noncoherent_ser(64, 6.0),
            0.0001696329205128982,
            1e-13,
            "64-FSK at 6 dB",
        );
        assert_rel(
            mfsk_noncoherent_ser(64, 8.0),
            1.8445695709968937e-7,
            1e-13,
            "64-FSK at 8 dB",
        );
        assert_rel(
            mfsk_noncoherent_ser(16, 8.0),
            2.3508202502470605e-5,
            1e-13,
            "16-FSK at 8 dB",
        );
        assert_rel(
            mfsk_noncoherent_ser(4, 8.0),
            0.0025255900294627294,
            1e-13,
            "4-FSK at 8 dB",
        );
        assert_rel(
            mfsk_noncoherent_ber(4, 8.0),
            0.0016837266863084864,
            1e-13,
            "4-FSK BER at 8 dB",
        );
    }

    #[test]
    fn both_evaluations_of_the_orthogonal_oracle_agree() {
        for m in [2u32, 4, 8, 16, 32, 64] {
            let k = f64::from(m.ilog2());
            for tenth in -20..=200 {
                let db = f64::from(tenth) * 0.1;
                let exact = super::orthogonal_ser_binomial(m, k, super::ebn0_lin(db));
                if exact < 1e-12 {
                    break;
                }
                let quadrature = super::orthogonal_ser_quadrature(m, k * super::ebn0_lin(db));
                assert_rel(
                    quadrature,
                    exact,
                    1e-6,
                    &format!("{m}-ary orthogonal SER at {db} dB"),
                );
            }
        }
    }

    #[test]
    fn the_large_alphabet_oracle_is_ordered_in_both_arguments() {
        for db in [0.0f64, 2.0, 4.0, 6.0] {
            let mut previous = mfsk_noncoherent_ser(64, db);
            for sf in 7..=12u32 {
                let ser = mfsk_noncoherent_ser(1 << sf, db);
                assert!(
                    ser.is_finite() && (0.0..=1.0).contains(&ser),
                    "SF{sf} at {db} dB: SER {ser}"
                );
                assert!(
                    ser < previous,
                    "SF{sf} at {db} dB: SER {ser:e} did not improve on {previous:e}"
                );
                previous = ser;
            }
        }
        let mut previous = f64::INFINITY;
        for tenth in 0..=120 {
            let ser = mfsk_noncoherent_ser(4096, f64::from(tenth) * 0.1);
            assert!(
                ser < previous,
                "SF12 SER not decreasing at {tenth} tenths dB"
            );
            previous = ser;
        }
    }

    #[test]
    fn mfsk_bit_conversion_is_the_orthogonal_factor() {
        for (m, factor) in [(4u32, 2.0 / 3.0), (16, 8.0 / 15.0), (64, 32.0 / 63.0)] {
            let ratio = mfsk_noncoherent_ber(m, 5.0) / mfsk_noncoherent_ser(m, 5.0);
            assert_rel(ratio, factor, 1e-14, &format!("{m}-FSK bit factor"));
        }
    }

    fn assert_strictly_decreasing(f: &dyn Fn(f64) -> f64, what: &str) {
        let mut prev = f(0.0);
        for tenth in 1..=140 {
            let db = f64::from(tenth) * 0.1;
            let cur = f(db);
            assert!(
                cur < prev,
                "{what} not strictly decreasing at {db} dB: {cur:e} !< {prev:e}"
            );
            prev = cur;
        }
    }

    #[test]
    fn every_curve_strictly_decreases_over_the_sweep_range() {
        assert_strictly_decreasing(&bpsk_ber, "BPSK");
        assert_strictly_decreasing(&qpsk_ber, "QPSK");
        assert_strictly_decreasing(&dbpsk_ber, "DBPSK");
        assert_strictly_decreasing(&dqpsk_ber, "DQPSK");
        for m in [2u32, 4, 8] {
            assert_strictly_decreasing(&|db| mpam_ser(m, db), &format!("{m}-PAM SER"));
            assert_strictly_decreasing(&|db| mpam_ber(m, db), &format!("{m}-PAM BER"));
        }
        for m in [4u32, 16, 64, 256, 1024] {
            assert_strictly_decreasing(&|db| mqam_ser(m, db), &format!("{m}-QAM SER"));
            assert_strictly_decreasing(&|db| mqam_ber(m, db), &format!("{m}-QAM BER"));
        }
        for m in [2u32, 4, 8, 16] {
            assert_strictly_decreasing(&|db| mpsk_ser(m, db), &format!("{m}-PSK SER"));
        }
        for m in [2u32, 4, 16, 64] {
            assert_strictly_decreasing(&|db| mfsk_noncoherent_ser(m, db), &format!("{m}-FSK SER"));
            assert_strictly_decreasing(&|db| mfsk_noncoherent_ber(m, db), &format!("{m}-FSK BER"));
        }
    }

    #[test]
    fn analog_figures_of_merit_match_published_values() {
        assert_rel(am_fom(1.0, 0.5), 1.0 / 3.0, 1e-12, "AM at full depth");
        let full = 10.0 * am_fom(1.0, 0.5).log10();
        assert_rel(full, -4.771, 1e-3, "AM full-depth penalty in dB");
        let broadcast = 10.0 * am_fom(0.8, 0.5).log10();
        assert_rel(broadcast, -6.150, 1e-3, "AM 0.8-depth penalty in dB");
        assert!((ssb_fom() - 1.0).abs() < 1e-15);
        assert_rel(
            10.0 * fm_fom(5.0, 0.5).log10(),
            15.740,
            1e-3,
            "WFM improvement",
        );
        assert_rel(
            10.0 * fm_fom(2.5 / 3.0, 0.5).log10(),
            0.1773,
            1e-2,
            "NFM improvement",
        );
        let over_am = 10.0 * (fm_fom(5.0, 0.5) / am_fom(0.8, 0.5)).log10();
        assert_rel(over_am, 21.890, 1e-3, "WFM over broadcast-depth AM");
        let gap = 10.0 * (fm_fom(1.0, 0.5) / pm_fom(1.0, 0.5)).log10();
        assert_rel(gap, 4.771, 1e-3, "FM over PM at equal deviation");
        assert_rel(
            analog_sinad_db(1.0, 20.0),
            20.0,
            1e-12,
            "unity FoM passes SNR through",
        );
        assert_rel(
            analog_sinad_db(fm_fom(5.0, 0.5), 10.0),
            25.740,
            1e-3,
            "WFM at 10 dB channel SNR",
        );
    }
}
