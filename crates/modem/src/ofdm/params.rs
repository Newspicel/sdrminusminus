use std::fmt;

use num_complex::Complex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfdmError {
    FftSize(usize),
    CyclicPrefix { fft: usize, cp: usize },
    NoDataSubcarriers,
    SubcarrierOutOfRange { offset: i32, fft: usize },
    DuplicateSubcarrier(i32),
    NotHermitian(i32),
    ShortStride { fft: usize, stride: usize },
    ShortTrainingEmpty,
    Repeats { what: &'static str, repeats: usize },
    LongGuard { fft: usize, guard: usize },
    EmptyPilotPattern,
    PilotCountMismatch { pilots: usize, values: usize },
}

impl fmt::Display for OfdmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FftSize(n) => write!(f, "FFT size {n} is not a power of two of at least 8"),
            Self::CyclicPrefix { fft, cp } => {
                write!(f, "cyclic prefix {cp} is not in 1..{fft}")
            }
            Self::NoDataSubcarriers => write!(f, "the map carries no data subcarrier"),
            Self::SubcarrierOutOfRange { offset, fft } => {
                write!(f, "subcarrier {offset} is outside a {fft}-point transform")
            }
            Self::DuplicateSubcarrier(k) => write!(f, "subcarrier {k} is mapped twice"),
            Self::NotHermitian(k) => {
                write!(f, "subcarrier {k} cannot be mirrored in a Hermitian map")
            }
            Self::ShortStride { fft, stride } => {
                write!(f, "short-training stride {stride} does not divide {fft}")
            }
            Self::ShortTrainingEmpty => write!(f, "the short-training stride energises no bin"),
            Self::Repeats { what, repeats } => {
                write!(
                    f,
                    "{what} repeats {repeats} times, fewer than the two a \
                    repetition metric needs"
                )
            }
            Self::LongGuard { fft, guard } => {
                write!(
                    f,
                    "long-training guard {guard} exceeds the {fft}-sample symbol"
                )
            }
            Self::EmptyPilotPattern => write!(f, "a pilot pattern needs values and a polarity"),
            Self::PilotCountMismatch { pilots, values } => {
                write!(f, "{pilots} pilot subcarriers but {values} base values")
            }
        }
    }
}

impl std::error::Error for OfdmError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subcarrier {
    pub bin: usize,
    pub offset: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubcarrierMap {
    fft: usize,
    data: Vec<Subcarrier>,
    pilots: Vec<Subcarrier>,
    occupied: Vec<Subcarrier>,
}

impl SubcarrierMap {
    pub fn new(fft: usize, data: &[i32], pilots: &[i32]) -> Result<Self, OfdmError> {
        if !fft.is_power_of_two() || fft < 8 {
            return Err(OfdmError::FftSize(fft));
        }
        if data.is_empty() {
            return Err(OfdmError::NoDataSubcarriers);
        }
        let half = (fft / 2) as i32;
        let mut seen = vec![false; fft];
        let mut carrier = |offset: i32| -> Result<Subcarrier, OfdmError> {
            if offset < -half || offset >= half {
                return Err(OfdmError::SubcarrierOutOfRange { offset, fft });
            }
            let bin = offset.rem_euclid(fft as i32) as usize;
            if std::mem::replace(&mut seen[bin], true) {
                return Err(OfdmError::DuplicateSubcarrier(offset));
            }
            Ok(Subcarrier { bin, offset })
        };
        let mut built_data: Vec<Subcarrier> =
            data.iter().map(|&k| carrier(k)).collect::<Result<_, _>>()?;
        let mut built_pilots: Vec<Subcarrier> = pilots
            .iter()
            .map(|&k| carrier(k))
            .collect::<Result<_, _>>()?;
        built_data.sort_unstable_by_key(|c| c.offset);
        built_pilots.sort_unstable_by_key(|c| c.offset);
        let mut occupied = built_data.clone();
        occupied.extend_from_slice(&built_pilots);
        occupied.sort_unstable_by_key(|c| c.offset);
        Ok(Self {
            fft,
            data: built_data,
            pilots: built_pilots,
            occupied,
        })
    }

