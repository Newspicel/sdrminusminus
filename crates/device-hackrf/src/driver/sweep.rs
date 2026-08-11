//! The firmware's own sweep: what to ask it for, and how to read what comes back.
//!
//! In sweep mode the LPC retunes the radio itself between captures, so a wideband survey costs
//! one control transfer instead of a host round trip per step — the retune happens inside the
//! ~760 µs the M0 spends discarding buffers, which no host-driven scanner can match.
//!
//! What arrives on the bulk endpoint is no longer a plain sample stream. It is a run of
//! [`BLOCK_BYTES`]-sized blocks, each stamped with the frequency it was captured at, because the
//! host no longer knows where the radio is pointing:
//!
//! ```text
//! 0        2                       10                                  16384
//! | 7f 7f  | sweep_freq, u64 LE Hz  | interleaved signed 8-bit IQ ...   |
//! ```
//!
//! Two things about that stamp are easy to get wrong, and both are load-bearing:
//!
//! - It is the *low edge* the firmware is sweeping from, not the tuned centre. The radio sits at
//!   `sweep_freq + offset` ([`SweepPlan::offset_hz`]) — see [`SweepBlock::tuned_hz`].
//! - A pass wraps silently. The way to see it is the way `hackrf_sweep` sees it: the stamp comes
//!   back to the first range's start.
//!
//! Everything here is pure. The plan is encoded and the blocks are parsed without touching USB,
//! which is the only way any of it is testable (PLAN §14: no hardware in CI, ever).

use super::error::{Error, Result};

/// Bytes the firmware emits per capture, header included. Fixed in `usb_api_sweep.c` as the M0's
/// buffer size; the dwell request is denominated in these and nothing else divides evenly.
pub(crate) const BLOCK_BYTES: usize = 16_384;
/// Frequency stamp: two magic bytes then a little-endian `u64` of Hz.
const HEADER_BYTES: usize = 10;
/// IQ pairs one block carries once its stamp is stripped.
pub(crate) const BLOCK_SAMPLES: usize = (BLOCK_BYTES - HEADER_BYTES) / 2;
/// USB transfer size a sweep streams at. A whole number of blocks, because a transfer that
/// ended mid-block would split a capture across two deliveries and strand the half without the
/// stamp; sixteen of them is the same 256 KiB the plain receive path runs at, which is what the
/// shared transport's queue depth and error policy are tuned for.
pub(crate) const TRANSFER_BYTES: usize = 16 * BLOCK_BYTES;
/// What marks the start of a block. A transfer that does not begin with it is out of step with
/// the firmware's framing, and its samples belong to no frequency anyone can name.
const MAGIC: [u8; 2] = [0x7f, 0x7f];
/// Ranges the firmware has room for (`MAX_RANGES` in `usb_api_sweep.c`).
const MAX_RANGES: usize = 10;
/// The firmware keeps its range list as whole MHz in a `u16`, so nothing finer can be asked for.
const RANGE_GRANULARITY_HZ: u64 = 1_000_000;
/// Tuner limits, as [`config::validate_frequency`] enforces them for a manual tune.
const FREQ_MIN_HZ: u64 = 1_000_000;
const FREQ_MAX_HZ: u64 = 6_000_000_000;

/// How the firmware walks a range.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum SweepStyle {
    /// One tuning per `step_width_hz`, in order.
    Linear = 0,
    /// Tunings alternate `+step/4` and `+3·step/4`, so the two quarter-band slices either side
    /// of the tuned centre tile the range without a gap. This is what `hackrf_sweep` runs, and
    /// the reason it can cover a band with a filter narrower than the sample rate.
    #[default]
    Interleaved = 1,
}

/// One span to sweep. Inclusive of `start_hz`, exclusive of `stop_hz` — the firmware advances
/// while the *next* tuning would still be below the top, so the last capture sits under it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SweepRange {
    pub start_hz: u64,
    pub stop_hz: u64,
}

