//! What an OFDM waveform *is*, as data: an FFT size, a cyclic prefix, which bins carry payload,
//! which carry pilots, what the preamble repeats, and whether the spectrum is folded Hermitian
//! ( §3.1 `ofdm/`: "pluggable pilot patterns and subcarrier maps").
//!
//! Everything a standard would put in a table lives here, so the engine below never matches on
//! one — the §3.3 rule that governs `constellation/` governs a framework the same way. The
//! reference configuration this catalog measures at, [`OfdmParams::wifi_like`], is therefore a
//! *function returning parameters*, not a type: 64-point FFT, 16-sample prefix, 48 data and 4
//! pilot subcarriers, which is 802.11a/g's geometry ( §7 phase 6).
//!
//! **The training sequences are this crate's, not a standard's.** Nothing here interoperates
//! with anything (§6's scope decision: the modulation is in scope, the protocol is not), and a
//! transcribed ±1 table that no test can check is a liability rather than an asset — so the
//! sequences are *generated* from a documented rule over the map the caller handed in, which
//! also means an arbitrary map gets a working preamble instead of a lookup miss.

use std::fmt;

use num_complex::Complex;

/// Why a parameter set was rejected. Construction is setup-time, so this is a `Result` and not
/// a panic — a configuration arriving from a file must surface as an error, never take an
/// engine down (the rule [`ConstellationError`](crate::constellation::ConstellationError)
/// follows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfdmError {
    /// The transform length must be a power of two of at least 8: the preamble's repeat period
    /// is `fft / stride` and the prefix arithmetic assumes an integer one.
    FftSize(usize),
    /// A prefix must be at least one sample and shorter than the symbol it prefixes.
    CyclicPrefix { fft: usize, cp: usize },
    /// A map with no data subcarrier carries no payload.
    NoDataSubcarriers,
    /// Offsets are signed bin indices in `-(fft/2) ..= fft/2 - 1`, DC being 0.
    SubcarrierOutOfRange { offset: i32, fft: usize },
    /// One bin, one role: a repeated offset would be transmitted twice and estimated once.
    DuplicateSubcarrier(i32),
    /// A Hermitian (DMT) map may only occupy the positive half `1 ..= fft/2 - 1`; bin 0 and the
    /// Nyquist bin have no conjugate partner to carry the mirrored copy.
    NotHermitian(i32),
    /// The short-training stride must divide the transform length, or its time-domain period is
    /// not a whole number of samples and the autocorrelation detector has nothing to lock to.
    ShortStride { fft: usize, stride: usize },
    /// The stride energised no occupied bin — a short training symbol of silence.
    ShortTrainingEmpty,
    /// Both preamble halves need at least two repeats: one for the autocorrelation to compare
    /// against, one to compare with.
    Repeats { what: &'static str, repeats: usize },
    /// The long training's guard may not exceed one symbol.
    LongGuard { fft: usize, guard: usize },
    /// A pilot pattern needs at least one base value and one polarity entry.
    EmptyPilotPattern,
    /// One base value per pilot subcarrier, in the map's own order.
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

/// One mapped subcarrier: where it sits in the transform (`bin`, the index an FFT output is
/// read at) and where it sits in the spectrum (`offset`, signed about DC). Both, because the
/// two are used for different things and deriving one from the other at every use site is how a
/// sign error hides — the bin indexes buffers, the offset is the *abscissa* the pilot fit and
/// the channel interpolation are linear in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subcarrier {
    pub bin: usize,
    pub offset: i32,
}

/// Which bins carry what. Data and pilots are held in ascending frequency order — the order
/// payload symbols are laid out in, and the order interpolation walks — and `occupied` is their
/// union, which is what the channel estimate and the preamble span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubcarrierMap {
    fft: usize,
    data: Vec<Subcarrier>,
    pilots: Vec<Subcarrier>,
    occupied: Vec<Subcarrier>,
}

impl SubcarrierMap {
    /// Builds a map from signed offsets about DC.
    ///
    /// # Errors
    /// [`OfdmError::FftSize`], [`OfdmError::NoDataSubcarriers`],
    /// [`OfdmError::SubcarrierOutOfRange`], [`OfdmError::DuplicateSubcarrier`].
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

    /// Data and pilots together — every bin the transmitter puts energy on, and so every bin
    /// the preamble trains and the estimator estimates.
    #[must_use]
    pub fn occupied(&self) -> &[Subcarrier] {
        &self.occupied
    }

