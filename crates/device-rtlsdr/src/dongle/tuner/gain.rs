const LNA_STEPS: [i32; 16] = [0, 9, 13, 40, 38, 13, 31, 22, 26, 31, 26, 14, 19, 5, 35, 13];
const MIXER_STEPS: [i32; 16] = [0, 5, 10, 10, 19, 9, 10, 25, 17, 10, 8, 16, 13, 6, 3, -8];
const ROUNDS: usize = 15;

pub(crate) const GAINS: &[i32] = &[
    0, 9, 14, 27, 37, 77, 87, 125, 144, 157, 166, 197, 207, 229, 254, 280, 297, 328, 338, 364, 372,
    386, 402, 421, 434, 439, 445, 480, 496,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stages {
    pub(crate) lna: u8,
    pub(crate) mixer: u8,
}

impl Stages {
    pub(crate) fn tenths(self) -> i32 {
        stage_sum(&LNA_STEPS, self.lna) + stage_sum(&MIXER_STEPS, self.mixer)
    }

    pub(crate) const fn from_status(status: u8) -> Self {
        Self {
            lna: status & 0x0f,
            mixer: status >> 4,
        }
    }
}

fn stage_sum(steps: &[i32; 16], index: u8) -> i32 {
    steps.iter().skip(1).take(usize::from(index)).sum()
}

pub(crate) fn stages_for(target_tenths: i32) -> Stages {
    let mut stages = Stages { lna: 0, mixer: 0 };
    let mut total = 0;
    for _ in 0..ROUNDS {
        if total >= target_tenths {
            break;
        }
        stages.lna += 1;
        total += LNA_STEPS[usize::from(stages.lna)];
        if total >= target_tenths {
            break;
        }
        stages.mixer += 1;
        total += MIXER_STEPS[usize::from(stages.mixer)];
    }
    stages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_gain_matches_the_vectors() {
        for (target, lna, mixer, total) in [
            (0, 0, 0, 0),
            (9, 1, 0, 9),
            (14, 1, 1, 14),
            (27, 2, 1, 27),
            (37, 2, 2, 37),
            (77, 3, 2, 77),
            (87, 3, 3, 87),
            (125, 4, 3, 125),
            (144, 4, 4, 144),
            (157, 5, 4, 157),
            (166, 5, 5, 166),
            (197, 6, 5, 197),
            (207, 6, 6, 207),
            (229, 7, 6, 229),
            (254, 7, 7, 254),
            (280, 8, 7, 280),
            (297, 8, 8, 297),
            (328, 9, 8, 328),
            (338, 9, 9, 338),
            (364, 10, 9, 364),
            (372, 10, 10, 372),
            (386, 11, 10, 386),
            (402, 11, 11, 402),
            (421, 12, 11, 421),
            (434, 12, 12, 434),
            (439, 13, 12, 439),
            (445, 13, 13, 445),
            (480, 14, 13, 480),
            (496, 15, 14, 496),
            (-10, 0, 0, 0),
            (1, 1, 0, 9),
            (100, 4, 3, 125),
            (300, 9, 8, 328),
            (500, 15, 15, 488),
            (1000, 15, 15, 488),
        ] {
            let stages = stages_for(target);
            assert_eq!(stages, Stages { lna, mixer }, "{target}");
            assert_eq!(stages.tenths(), total, "{target}");
        }
    }

    #[test]
    fn every_offered_gain_reads_back_as_itself() {
        for &gain in GAINS {
            let stages = stages_for(gain);
            let status = stages.lna | (stages.mixer << 4);
            assert_eq!(Stages::from_status(status).tenths(), gain);
        }
    }

    #[test]
    fn a_zero_status_is_zero_gain() {
        assert_eq!(Stages::from_status(0).tenths(), 0);
    }
}
