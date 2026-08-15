use crate::soft::SoftBit;

#[derive(Clone, Debug, PartialEq)]
pub struct Mapping {
    levels: Vec<f32>,
    sorted: Vec<(f32, u8)>,
    max_level: f32,
    min_spacing: f32,
}

impl Mapping {
    #[must_use]
    pub fn new(levels: Vec<f32>) -> Self {
        let m = levels.len();
        assert!(
            m >= 2 && m.is_power_of_two() && m <= 256,
            "M must be a power of two in 2..=256, got {m}"
        );
        assert!(
            levels.iter().all(|l| l.is_finite()),
            "levels must be finite"
        );
        let mut sorted: Vec<(f32, u8)> = levels
            .iter()
            .enumerate()
            .map(|(i, &l)| (l, i as u8))
            .collect();
        sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
        let min_spacing = sorted
            .windows(2)
            .map(|w| w[1].0 - w[0].0)
            .fold(f32::INFINITY, f32::min);
        assert!(min_spacing > 0.0, "levels must be distinct");
        let max_level = sorted.iter().map(|&(l, _)| l.abs()).fold(0.0f32, f32::max);
        assert!(max_level > 0.0, "at least one level must be nonzero");
        Self {
            levels,
            sorted,
            max_level,
            min_spacing,
        }
    }

    #[must_use]
    pub fn natural(m: usize) -> Self {
        Self::new(
            (0..m)
                .map(|i| (2 * i as i32 - (m as i32 - 1)) as f32)
                .collect(),
        )
    }

    #[must_use]
    pub fn gray(m: usize) -> Self {
        let mut levels = vec![0.0f32; m];
        for (rank, level) in (0..m)
            .map(|i| (2 * i as i32 - (m as i32 - 1)) as f32)
            .enumerate()
        {
            levels[rank ^ (rank >> 1)] = level;
        }
        Self::new(levels)
    }

    #[must_use]
    pub fn m(&self) -> usize {
        self.levels.len()
    }

    #[must_use]
    pub fn bits_per_symbol(&self) -> u32 {
        self.levels.len().trailing_zeros()
    }

    #[must_use]
    pub fn levels(&self) -> &[f32] {
        &self.levels
    }

    #[must_use]
    pub fn level(&self, index: u8) -> f32 {
        self.levels[index as usize & (self.levels.len() - 1)]
    }

    #[must_use]
    pub fn max_level(&self) -> f32 {
        self.max_level
    }

    #[must_use]
    pub fn min_spacing(&self) -> f32 {
        self.min_spacing
    }

    #[must_use]
    pub fn slice(&self, y: f32) -> u8 {
        let at = self.sorted.partition_point(|&(l, _)| l < y);
        match (self.sorted.get(at.wrapping_sub(1)), self.sorted.get(at)) {
            (Some(&(lo, i)), Some(&(hi, j))) => {
                if y - lo <= hi - y {
                    i
                } else {
                    j
                }
            }
            (Some(&(_, i)), None) | (None, Some(&(_, i))) => i,
            (None, None) => 0,
        }
    }

    pub fn soft_bits(&self, y: f32, out: &mut Vec<SoftBit>) {
        let scale = 0.5 / (self.min_spacing * self.min_spacing);
        for k in (0..self.bits_per_symbol()).rev() {
            let (mut d0, mut d1) = (f32::INFINITY, f32::INFINITY);
            for (i, &level) in self.levels.iter().enumerate() {
                let d = (y - level) * (y - level);
                if i >> k & 1 == 0 {
                    d0 = d0.min(d);
                } else {
                    d1 = d1.min(d);
                }
            }
            out.push(SoftBit(((d0 - d1) * scale).clamp(-1.0, 1.0)));
        }
    }
}

#[derive(Clone, Debug)]
pub struct CpmParams {
    mapping: Mapping,
    h: f64,
    freq_pulse: Vec<f32>,
    sps: f64,
}

impl CpmParams {
    #[must_use]
    pub fn from_h(mapping: Mapping, h: f64, freq_pulse: Vec<f32>, sps: f64) -> Self {
        assert!(
            h.is_finite() && h > 0.0,
            "modulation index must be positive"
        );
        assert!(
            sps.is_finite() && sps >= 2.0,
            "need at least two samples per symbol"
        );
        assert!(
            h * f64::from(mapping.max_level()) < sps,
            "outer deviation exceeds Nyquist: h·max_level = {} at {sps} samples/symbol",
            h * f64::from(mapping.max_level())
        );
        assert!(!freq_pulse.is_empty(), "frequency pulse must have taps");
        assert!(
            freq_pulse.iter().all(|t| t.is_finite()),
            "frequency pulse taps must be finite"
        );
        let area: f64 = freq_pulse.iter().map(|&t| f64::from(t)).sum();
        assert!(
            (area - 1.0).abs() < 1e-3,
            "frequency pulse must be unit-area (pulse::Norm::Area), got Σ = {area}"
        );
        Self {
            mapping,
            h,
            freq_pulse,
            sps,
        }
    }

    #[must_use]
    pub fn from_deviation(
        mapping: Mapping,
        deviation_hz: f64,
        baud: f64,
        freq_pulse: Vec<f32>,
        sps: f64,
    ) -> Self {
        assert!(
            deviation_hz.is_finite() && deviation_hz > 0.0,
            "deviation must be positive"
        );
        assert!(baud.is_finite() && baud > 0.0, "baud must be positive");
        let h = 2.0 * deviation_hz / (f64::from(mapping.max_level()) * baud);
        Self::from_h(mapping, h, freq_pulse, sps)
    }