/// What the firmware is asked to sweep, and how.
///
/// Construct it with [`SweepPlan::interleaved`] unless you have a reason not to: the defaults
/// there are `hackrf_sweep`'s, and they are the ones the slice arithmetic below assumes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SweepPlan {
    pub ranges: Vec<SweepRange>,
    /// Captures per tuning. One is `hackrf_sweep`'s default and the fastest possible sweep;
    /// more trades sweep rate for averaging depth at each step.
    pub blocks_per_tuning: u32,
    /// How far the firmware advances between tunings.
    pub step_width_hz: u32,
    /// Distance from the stamped frequency to the tuned centre.
    pub offset_hz: u32,
    pub style: SweepStyle,
}

impl SweepPlan {
    /// `hackrf_sweep`'s geometry at `sample_rate_hz`: step a whole passband at a time and tune
    /// three eighths of it above the stamp, which is what puts the two usable quarter-band
    /// slices at `[stamp, stamp + rate/4]` and `[stamp + rate/2, stamp + 3·rate/4]` and lets
    /// consecutive interleaved tunings tile a range with no gap and no overlap.
    #[must_use]
    pub fn interleaved(ranges: Vec<SweepRange>, sample_rate_hz: u32) -> Self {
        Self {
            ranges,
            blocks_per_tuning: 1,
            step_width_hz: sample_rate_hz,
            offset_hz: (u64::from(sample_rate_hz) * 3 / 8) as u32,
            style: SweepStyle::Interleaved,
        }
    }

    /// Bytes the firmware captures per tuning, which is how the dwell is expressed on the wire.
    fn bytes_per_tuning(&self) -> u64 {
        u64::from(self.blocks_per_tuning) * BLOCK_BYTES as u64
    }

    /// Reject everything the firmware would stall on, plus the arithmetic traps it would take
    /// silently — before any control transfer runs, so a bad plan cannot leave the radio in
    /// sweep mode with a range list it will never finish.
    fn validate(&self) -> Result<()> {
        if self.ranges.is_empty() || self.ranges.len() > MAX_RANGES {
            return Err(Error::invalid_config(
                "sweep ranges",
                "a sweep needs between 1 and 10 ranges",
            ));
        }
        if self.blocks_per_tuning == 0 {
            return Err(Error::invalid_config(
                "blocks_per_tuning",
                "each tuning must capture at least one block",
            ));
        }
        if u32::try_from(self.bytes_per_tuning()).is_err() {
            return Err(Error::invalid_config(
                "blocks_per_tuning",
                "the dwell must fit in the request's 32-bit byte count",
            ));
        }
        if self.step_width_hz == 0 {
            return Err(Error::invalid_config(
                "step_width_hz",
                "the sweep must advance by at least 1 Hz per tuning",
            ));
        }
        // The firmware advances an interleaved sweep by `step/4` then `3·step/4`, each an
        // integer division: a step that is not a multiple of four loses the remainder every
        // pair of tunings and the sweep walks off its own grid.
        if self.style == SweepStyle::Interleaved && !self.step_width_hz.is_multiple_of(4) {
            return Err(Error::invalid_config(
                "step_width_hz",
                "an interleaved sweep steps in quarters, so the width must be a multiple of 4 Hz",
            ));
        }
        for range in &self.ranges {
            self.validate_range(range)?;
        }
        Ok(())
    }

    fn validate_range(&self, range: &SweepRange) -> Result<()> {
        if !range.start_hz.is_multiple_of(RANGE_GRANULARITY_HZ)
            || !range.stop_hz.is_multiple_of(RANGE_GRANULARITY_HZ)
        {
            return Err(Error::invalid_config(
                "sweep range",
                "the firmware holds range bounds as whole MHz",
            ));
        }
        if range.start_hz < FREQ_MIN_HZ || range.stop_hz > FREQ_MAX_HZ {
            return Err(Error::invalid_config(
                "sweep range",
                "must be between 1 MHz and 6 GHz inclusive",
            ));
        }
        if range.start_hz >= range.stop_hz {
            return Err(Error::invalid_config(
                "sweep range",
                "must end above where it starts",
            ));
        }
        // Conservative by up to one step: the highest tuning is the last stamp *below* the top
        // plus the offset, so this refuses a plan whose final tunings would ask the synthesizer
        // for a frequency it cannot reach — where the firmware would simply fail to lock and
        // hand back blocks of noise stamped with a frequency the radio was never on.
        if range.stop_hz + u64::from(self.offset_hz) > FREQ_MAX_HZ {
            return Err(Error::invalid_config(
                "sweep range",
                "the tuning offset carries the top of this range past 6 GHz",
            ));
        }
        Ok(())
    }