    /// Whether every occupied bin has a conjugate partner available, i.e. whether this map can
    /// drive a real-valued (DMT) transmitter.
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

/// Whether the transmitter emits complex baseband or a real-valued waveform.
///
/// [`Domain::RealHermitian`] is the DMT flag ( §3.1: "a Hermitian-symmetry flag
/// yields real-baseband DMT from the same engine"), and it is deliberately a *transmitter*
/// property alone: the modulator mirrors each occupied bin onto its conjugate partner, and the
/// receiver reads exactly the map it was given, which is the lower half either way. There is no
/// second receive path, and that is the claim the flag exists to make — asserted by test rather
/// than stated here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Domain {
    Complex,
    RealHermitian,
}

/// The preamble's shape. Two halves with different jobs, which is why they are parameterised
/// separately: the short half repeats a *sub-symbol* period so a receiver can find the burst and
/// bound its carrier offset before it knows anything; the long half repeats a whole symbol so
/// the residual offset can be measured finely and the channel estimated on known symbols.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preamble {
    /// Only every `short_stride`-th occupied bin is energised, so the short symbol repeats with
    /// period `fft / short_stride` samples. That period is the coarse estimator's whole range:
    /// a phase measured across it wraps past ±1/(2·period) cycles per sample.
    pub short_stride: usize,
    /// Periods transmitted. More is a longer plateau to detect on and a quieter coarse
    /// estimate; it is also energy charged to Eb.
    pub short_repeats: usize,
    /// Whole symbols transmitted in the long half. Two is the minimum a fine offset estimate
    /// needs and what every practical design uses.
    pub long_repeats: usize,
    /// Guard ahead of the long half, cyclic like any prefix. Longer than a data symbol's
    /// prefix in the reference configuration for the usual reason: it absorbs the timing
    /// uncertainty the short half leaves behind as well as the channel's delay spread.
    pub long_guard: usize,
}

impl Preamble {
    /// The short symbol's repeat period in samples.
    #[must_use]
    pub fn short_period(&self, fft: usize) -> usize {
        fft / self.short_stride.max(1)
    }

    /// Samples the whole preamble occupies.
    #[must_use]
    pub fn samples(&self, fft: usize) -> usize {
        self.short_repeats * self.short_period(fft) + self.long_guard + self.long_repeats * fft
    }
}

/// The values transmitted on the pilot subcarriers, as data: one base value per pilot, times a
/// per-symbol polarity.
///
/// The split is what makes the pattern *pluggable* rather than hard-coded — a pilot set that
/// never changed sign would let a receiver lock its residual-phase fit to a spurious tone, and
/// every real design therefore modulates the whole set by a pseudo-random sequence. Here that
/// sequence is generated ([`PilotPattern::m_sequence`]), not transcribed.
#[derive(Clone, Debug, PartialEq)]
pub struct PilotPattern {
    base: Vec<Complex<f32>>,
    polarity: Vec<f32>,
}

impl PilotPattern {
    /// # Errors
    /// [`OfdmError::EmptyPilotPattern`] if either half is empty.
    pub fn new(base: Vec<Complex<f32>>, polarity: Vec<f32>) -> Result<Self, OfdmError> {
        if base.is_empty() || polarity.is_empty() {
            return Err(OfdmError::EmptyPilotPattern);
        }
        Ok(Self { base, polarity })
    }

    /// `base` modulated by the ±1 output of a maximal-length 7-stage LFSR (x⁷ + x⁴ + 1, all-ones
    /// seed) — period 127, the length every OFDM pilot polarity in the wild happens to use, and
    /// short enough that a frame of any practical length sees an essentially balanced sequence.
    ///
    /// # Errors
    /// As [`PilotPattern::new`].
    pub fn m_sequence(base: Vec<Complex<f32>>) -> Result<Self, OfdmError> {
        let mut state = 0x7fu8;
        let polarity = (0..127)
            .map(|_| {
                let out = state & 1;
                // Taps 7 and 4 (bits 0 and 3 of the shift register), fed back into bit 6.
                let feedback = (state ^ (state >> 3)) & 1;
                state = (state >> 1) | (feedback << 6);
                if out == 1 { 1.0 } else { -1.0 }
            })
            .collect();
        Self::new(base, polarity)
    }

    /// The value pilot `index` carries in data symbol `symbol` of the frame.
    ///
    /// # Panics
    /// If `index` is past the base values — a map/pattern mismatch construction rejects.
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

/// A validated OFDM waveform definition. Immutable once built, so every invariant checked at
/// construction holds for the lifetime of the modulator and demodulator built from it.
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
    /// # Errors
    /// Every variant of [`OfdmError`]: the map's own, plus the prefix, preamble and pilot
    /// consistency checks this level owns.
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

