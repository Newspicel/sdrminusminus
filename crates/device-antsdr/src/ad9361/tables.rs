#[cfg(test)]
const GAIN_STEPS: usize = 77;
pub(super) const GAIN_SLOTS: usize = 91;
const STEP_CHANGES_STAGE: u8 = 0x20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Band {
    Low,
    Mid,
    High,
}

impl Band {
    pub(super) fn of(rx_hz: f64) -> Self {
        if rx_hz < 1300e6 {
            Self::Low
        } else if rx_hz < 4e9 {
            Self::Mid
        } else {
            Self::High
        }
    }
}

#[derive(Clone, Copy)]
struct Run {
    lna: u8,
    mixer: u8,
    tia: u8,
    lpf: u8,
    steps: u8,
    held: u8,
}

const fn run(lna: u8, mixer: u8, tia: u8, lpf: u8, steps: u8) -> Run {
    Run {
        lna,
        mixer,
        tia,
        lpf,
        steps,
        held: 0,
    }
}

const fn held(run: Run, held: u8) -> Run {
    Run { held, ..run }
}

const MIXER_RAMP: [Run; 11] = {
    let mut ramp = [run(3, 5, 1, 24, 1); 11];
    let mut step = 0;
    while step < ramp.len() {
        ramp[step].mixer = 5 + step as u8;
        step += 1;
    }
    ramp
};

const LOW: [Run; 8] = [
    held(run(0, 0, 0, 0, 6), 2),
    run(0, 1, 0, 3, 12),
    run(0, 2, 0, 9, 8),
    run(0, 2, 1, 11, 2),
    run(0, 4, 1, 8, 4),
    run(1, 4, 1, 0, 2),
    run(2, 4, 1, 0, 19),
    run(3, 4, 1, 14, 11),
];

const MID: [Run; 8] = [
    held(run(0, 0, 0, 0, 6), 2),
    run(0, 1, 0, 3, 12),
    run(0, 2, 0, 9, 8),
    run(0, 2, 1, 11, 2),
    run(0, 4, 1, 7, 5),
    run(1, 4, 1, 1, 2),
    run(2, 4, 1, 0, 18),
    run(3, 4, 1, 14, 11),
];

const HIGH: [Run; 10] = [
    held(run(0, 0, 0, 0, 4), 4),
    run(0, 1, 0, 1, 3),
    run(0, 1, 0, 4, 9),
    run(0, 2, 0, 8, 3),
    run(0, 2, 0, 11, 5),
    run(0, 2, 1, 10, 2),
    run(0, 4, 1, 7, 7),
    run(1, 4, 1, 0, 3),
    run(2, 4, 1, 0, 15),
    run(3, 4, 1, 14, 11),
];

pub(super) fn gain_table(band: Band) -> Vec<[u8; 3]> {
    let runs: &[Run] = match band {
        Band::Low => &LOW,
        Band::Mid => &MID,
        Band::High => &HIGH,
    };
    let mut table = Vec::with_capacity(GAIN_SLOTS);
    for run in runs.iter().chain(&MIXER_RAMP) {
        let front = run.lna << 5 | run.mixer;
        let back = |lpf: u8| run.tia << 5 | lpf;
        table.push([front, back(run.lpf), STEP_CHANGES_STAGE]);
        for _ in 0..run.held {
            table.push([front, back(run.lpf), 0]);
        }
        for step in 1..run.steps {
            table.push([front, back(run.lpf + step), 0]);
        }
    }
    table.resize(GAIN_SLOTS, [0; 3]);
    table
}

pub(super) const MIXER_GM: [(u8, u8); 16] = [
    (0x78, 0x00),
    (0x74, 0x0d),
    (0x70, 0x15),
    (0x6c, 0x1b),
    (0x68, 0x21),
    (0x64, 0x25),
    (0x60, 0x29),
    (0x5c, 0x2c),
    (0x58, 0x2f),
    (0x54, 0x31),
    (0x50, 0x33),
    (0x4c, 0x34),
    (0x48, 0x35),
    (0x30, 0x3a),
    (0x18, 0x3d),
    (0x00, 0x3e),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_band_fills_the_steps_the_gain_index_reaches() {
        for band in [Band::Low, Band::Mid, Band::High] {
            let table = gain_table(band);
            assert_eq!(table.len(), GAIN_SLOTS);
            assert_ne!(table[GAIN_STEPS - 1], [0; 3], "{band:?}");
            assert!(table[GAIN_STEPS..].iter().all(|row| *row == [0; 3]));
        }
    }

    #[test]
    fn the_top_of_every_table_ramps_the_mixer() {
        for band in [Band::Low, Band::Mid, Band::High] {
            let table = gain_table(band);
            assert_eq!(table[65], [0x64, 0x38, 0x00]);
            assert_eq!(table[66], [0x65, 0x38, 0x20]);
            assert_eq!(table[76], [0x6f, 0x38, 0x20]);
        }
    }

    #[test]
    fn a_stage_change_is_flagged_where_the_front_end_switches() {
        let low = gain_table(Band::Low);
        assert_eq!(low[0], [0x00, 0x00, 0x20]);
        assert_eq!(low[2], [0x00, 0x00, 0x00]);
        assert_eq!(low[3], [0x00, 0x01, 0x00]);
        assert_eq!(low[8], [0x01, 0x03, 0x20]);
        assert_eq!(low[34], [0x24, 0x20, 0x20]);
        assert_eq!(low[54], [0x44, 0x32, 0x00]);
        let high = gain_table(Band::High);
        assert_eq!(high[4], [0x00, 0x00, 0x00]);
        assert_eq!(high[5], [0x00, 0x01, 0x00]);
        assert_eq!(high[11], [0x01, 0x04, 0x20]);
        assert_eq!(high[23], [0x02, 0x0b, 0x20]);
        assert_eq!(high[37], [0x24, 0x20, 0x20]);
    }

    #[test]
    fn bands_split_where_the_front_end_changes() {
        assert_eq!(Band::of(100e6), Band::Low);
        assert_eq!(Band::of(2.4e9), Band::Mid);
        assert_eq!(Band::of(5.8e9), Band::High);
    }
}
