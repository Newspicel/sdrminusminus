const MAX_ARGUMENT: f64 = 64.0;
const TINY_ARGUMENT: f64 = 1e-8;
const OVERFLOW: f64 = 1e250;
const RESCALE: f64 = 1e-250;

#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
pub enum SpecialError {
    #[error("bessel argument {0} is outside 0..=64")]
    BesselRange(f64),
}

pub fn bessel_j(order: u32, x: f64) -> Result<f64, SpecialError> {
    if !(0.0..=MAX_ARGUMENT).contains(&x) {
        return Err(SpecialError::BesselRange(x));
    }
    if x == 0.0 {
        return Ok(if order == 0 { 1.0 } else { 0.0 });
    }
    if x < TINY_ARGUMENT {
        return Ok(small_argument(order, x));
    }
    Ok(miller(order, x))
}

fn small_argument(order: u32, x: f64) -> f64 {
    (1..=order).fold(1.0, |term, k| term * (0.5 * x) / f64::from(k))
}

fn start_index(order: u32, x: f64) -> u64 {
    let m = f64::from(order);
    let reach = m.max(x.ceil()) + 16.0 + (40.0 * m.max(x)).sqrt().floor();
    2 * (reach / 2.0).floor() as u64
}

fn miller(order: u32, x: f64) -> f64 {
    let top = start_index(order, x);
    let order = u64::from(order);
    let mut above = 0.0f64;
    let mut current = 1.0f64;
    let mut even_sum = if top.is_multiple_of(2) { current } else { 0.0 };
    let mut wanted = if order == top { current } else { 0.0 };
    for k in (1..=top).rev() {
        let below = 2.0 * k as f64 / x * current - above;
        above = current;
        current = below;
        let index = k - 1;
        if index == order {
            wanted = current;
        }
        if index > 0 && index.is_multiple_of(2) {
            even_sum += current;
        }
        if current.abs() > OVERFLOW {
            above *= RESCALE;
            current *= RESCALE;
            even_sum *= RESCALE;
            wanted *= RESCALE;
        }
    }
    wanted / (current + 2.0 * even_sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bessel_matches_reference_values() {
        for (order, x, want) in [
            (0, 1.0, 0.765_197_686_6),
            (1, 1.0, 0.440_050_585_7),
            (2, 3.0, 0.486_091_260_6),
            (5, 10.0, -0.234_061_528_2),
        ] {
            let got = bessel_j(order, x).unwrap();
            assert!((got - want).abs() < 1e-9, "J{order}({x}) = {got}");
        }
    }

    #[test]
    fn bessel_covers_the_edges_of_its_range() {
        assert_eq!(bessel_j(0, 0.0), Ok(1.0));
        assert_eq!(bessel_j(3, 0.0), Ok(0.0));
        assert_eq!(bessel_j(0, -0.1), Err(SpecialError::BesselRange(-0.1)));
        assert_eq!(bessel_j(0, 64.5), Err(SpecialError::BesselRange(64.5)));
        assert!(bessel_j(0, f64::NAN).is_err());
        let tiny = bessel_j(2, 1e-9).unwrap();
        assert!((tiny - 1.25e-19).abs() < 1e-30);
        let far = bessel_j(0, 64.0).unwrap();
        assert!((far - 0.092_590_012_216).abs() < 1e-9, "J0(64) = {far}");
    }

    #[test]
    fn a_high_order_keeps_its_precision() {
        let value = bessel_j(40, 1.0).unwrap();
        let want = 1.107_915_851_128_632_7e-60;
        assert!(((value - want) / want).abs() < 1e-9, "J40(1) = {value}");
    }

    #[test]
    fn neighbouring_orders_obey_the_recurrence() {
        let x = 7.3;
        for order in 1..20u32 {
            let below = bessel_j(order - 1, x).unwrap();
            let here = bessel_j(order, x).unwrap();
            let above = bessel_j(order + 1, x).unwrap();
            let residual = below + above - 2.0 * f64::from(order) / x * here;
            assert!(residual.abs() < 1e-12, "order {order}: {residual}");
        }
    }
}