    /// The validated request: the `wValue`/`wIndex` byte count and the payload, in libhackrf's
    /// `hackrf_init_sweep` layout.
    pub(crate) fn encode(&self) -> Result<(u32, Vec<u8>)> {
        self.validate()?;
        let mut data = Vec::with_capacity(9 + self.ranges.len() * 4);
        data.extend_from_slice(&self.step_width_hz.to_le_bytes());
        data.extend_from_slice(&self.offset_hz.to_le_bytes());
        data.push(self.style as u8);
        for range in &self.ranges {
            for bound in [range.start_hz, range.stop_hz] {
                let mhz = (bound / RANGE_GRANULARITY_HZ) as u16;
                data.extend_from_slice(&mhz.to_le_bytes());
            }
        }
        Ok((self.bytes_per_tuning() as u32, data))
    }

    /// Where a pass starts over. `hackrf_sweep` counts a sweep complete when the stamp comes
    /// back to it, and a consumer here has nothing else to go on.
    #[must_use]
    pub fn first_stamp_hz(&self) -> Option<u64> {
        self.ranges.first().map(|range| range.start_hz)
    }
}

/// One capture, located.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SweepBlock<'a> {
    /// The firmware's own stamp: the low edge of the span this capture covers.
    pub(crate) stamp_hz: u64,
    /// Where the radio actually was — the stamp plus the plan's offset.
    pub(crate) tuned_hz: u64,
    /// The capture, interleaved signed 8-bit IQ, header stripped.
    ///
    /// `hackrf_sweep` reads only the *tail* of this: the retune that precedes a block settles
    /// during it, so the last samples are the trustworthy ones. A consumer that wants one FFT
    /// per tuning should take its window from the end.
    pub(crate) iq: &'a [u8],
}

/// The blocks in one USB transfer, in order.
///
/// Blocks that do not carry the magic are skipped rather than decoded, exactly as
/// `hackrf_sweep` does: a transfer that has slipped out of the firmware's framing holds samples
/// belonging to no announced frequency, and guessing one would put a signal on the wrong dial.
pub(crate) struct SweepBlocks<'a> {
    rest: &'a [u8],
    offset_hz: u64,
}

impl<'a> SweepBlocks<'a> {
    pub(crate) const fn new(transfer: &'a [u8], offset_hz: u64) -> Self {
        Self {
            rest: transfer,
            offset_hz,
        }
    }
}

impl<'a> Iterator for SweepBlocks<'a> {
    type Item = SweepBlock<'a>;

