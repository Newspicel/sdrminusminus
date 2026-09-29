use super::*;

struct Script {
    messages: Vec<Vec<u8>>,
    pins: Vec<(u8, bool)>,
    status: [u8; 5],
    unlocked: usize,
    pauses: usize,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            pins: Vec::new(),
            status: [0x96, 0x00, 0x40, 0x00, 0x25],
            unlocked: 0,
            pauses: 0,
        }
    }
}

impl TunerBus for Script {
    fn write(&mut self, data: &[u8]) -> Result<()> {
        self.messages.push(data.to_vec());
        Ok(())
    }

    fn read(&mut self, len: usize) -> Result<Vec<u8>> {
        let mut status = self.status;
        if len == 3 && self.unlocked > 0 {
            self.unlocked -= 1;
            status[2] &= !PLL_LOCKED;
        }
        Ok(status
            .iter()
            .take(len)
            .map(|byte| byte.reverse_bits())
            .collect())
    }

    fn drive_pin(&mut self, pin: u8, high: bool) -> Result<()> {
        self.pins.push((pin, high));
        Ok(())
    }

    fn pause(&mut self, _duration: Duration) {
        self.pauses += 1;
    }
}

impl Script {
    fn writes(&self) -> Vec<(u8, u8)> {
        self.messages
            .iter()
            .filter(|message| message.len() == 2)
            .map(|message| (message[0], message[1]))
            .collect()
    }

    fn last(&self, reg: u8) -> Option<u8> {
        self.writes()
            .into_iter()
            .rev()
            .find(|(written, _)| *written == reg)
            .map(|(_, value)| value)
    }

    fn clear(&mut self) {
        self.messages.clear();
        self.pins.clear();
    }
}

fn assert_masked(writes: &[(u8, u8)], expected: &[Write]) {
    assert_eq!(writes.len(), expected.len(), "{writes:02x?}");
    for (at, (&(reg, byte), &(want_reg, value, mask))) in writes.iter().zip(expected).enumerate() {
        assert_eq!(reg, want_reg, "write {at}");
        assert_eq!(byte & mask, value & mask, "write {at} to 0x{reg:02x}");
    }
}

fn tuner(kind: TunerKind, board: Board) -> (Tuner, Script) {
    let mut tuner = Tuner::new(kind, board);
    let mut bus = Script::default();
    tuner.init(&mut bus).unwrap();
    bus.clear();
    (tuner, bus)
}

#[test]
fn init_sends_the_table_in_four_messages() {
    let mut bus = Script::default();
    Tuner::new(TunerKind::R820T, Board::Generic)
        .init(&mut bus)
        .unwrap();
    assert_eq!(
        bus.messages[..4],
        [
            vec![0x05, 0x83, 0x32, 0x75, 0xc0, 0x40, 0xd6, 0x6c],
            vec![0x0c, 0xf5, 0x63, 0x75, 0x68, 0x6c, 0x83, 0x80],
            vec![0x13, 0x00, 0x0f, 0x00, 0xc0, 0x30, 0x48, 0xcc],
            vec![0x1a, 0x60, 0x00, 0x54, 0xae, 0x4a, 0xc0],
        ]
    );
}

#[test]
fn init_ends_with_calibration_code_and_the_system_setup() {
    let mut bus = Script::default();
    Tuner::new(TunerKind::R820T, Board::Generic)
        .init(&mut bus)
        .unwrap();
    let writes = bus.writes();
    assert_masked(&writes[..3], &SETUP);
    let mut tail = vec![(0x0f, 0x00, 0x04), (0x0a, 0x15, 0x1f)];
    tail.extend(AFTER_CALIBRATION);
    tail.extend(SYSTEM);
    assert_masked(&writes[writes.len() - tail.len()..], &tail);
    assert_eq!(bus.pauses, 1);
}

#[test]
fn a_calibration_pll_that_never_locks_falls_back_to_code_zero() {
    let mut bus = Script {
        unlocked: usize::MAX,
        ..Script::default()
    };
    let mut tuner = Tuner::new(TunerKind::R820T, Board::Generic);
    tuner.init(&mut bus).unwrap();
    let starts = bus
        .writes()
        .iter()
        .filter(|(reg, value)| *reg == 0x0b && value & 0x10 != 0)
        .count();
    assert_eq!(starts, 0, "calibration never started");
    let writes = bus.writes();
    let tail = SYSTEM.len() + AFTER_CALIBRATION.len() + 1;
    assert_eq!(writes[writes.len() - tail].0, 0x0a);
    assert_eq!(writes[writes.len() - tail].1 & 0x1f, 0x10);
    assert_eq!(tuner.if_hz(), 3_570_000);
}

