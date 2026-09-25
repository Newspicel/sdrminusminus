use std::fmt::Write as _;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Colormap {
    #[default]
    Classic,
    Magma,
    Inferno,
    Plasma,
    Viridis,
    Gray,
}

pub const COLORMAPS: [Colormap; 6] = [
    Colormap::Classic,
    Colormap::Magma,
    Colormap::Inferno,
    Colormap::Plasma,
    Colormap::Viridis,
    Colormap::Gray,
];

pub type Rgb = [f64; 3];

const CLASSIC_STOPS: [Rgb; 15] = [
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

const MAGMA: [Rgb; 7] = [
    [-0.00213649, -0.00074966, -0.00538613],
    [0.25166054, 0.67752324, 2.4940266],
    [8.35371728, -3.57771951, 0.3144679],
    [-27.66873309, 14.26473078, -13.64921319],
    [52.17613981, -27.94360607, 12.94416944],
    [-50.76852536, 29.04658282, 4.23415299],
    [18.65570507, -11.48977352, -5.60196151],
];

const INFERNO: [Rgb; 7] = [
    [0.00021894, 0.001651, -0.0194809],
    [0.10651342, 0.56395644, 3.93271239],
    [11.60249308, -3.97285397, -15.94239411],
    [-41.70399613, 17.43639888, 44.3541452],
    [77.1629357, -33.40235894, -81.80730926],
    [-71.31942824, 32.62606426, 73.20951986],
    [25.13112622, -12.24266895, -23.070325],
];

const PLASMA: [Rgb; 7] = [
    [0.05873234, 0.02333671, 0.54334018],
    [2.17651463, 0.23838342, 0.75396046],
    [-2.68946048, -7.45585114, 3.11079994],
    [6.13034835, 42.34618815, -28.51885465],
    [-11.10743619, -82.66631109, 60.13984767],
    [10.02306558, 71.4136177, -54.07218656],
    [-3.65871384, -22.93153465, 18.19190779],
];

const VIRIDIS: [Rgb; 7] = [
    [0.27772733, 0.00540734, 0.33409981],
    [0.10509304, 1.40461353, 1.38459016],
    [-0.33086183, 0.21484756, 0.09509516],
    [-4.6342305, -5.79910097, -19.33244096],
    [6.22826994, 14.17993337, 56.6905520],
    [4.775385, -13.74514538, -65.35303263],
    [-5.43545586, 4.64585261, 26.31243525],
];

impl Colormap {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Magma => "magma",
            Self::Inferno => "inferno",
            Self::Plasma => "plasma",
            Self::Viridis => "viridis",
            Self::Gray => "gray",
        }
    }

    #[must_use]
    pub fn index(self) -> u32 {
        COLORMAPS.iter().position(|map| *map == self).unwrap_or(0) as u32
    }

    #[must_use]
    pub fn from_index(index: u8) -> Self {
        COLORMAPS
            .get(usize::from(index))
            .copied()
            .unwrap_or_default()
    }

    fn polynomial(self) -> Option<&'static [Rgb; 7]> {
        match self {
            Self::Magma => Some(&MAGMA),
            Self::Inferno => Some(&INFERNO),
            Self::Plasma => Some(&PLASMA),
            Self::Viridis => Some(&VIRIDIS),
            Self::Classic | Self::Gray => None,
        }
    }

    #[must_use]
    pub fn sample(self, t: f64) -> Rgb {
        let x = if t.is_finite() {
            t.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if self == Self::Gray {
            return [x, x, x];
        }
        let Some(poly) = self.polynomial() else {
            return classic(x);
        };
        let mut out = [0.0; 3];
        for (channel, slot) in out.iter_mut().enumerate() {
            let value = poly
                .iter()
                .rev()
                .fold(0.0, |acc, row| row[channel] + x * acc);
            *slot = value.clamp(0.0, 1.0);
        }
        out
    }

    #[must_use]
    pub fn lut(self) -> Vec<u8> {
        (0..256)
            .flat_map(|step| {
                self.sample(f64::from(step) / 255.0)
                    .map(|channel| (channel * 255.0).round() as u8)
            })
            .collect()
    }

    #[must_use]
    pub fn gradient(self, direction: &str) -> String {
        const STOPS: u32 = 8;
        let stops: Vec<String> = (0..STOPS)
            .map(|at| {
                let [r, g, b] = self.sample(f64::from(at) / f64::from(STOPS - 1));
                format!(
                    "rgb({} {} {})",
                    (r * 255.0).round(),
                    (g * 255.0).round(),
                    (b * 255.0).round()
                )
            })
            .collect();
        format!("linear-gradient({direction}, {})", stops.join(", "))
    }
}

fn classic(t: f64) -> Rgb {
    let x = t * (CLASSIC_STOPS.len() - 1) as f64;
    let at = (x.floor() as usize).min(CLASSIC_STOPS.len() - 2);
    let (low, high) = (CLASSIC_STOPS[at], CLASSIC_STOPS[at + 1]);
    let f = x - at as f64;
    [
        low[0] + (high[0] - low[0]) * f,
        low[1] + (high[1] - low[1]) * f,
        low[2] + (high[2] - low[2]) * f,
    ]
}

fn wgsl_vec3([r, g, b]: Rgb) -> String {
    format!("vec3<f32>({r:.8}, {g:.8}, {b:.8})")
}

