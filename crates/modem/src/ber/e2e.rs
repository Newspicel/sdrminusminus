//! Level-1 end-to-end scaffolding ( §4.4): *payload in equals payload out, at a
//! stated margin above sensitivity* — the property-style loopback every catalog entry runs as
//! part of its §5 bundle. Because `tx.rs` drives the same modulators, a green loopback is also
//! the transmit path's correctness test.
//!
//! §4.4 defines five E2E levels; this module is level 1 and the one place that states where
//! the rest live, so later phases do not re-litigate the map:
//!
//! 1. **Modem loopback** — here. Random payload → modulator → impairment channel at a stated
//!    margin above sensitivity → demodulator → payload equality.
//! 2. **Protocol E2E, synthetic** — `crates/channels` tests. `channels::testgen` frame
//!    builders construct a complete transmission, the library modulator and an impairment
//!    channel carry it, and the actual channel implementation's decoded events are asserted
//!    field-by-field.
//! 3. **Recorded-fixture E2E** — `crates/channels` tests. Short off-air SigMF captures with
//!    committed expected output; `decodes_a_recorded_call` is the model, and a failure there
//!    is blocking ( §8).
//! 4. **Engine integration E2E** — `crates/engine` tests. Raw samples at a native device rate
//!    through the runtime (DDC construction, resampling, scheduling) to an asserted
//!    `DecoderEvent` stream, including the multi-channel case.
//! 5. **Cross-validation** — ad hoc, wherever an independent implementation exists to compare
//!    against on identical input; results land in the entry's `CATALOG.md` row, not in a
//!    permanent harness.
//!
//! Why a *margin* rather than a fixed Eb/N0: the loopback asserts perfection, and perfection
//! is only a fair demand where the entry's own sensitivity says errors are negligible. Stated
//! as "+N dB above the 1e-3 sensitivity", the operating point carries the same meaning for a
//! chain whose sensitivity is 7 dB and one whose sensitivity is 17 dB, and it tightens
//! automatically if a detector improves. The margin must put `residual BER × total bits ≪ 1`;
//! the fixed seed then makes the outcome a fact of the entry rather than a coin flip — a seed
//! that happened to land on the residual tail would fail once, at authoring time, loudly.
//!
//! Determinism follows the sweep runner's doctrine: every payload is named by its own seed,
//! and the channel realisation continues that payload's stream — so the [`Mismatch`] a failed
//! run reports regenerates alone, via [`Payload::from_seed`], without replaying the payloads
//! before it.

use std::{error::Error, fmt};

use super::{
    impair::{Awgn, Channel, ChannelSpec, Impairment},
    rng::Rng,
    sweep::Link,
};

/// One trial's worth of random payload bits plus the RNG state the channel realisation will
/// continue from. Bits and noise share one stream on purpose: a single `seed` then names the
/// *entire* trial — payload, channel draws, everything — which is what lets a [`Mismatch`] be
/// reproduced without the payloads that preceded it.
pub struct Payload {
    /// The seed that regenerates this trial via [`Payload::from_seed`].
    pub seed: u64,
    pub bits: Vec<bool>,
    /// Private: the stream position after the bits were drawn. Handing it out would let a
    /// caller decouple channel noise from the payload seed and silently break reproduction.
    channel_rng: Rng,
}

impl Payload {
    /// Regenerates the exact trial a [`Mismatch`] names: same bits, and — because the channel
    /// continues this RNG — the same channel realisation, from one u64.
    #[must_use]
    pub fn from_seed(seed: u64, bit_count: usize) -> Self {
        let mut rng = Rng::new(seed);
        let bits = random_bits(&mut rng, bit_count);
        Self {
            seed,
            bits,
            channel_rng: rng,
        }
    }
}

