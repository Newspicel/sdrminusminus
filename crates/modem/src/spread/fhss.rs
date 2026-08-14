use num_complex::Complex;

use super::pn::{PnError, PnSequence};

/// The hop schedule as data: which channels exist, how far apart they sit, how long a dwell
/// lasts, and the order they are visited in.
#[derive(Clone, Debug, PartialEq)]
pub struct HopSequence {
    channels: usize,
    spacing_cycles: f64,
    dwell_samples: usize,
    order: Vec<usize>,
}

/// Why a schedule was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FhssError {
    /// Fewer than two channels is not a hopping link, and a zero-sample dwell is not a dwell.
    DegenerateSchedule,
    ChannelOutOfRange(usize),
    /// An empty hop order visits nothing.
    EmptyOrder,
    /// The code ran through its whole period without drawing enough in-range channels.
    ExhaustedCode,
    /// The sequence generator could not build the code the schedule is derived from.
    Sequence(PnError),
}

impl std::fmt::Display for FhssError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DegenerateSchedule => {
                write!(f, "a hop plan needs ≥ 2 channels and a dwell of ≥ 1 sample")
            }
            Self::ChannelOutOfRange(c) => write!(
                f,
                "the order names channel {c}, which the plan does not have"
            ),
            Self::EmptyOrder => write!(f, "an empty hop order visits nothing"),
            Self::ExhaustedCode => write!(
                f,
                "the code's period never drew enough in-range channels to fill the schedule"
            ),
            Self::Sequence(why) => write!(f, "hop sequence: {why}"),
        }
    }
}

impl std::error::Error for FhssError {}

impl HopSequence {
    /// From an explicit order — the arbitrary-schedule constructor, and what a protocol
    /// attachment with a standard's own hop table would use.
    ///
    /// # Errors
    /// [`FhssError::DegenerateSchedule`], [`FhssError::EmptyOrder`],
    /// [`FhssError::ChannelOutOfRange`].
    pub fn new(
        channels: usize,
        spacing_cycles: f64,
        dwell_samples: usize,
        order: Vec<usize>,
    ) -> Result<Self, FhssError> {
        if channels < 2 || dwell_samples == 0 {
            return Err(FhssError::DegenerateSchedule);
        }
        if order.is_empty() {
            return Err(FhssError::EmptyOrder);
        }
        if let Some(&bad) = order.iter().find(|&&c| c >= channels) {
            return Err(FhssError::ChannelOutOfRange(bad));
        }
        Ok(Self {
            channels,
            spacing_cycles,
            dwell_samples,
            order,
        })
    }

    pub fn from_m_sequence(
        channels: usize,
        spacing_cycles: f64,
        dwell_samples: usize,
        hops: usize,
        degree: u32,
    ) -> Result<Self, FhssError> {
        if channels < 2 || dwell_samples == 0 {
            return Err(FhssError::DegenerateSchedule);
        }
        if hops == 0 {
            return Err(FhssError::EmptyOrder);
        }
        let code = PnSequence::maximal_length(degree).map_err(FhssError::Sequence)?;
        let bits = (usize::BITS - (channels - 1).leading_zeros()) as usize;
        let chips = code.chips();
        let budget = chips.len().saturating_mul(hops);
        let mut at = 0usize;
        let mut draws = 0usize;
        let mut order = Vec::with_capacity(hops);
        while order.len() < hops {
            if draws == budget {
                return Err(FhssError::ExhaustedCode);
            }
            draws += 1;
            let mut index = 0usize;
            for _ in 0..bits {
                index = (index << 1) | usize::from(chips[at % chips.len()] < 0.0);
                at += 1;
            }
            if index < channels {
                order.push(index);
            }
        }
        Self::new(channels, spacing_cycles, dwell_samples, order)
    }

    #[must_use]
    pub fn channels(&self) -> usize {
        self.channels
    }

    #[must_use]
    pub fn dwell_samples(&self) -> usize {
        self.dwell_samples
    }

    #[must_use]
    pub fn order(&self) -> &[usize] {
        &self.order
    }

    /// Channel spacing, in cycles per sample.
    #[must_use]
    pub fn spacing_cycles(&self) -> f64 {
        self.spacing_cycles
    }

    /// Samples the whole schedule covers. Past it the sequence repeats, which is what a hop
    /// order shorter than a burst means.
    #[must_use]
    pub fn span_samples(&self) -> usize {
        self.order.len() * self.dwell_samples
    }

    /// The channel dwell `hop` sits on, the order wrapping so a short schedule repeats.
    #[must_use]
    pub fn channel(&self, hop: usize) -> usize {
        self.order[hop % self.order.len()]
    }

    #[must_use]
    pub fn offset_cycles(&self, channel: usize) -> f64 {
        (channel as f64 - (self.channels as f64 - 1.0) / 2.0) * self.spacing_cycles
    }