    /// The catalog's reference configuration ( §7 phase 6): a 64-point transform with
    /// a 16-sample prefix, 48 data subcarriers and 4 pilots at ±7 and ±21, DC and the band edges
    /// left empty — 802.11a/g's geometry, measured on synthetic vectors only, with no protocol
    /// attached (§6's scope decision).
    ///
    /// # Panics
    /// Never: the configuration is a constant of this crate and its validity is asserted by this
    /// module's own tests. The `Result` shape belongs on the general constructor, where a caller
    /// can genuinely be wrong.
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

    /// The same geometry folded onto a real-valued transmitter: the positive half of
    /// [`OfdmParams::wifi_like`]'s map — 24 data subcarriers and pilots at 7 and 21 — with the
    /// Hermitian flag set, which is the DMT row of the catalog.
    ///
    /// # Panics
    /// As [`OfdmParams::wifi_like`].
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

    /// Samples one data symbol occupies, prefix included.
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

    /// Data subcarriers per symbol — the number of constellation points one symbol carries.
    #[must_use]
    pub fn data_subcarriers(&self) -> usize {
        self.map.data().len()
    }

    /// Samples from the frame's first sample to its first *data* symbol.
    #[must_use]
    pub fn data_offset(&self) -> usize {
        self.preamble.samples(self.fft)
    }

    /// Samples a frame of `symbols` data symbols occupies.
    #[must_use]
    pub fn frame_samples(&self, symbols: usize) -> usize {
        self.data_offset() + symbols * self.symbol_samples()
    }

    /// The bins the short training energises: every occupied bin whose offset is a multiple of
    /// the stride, which is what makes the short symbol repeat with period `fft / stride`.
    #[must_use]
    pub fn short_bins(&self) -> &[Subcarrier] {
        &self.short_bins
    }

    /// The long training's value on occupied subcarrier `index`: ±1 from a deterministic rule
    /// over the *offset*, so the sequence is a property of the map rather than of the order the
    /// caller listed its subcarriers in.
    ///
    /// The rule is a 32-bit avalanche of the offset — an arbitrary but fixed and reproducible
    /// draw. What matters about a training sequence here is that it is known, balanced and not
    /// periodic in the bin index (a periodic one would put nulls in the time-domain symbol the
    /// correlator has to find); which particular draw it is does not, because nothing
    /// interoperates.
    ///
    /// # Panics
    /// If `index` is past the occupied set.
    #[must_use]
    pub fn long_training(&self, index: usize) -> Complex<f32> {
        let sign = if training_bit(self.map.occupied()[index].offset) {
            1.0
        } else {
            -1.0
        };
        Complex::new(sign, 0.0)
    }

    /// The short training's value on [`short_bins`](Self::short_bins) entry `index`, at the
    /// amplitude that gives the short symbol the *same* energy as a data symbol.
    ///
    /// The equal-energy scaling is not cosmetic: the whole preamble is charged to Eb like any
    /// other framing, and a short symbol energised on a twelfth of the band at data amplitude
    /// would make the detector's job easier or harder than the payload's SNR says it should be.
    ///
    /// # Panics
    /// If `index` is past the short-training set.
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

    /// Copies of the occupied spectrum the transmitter radiates: two under
    /// [`Domain::RealHermitian`], where every bin is mirrored onto its conjugate partner, one
    /// otherwise. It is the DMT flag's whole cost, and it belongs in the energy accounting
    /// rather than in a footnote.
    #[must_use]
    pub fn spectral_copies(&self) -> f64 {
        match self.domain {
            Domain::Complex => 1.0,
            Domain::RealHermitian => 2.0,
        }
    }

    /// Energy one frame of `symbols` data symbols radiates, in units of one occupied
    /// subcarrier's symbol energy — the closed form behind every OFDM curve's Eb accounting.
    ///
    /// Both training halves are scaled to a data symbol's energy and every prefix repeats a
    /// fraction of one, so the whole frame reduces to a count of transform-lengths:
    /// `copies · occupied · (preamble samples + symbols · (fft + cp)) / fft`.
    #[must_use]
    pub fn frame_energy(&self, symbols: usize) -> f64 {
        self.spectral_copies()
            * self.map.occupied().len() as f64
            * self.frame_samples(symbols) as f64
            / self.fft as f64
    }

