use super::super::pl;

pub const LENGTH: usize = 612_540;
pub const SOSF: usize = 270;
pub const HEADER: usize = 720;
pub const CU: usize = pl::SLOT;
pub const PERIOD: usize = 1476;
pub const PILOT_START: usize = 1440;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    pub start: usize,
    pub period: usize,
    pub length: usize,
    pub count: usize,
}

pub const TYPE_A: Grid = Grid {
    start: PILOT_START,
    period: PERIOD,
    length: pl::PILOT_LENGTH,
    count: usize::MAX,
};

impl Grid {
    #[must_use]
    pub const fn inside(&self, position: usize) -> bool {
        position >= self.start
            && (position - self.start) / self.period < self.count
            && (position - self.start) % self.period < self.length
    }

    #[must_use]
    pub const fn starts(&self, position: usize) -> bool {
        position >= self.start
            && (position - self.start) / self.period < self.count
            && (position - self.start).is_multiple_of(self.period)
    }

    #[must_use]
    pub const fn next(&self, position: usize) -> usize {
        if position <= self.start {
            return self.start;
        }
        let index = (position - self.start).div_ceil(self.period);
        if index >= self.count {
            usize::MAX
        } else {
            self.start + index * self.period
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bundles {
    pub replicas: usize,
    pub known: usize,
    pub payload: usize,
    pub count: usize,
    pub grid: Grid,
    pub short_pilots: bool,
}

impl Bundles {
    #[must_use]
    pub const fn header(&self) -> usize {
        self.replicas * 64
    }

    #[must_use]
    pub const fn stream(&self) -> usize {
        self.header() + self.known + self.payload
    }

    #[must_use]
    pub const fn tail(&self) -> usize {
        HEADER + self.count * (self.stream() + self.pilots_per_bundle() * self.grid.length)
    }

    #[must_use]
    pub const fn pilots_per_bundle(&self) -> usize {
        self.grid.count / self.count
    }
}

pub const LONG: Bundles = Bundles {
    replicas: 6,
    known: 180,
    payload: 64_800,
    count: 9,
    grid: Grid {
        start: 1664,
        period: 956,
        length: 36,
        count: 639,
    },
    short_pilots: false,
};

pub const SHORT: Bundles = Bundles {
    replicas: 4,
    known: 96,
    payload: 16_200,
    count: 36,
    grid: Grid {
        start: 1800,
        period: 1887,
        length: 48,
        count: 324,
    },
    short_pilots: true,
};

#[must_use]
pub const fn bundles(format: u8) -> Option<Bundles> {
    match format {
        2 => Some(LONG),
        3 => Some(SHORT),
        _ => None,
    }
}

#[must_use]
pub const fn payload_start(format: u8) -> usize {
    if format == 7 { HEADER } else { PILOT_START }
}

#[must_use]
pub const fn first_unit(format: u8) -> u64 {
    (payload_start(format) / CU) as u64
}

#[must_use]
pub const fn fixed_length(format: u8) -> bool {
    format <= 4
}

#[must_use]
pub const fn always_pilots(format: u8) -> bool {
    matches!(format, 6 | 7)
}

#[must_use]
pub const fn fragments(format: u8) -> bool {
    matches!(format, 4 | 5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_layouts_add_up_to_a_superframe() {
        assert_eq!(LONG.stream() + LONG.pilots_per_bundle() * 36, 67_920);
        assert_eq!(SHORT.stream() + SHORT.pilots_per_bundle() * 48, 16_984);
        assert_eq!(LONG.tail() + 540, LENGTH);
        assert_eq!(SHORT.tail() + 396, LENGTH);
        for bundles in [LONG, SHORT] {
            let span = bundles.stream() + bundles.pilots_per_bundle() * bundles.grid.length;
            for index in 0..bundles.count {
                let start = HEADER + index * span;
                for position in start..start + bundles.header() {
                    assert!(!bundles.grid.inside(position), "{position}");
                }
                let inside = (start..start + span)
                    .filter(|&position| bundles.grid.starts(position))
                    .count();
                assert_eq!(inside, bundles.pilots_per_bundle());
            }
        }
    }

    #[test]
    fn type_a_pilots_close_a_fixed_superframe() {
        assert!(TYPE_A.inside(LENGTH - 1));
        assert!(TYPE_A.starts(LENGTH - 36));
        assert_eq!((LENGTH - PILOT_START - 415 * 36) % CU, 0);
        assert_eq!((LENGTH - PILOT_START) % CU, 0);
        assert_eq!(TYPE_A.next(1441), 1440 + 1476);
    }
}