    #[must_use]
    pub fn fft(&self) -> usize {
        self.fft
    }

    #[must_use]
    pub fn data(&self) -> &[Subcarrier] {
        &self.data
    }

    #[must_use]
    pub fn pilots(&self) -> &[Subcarrier] {
        &self.pilots
    }

    #[must_use]
    pub fn occupied(&self) -> &[Subcarrier] {
        &self.occupied
    }

    fn is_hermitian_capable(&self) -> Result<(), OfdmError> {
        let half = (self.fft / 2) as i32;
        for c in &self.occupied {
            if c.offset <= 0 || c.offset >= half {
                return Err(OfdmError::NotHermitian(c.offset));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Domain {
    Complex,
    RealHermitian,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preamble {
    pub short_stride: usize,
    pub short_repeats: usize,
    pub long_repeats: usize,
    pub long_guard: usize,
}

impl Preamble {
    #[must_use]
    pub fn short_period(&self, fft: usize) -> usize {
        fft / self.short_stride.max(1)
    }

    #[must_use]
    pub fn samples(&self, fft: usize) -> usize {
        self.short_repeats * self.short_period(fft) + self.long_guard + self.long_repeats * fft
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PilotPattern {
    base: Vec<Complex<f32>>,
    polarity: Vec<f32>,
}

impl PilotPattern {
    pub fn new(base: Vec<Complex<f32>>, polarity: Vec<f32>) -> Result<Self, OfdmError> {
        if base.is_empty() || polarity.is_empty() {
            return Err(OfdmError::EmptyPilotPattern);
        }
        Ok(Self { base, polarity })
    }

    pub fn m_sequence(base: Vec<Complex<f32>>) -> Result<Self, OfdmError> {
        let mut state = 0x7fu8;
        let polarity = (0..127)
            .map(|_| {
                let out = state & 1;
                let feedback = (state ^ (state >> 3)) & 1;
                state = (state >> 1) | (feedback << 6);
                if out == 1 { 1.0 } else { -1.0 }
            })
            .collect();
        Self::new(base, polarity)
    }

    #[must_use]
    pub fn value(&self, index: usize, symbol: usize) -> Complex<f32> {
        self.base[index] * self.polarity[symbol % self.polarity.len()]
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.base.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.base.is_empty()
    }

    #[must_use]
    pub fn polarity(&self) -> &[f32] {
        &self.polarity
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OfdmParams {
    fft: usize,
    cp: usize,
    map: SubcarrierMap,
    pilots: PilotPattern,
    preamble: Preamble,
    domain: Domain,
    short_bins: Vec<Subcarrier>,
}

impl OfdmParams {
    pub fn new(
        cp: usize,
        map: SubcarrierMap,
        pilots: PilotPattern,
        preamble: Preamble,
        domain: Domain,
    ) -> Result<Self, OfdmError> {
        let fft = map.fft();
        if cp == 0 || cp >= fft {
            return Err(OfdmError::CyclicPrefix { fft, cp });
        }
        if map.pilots().len() != pilots.len() {
            return Err(OfdmError::PilotCountMismatch {
                pilots: map.pilots().len(),
                values: pilots.len(),
            });
        }
        if preamble.short_stride == 0 || !fft.is_multiple_of(preamble.short_stride) {
            return Err(OfdmError::ShortStride {
                fft,
                stride: preamble.short_stride,
            });
        }
        for (what, repeats) in [
            ("short training", preamble.short_repeats),
            ("long training", preamble.long_repeats),
        ] {
            if repeats < 2 {
                return Err(OfdmError::Repeats { what, repeats });
            }
        }
        if preamble.long_guard > fft {
            return Err(OfdmError::LongGuard {
                fft,
                guard: preamble.long_guard,
            });
        }
        if domain == Domain::RealHermitian {
            map.is_hermitian_capable()?;
        }
        let short_bins: Vec<Subcarrier> = map
            .occupied()
            .iter()
            .copied()
            .filter(|c| {
                c.offset != 0
                    && (c.offset.unsigned_abs() as usize).is_multiple_of(preamble.short_stride)
            })
            .collect();
        if short_bins.is_empty() {
            return Err(OfdmError::ShortTrainingEmpty);
        }
        Ok(Self {
            fft,
            cp,
            map,
            pilots,
            preamble,
            domain,
            short_bins,
        })
    }

    #[must_use]
    pub fn wifi_like() -> Self {
        const PILOTS: [i32; 4] = [-21, -7, 7, 21];
        let data: Vec<i32> = (-26..=26)
            .filter(|k| *k != 0 && !PILOTS.contains(k))
            .collect();
        let built = SubcarrierMap::new(64, &data, &PILOTS)
            .and_then(|map| {
                let base = vec![
                    Complex::new(1.0, 0.0),
                    Complex::new(1.0, 0.0),
                    Complex::new(1.0, 0.0),
                    Complex::new(-1.0, 0.0),
                ];
                Ok((map, PilotPattern::m_sequence(base)?))
            })
            .and_then(|(map, pilots)| {
                Self::new(
                    16,
                    map,
                    pilots,
                    Preamble {
                        short_stride: 4,
                        short_repeats: 10,
                        long_repeats: 2,
                        long_guard: 32,
                    },
                    Domain::Complex,
                )
            });
        match built {
            Ok(params) => params,
            Err(why) => panic!("the reference OFDM configuration is invalid: {why}"),
        }
    }

    #[must_use]
    pub fn dmt_like() -> Self {
        const PILOTS: [i32; 2] = [7, 21];
        let data: Vec<i32> = (1..=26).filter(|k| !PILOTS.contains(k)).collect();
        let built = SubcarrierMap::new(64, &data, &PILOTS)
            .and_then(|map| {
                let base = vec![Complex::new(1.0, 0.0), Complex::new(-1.0, 0.0)];
                Ok((map, PilotPattern::m_sequence(base)?))
            })
            .and_then(|(map, pilots)| {
                Self::new(
                    16,
                    map,
                    pilots,
                    Preamble {
                        short_stride: 4,
                        short_repeats: 10,
                        long_repeats: 2,
                        long_guard: 32,
                    },
                    Domain::RealHermitian,
                )
            });
        match built {
            Ok(params) => params,
            Err(why) => panic!("the reference DMT configuration is invalid: {why}"),
        }
    }

    #[must_use]
    pub fn fft(&self) -> usize {
        self.fft
    }

    #[must_use]
    pub fn cp(&self) -> usize {
        self.cp
    }

    #[must_use]
    pub fn symbol_samples(&self) -> usize {
        self.fft + self.cp
    }

    #[must_use]
    pub fn map(&self) -> &SubcarrierMap {
        &self.map
    }

    #[must_use]
    pub fn pilot_pattern(&self) -> &PilotPattern {
        &self.pilots
    }

    #[must_use]
    pub fn preamble(&self) -> Preamble {
        self.preamble
    }

    #[must_use]
    pub fn domain(&self) -> Domain {
        self.domain
    }

    #[must_use]
    pub fn data_subcarriers(&self) -> usize {
        self.map.data().len()
    }

    #[must_use]
    pub fn data_offset(&self) -> usize {
        self.preamble.samples(self.fft)
    }

    #[must_use]
    pub fn frame_samples(&self, symbols: usize) -> usize {
        self.data_offset() + symbols * self.symbol_samples()
    }

    #[must_use]
    pub fn short_bins(&self) -> &[Subcarrier] {
        &self.short_bins
    }

    #[must_use]
    pub fn long_training(&self, index: usize) -> Complex<f32> {
        let sign = if training_bit(self.map.occupied()[index].offset) {
            1.0
        } else {
            -1.0
        };
        Complex::new(sign, 0.0)
    }

    #[must_use]
    pub fn short_training(&self, index: usize) -> Complex<f32> {
        let scale = (self.map.occupied().len() as f64 / self.short_bins.len() as f64).sqrt()
            / std::f64::consts::SQRT_2;
        let sign = if training_bit(self.short_bins[index].offset) {
            1.0
        } else {
            -1.0
        };
        Complex::new((sign * scale) as f32, (sign * scale) as f32)
    }

    #[must_use]
    pub fn spectral_copies(&self) -> f64 {
        match self.domain {
            Domain::Complex => 1.0,
            Domain::RealHermitian => 2.0,
        }
    }

    #[must_use]
    pub fn frame_energy(&self, symbols: usize) -> f64 {
        self.spectral_copies()
            * self.map.occupied().len() as f64
            * self.frame_samples(symbols) as f64
            / self.fft as f64
    }

    #[must_use]
    pub fn framing_overhead_db(&self, symbols: usize) -> f64 {
        let carried = (symbols * self.data_subcarriers()) as f64;
        10.0 * (self.frame_energy(symbols) / carried).log10()
    }
}

fn training_bit(offset: i32) -> bool {
    let mut h = (offset as u32).wrapping_mul(0x9E37_79B9);
    h ^= h >> 16;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h & 1 == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_map_is_the_wifi_geometry() {
        let p = OfdmParams::wifi_like();
        assert_eq!(p.fft(), 64);
        assert_eq!(p.cp(), 16);
        assert_eq!(p.data_subcarriers(), 48);
        assert_eq!(p.map().pilots().len(), 4);
        assert_eq!(p.map().occupied().len(), 52);
        let offsets: Vec<i32> = p.map().occupied().iter().map(|c| c.offset).collect();
        assert!(offsets.windows(2).all(|w| w[1] > w[0]));
        assert_eq!(offsets.first(), Some(&-26));
        assert_eq!(offsets.last(), Some(&26));
        assert!(!offsets.contains(&0));
        let dc_neighbour = p.map().occupied().iter().find(|c| c.offset == -1).unwrap();
        assert_eq!(dc_neighbour.bin, 63);
        assert_eq!(p.preamble().short_period(64), 16);
        assert_eq!(p.short_bins().len(), 12);
        assert_eq!(p.data_offset(), 10 * 16 + 32 + 2 * 64);
    }

    #[test]
    fn the_frame_energy_is_the_geometry_and_nothing_else() {
        let p = OfdmParams::wifi_like();
        assert!((p.frame_energy(1) - (260.0 + 65.0)).abs() < 1e-9);
        assert!((p.frame_energy(64) - (260.0 + 65.0 * 64.0)).abs() < 1e-9);
        let want = 10.0 * f64::log10((260.0 + 65.0 * 64.0) / (48.0 * 64.0));
        assert!((p.framing_overhead_db(64) - want).abs() < 1e-12);
        assert!(p.framing_overhead_db(1024) < p.framing_overhead_db(64));
        assert!(p.framing_overhead_db(1_000_000) > 10.0 * (65.0f64 / 48.0).log10() - 1e-6);
    }

    #[test]
    fn both_training_halves_carry_a_data_symbols_energy() {
        let p = OfdmParams::wifi_like();
        let long: f64 = (0..p.map().occupied().len())
            .map(|i| f64::from(p.long_training(i).norm_sqr()))
            .sum();
        let short: f64 = (0..p.short_bins().len())
            .map(|i| f64::from(p.short_training(i).norm_sqr()))
            .sum();
        assert!((long - 52.0).abs() < 1e-6, "long training energy {long}");
        assert!((short - 52.0).abs() < 1e-4, "short training energy {short}");
    }

    #[test]
    fn the_training_sequence_follows_the_offset_not_the_order() {
        let forwards = SubcarrierMap::new(64, &[1, 2, 3, 4], &[]).unwrap();
        let backwards = SubcarrierMap::new(64, &[4, 3, 2, 1], &[]).unwrap();
        assert_eq!(forwards, backwards);
        let p = OfdmParams::wifi_like();
        let mixed: Vec<f32> = (0..p.map().occupied().len())
            .map(|i| p.long_training(i).re)
            .collect();
        assert!(mixed.iter().any(|&s| s > 0.0) && mixed.iter().any(|&s| s < 0.0));
        let positives = mixed.iter().filter(|&&s| s > 0.0).count();
        assert!((17..=35).contains(&positives), "{positives} of 52 positive");
    }

    #[test]
    fn the_pilot_polarity_is_a_balanced_maximal_length_sequence() {
        let p = OfdmParams::wifi_like();
        let polarity = p.pilot_pattern().polarity();
        assert_eq!(polarity.len(), 127);
        let sum: f32 = polarity.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6, "polarity sum {sum}");
        for pilot in 0..p.pilot_pattern().len() {
            assert_eq!(
                p.pilot_pattern().value(pilot, 0),
                p.pilot_pattern().value(pilot, 127)
            );
        }
    }

    #[test]
    fn the_dmt_map_is_the_positive_half_and_only_it() {
        let p = OfdmParams::dmt_like();
        assert_eq!(p.domain(), Domain::RealHermitian);
        assert_eq!(p.data_subcarriers(), 24);
        assert!(
            p.map()
                .occupied()
                .iter()
                .all(|c| c.offset > 0 && c.offset < 32)
        );
        let ofdm = OfdmParams::wifi_like();
        assert!((p.frame_energy(64) - ofdm.frame_energy(64)).abs() < 1e-9);
        let cost = p.framing_overhead_db(64) - ofdm.framing_overhead_db(64);
        assert!((cost - 3.0103).abs() < 1e-3, "DMT costs {cost} dB");
    }

    #[test]
    fn bad_parameter_sets_are_rejected_with_the_right_error() {
        assert_eq!(
            SubcarrierMap::new(48, &[1], &[]).unwrap_err(),
            OfdmError::FftSize(48)
        );
        assert_eq!(
            SubcarrierMap::new(64, &[], &[]).unwrap_err(),
            OfdmError::NoDataSubcarriers
        );
        assert_eq!(
            SubcarrierMap::new(64, &[32], &[]).unwrap_err(),
            OfdmError::SubcarrierOutOfRange {
                offset: 32,
                fft: 64
            }
        );
        assert_eq!(
            SubcarrierMap::new(64, &[3], &[3]).unwrap_err(),
            OfdmError::DuplicateSubcarrier(3)
        );
        assert_eq!(
            SubcarrierMap::new(64, &[3, -61], &[]).unwrap_err(),
            OfdmError::SubcarrierOutOfRange {
                offset: -61,
                fft: 64
            }
        );

        let map = || SubcarrierMap::new(64, &[1, 2, 3, 4], &[8]).unwrap();
        let pattern = || PilotPattern::m_sequence(vec![Complex::new(1.0, 0.0)]).unwrap();
        let preamble = Preamble {
            short_stride: 4,
            short_repeats: 10,
            long_repeats: 2,
            long_guard: 32,
        };
        let build = |cp, pilots: PilotPattern, preamble, domain| {
            OfdmParams::new(cp, map(), pilots, preamble, domain).unwrap_err()
        };
        assert_eq!(
            build(64, pattern(), preamble, Domain::Complex),
            OfdmError::CyclicPrefix { fft: 64, cp: 64 }
        );
        assert_eq!(
            build(
                16,
                PilotPattern::m_sequence(vec![Complex::new(1.0, 0.0); 2]).unwrap(),
                preamble,
                Domain::Complex
            ),
            OfdmError::PilotCountMismatch {
                pilots: 1,
                values: 2
            }
        );
        assert_eq!(
            build(
                16,
                pattern(),
                Preamble {
                    short_stride: 5,
                    ..preamble
                },
                Domain::Complex
            ),
            OfdmError::ShortStride { fft: 64, stride: 5 }
        );
        assert_eq!(
            build(
                16,
                pattern(),
                Preamble {
                    long_repeats: 1,
                    ..preamble
                },
                Domain::Complex
            ),
            OfdmError::Repeats {
                what: "long training",
                repeats: 1
            }
        );
        assert_eq!(
            build(
                16,
                pattern(),
                Preamble {
                    short_stride: 16,
                    ..preamble
                },
                Domain::Complex
            ),
            OfdmError::ShortTrainingEmpty
        );
        let straddling = SubcarrierMap::new(64, &[-4, 4], &[8]).unwrap();
        assert_eq!(
            OfdmParams::new(16, straddling, pattern(), preamble, Domain::RealHermitian)
                .unwrap_err(),
            OfdmError::NotHermitian(-4)
        );
    }
}