/// Payload bits drawn 64 per `next_u64` word. The word-wise consumption is part of what a
/// payload seed means — changing it would silently rename every committed reproduction — so it
/// mirrors the sweep runner's payload generation rather than inventing a second stream shape.
fn random_bits(rng: &mut Rng, n: usize) -> Vec<bool> {
    let mut bits = Vec::with_capacity(n);
    while bits.len() < n {
        let mut word = rng.next_u64();
        let take = 64.min(n - bits.len());
        for _ in 0..take {
            bits.push(word & 1 == 1);
            word >>= 1;
        }
    }
    bits
}

/// Seeded iterator of `count` random payloads of `bits_per_payload` bits — the argument shape
/// [`loopback`] expects. Each item's seed derives from the run seed by the same golden-ratio
/// stride the sweep runner uses for its points: injective in the index, and unrelated streams
/// after [`Rng::new`]'s SplitMix64 expansion, so payloads are independent trials.
#[derive(Clone, Debug)]
pub struct Payloads {
    seed: u64,
    bits_per_payload: usize,
    count: usize,
    produced: usize,
}

impl Payloads {
    #[must_use]
    pub fn new(seed: u64, count: usize, bits_per_payload: usize) -> Self {
        Self {
            seed,
            bits_per_payload,
            count,
            produced: 0,
        }
    }
}

impl Iterator for Payloads {
    type Item = Payload;

    fn next(&mut self) -> Option<Payload> {
        if self.produced == self.count {
            return None;
        }
        let stride = (self.produced as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        self.produced += 1;
        Some(Payload::from_seed(
            self.seed.wrapping_add(stride),
            self.bits_per_payload,
        ))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.count - self.produced;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for Payloads {}

/// The report a failed [`loopback`] returns: enough to reproduce the failing trial alone
/// ([`Payload::from_seed`] with `payload_seed`) and enough to read the failure mode without
/// reproducing it — `first_bit` localises it, the counts distinguish a corrupted payload
/// (`bit_errors` small, `decoded_bits` full) from a lost one (`decoded_bits` short; the
/// missing positions count as errors, per the sweep runner's rule that lost bits are never
/// silently fewer trials).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mismatch {
    /// Position of the failing payload in the run's iterator; 0 when reproduced solo.
    pub payload_index: usize,
    pub payload_seed: u64,
    /// First payload position where the decoded bit differs (or is missing).
    pub first_bit: usize,
    pub bit_errors: u64,
    pub payload_bits: usize,
    pub decoded_bits: usize,
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "payload {} (seed {:#x}): {}/{} bits differ, first at bit {}, \
             demodulator returned {} bits",
            self.payload_index,
            self.payload_seed,
            self.bit_errors,
            self.payload_bits,
            self.first_bit,
            self.decoded_bits
        )
    }
}

impl Error for Mismatch {}

/// The level-1 property: every payload survives the link and channel bit-for-bit, or the
/// first failure comes back as a [`Mismatch`]. Stops at the first failing payload — the
/// property is "no errors at this margin", so one counterexample settles it, and the report
/// stays about a single reproducible trial instead of averaging over the rest.
///
/// `&mut` on the link and channel is for what they become, not what they are: the phase-0
/// [`Link`] is a closure pair and today's impairments draw all state from the RNG, but the
/// phase-3+ engines hold loop state, and this signature — the one every catalog entry's
/// loopback test is written against — must not change when they arrive.
///
/// Bits the demodulator returns beyond the payload length are ignored, exactly as in the
/// sweep runner: the [`Link`] contract is payload-aligned bits, so trailing filter-tail
/// output is meaningless rather than wrong.
pub fn loopback(
    link: &mut Link,
    channel: &mut dyn Impairment,
    payloads: impl IntoIterator<Item = Payload>,
) -> Result<(), Mismatch> {
    for (payload_index, payload) in payloads.into_iter().enumerate() {
        let Payload {
            seed,
            bits,
            mut channel_rng,
        } = payload;
        let mut wave = (link.modulate)(&bits);
        channel.apply(&mut wave, &mut channel_rng);
        let decoded = (link.demodulate)(&wave);

        let mut first_bit = None;
        let mut bit_errors = 0u64;
        for (i, &sent) in bits.iter().enumerate() {
            if decoded.get(i) != Some(&sent) {
                bit_errors += 1;
                if first_bit.is_none() {
                    first_bit = Some(i);
                }
            }
        }
        if let Some(first_bit) = first_bit {
            return Err(Mismatch {
                payload_index,
                payload_seed: seed,
                first_bit,
                bit_errors,
                payload_bits: bits.len(),
                decoded_bits: decoded.len(),
            });
        }
    }
    Ok(())
}