    #[must_use]
    pub fn mapping(&self) -> &Mapping {
        &self.mapping
    }

    #[must_use]
    pub fn h(&self) -> f64 {
        self.h
    }

    #[must_use]
    pub fn freq_pulse(&self) -> &[f32] {
        &self.freq_pulse
    }

    #[must_use]
    pub fn sps(&self) -> f64 {
        self.sps
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pulse::{self, Norm};

    fn dmr_mapping() -> Mapping {
        Mapping::new(vec![1.0, 3.0, -1.0, -3.0])
    }

    #[test]
    fn the_dmr_table_reproduces_fsk4s_slicer() {
        let map = dmr_mapping();
        for (y, want) in [
            (2.5, 0b01),
            (3.5, 0b01),
            (1.9, 0b00),
            (0.1, 0b00),
            (-0.1, 0b10),
            (-1.9, 0b10),
            (-2.1, 0b11),
            (-9.0, 0b11),
        ] {
            assert_eq!(map.slice(y), want, "slice({y})");
        }
    }

    #[test]
    fn soft_bits_match_the_fsk4_calibration_on_the_dmr_table() {
        let map = dmr_mapping();
        let mut out = Vec::new();
        map.soft_bits(3.0, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, -1.0);
        assert!((out[1].0 - 0.5).abs() < 1e-6);
        out.clear();
        map.soft_bits(1.0, &mut out);
        assert!((out[0].0 + 0.5).abs() < 1e-6);
        assert!((out[1].0 + 0.5).abs() < 1e-6);
        out.clear();
        map.soft_bits(2.0, &mut out);
        assert_eq!(out[1].0, 0.0);
        out.clear();
        map.soft_bits(-3.0, &mut out);
        assert_eq!(out[0].0, 1.0);
        assert!((out[1].0 - 0.5).abs() < 1e-6);
    }

    #[test]
    fn an_inverted_two_level_table_demaps_through_the_table_not_the_sign() {
        let map = Mapping::new(vec![1.0, -1.0]);
        let mut out = Vec::new();
        map.soft_bits(-1.0, &mut out);
        assert!(out[0].0 > 0.0, "mark at level −1 must vote for bit 1");
        assert_eq!(map.slice(-1.0), 1);
        assert_eq!(map.slice(1.0), 0);
    }

    #[test]
    fn natural_and_gray_orders_lay_out_the_odd_integers() {
        let nat = Mapping::natural(4);
        assert_eq!(nat.levels(), &[-3.0, -1.0, 1.0, 3.0]);
        let gray = Mapping::gray(4);
        assert_eq!(gray.levels(), &[-3.0, -1.0, 3.0, 1.0]);
        let mut prev: Option<u8> = None;
        for &(_, i) in &gray.sorted {
            if let Some(p) = prev {
                assert_eq!((i ^ p).count_ones(), 1, "gray step {p} -> {i}");
            }
            prev = Some(i);
        }
        assert_eq!(Mapping::natural(2).levels(), &[-1.0, 1.0]);
    }

    #[test]
    fn eight_level_slicing_and_scale() {
        let map = Mapping::natural(8);
        assert_eq!(map.m(), 8);
        assert_eq!(map.bits_per_symbol(), 3);
        assert_eq!(map.max_level(), 7.0);
        assert_eq!(map.min_spacing(), 2.0);
        assert_eq!(map.slice(6.7), 7);
        assert_eq!(map.slice(-4.2), 1);
        let mut out = Vec::new();
        map.soft_bits(7.0, &mut out);
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|b| b.0 > 0.0), "clean +7 is index 0b111");
    }

    #[test]
    fn deviation_conversion_reproduces_the_documented_formula() {
        let p = CpmParams::from_deviation(
            dmr_mapping(),
            1_944.0,
            4_800.0,
            pulse::root_raised_cosine(10.0, 0.2, 8, Norm::Area),
            10.0,
        );
        assert!((p.h() - 0.27).abs() < 1e-12, "h = {}", p.h());
        let ais = CpmParams::from_deviation(
            Mapping::natural(2),
            2_400.0,
            9_600.0,
            pulse::gaussian_freq(5.0, 0.4, 3, Norm::Area),
            5.0,
        );
        assert!((ais.h() - 0.5).abs() < 1e-12);
    }

    #[test]
    #[should_panic(expected = "unit-area")]
    fn an_energy_normalised_pulse_is_rejected() {
        let _ = CpmParams::from_h(
            Mapping::natural(2),
            0.5,
            pulse::rect(8.0, Norm::Energy),
            8.0,
        );
    }

    #[test]
    #[should_panic(expected = "power of two")]
    fn a_three_level_table_is_rejected() {
        let _ = Mapping::new(vec![-1.0, 0.0, 1.0]);
    }

    #[test]
    #[should_panic(expected = "distinct")]
    fn coincident_levels_are_rejected() {
        let _ = Mapping::new(vec![1.0, 1.0]);
    }

    #[test]
    #[should_panic(expected = "Nyquist")]
    fn a_deviation_past_nyquist_is_rejected() {
        let _ = CpmParams::from_h(Mapping::natural(4), 4.0, pulse::rect(8.0, Norm::Area), 8.0);
    }
}