#[test]
fn a_calibration_code_of_0x0f_is_tried_twice_then_dropped() {
    let mut bus = Script::default();
    bus.status[4] = 0x2f;
    Tuner::new(TunerKind::R820T, Board::Generic)
        .init(&mut bus)
        .unwrap();
    assert_eq!(bus.pauses, 2);
    let writes = bus.writes();
    let at = writes.len() - SYSTEM.len() - AFTER_CALIBRATION.len() - 1;
    assert_eq!(writes[at].0, 0x0a);
    assert_eq!(writes[at].1 & 0x1f, 0x10);
}

#[test]
fn tuning_100_mhz_programs_the_documented_pll() {
    let (mut tuner, mut bus) = tuner(TunerKind::R820T, Board::Generic);
    tuner.tune(&mut bus, 100_000_000).unwrap();
    let writes = bus.writes();
    assert_masked(
        &writes[..6],
        &[
            (0x17, 0x00, 0x08),
            (0x1a, 0x02, 0xc3),
            (0x1b, 0x34, 0xff),
            (0x10, 0x00, 0x0b),
            (0x08, 0x00, 0x3f),
            (0x09, 0x00, 0x3f),
        ],
    );
    assert_masked(
        &writes[6..],
        &[
            (0x10, 0x00, 0x10),
            (0x1a, 0x00, 0x0c),
            (0x12, 0x80, 0xe0),
            (0x10, 4 << 5, 0xe0),
            (0x14, 0x0b, 0xff),
            (0x12, 0x00, 0x18),
            (0x16, 0x89, 0xff),
            (0x15, 0xf5, 0xff),
            (0x1a, 0x08, 0x08),
        ],
    );
    assert!(bus.pins.is_empty());
}

#[test]
fn an_unlocked_pll_raises_the_vco_current_then_fails() {
    let (mut tuner, mut bus) = tuner(TunerKind::R820T, Board::Generic);
    bus.unlocked = 1;
    tuner.tune(&mut bus, 100_000_000).unwrap();
    let boosts: Vec<u8> = bus
        .writes()
        .into_iter()
        .filter(|(reg, _)| *reg == 0x12)
        .map(|(_, value)| value & 0xe0)
        .collect();
    assert_eq!(boosts, [0x80, 0x80, 0x60]);

    bus.clear();
    bus.unlocked = 3;
    let error = tuner.tune(&mut bus, 100_000_000).unwrap_err();
    assert!(matches!(
        error,
        Error::Pll {
            lo_hz: 103_570_000,
            fault: PllFault::NoLock
        }
    ));
    let boosts: Vec<u8> = bus
        .writes()
        .into_iter()
        .filter(|(reg, _)| *reg == 0x12)
        .map(|(_, value)| value & 0xe0)
        .collect();
    assert_eq!(boosts, [0x80, 0x80, 0x60, 0x00]);
    assert_eq!(bus.writes().last().map(|(reg, _)| *reg), Some(0x1a));
}

#[test]
fn below_the_vco_range_the_tune_fails() {
    let (mut tuner, mut bus) = tuner(TunerKind::R820T, Board::Generic);
    assert!(matches!(
        tuner.tune(&mut bus, 24_000_000),
        Err(Error::Pll {
            fault: PllFault::NoDivider,
            ..
        })
    ));
    assert!(tuner.tune(&mut bus, 24_100_000).is_ok());
}

#[test]
fn fine_tune_above_the_reference_lowers_the_divider_code() {
    let (mut tuner, mut bus) = tuner(TunerKind::R828D, Board::Generic);
    tuner.tune(&mut bus, 100_000_000).unwrap();
    let codes: Vec<u8> = bus
        .writes()
        .into_iter()
        .filter(|(reg, _)| *reg == 0x10)
        .map(|(_, value)| value >> 5)
        .collect();
    assert_eq!(codes.last(), Some(&3));
    assert_eq!(bus.last(0x14), Some(0x96));
    assert_eq!(bus.last(0x16), Some(0x91));
    assert_eq!(bus.last(0x15), Some(0xec));
}

#[test]
fn dither_off_stops_the_modulator_dither() {
    let (mut tuner, mut bus) = tuner(TunerKind::R820T, Board::Generic);
    tuner.set_dither(false);
    tuner.tune(&mut bus, 100_000_000).unwrap();
    assert_eq!(bus.last(0x12).map(|value| value & 0x18), Some(0x10));
}

