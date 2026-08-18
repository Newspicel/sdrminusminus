const MAX_ROWS: usize = 64;
const MAX_ROW_BITS: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Coding {
    Pcm {
        short_us: u32,
        long_us: u32,
    },
    Ppm {
        short_us: u32,
        long_us: u32,
    },
    Pwm {
        short_us: u32,
        long_us: u32,
        sync_us: u32,
    },
    Manchester {
        short_us: u32,
    },
    Dmc {
        short_us: u32,
        long_us: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Framing {
    pub coding: Coding,
    pub gap_us: u32,
    pub reset_us: u32,
    pub tolerance_us: u32,
}

struct Pulses<'a> {
    edges_us: &'a [u32],
}

impl<'a> Pulses<'a> {
    fn new(edges_us: &'a [u32]) -> Self {
        Self { edges_us }
    }

    fn len(&self) -> usize {
        self.edges_us.len().div_ceil(2)
    }

    fn pulse(&self, index: usize) -> u32 {
        self.edges_us[index * 2]
    }

    fn gap(&self, index: usize) -> u32 {
        self.edges_us
            .get(index * 2 + 1)
            .copied()
            .unwrap_or(u32::MAX)
    }

    fn symbols(&self) -> usize {
        self.edges_us.len()
    }

    fn symbol(&self, index: usize) -> u32 {
        self.edges_us[index]
    }
}

#[derive(Default)]
struct Rows {
    rows: Vec<Vec<bool>>,
}

impl Rows {
    fn new() -> Self {
        Self {
            rows: vec![Vec::new()],
        }
    }

    fn bit(&mut self, value: bool) {
        if let Some(row) = self.rows.last_mut()
            && row.len() < MAX_ROW_BITS
        {
            row.push(value);
        }
    }

    fn break_row(&mut self) {
        if self.rows.len() < MAX_ROWS {
            self.rows.push(Vec::new());
        }
    }

    fn sync(&mut self) {
        if self.rows.last().is_some_and(|row| !row.is_empty()) {
            self.break_row();
        }
    }

    fn discard(&mut self) {
        self.rows.clear();
        self.rows.push(Vec::new());
    }

    fn finish(self) -> Vec<Vec<bool>> {
        self.rows
            .into_iter()
            .filter(|row| !row.is_empty())
            .collect()
    }
}

pub(super) fn slice(framing: &Framing, edges_us: &[u32]) -> Vec<Vec<bool>> {
    if edges_us.is_empty() {
        return Vec::new();
    }
    let pulses = Pulses::new(edges_us);
    match framing.coding {
        Coding::Pcm { short_us, long_us } => pcm(framing, &pulses, short_us, long_us),
        Coding::Ppm { short_us, long_us } => ppm(framing, &pulses, short_us, long_us),
        Coding::Pwm {
            short_us,
            long_us,
            sync_us,
        } => pwm(framing, &pulses, short_us, long_us, sync_us),
        Coding::Manchester { short_us } => manchester(framing, &pulses, short_us),
        Coding::Dmc { short_us, long_us } => dmc(framing, &pulses, short_us, long_us),
    }
    .finish()
}

fn divide_round(value: i64, divisor: u32) -> i64 {
    let divisor = i64::from(divisor);
    if divisor <= 0 {
        return 0;
    }
    (value * 2 + divisor) / (divisor * 2)
}

fn pcm(framing: &Framing, pulses: &Pulses, short_us: u32, long_us: u32) -> Rows {
    let mut rows = Rows::new();
    if short_us == 0 || long_us == 0 {
        return rows;
    }
    let gap_limit = if framing.gap_us == 0 {
        framing.reset_us
    } else {
        framing.gap_us
    };
    let tolerance = if framing.tolerance_us == 0 {
        long_us / 4
    } else {
        framing.tolerance_us
    };
    let max_zeros = i64::from(gap_limit / long_us);
    let last = pulses.len() - 1;
    for index in 0..pulses.len() {
        let pulse = pulses.pulse(index);
        let gap = pulses.gap(index);
        let highs = divide_round(i64::from(pulse), short_us);
        for _ in 0..highs {
            rows.bit(true);
        }
        if index != last {
            let span = i64::from(gap) + i64::from(short_us) - i64::from(long_us);
            let lows = divide_round(span, long_us).clamp(0, max_zeros);
            for _ in 0..lows {
                rows.bit(false);
            }
        }
        if short_us != long_us && pulse.abs_diff(short_us) > tolerance {
            rows.discard();
        } else if gap > gap_limit {
            rows.break_row();
        }
    }
    rows
}

struct Band {
    lower: u32,
    upper: u32,
}

impl Band {
    const NONE: Self = Self { lower: 0, upper: 0 };

    fn holds(&self, measured: u32) -> bool {
        measured > self.lower && measured < self.upper
    }
}

fn ppm(framing: &Framing, pulses: &Pulses, short_us: u32, long_us: u32) -> Rows {
    let (zero, one, sync) = if framing.tolerance_us > 0 {
        let slack = framing.tolerance_us;
        (
            Band {
                lower: short_us.saturating_sub(slack),
                upper: short_us + slack,
            },
            Band {
                lower: long_us.saturating_sub(slack),
                upper: long_us + slack,
            },
            Band::NONE,
        )
    } else {
        let middle = (short_us + long_us) / 2 + 1;
        let ceiling = if framing.gap_us == 0 {
            framing.reset_us
        } else {
            framing.gap_us
        };
        (
            Band {
                lower: 0,
                upper: middle,
            },
            Band {
                lower: middle - 1,
                upper: ceiling,
            },
            Band::NONE,
        )
    };
    let mut rows = Rows::new();
    for index in 0..pulses.len() {
        let gap = pulses.gap(index);
        if zero.holds(gap) {
            rows.bit(false);
        } else if one.holds(gap) {
            rows.bit(true);
        } else if sync.holds(gap) {
            rows.sync();
        } else {
            rows.break_row();
        }
    }
    rows
}

fn pwm_bands(short_us: u32, long_us: u32, sync_us: u32, tolerance_us: u32) -> (Band, Band, Band) {
    if tolerance_us > 0 {
        let band = |nominal: u32| Band {
            lower: nominal.saturating_sub(tolerance_us),
            upper: nominal + tolerance_us,
        };
        let sync = if sync_us > 0 {
            band(sync_us)
        } else {
            Band::NONE
        };
        return (band(short_us), band(long_us), sync);
    }
    let middle = |a: u32, b: u32| (a + b) / 2 + 1;
    if sync_us == 0 {
        let edge = middle(short_us, long_us);
        (
            Band {
                lower: 0,
                upper: edge,
            },
            Band {
                lower: edge - 1,
                upper: u32::MAX,
            },
            Band::NONE,
        )
    } else if sync_us < short_us {
        let sync_edge = middle(sync_us, short_us);
        let one_edge = middle(short_us, long_us);
        (
            Band {
                lower: sync_edge - 1,
                upper: one_edge,
            },
            Band {
                lower: one_edge - 1,
                upper: u32::MAX,
            },
            Band {
                lower: 0,
                upper: sync_edge,
            },
        )
    } else if sync_us < long_us {
        let one_edge = middle(short_us, sync_us);
        let sync_edge = middle(sync_us, long_us);
        (
            Band {
                lower: 0,
                upper: one_edge,
            },
            Band {
                lower: sync_edge - 1,
                upper: u32::MAX,
            },
            Band {
                lower: one_edge - 1,
                upper: sync_edge,
            },
        )
    } else {
        let one_edge = middle(short_us, long_us);
        let zero_edge = middle(long_us, sync_us);
        (
            Band {
                lower: 0,
                upper: one_edge,
            },
            Band {
                lower: one_edge - 1,
                upper: zero_edge,
            },
            Band {
                lower: zero_edge - 1,
                upper: u32::MAX,
            },
        )
    }
}

fn pwm(framing: &Framing, pulses: &Pulses, short_us: u32, long_us: u32, sync_us: u32) -> Rows {
    let (one, zero, sync) = pwm_bands(short_us, long_us, sync_us, framing.tolerance_us);
    let mut rows = Rows::new();
    for index in 0..pulses.len() {
        let pulse = pulses.pulse(index);
        if one.holds(pulse) {
            rows.bit(true);
        } else if zero.holds(pulse) {
            rows.bit(false);
        } else if sync.holds(pulse) {
            rows.sync();
        } else if pulse > one.lower {
            rows.break_row();
        }
        let gap = pulses.gap(index);
        if gap <= framing.reset_us && framing.gap_us > 0 && gap > framing.gap_us {
            rows.break_row();
        }
    }
    rows
}

fn manchester(framing: &Framing, pulses: &Pulses, short_us: u32) -> Rows {
    let mut rows = Rows::new();
    let mut since_last = 0u32;
    let half = short_us + short_us / 2;
    let last = pulses.len() - 1;
    rows.bit(false);
    for index in 0..pulses.len() {
        let pulse = pulses.pulse(index);
        let gap = pulses.gap(index);
        let slack = framing.tolerance_us;
        let out_of_band = slack > 0
            && (pulse < short_us.saturating_sub(slack)
                || pulse > short_us * 2 + slack
                || gap < short_us.saturating_sub(slack)
                || gap > short_us * 2 + slack);
        if out_of_band {
            if pulse > half && pulse <= short_us * 2 + slack {
                rows.bit(true);
            }
            rows.break_row();
            if index != last {
                rows.bit(false);
            }
            since_last = 0;
        } else if pulse + since_last > half {
            rows.bit(true);
            since_last = 0;
        } else {
            since_last += pulse;
        }

        if gap > framing.reset_us {
            if index != last {
                rows.break_row();
                rows.bit(false);
            }
            since_last = 0;
        } else if gap + since_last > half {
            rows.bit(false);
            since_last = 0;
        } else {
            since_last += gap;
        }
    }
    rows
}

fn dmc(framing: &Framing, pulses: &Pulses, short_us: u32, long_us: u32) -> Rows {
    let mut rows = Rows::new();
    let slack = framing.tolerance_us;
    let mut index = 0;
    while index < pulses.symbols() {
        let symbol = pulses.symbol(index);
        if symbol.abs_diff(short_us) < slack {
            rows.bit(true);
            index += 1;
            let next = if index < pulses.symbols() {
                pulses.symbol(index)
            } else {
                0
            };
            if next.abs_diff(short_us) > slack && next < framing.reset_us.saturating_sub(slack) {
                rows.break_row();
            }
        } else if symbol.abs_diff(long_us) < slack {
            rows.bit(false);
        } else if symbol >= framing.reset_us.saturating_sub(slack) {
            rows.break_row();
        }
        index += 1;
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framing(coding: Coding, gap_us: u32, reset_us: u32, tolerance_us: u32) -> Framing {
        Framing {
            coding,
            gap_us,
            reset_us,
            tolerance_us,
        }
    }

    fn bits_of(text: &str) -> Vec<bool> {
        text.chars().map(|c| c == '1').collect()
    }

    #[test]
    fn nrz_reads_a_run_of_ones_and_zeros_from_one_pulse_and_gap() {
        let edges = [58 * 3, 58 * 2, 58, 58 * 4, 58];
        let rows = slice(
            &framing(
                Coding::Pcm {
                    short_us: 58,
                    long_us: 58,
                },
                0,
                4_000,
                0,
            ),
            &edges,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], bits_of("11100100001"));
    }

    #[test]
    fn rz_gives_one_bit_per_pulse_and_counts_the_gap_in_bit_periods() {
        let edges = [100, 900, 100, 1_900, 100];
        let rows = slice(
            &framing(
                Coding::Pcm {
                    short_us: 100,
                    long_us: 1_000,
                },
                0,
                5_000,
                0,
            ),
            &edges,
        );
        assert_eq!(rows[0], bits_of("1101"));
    }

    #[test]
    fn a_pcm_pulse_outside_tolerance_throws_the_message_away() {
        let edges = [58, 58, 400, 58, 58, 58];
        let rows = slice(
            &framing(
                Coding::Pcm {
                    short_us: 58,
                    long_us: 116,
                },
                0,
                4_000,
                20,
            ),
            &edges,
        );
        assert!(
            rows.iter().all(|row| row.len() <= 2),
            "the corrupt pulse must not leave a long row: {rows:?}"
        );
    }

    #[test]
    fn ppm_reads_the_gap_and_breaks_the_row_on_anything_else() {
        let edges = [500, 1_000, 500, 2_000, 500, 4_000, 500, 1_000, 500];
        let rows = slice(
            &framing(
                Coding::Ppm {
                    short_us: 1_000,
                    long_us: 2_000,
                },
                3_000,
                8_000,
                0,
            ),
            &edges,
        );
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0], bits_of("01"));
        assert_eq!(rows[1], bits_of("0"));
    }

    #[test]
    fn pwm_calls_the_short_pulse_a_one_the_way_the_specs_are_written() {
        let edges = [208, 417, 417, 208, 208, 417, 208];
        let rows = slice(
            &framing(
                Coding::Pwm {
                    short_us: 208,
                    long_us: 417,
                    sync_us: 0,
                },
                0,
                1_700,
                0,
            ),
            &edges,
        );
        assert_eq!(rows[0], bits_of("1011"));
    }

    #[test]
    fn a_pwm_sync_pulse_starts_a_new_row_without_adding_a_bit() {
        let edges = [833, 833, 833, 833, 208, 417, 417, 208, 208];
        let rows = slice(
            &framing(
                Coding::Pwm {
                    short_us: 208,
                    long_us: 417,
                    sync_us: 833,
                },
                0,
                1_700,
                100,
            ),
            &edges,
        );
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0], bits_of("101"));
    }

    #[test]
    fn manchester_starts_every_message_with_the_hardcoded_zero_bit() {
        let edges = [500, 500, 500, 500, 500, 500];
        let rows = slice(
            &framing(Coding::Manchester { short_us: 500 }, 0, 2_400, 0),
            &edges,
        );
        assert_eq!(rows[0].first(), Some(&false));
        assert!(rows[0].len() > 1, "{rows:?}");
    }

    #[test]
    fn manchester_reads_a_double_width_pulse_as_a_change_of_bit() {
        let edges = [500, 1_000, 1_000, 500, 500, 500, 500];
        let rows = slice(
            &framing(Coding::Manchester { short_us: 500 }, 0, 2_400, 0),
            &edges,
        );
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(rows[0].len() >= 4, "{rows:?}");
    }

    #[test]
    fn dmc_reads_a_short_symbol_pair_as_one_and_a_long_symbol_as_zero() {
        let edges = [230, 230, 460, 230, 230, 460, 460];
        let rows = slice(
            &framing(
                Coding::Dmc {
                    short_us: 230,
                    long_us: 460,
                },
                0,
                4_000,
                120,
            ),
            &edges,
        );
        assert_eq!(rows[0], bits_of("10100"));
    }

    #[test]
    fn an_empty_edge_list_slices_to_no_rows() {
        for coding in [
            Coding::Pcm {
                short_us: 58,
                long_us: 58,
            },
            Coding::Ppm {
                short_us: 1_000,
                long_us: 2_000,
            },
            Coding::Pwm {
                short_us: 208,
                long_us: 417,
                sync_us: 0,
            },
            Coding::Manchester { short_us: 500 },
            Coding::Dmc {
                short_us: 230,
                long_us: 460,
            },
        ] {
            assert!(slice(&framing(coding, 0, 4_000, 100), &[]).is_empty());
        }
    }
}