    /// Distinct channels the schedule's first `hops` dwells actually land on. A generated
    /// schedule that visited two of sixteen channels would spread nothing, and no BER curve
    /// would ever say so — this is the property that makes the generator acceptable.
    #[must_use]
    pub fn visits(&self, hops: usize) -> usize {
        let mut seen = vec![false; self.channels];
        for hop in 0..hops {
            seen[self.channel(hop)] = true;
        }
        seen.iter().filter(|s| **s).count()
    }

    /// The occupied span, in cycles per sample — what the hopping costs in bandwidth over the
    /// underlying entry's own.
    #[must_use]
    pub fn occupied_cycles(&self) -> f64 {
        (self.channels as f64 - 1.0) * self.spacing_cycles
    }
}

/// The hopper: moves each dwell onto its channel. Applied to an underlying entry's baseband
/// output, in place, so the framework adds no buffer of its own.
#[derive(Clone, Debug)]
pub struct FhssMod {
    sequence: HopSequence,
}

/// The de-hopper: moves each dwell back to baseband. Structurally the hopper with the sign
/// flipped, and written as such rather than as a second derivation — the two must be exact
/// inverses or the framework would cost the underlying entry a measurable, unattributable
/// fraction of a dB.
#[derive(Clone, Debug)]
pub struct FhssDemod {
    sequence: HopSequence,
}

/// Applies the schedule's per-dwell carrier, `sign` distinguishing hop from de-hop. Dwells are
/// counted from sample 0 of the buffer — the shared time base "with the sequencer known" means —
/// and the oscillator's phase is that absolute index's, so hop and de-hop cancel exactly.
fn apply(sequence: &HopSequence, wave: &mut [Complex<f32>], sign: f64) {
    for (index, sample) in wave.iter_mut().enumerate() {
        let hop = index / sequence.dwell_samples();
        let offset = sign * sequence.offset_cycles(sequence.channel(hop));
        let turns = offset * index as f64;
        let phase = std::f64::consts::TAU * (turns - turns.floor());
        let (sin, cos) = phase.sin_cos();
        let re = f64::from(sample.re);
        let im = f64::from(sample.im);
        *sample = Complex::new((re * cos - im * sin) as f32, (re * sin + im * cos) as f32);
    }
}

impl FhssMod {
    #[must_use]
    pub fn new(sequence: HopSequence) -> Self {
        Self { sequence }
    }

    #[must_use]
    pub fn sequence(&self) -> &HopSequence {
        &self.sequence
    }

    /// Moves each dwell onto its channel, in place.
    pub fn hop(&self, wave: &mut [Complex<f32>]) {
        apply(&self.sequence, wave, 1.0);
    }
}

impl FhssDemod {
    #[must_use]
    pub fn new(sequence: HopSequence) -> Self {
        Self { sequence }
    }

    #[must_use]
    pub fn sequence(&self) -> &HopSequence {
        &self.sequence
    }

    /// Moves each dwell back to baseband, in place.
    pub fn dehop(&self, wave: &mut [Complex<f32>]) {
        apply(&self.sequence, wave, -1.0);
    }

    /// Dwells of the first `hops` that sit on `channel` — how much of a burst a jammer parked
    /// there can reach, which is the framework's whole claim reduced to a count.
    #[must_use]
    pub fn dwells_on(&self, channel: usize, hops: usize) -> usize {
        (0..hops)
            .filter(|&hop| self.sequence.channel(hop) == channel)
            .count()
    }
}

/// The hop-order generator's interface, for a schedule a caller would rather compute than list.
/// [`HopSequence::from_m_sequence`] is the implementation this crate ships; a protocol attachment
/// with a standard's own generator implements this and hands the order to [`HopSequence::new`].
pub trait HopSequencer {
    /// The channel dwell `hop` visits.
    fn channel(&self, hop: usize) -> usize;

    /// The first `hops` channels as an order [`HopSequence::new`] takes.
    fn order(&self, hops: usize) -> Vec<usize> {
        (0..hops).map(|hop| self.channel(hop)).collect()
    }
}