#[must_use]
pub fn colormap_wgsl() -> String {
    let mut source = String::new();
    let stops: Vec<String> = CLASSIC_STOPS.iter().map(|stop| wgsl_vec3(*stop)).collect();
    let count = CLASSIC_STOPS.len();
    let _ = writeln!(
        source,
        "var<private> CLASSIC: array<vec3<f32>, {count}> = array<vec3<f32>, {count}>(\n    {}\n);",
        stops.join(",\n    ")
    );
    source.push_str(
        "fn poly(t: f32, c0: vec3<f32>, c1: vec3<f32>, c2: vec3<f32>, c3: vec3<f32>, c4: vec3<f32>, c5: vec3<f32>, c6: vec3<f32>) -> vec3<f32> {\n    return clamp(c0 + t * (c1 + t * (c2 + t * (c3 + t * (c4 + t * (c5 + t * c6))))), vec3<f32>(0.0), vec3<f32>(1.0));\n}\n",
    );
    let _ = writeln!(
        source,
        "fn classic(t: f32) -> vec3<f32> {{\n    let x = t * {:.1};\n    let i = min(i32(floor(x)), {});\n    return mix(CLASSIC[i], CLASSIC[i + 1], x - f32(i));\n}}",
        (count - 1) as f64,
        count - 2
    );
    source.push_str(
        "fn colormap(level: f32, map: u32) -> vec3<f32> {\n    let t = clamp(level, 0.0, 1.0);\n",
    );
    for map in COLORMAPS {
        if let Some(poly) = map.polynomial() {
            let terms: Vec<String> = poly.iter().map(|row| wgsl_vec3(*row)).collect();
            let _ = writeln!(
                source,
                "    if (map == {}u) {{ return poly(t, {}); }}",
                map.index(),
                terms.join(", ")
            );
        }
    }
    let _ = writeln!(
        source,
        "    if (map == {}u) {{ return vec3<f32>(t); }}\n    return classic(t);\n}}",
        Colormap::Gray.index()
    );
    source
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ramp_stays_inside_the_unit_cube() {
        for map in COLORMAPS {
            for step in 0..=64 {
                for channel in map.sample(f64::from(step) / 64.0) {
                    assert!((0.0..=1.0).contains(&channel), "{map:?}");
                }
            }
        }
    }

    #[test]
    fn out_of_range_inputs_clamp_to_the_ends() {
        assert_eq!(Colormap::Gray.sample(-1.0), Colormap::Gray.sample(0.0));
        assert_eq!(Colormap::Gray.sample(2.0), Colormap::Gray.sample(1.0));
        assert_eq!(Colormap::Gray.sample(f64::NAN), Colormap::Gray.sample(0.0));
    }

    #[test]
    fn classic_ends_on_its_first_and_last_stop() {
        assert_eq!(Colormap::Classic.sample(0.0), [0.0, 0.0, 0.12549]);
        let last = Colormap::Classic.sample(1.0);
        assert!((last[0] - 0.2902).abs() < 1e-12 && last[1] == 0.0 && last[2] == 0.0);
    }

    #[test]
    fn gray_rises_monotonically() {
        let mut previous = -1.0;
        for step in 0..=32 {
            let [value, _, _] = Colormap::Gray.sample(f64::from(step) / 32.0);
            assert!(value > previous);
            previous = value;
        }
    }

    #[test]
    fn every_ramp_has_a_distinct_midpoint() {
        let seen: std::collections::HashSet<String> = COLORMAPS
            .iter()
            .map(|map| format!("{:?}", map.sample(0.5)))
            .collect();
        assert_eq!(seen.len(), COLORMAPS.len());
    }

    #[test]
    fn the_index_order_is_the_one_the_shader_switches_on() {
        let names: Vec<&str> = COLORMAPS.iter().map(|map| map.name()).collect();
        assert_eq!(
            names,
            ["classic", "magma", "inferno", "plasma", "viridis", "gray"]
        );
        assert_eq!(Colormap::default().index(), 0);
        assert_eq!(Colormap::from_index(4), Colormap::Viridis);
        assert_eq!(Colormap::from_index(99), Colormap::Classic);
    }

    #[test]
    fn the_shader_branches_on_each_polynomial_ramp() {
        let source = colormap_wgsl();
        for map in [
            Colormap::Magma,
            Colormap::Inferno,
            Colormap::Plasma,
            Colormap::Viridis,
        ] {
            assert!(source.contains(&format!("if (map == {}u) {{ return poly(t,", map.index())));
        }
        assert!(source.contains("if (map == 5u) { return vec3<f32>(t); }"));
        assert!(
            source.contains("var<private> CLASSIC: array<vec3<f32>, 15> = array<vec3<f32>, 15>(")
        );
        assert!(source.contains("vec3<f32>(0.00000000, 0.00000000, 0.12549000)"));
    }

    #[test]
    fn a_lookup_table_holds_256_triples_matching_the_ends() {
        let lut = Colormap::Gray.lut();
        assert_eq!(lut.len(), 768);
        assert_eq!(lut[..3], [0, 0, 0]);
        assert_eq!(lut[765..], [255, 255, 255]);
    }

    #[test]
    fn a_gradient_names_eight_stops() {
        let gradient = Colormap::Gray.gradient("to right");
        assert!(gradient.starts_with("linear-gradient(to right, rgb(0 0 0)"));
        assert!(gradient.ends_with("rgb(255 255 255))"));
        assert_eq!(gradient.matches("rgb(").count(), 8);
    }
}