/// The margin convention, §4.3's "operating N dB above the measured 1e-3 sensitivity", as a
/// channel builder: `template` carries whatever other axes the test wants, and the AWGN axis
/// is set to `sensitivity_1e3_db + margin_db` for this link. The limits runner uses
/// [`SENSITIVITY_MARGIN_DB`](super::SENSITIVITY_MARGIN_DB); loopback tests state their own,
/// larger, margin — see the module docs for how large is large enough.
///
/// The sigma derivation is not repeated here: [`Awgn::for_ebn0`] measures the waveform's own
/// energy at apply time, against `link.bits_per_trial` information bits — the identical
/// accounting the sweep runner's curves rest on, still true after the template's other axes
/// have reshaped the waveform, because the composed [`Channel`] applies AWGN canonically last.
/// The payloads handed to [`loopback`] must therefore be `link.bits_per_trial` bits long, or
/// the stated Eb/N0 is off by the length ratio.
#[must_use]
pub fn channel_at_margin(
    template: &ChannelSpec,
    link: &Link,
    sensitivity_1e3_db: f64,
    margin_db: f64,
) -> Channel {
    template
        .awgn(Awgn::for_ebn0(
            sensitivity_1e3_db + margin_db,
            link.bits_per_trial as u64,
        ))
        .build()
}

#[cfg(test)]
mod tests {
    use std::iter;

    use super::*;
    use crate::ber::{reference::ideal_bpsk, theory};