    fn next(&mut self) -> Option<SweepBlock<'a>> {
        loop {
            let (block, rest) = self.rest.split_at_checked(BLOCK_BYTES)?;
            self.rest = rest;
            let (header, iq) = block.split_at(HEADER_BYTES);
            let (magic, stamp) = header.split_at(MAGIC.len());
            // Both `Option`s are `Some` for any [`BLOCK_BYTES`] slice; matching rather than
            // asserting keeps the framing check and the decode in one expression.
            let (Some(stamp), true) = (stamp.first_chunk::<8>(), magic == MAGIC) else {
                continue;
            };
            let stamp_hz = u64::from_le_bytes(*stamp);
            return Some(SweepBlock {
                stamp_hz,
                tuned_hz: stamp_hz.saturating_add(self.offset_hz),
                iq,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start_mhz: u64, stop_mhz: u64) -> SweepRange {
        SweepRange {
            start_hz: start_mhz * 1_000_000,
            stop_hz: stop_mhz * 1_000_000,
        }
    }

    fn plan() -> SweepPlan {
        SweepPlan::interleaved(vec![range(88, 108)], 20_000_000)
    }

    /// The geometry `hackrf_sweep` runs with, spelled out — a different offset silently moves
    /// every measurement by megahertz.
    #[test]
    fn the_interleaved_default_is_hackrf_sweeps_geometry() {
        let plan = plan();
        assert_eq!(plan.step_width_hz, 20_000_000);
        assert_eq!(plan.offset_hz, 7_500_000);
        assert_eq!(plan.blocks_per_tuning, 1);
        assert_eq!(plan.style, SweepStyle::Interleaved);
    }

    /// Golden bytes against libhackrf's `hackrf_init_sweep`: four bytes of step width, four of
    /// offset, the style, then the range bounds as little-endian MHz pairs.
    #[test]
    fn a_plan_encodes_in_libhackrfs_layout() {
        let (bytes_per_tuning, data) = plan().encode().expect("a valid plan");
        assert_eq!(bytes_per_tuning, 16_384);
        assert_eq!(data.len(), 9 + 4);
        assert_eq!(&data[0..4], &20_000_000_u32.to_le_bytes());
        assert_eq!(&data[4..8], &7_500_000_u32.to_le_bytes());
        assert_eq!(data[8], 1, "interleaved");
        assert_eq!(&data[9..11], &88_u16.to_le_bytes());
        assert_eq!(&data[11..13], &108_u16.to_le_bytes());
    }

    /// The firmware reads its range count out of the request length, so every extra range must
    /// add exactly four bytes and nothing else may.
    #[test]
    fn each_range_adds_one_le_mhz_pair() {
        let mut plan = plan();
        plan.ranges.push(range(430, 440));
        plan.style = SweepStyle::Linear;
        let (_, data) = plan.encode().expect("two ranges");
        assert_eq!(data.len(), 9 + 8);
        assert_eq!(data[8], 0, "linear");
        assert_eq!(&data[13..15], &430_u16.to_le_bytes());
        assert_eq!(&data[15..17], &440_u16.to_le_bytes());
        // What the firmware divides back out to get the range count.
        assert_eq!((data.len() - 9) / 4, 2);
    }

    /// A dwell is denominated in whole blocks because that is the only unit the firmware's
    /// `num_bytes / 0x4000` divides evenly.
    #[test]
    fn the_dwell_is_a_whole_number_of_blocks() {
        let mut plan = plan();
        plan.blocks_per_tuning = 8;
        assert_eq!(plan.encode().expect("eight blocks").0, 8 * 16_384);
    }

    #[test]
    fn a_plan_the_firmware_would_stall_on_is_refused() {
        let cases: Vec<(&str, SweepPlan)> = vec![
            ("no ranges", SweepPlan::interleaved(Vec::new(), 20_000_000)),
            (
                "eleven ranges",
                SweepPlan::interleaved(
                    (0..11).map(|i| range(100 + i * 2, 101 + i * 2)).collect(),
                    20_000_000,
                ),
            ),
            (
                "zero dwell",
                SweepPlan {
                    blocks_per_tuning: 0,
                    ..plan()
                },
            ),
            (
                "zero step",
                SweepPlan {
                    step_width_hz: 0,
                    ..plan()
                },
            ),
            // Interleaved stepping divides by four; a remainder walks the sweep off its grid.
            (
                "odd step",
                SweepPlan {
                    step_width_hz: 20_000_001,
                    ..plan()
                },
            ),
            (
                "sub-MHz bound",
                SweepPlan {
                    ranges: vec![SweepRange {
                        start_hz: 88_500_000,
                        stop_hz: 108_000_000,
                    }],
                    ..plan()
                },
            ),
            (
                "inverted",
                SweepPlan {
                    ranges: vec![range(108, 88)],
                    ..plan()
                },
            ),
            (
                "empty range",
                SweepPlan {
                    ranges: vec![range(88, 88)],
                    ..plan()
                },
            ),
            (
                "below the tuner",
                SweepPlan {
                    ranges: vec![SweepRange {
                        start_hz: 0,
                        stop_hz: 10_000_000,
                    }],
                    ..plan()
                },
            ),
            (
                "above the tuner",
                SweepPlan {
                    ranges: vec![range(5_000, 6_001)],
                    ..plan()
                },
            ),
            // 6000 MHz + a 7.5 MHz offset is past the synthesizer, whatever the range says.
            (
                "offset past the top",
                SweepPlan {
                    ranges: vec![range(5_900, 6_000)],
                    ..plan()
                },
            ),
        ];
        for (name, plan) in cases {
            assert!(plan.encode().is_err(), "{name} must be refused");
        }
    }

    /// A linear sweep steps whole, so it has no multiple-of-four constraint to break.
    #[test]
    fn a_linear_sweep_takes_any_step_width() {
        let plan = SweepPlan {
            step_width_hz: 20_000_001,
            style: SweepStyle::Linear,
            ..plan()
        };
        assert!(plan.encode().is_ok());
    }

    fn block(stamp_hz: u64, magic: [u8; 2], fill: u8) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(BLOCK_BYTES);
        bytes.extend_from_slice(&magic);
        bytes.extend_from_slice(&stamp_hz.to_le_bytes());
        bytes.resize(BLOCK_BYTES, fill);
        bytes
    }

    #[test]
    fn a_transfer_splits_into_stamped_blocks() {
        let mut transfer = block(88_000_000, MAGIC, 0x11);
        transfer.extend(block(93_000_000, MAGIC, 0x22));
        let blocks: Vec<_> = SweepBlocks::new(&transfer, 7_500_000).collect();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].stamp_hz, 88_000_000);
        // The stamp is the low edge; the radio is three eighths of a passband above it.
        assert_eq!(blocks[0].tuned_hz, 95_500_000);
        assert_eq!(blocks[0].iq.len(), BLOCK_BYTES - HEADER_BYTES);
        assert!(blocks[0].iq.iter().all(|byte| *byte == 0x11));
        assert_eq!(blocks[1].stamp_hz, 93_000_000);
        assert_eq!(blocks[1].tuned_hz, 100_500_000);
    }

    /// A block without the magic belongs to no announced frequency. Skipping it is what
    /// `hackrf_sweep` does, and the alternative — decoding it under the previous stamp — would
    /// draw a signal at a frequency the radio was never tuned to.
    #[test]
    fn blocks_without_the_magic_are_skipped_not_guessed() {
        let mut transfer = block(88_000_000, [0x00, 0x00], 0x11);
        transfer.extend(block(93_000_000, MAGIC, 0x22));
        let blocks: Vec<_> = SweepBlocks::new(&transfer, 0).collect();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].stamp_hz, 93_000_000);
    }

    /// A short tail is not a block: half a capture stamped with half a frequency is worse than
    /// no capture at all.
    #[test]
    fn a_partial_trailing_block_is_dropped() {
        let mut transfer = block(88_000_000, MAGIC, 0x11);
        transfer.extend_from_slice(&block(93_000_000, MAGIC, 0x22)[..BLOCK_BYTES - 1]);
        let blocks: Vec<_> = SweepBlocks::new(&transfer, 0).collect();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].stamp_hz, 88_000_000);

        assert_eq!(SweepBlocks::new(&[], 0).count(), 0);
        assert_eq!(SweepBlocks::new(&[0x7f, 0x7f], 0).count(), 0);
    }

    /// Two framing invariants the reader depends on and cannot check at runtime: a transfer is
    /// a whole number of blocks (so no capture is ever split across two of them), and a block's
    /// payload is a whole number of IQ pairs (so the shared converter's carry byte — which
    /// exists for short blocks and would shift every later sample by one — never engages).
    #[test]
    fn the_framing_divides_evenly_at_both_levels() {
        assert_eq!(TRANSFER_BYTES % BLOCK_BYTES, 0);
        assert_eq!((BLOCK_BYTES - HEADER_BYTES) % 2, 0);
        assert_eq!(BLOCK_SAMPLES, 8_187);
    }

    #[test]
    fn the_first_stamp_is_where_a_pass_starts_over() {
        assert_eq!(plan().first_stamp_hz(), Some(88_000_000));
        assert_eq!(
            SweepPlan::interleaved(Vec::new(), 20_000_000).first_stamp_hz(),
            None
        );
    }
}
