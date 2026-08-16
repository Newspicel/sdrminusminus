use super::error::{Error, Result};

pub(crate) const BLOCK_BYTES: usize = 16_384;
const HEADER_BYTES: usize = 10;
pub(crate) const BLOCK_SAMPLES: usize = (BLOCK_BYTES - HEADER_BYTES) / 2;
pub(crate) const TRANSFER_BYTES: usize = 16 * BLOCK_BYTES;
const MAGIC: [u8; 2] = [0x7f, 0x7f];
const MAX_RANGES: usize = 10;
const RANGE_GRANULARITY_HZ: u64 = 1_000_000;
const FREQ_MIN_HZ: u64 = 1_000_000;
const FREQ_MAX_HZ: u64 = 6_000_000_000;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum SweepStyle {
    Linear = 0,
    #[default]
    Interleaved = 1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SweepRange {
    pub start_hz: u64,
    pub stop_hz: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SweepPlan {
    pub ranges: Vec<SweepRange>,
    pub blocks_per_tuning: u32,
    pub step_width_hz: u32,
    pub offset_hz: u32,
    pub style: SweepStyle,
}

impl SweepPlan {
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

    fn bytes_per_tuning(&self) -> u64 {
        u64::from(self.blocks_per_tuning) * BLOCK_BYTES as u64
    }

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
        if range.stop_hz + u64::from(self.offset_hz) > FREQ_MAX_HZ {
            return Err(Error::invalid_config(
                "sweep range",
                "the tuning offset carries the top of this range past 6 GHz",
            ));
        }
        Ok(())
    }

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

    #[must_use]
    pub fn first_stamp_hz(&self) -> Option<u64> {
        self.ranges.first().map(|range| range.start_hz)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SweepBlock<'a> {
    pub(crate) stamp_hz: u64,
    pub(crate) tuned_hz: u64,
    pub(crate) iq: &'a [u8],
}

pub(crate) struct SweepBlocks<'a> {
    rest: &'a [u8],
    offset_hz: u64,
    skipped: usize,
}

impl<'a> SweepBlocks<'a> {
    pub(crate) const fn new(transfer: &'a [u8], offset_hz: u64) -> Self {
        Self {
            rest: transfer,
            offset_hz,
            skipped: 0,
        }
    }

    /// Blocks this transfer carried that were not sweep blocks, so a framing fault is reported
    /// rather than read as a quiet gap in the sweep.
    pub(crate) const fn skipped(&self) -> usize {
        self.skipped
    }
}

impl<'a> Iterator for SweepBlocks<'a> {
    type Item = SweepBlock<'a>;

    fn next(&mut self) -> Option<SweepBlock<'a>> {
        loop {
            let Some((block, rest)) = self.rest.split_at_checked(BLOCK_BYTES) else {
                if !self.rest.is_empty() {
                    self.skipped += 1;
                    self.rest = &self.rest[..0];
                }
                return None;
            };
            self.rest = rest;
            let (header, iq) = block.split_at(HEADER_BYTES);
            let (magic, stamp) = header.split_at(MAGIC.len());
            let (Some(stamp), true) = (stamp.first_chunk::<8>(), magic == MAGIC) else {
                self.skipped += 1;
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

    #[test]
    fn the_interleaved_default_is_hackrf_sweeps_geometry() {
        let plan = plan();
        assert_eq!(plan.step_width_hz, 20_000_000);
        assert_eq!(plan.offset_hz, 7_500_000);
        assert_eq!(plan.blocks_per_tuning, 1);
        assert_eq!(plan.style, SweepStyle::Interleaved);
    }

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
        assert_eq!((data.len() - 9) / 4, 2);
    }

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
        assert_eq!(blocks[0].tuned_hz, 95_500_000);
        assert_eq!(blocks[0].iq.len(), BLOCK_BYTES - HEADER_BYTES);
        assert!(blocks[0].iq.iter().all(|byte| *byte == 0x11));
        assert_eq!(blocks[1].stamp_hz, 93_000_000);
        assert_eq!(blocks[1].tuned_hz, 100_500_000);
    }

    #[test]
    fn blocks_without_the_magic_are_skipped_not_guessed() {
        let mut transfer = block(88_000_000, [0x00, 0x00], 0x11);
        transfer.extend(block(93_000_000, MAGIC, 0x22));
        let mut blocks = SweepBlocks::new(&transfer, 0);
        let found: Vec<_> = blocks.by_ref().collect();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].stamp_hz, 93_000_000);
        assert_eq!(blocks.skipped(), 1, "a dropped block has to surface");
    }

    #[test]
    fn a_partial_trailing_block_is_dropped_and_counted() {
        let mut transfer = block(88_000_000, MAGIC, 0x11);
        transfer.extend_from_slice(&block(93_000_000, MAGIC, 0x22)[..BLOCK_BYTES - 1]);
        let mut blocks = SweepBlocks::new(&transfer, 0);
        let found: Vec<_> = blocks.by_ref().collect();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].stamp_hz, 88_000_000);
        assert_eq!(blocks.skipped(), 1, "the truncated tail has to surface");

        let mut empty = SweepBlocks::new(&[], 0);
        assert_eq!(empty.by_ref().count(), 0);
        assert_eq!(empty.skipped(), 0, "an empty transfer lost nothing");

        let mut runt = SweepBlocks::new(&[0x7f, 0x7f], 0);
        assert_eq!(runt.by_ref().count(), 0);
        assert_eq!(runt.skipped(), 1);
    }

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
