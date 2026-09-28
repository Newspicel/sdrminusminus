const CLASSIC_STOPS: [[f64; 3]; 15] = [
    [0.0, 0.0, 0.12549],
    [0.0, 0.0, 0.18824],
    [0.0, 0.0, 0.31373],
    [0.0, 0.0, 0.56863],
    [0.11765, 0.56471, 1.0],
    [1.0, 1.0, 1.0],
    [1.0, 1.0, 0.0],
    [0.99608, 0.42745, 0.08627],
    [0.99608, 0.42745, 0.08627],
    [1.0, 0.0, 0.0],
    [1.0, 0.0, 0.0],
    [0.77647, 0.0, 0.0],
    [0.62353, 0.0, 0.0],
    [0.45882, 0.0, 0.0],
    [0.2902, 0.0, 0.0],
];

pub(crate) const LUT_SIZE: usize = 256;

pub(crate) fn sample(t: f64) -> [f64; 3] {
    let x = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let last = CLASSIC_STOPS.len() - 1;
    let scaled = x * last as f64;
    let index = (scaled.floor() as usize).min(last - 1);
    let fraction = scaled - index as f64;
    let (low, high) = (CLASSIC_STOPS[index], CLASSIC_STOPS[index + 1]);
    std::array::from_fn(|channel| (high[channel] - low[channel]).mul_add(fraction, low[channel]))
}

pub(crate) fn lut() -> [[u8; 4]; LUT_SIZE] {
    std::array::from_fn(|index| {
        let [r, g, b] = sample(index as f64 / (LUT_SIZE - 1) as f64).map(to_byte);
        [r, g, b, u8::MAX]
    })
}

fn to_byte(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * f64::from(u8::MAX)).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colormap_endpoints() {
        let lut = lut();
        assert_eq!(lut[0], [0, 0, 32, 255]);
        assert_eq!(lut[255], [74, 0, 0, 255]);
    }

    fn close(actual: [f64; 3], expected: [f64; 3]) -> bool {
        actual
            .iter()
            .zip(expected)
            .all(|(a, e)| (a - e).abs() < 1e-9)
    }

    #[test]
    fn classic_matches_the_web_stops_between_endpoints() {
        for (index, stop) in CLASSIC_STOPS.iter().enumerate() {
            assert!(close(sample(index as f64 / 14.0), *stop), "stop {index}");
        }
        assert!(close(sample(4.5 / 14.0), [0.558_825, 0.782_355, 1.0]));
    }

    #[test]
    fn out_of_range_input_is_clamped() {
        assert_eq!(sample(-1.0), sample(0.0));
        assert_eq!(sample(2.0), sample(1.0));
        assert_eq!(sample(f64::NAN), sample(0.0));
        assert!(lut().iter().all(|rgba| rgba[3] == u8::MAX));
    }
}