#[test]
fn the_ppm_corrected_reference_moves_the_pll() {
    let (mut tuner, mut bus) = tuner(TunerKind::R820T, Board::Generic);
    tuner.tune(&mut bus, 100_000_000).unwrap();
    let nominal = (bus.last(0x16), bus.last(0x15));
    tuner.set_reference(28_805_760);
    tuner.tune(&mut bus, 100_000_000).unwrap();
    assert_ne!((bus.last(0x16), bus.last(0x15)), nominal);
}

#[test]
fn a_v4_tunes_hf_through_the_upconverter() {
    let (mut tuner, mut bus) = tuner(TunerKind::R828D, Board::BlogV4);
    tuner.tune(&mut bus, 7_100_000).unwrap();
    assert_eq!(bus.pins, [(5, false)]);
    assert_eq!(bus.last(0x1b), Some(0x00));
    assert_eq!(bus.last(0x05).map(|value| value & 0x60), Some(0x20));
    bus.clear();
    tuner.tune(&mut bus, 7_200_000).unwrap();
    assert!(bus.pins.is_empty(), "the band did not change");
    tuner.standby(&mut bus).unwrap();
    tuner.tune(&mut bus, 7_200_000).unwrap();
    assert_eq!(bus.pins, [(5, false)], "standby forgets the band");
}

#[test]
fn the_v4_notch_overrides_the_mux_open_drain() {
    let (mut tuner, mut bus) = tuner(TunerKind::R828D, Board::BlogV4);
    tuner.tune(&mut bus, 433_920_000).unwrap();
    let drains: Vec<u8> = bus
        .writes()
        .into_iter()
        .filter(|(reg, _)| *reg == 0x17)
        .map(|(_, value)| value & 0x08)
        .collect();
    assert_eq!(drains, [0x00, 0x08]);
}

#[test]
fn bandwidth_moves_the_if() {
    let (mut tuner, mut bus) = tuner(TunerKind::R820T, Board::Generic);
    assert_eq!(tuner.set_bandwidth(&mut bus, 2_400_000).unwrap(), 1_815_000);
    assert_masked(&bus.writes(), &[(0x0a, 0x00, 0x10), (0x0b, 0x8f, 0xef)]);
    assert_eq!(tuner.if_hz(), 1_815_000);
    bus.clear();
    tuner.tune(&mut bus, 100_000_000).unwrap();
    let lo = 101_815_000u64;
    let div = pll::divider(lo).unwrap();
    let synth = pll::synth(lo, div, 28_800_000, 2).unwrap();
    assert_eq!(bus.last(0x14), Some(pll::nint_reg(synth.nint)));
}

#[test]
fn standby_writes_full_values_in_order() {
    let (mut tuner, mut bus) = tuner(TunerKind::R820T, Board::Generic);
    tuner.standby(&mut bus).unwrap();
    assert_eq!(bus.writes(), STANDBY);
}

#[test]
fn manual_gain_picks_the_stage_indices() {
    let (mut tuner, mut bus) = tuner(TunerKind::R820T, Board::Generic);
    tuner.manual_gain(&mut bus, 297).unwrap();
    let mut expected = MANUAL_GAIN.to_vec();
    expected.extend([(0x05, 8, 0x0f), (0x07, 8, 0x0f)]);
    assert_masked(&bus.writes(), &expected);
}

#[test]
fn auto_gain_hands_both_stages_to_the_agc() {
    let (mut tuner, mut bus) = tuner(TunerKind::R820T, Board::Generic);
    tuner.auto_gain(&mut bus).unwrap();
    assert_masked(&bus.writes(), &AUTO_GAIN);
}

#[test]
fn gain_reads_back_from_the_status() {
    let (tuner, mut bus) = tuner(TunerKind::R820T, Board::Generic);
    bus.status[3] = 0x88;
    assert_eq!(tuner.gain(&mut bus).unwrap(), 297);
    assert_eq!(bus.messages.last(), Some(&vec![0x00]));
}

#[test]
fn a_masked_write_keeps_the_shadowed_bits() {
    let (mut tuner, mut bus) = tuner(TunerKind::R820T, Board::Generic);
    let before = tuner.shadow[usize::from(0x0c - FIRST_REG)];
    tuner.masked(&mut bus, 0x0c, 0x00, 0x0f).unwrap();
    assert_eq!(bus.writes(), [(0x0c, before & 0xf0)]);
    assert!(matches!(
        tuner.masked(&mut bus, 0x04, 0x00, 0xff),
        Err(Error::Invalid(Invalid::TunerRegister(0x04)))
    ));
}