impl HopSequencer for HopSequence {
    fn channel(&self, hop: usize) -> usize {
        Self::channel(self, hop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schedule(channels: usize, hops: usize) -> HopSequence {
        HopSequence::from_m_sequence(channels, 0.01, 64, hops, 9).unwrap()
    }

    /// The framework's defining property: hop then de-hop is the identity. Not "close to" — the
    /// two oscillators are the same expression with one sign, so anything but bit-level agreement
    /// would be a phase-accumulation defect that would show up as an unattributable loss in the
    /// underlying entry's curve.
    #[test]
    fn hopping_and_dehopping_is_the_identity() {
        let sequence = schedule(16, 32);
        let mut wave: Vec<Complex<f32>> = (0..4_000)
            .map(|k| {
                let a = f32::from(u8::try_from(k % 7).unwrap_or(0)) - 3.0;
                let b = f32::from(u8::try_from(k % 5).unwrap_or(0)) - 2.0;
                Complex::new(a * 0.1, b * 0.1)
            })
            .collect();
        let original = wave.clone();
        FhssMod::new(sequence.clone()).hop(&mut wave);
        let hopped = wave.clone();
        FhssDemod::new(sequence).dehop(&mut wave);
        for (k, (&got, &want)) in wave.iter().zip(&original).enumerate() {
            assert!((got - want).norm() < 1e-5, "sample {k}: {got} vs {want}");
        }
        // …and the hop is not a no-op it could have trivially passed by being.
        let moved = hopped
            .iter()
            .zip(&original)
            .filter(|&(&a, &b)| (a - b).norm() > 1e-3)
            .count();
        assert!(moved > original.len() / 2, "only {moved} samples moved");
    }

    /// Each dwell really is on its own carrier: the phase step across a dwell must match the
    /// channel's offset, which is what makes `offset_cycles` a physical statement rather than a
    /// label.
    #[test]
    fn each_dwell_sits_on_its_own_carrier() {
        let sequence = schedule(8, 16);
        let dwell = sequence.dwell_samples();
        let mut wave = vec![Complex::new(1.0f32, 0.0); dwell * 8];
        FhssMod::new(sequence.clone()).hop(&mut wave);
        for hop in 0..8 {
            let at = hop * dwell + dwell / 2;
            let step = wave[at + 1] * wave[at].conj();
            let measured = f64::from(step.arg()) / std::f64::consts::TAU;
            let want = sequence.offset_cycles(sequence.channel(hop));
            assert!(
                (measured - want).abs() < 1e-4,
                "hop {hop} on channel {}: measured {measured}, planned {want}",
                sequence.channel(hop)
            );
        }
    }

    #[test]
    fn the_generated_schedule_visits_its_whole_plan() {
        for channels in [3usize, 4, 5, 8, 16, 32] {
            let hops = channels * 8;
            let sequence = schedule(channels, hops);
            assert_eq!(
                sequence.visits(hops),
                channels,
                "{channels} channels over {hops} hops"
            );
            let demod = FhssDemod::new(sequence);
            for channel in 0..channels {
                let dwells = demod.dwells_on(channel, hops);
                assert!(
                    (2..=24).contains(&dwells),
                    "{channels} channels: channel {channel} takes {dwells} of {hops} dwells"
                );
            }
        }
    }

    #[test]
    fn the_channel_plan_is_centred_on_baseband() {
        let sequence = schedule(9, 16);
        assert!(sequence.offset_cycles(4).abs() < 1e-12);
        assert!((sequence.offset_cycles(8) + sequence.offset_cycles(0)).abs() < 1e-12);
        assert!((sequence.occupied_cycles() - 8.0 * 0.01).abs() < 1e-12);
        let even = schedule(8, 16);
        assert!((even.offset_cycles(3) + even.offset_cycles(4)).abs() < 1e-12);
    }

    /// A short order repeats rather than running out — the case a burst longer than the schedule
    /// hits, and the one an indexing slip turns into a panic.
    #[test]
    fn a_short_order_wraps() {
        let sequence = HopSequence::new(4, 0.02, 16, vec![2, 0, 3]).unwrap();
        assert_eq!(sequence.channel(0), 2);
        assert_eq!(sequence.channel(3), 2);
        assert_eq!(sequence.channel(7), 0);
        assert_eq!(sequence.span_samples(), 48);
    }

    #[test]
    fn degenerate_schedules_are_rejected_with_the_right_error() {
        assert_eq!(
            HopSequence::new(1, 0.01, 16, vec![0]).unwrap_err(),
            FhssError::DegenerateSchedule
        );
        assert_eq!(
            HopSequence::new(4, 0.01, 0, vec![0]).unwrap_err(),
            FhssError::DegenerateSchedule
        );
        assert_eq!(
            HopSequence::new(4, 0.01, 16, vec![]).unwrap_err(),
            FhssError::EmptyOrder
        );
        assert_eq!(
            HopSequence::new(4, 0.01, 16, vec![0, 9]).unwrap_err(),
            FhssError::ChannelOutOfRange(9)
        );
        assert!(matches!(
            HopSequence::from_m_sequence(4, 0.01, 16, 8, 99).unwrap_err(),
            FhssError::Sequence(_)
        ));
    }

    #[test]
    fn the_generator_refuses_a_degenerate_plan_instead_of_drawing_for_it() {
        for channels in [0usize, 1] {
            assert_eq!(
                HopSequence::from_m_sequence(channels, 0.01, 16, 8, 7).unwrap_err(),
                FhssError::DegenerateSchedule
            );
        }
        assert_eq!(
            HopSequence::from_m_sequence(4, 0.01, 0, 8, 7).unwrap_err(),
            FhssError::DegenerateSchedule
        );
        assert_eq!(
            HopSequence::from_m_sequence(4, 0.01, 16, 0, 7).unwrap_err(),
            FhssError::EmptyOrder
        );
    }
}