    /// Eb/N0 where a strictly decreasing oracle crosses `ber`, by bisection. Test-local: the
    /// shipped comparators in `sweep` invert oracles inside their own gates; here only the
    /// sensitivity number is needed. The bracket spans every curve in the catalog.
    fn ebn0_at_ber(oracle: impl Fn(f64) -> f64, ber: f64) -> f64 {
        let (mut lo, mut hi) = (-10.0f64, 20.0f64);
        assert!(oracle(lo) >= ber && oracle(hi) <= ber);
        while hi - lo > 1e-9 {
            let mid = 0.5 * (lo + hi);
            if oracle(mid) >= ber {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    }

    /// Wraps a link's demodulator to build a deliberately broken link; modulator and bit
    /// accounting stay the original's, so the failure is purely the injected one.
    fn with_demod(link: Link, f: impl Fn(Vec<bool>) -> Vec<bool> + 'static) -> Link {
        let inner = link.demodulate;
        Link {
            label: format!("{} (broken)", link.label),
            bits_per_trial: link.bits_per_trial,
            modulate: link.modulate,
            demodulate: Box::new(move |wave| f(inner(wave))),
        }
    }

    /// The task-level property for the reference link: 20 payloads of 4096 bits survive at
    /// +6 dB over the theoretical 1e-3 sensitivity (≈6.79 dB, so ≈12.79 dB operating point).
    /// Residual BER there is ~3e-10; across the 81 920 trial bits that is ~3e-5 expected
    /// errors — comfortably inside the module-doc margin rule, and fixed by the seed.
    #[test]
    fn ideal_bpsk_loops_back_clean_at_6db_margin() {
        let mut link = ideal_bpsk();
        let sensitivity = ebn0_at_ber(theory::bpsk_ber, 1e-3);
        let payloads = Payloads::new(0x00e2e, 20, link.bits_per_trial);
        let mut channel = channel_at_margin(&ChannelSpec::default(), &link, sensitivity, 6.0);
        assert_eq!(loopback(&mut link, &mut channel, payloads), Ok(()));
    }

    #[test]
    fn inverted_link_reports_first_bit_zero_and_every_bit_counted() {
        let mut link = with_demod(ideal_bpsk(), |bits| bits.into_iter().map(|b| !b).collect());
        let mut channel = ChannelSpec::default().build();
        let n = 512;
        let err = loopback(&mut link, &mut channel, Payloads::new(1, 3, n)).unwrap_err();
        assert_eq!(err.payload_index, 0);
        assert_eq!(err.first_bit, 0);
        assert_eq!(err.bit_errors, n as u64);
        assert_eq!(err.payload_bits, n);
        assert_eq!(err.decoded_bits, n);
    }

    /// A single flipped bit must be located exactly — this is the `first_bit` field earning
    /// its keep, where the all-inverted case would pass with any off-by-one.
    #[test]
    fn single_flipped_bit_is_located_exactly() {
        let mut link = with_demod(ideal_bpsk(), |mut bits| {
            bits[137] = !bits[137];
            bits
        });
        let mut channel = ChannelSpec::default().build();
        let err = loopback(&mut link, &mut channel, Payloads::new(2, 1, 512)).unwrap_err();
        assert_eq!(err.first_bit, 137);
        assert_eq!(err.bit_errors, 1);
        assert_eq!(err.decoded_bits, 512);
    }

    /// Lost bits are errors, never silently a shorter comparison — the sweep runner's rule,
    /// held here too, and the `decoded_bits` count is what names the failure as truncation.
    #[test]
    fn truncated_demodulator_counts_missing_bits_as_errors() {
        let mut link = with_demod(ideal_bpsk(), |mut bits| {
            bits.truncate(100);
            bits
        });
        let mut channel = ChannelSpec::default().build();
        let err = loopback(&mut link, &mut channel, Payloads::new(3, 1, 512)).unwrap_err();
        assert_eq!(err.first_bit, 100);
        assert_eq!(err.bit_errors, 412);
        assert_eq!(err.payload_bits, 512);
        assert_eq!(err.decoded_bits, 100);
    }

    /// Determinism, both halves of the doctrine: the same seeds give the identical Mismatch,
    /// and the Mismatch's own payload seed regenerates the failing trial alone. Run 3 dB
    /// *below* sensitivity (BER ≈ 1.4e-2, ~58 expected errors in the first payload) so there
    /// is a rich Mismatch to compare.
    #[test]
    fn same_seeds_reproduce_the_identical_mismatch() {
        let sensitivity = ebn0_at_ber(theory::bpsk_ber, 1e-3);
        let run = || {
            let mut link = ideal_bpsk();
            let payloads = Payloads::new(0xd37, 4, link.bits_per_trial);
            let mut channel = channel_at_margin(&ChannelSpec::default(), &link, sensitivity, -3.0);
            loopback(&mut link, &mut channel, payloads)
        };
        let a = run().unwrap_err();
        let b = run().unwrap_err();
        assert_eq!(a, b);

        let mut link = ideal_bpsk();
        let bit_count = link.bits_per_trial;
        let mut channel = channel_at_margin(&ChannelSpec::default(), &link, sensitivity, -3.0);
        let solo = loopback(
            &mut link,
            &mut channel,
            iter::once(Payload::from_seed(a.payload_seed, bit_count)),
        )
        .unwrap_err();
        assert_eq!(solo.payload_index, 0, "solo reproduction is its own run");
        assert_eq!(solo.payload_seed, a.payload_seed);
        assert_eq!(solo.first_bit, a.first_bit);
        assert_eq!(solo.bit_errors, a.bit_errors);
    }
}