    /// The dB a frame's own overhead adds to every point of its BER curve, relative to the same
    /// constellation measured on a bare subcarrier.
    ///
    /// This is the number that lets an OFDM row be held to a *closed form* rather than to itself
    /// (§4.1): the prefix, the pilots and the preamble are transmitted energy carrying no
    /// information, the sweep runner charges all of it to Eb, and the ratio is exactly
    /// `frame_energy / (symbols · data subcarriers)`. Note what it does *not* contain — the bits
    /// per subcarrier cancel, so one number covers every modulation order the same geometry
    /// carries.
    ///
    /// It is an expectation, to 0.02 dB: a prefix copies the symbol's last `cp` samples, whose
    /// share of its energy is `cp/fft` only on average
    /// (`the_radiated_energy_is_the_closed_form_on_average`).
    #[must_use]
    pub fn framing_overhead_db(&self, symbols: usize) -> f64 {
        let carried = (symbols * self.data_subcarriers()) as f64;
        10.0 * (self.frame_energy(symbols) / carried).log10()
    }
}

/// The training sequence's sign rule: a 32-bit integer avalanche of the subcarrier offset,
/// reduced to one bit. Deterministic, balanced over any run of offsets, and — the property that
/// matters — free of period in the bin index.
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
        // Ascending frequency order, DC and the band edges empty.
        let offsets: Vec<i32> = p.map().occupied().iter().map(|c| c.offset).collect();
        assert!(offsets.windows(2).all(|w| w[1] > w[0]));
        assert_eq!(offsets.first(), Some(&-26));
        assert_eq!(offsets.last(), Some(&26));
        assert!(!offsets.contains(&0));
        // Negative offsets wrap to the top of the transform.
        let dc_neighbour = p.map().occupied().iter().find(|c| c.offset == -1).unwrap();
        assert_eq!(dc_neighbour.bin, 63);
        // The short training's stride gives a 16-sample period and 12 energised bins.
        assert_eq!(p.preamble().short_period(64), 16);
        assert_eq!(p.short_bins().len(), 12);
        assert_eq!(p.data_offset(), 10 * 16 + 32 + 2 * 64);
    }

    /// The Eb accounting behind every committed curve, checked against the arithmetic a reader
    /// can do by hand: 52 occupied bins over 80 samples per symbol is 65 units of energy per
    /// symbol, both preamble halves are four symbols' worth, and 48 of the 52 bins carry payload.
    #[test]
    fn the_frame_energy_is_the_geometry_and_nothing_else() {
        let p = OfdmParams::wifi_like();
        assert!((p.frame_energy(1) - (260.0 + 65.0)).abs() < 1e-9);
        assert!((p.frame_energy(64) - (260.0 + 65.0 * 64.0)).abs() < 1e-9);
        let want = 10.0 * f64::log10((260.0 + 65.0 * 64.0) / (48.0 * 64.0));
        assert!((p.framing_overhead_db(64) - want).abs() < 1e-12);
        // Longer frames amortise the preamble but never the prefix or the pilots.
        assert!(p.framing_overhead_db(1024) < p.framing_overhead_db(64));
        assert!(p.framing_overhead_db(1_000_000) > 10.0 * (65.0f64 / 48.0).log10() - 1e-6);
    }

    /// The preamble is energy-matched to a data symbol, which is what makes the closed form
    /// above true. Read off the training values rather than off the doc comment.
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

    /// The sign rule has to be a function of the offset alone — otherwise a receiver built from
    /// a differently-ordered map would train against a different sequence than the transmitter
    /// sent, and nothing would say so.
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
        // Balanced enough that the training symbol has no DC-like bias: 52 draws, and a split
        // worse than 2:1 would mean the avalanche is not doing its job.
        let positives = mixed.iter().filter(|&&s| s > 0.0).count();
        assert!((17..=35).contains(&positives), "{positives} of 52 positive");
    }

    #[test]
    fn the_pilot_polarity_is_a_balanced_maximal_length_sequence() {
        let p = OfdmParams::wifi_like();
        let polarity = p.pilot_pattern().polarity();
        assert_eq!(polarity.len(), 127);
        // A maximal-length sequence has one more +1 than −1 over its period.
        let sum: f32 = polarity.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6, "polarity sum {sum}");
        // It repeats with its period and modulates the whole pilot set together.
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
        // The mirror doubles the radiated spectrum while the payload halves, so a DMT frame
        // costs exactly 3.01 dB more Eb than the complex frame of the same geometry — the DMT
        // row's headline number, and a closed form rather than a measured coincidence.
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
        // −61 aliases onto bin 3 in a 64-point transform, so it is out of range rather than a
        // second name for the same bin.
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
        // Bins 1..4 are not multiples of the stride, so the short symbol would be silent.
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
        // A map straddling DC has no Hermitian reading.
        let straddling = SubcarrierMap::new(64, &[-4, 4], &[8]).unwrap();
        assert_eq!(
            OfdmParams::new(16, straddling, pattern(), preamble, Domain::RealHermitian)
                .unwrap_err(),
            OfdmError::NotHermitian(-4)
        );
    }
}
