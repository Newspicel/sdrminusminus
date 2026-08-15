mod cpm;
mod nyquist;

pub use cpm::{gaussian, gaussian_freq, half_sine, lrc, lrec, phase_pulse, rect};
pub use nyquist::{raised_cosine, root_raised_cosine};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Norm {
    Energy,
    Area,
}

fn normalise(mut h: Vec<f64>, norm: Norm) -> Vec<f32> {
    let scale = match norm {
        Norm::Energy => h.iter().map(|v| v * v).sum::<f64>().sqrt().recip(),
        Norm::Area => h.iter().sum::<f64>().recip(),
    };
    assert!(
        scale.is_finite(),
        "pulse shape has no energy/area to normalise"
    );
    for v in &mut h {
        *v *= scale;
    }
    h.into_iter().map(|v| v as f32).collect()
}

fn renorm_designed(taps: Vec<f32>, norm: Norm) -> Vec<f32> {
    match norm {
        Norm::Area => taps,
        Norm::Energy => {
            let energy: f64 = taps.iter().map(|&h| f64::from(h) * f64::from(h)).sum();
            let scale = energy.sqrt().recip();
            taps.iter()
                .map(|&h| (f64::from(h) * scale) as f32)
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Entry = (&'static str, Box<dyn Fn(Norm) -> Vec<f32>>);

    fn catalog() -> Vec<Entry> {
        vec![
            ("rect", Box::new(|n| rect(8.0, n))),
            ("half_sine", Box::new(|n| half_sine(10.0, n))),
            ("lrec(3)", Box::new(|n| lrec(8.0, 3, n))),
            ("lrc(2)", Box::new(|n| lrc(8.0, 2, n))),
            ("lrc(2) fractional sps", Box::new(|n| lrc(6.4, 2, n))),
            (
                "raised_cosine",
                Box::new(|n| raised_cosine(8.0, 0.35, 6, n)),
            ),
            (
                "root_raised_cosine",
                Box::new(|n| root_raised_cosine(8.0, 0.2, 8, n)),
            ),
            ("gaussian", Box::new(|n| gaussian(8.0, 0.5, 3, n))),
            (
                "gaussian_freq",
                Box::new(|n| gaussian_freq(10.0, 0.3, 4, n)),
            ),
        ]
    }

    #[test]
    fn every_pulse_is_unit_energy_under_energy_norm() {
        for (name, build) in catalog() {
            let taps = build(Norm::Energy);
            let energy: f64 = taps.iter().map(|&h| f64::from(h) * f64::from(h)).sum();
            assert!((energy - 1.0).abs() < 1e-5, "{name}: Σh² = {energy}");
        }
    }

    #[test]
    fn every_pulse_is_unit_area_under_area_norm() {
        for (name, build) in catalog() {
            let taps = build(Norm::Area);
            let area: f64 = taps.iter().map(|&h| f64::from(h)).sum();
            assert!((area - 1.0).abs() < 1e-5, "{name}: Σh = {area}");
        }
    }

    #[test]
    fn the_two_normalisations_are_exact_scalings_of_one_shape() {
        for (name, build) in catalog() {
            let e = build(Norm::Energy);
            let a = build(Norm::Area);
            assert_eq!(e.len(), a.len(), "{name}");
            let peak = e
                .iter()
                .enumerate()
                .max_by(|(_, x), (_, y)| x.abs().total_cmp(&y.abs()))
                .map(|(i, _)| i)
                .unwrap();
            let ratio = f64::from(a[peak]) / f64::from(e[peak]);
            for (&he, &ha) in e.iter().zip(&a) {
                let err = f64::from(ha) - f64::from(he) * ratio;
                assert!(err.abs() < 1e-6, "{name}: shapes diverge by {err}");
            }
        }
    }
}
