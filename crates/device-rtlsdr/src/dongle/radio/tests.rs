use super::*;
use crate::dongle::{
    error::{Error, PllFault},
    fake::{Fake, Transfer},
};

fn open(fake: &Fake, board: Board) -> Radio<Fake> {
    Radio::start(fake.clone(), board).unwrap()
}

fn generic() -> (Radio<Fake>, Fake) {
    let fake = Fake::default();
    let radio = open(&fake, Board::Generic);
    fake.clear();
    (radio, fake)
}

fn if_writes(fake: &Fake) -> Vec<[u8; 3]> {
    let bytes: Vec<(u8, u8)> = fake
        .demod_writes()
        .into_iter()
        .filter(|(page, reg, _)| *page == 1 && (0x19..=0x1b).contains(reg))
        .map(|(_, reg, data)| (reg, data[0]))
        .collect();
    bytes
        .as_chunks::<3>()
        .0
        .iter()
        .map(|[a, b, c]| [a.1, b.1, c.1])
        .collect()
}

fn position(transfers: &[Transfer], wanted: &Transfer) -> usize {
    transfers
        .iter()
        .position(|transfer| transfer == wanted)
        .unwrap_or(usize::MAX)
}

#[test]
fn open_writes_sysctl_twice_before_anything_else() {
    let fake = Fake::default();
    let _radio = open(&fake, Board::Generic);
    let writes = fake.block_writes();
    assert_eq!(writes[0], (0x2000, vec![0x09]));
    assert_eq!(writes[1], (0x2000, vec![0x09]));
}

#[test]
fn open_brackets_the_tuner_with_the_repeater_and_reads_the_eeprom_outside_it() {
    let fake = Fake::default();
    let _radio = open(&fake, Board::Generic);
    let log = fake.transfers();
    let opened = position(&log, &Transfer::write(0x0120, 0x0011, &[0x18]));
    let probed = position(&log, &Transfer::write(0x0034, 0x0610, &[0x00]));
    let closed = log
        .iter()
        .rposition(|transfer| *transfer == Transfer::write(0x0120, 0x0011, &[0x10]))
        .unwrap_or(0);
    let eeprom = position(&log, &Transfer::write(0x00a0, 0x0610, &[0x00]));
    assert!(opened < probed && probed < closed && closed < eeprom);
    let reopened = log[closed..]
        .iter()
        .any(|transfer| *transfer == Transfer::write(0x0120, 0x0011, &[0x18]));
    assert!(!reopened);
}

#[test]
fn open_leaves_the_demod_on_the_low_if_path() {
    let fake = Fake::default();
    let radio = open(&fake, Board::Generic);
    let tail: Vec<_> = fake
        .demod_writes()
        .into_iter()
        .filter(|(page, reg, _)| !(*page == 1 && *reg == 0x01))
        .rev()
        .take(6)
        .collect();
    assert_eq!(
        tail,
        [
            (1, 0x15, vec![0x01]),
            (1, 0x1b, vec![0x12]),
            (1, 0x1a, vec![0x11]),
            (1, 0x19, vec![0x38]),
            (0, 0x08, vec![0x4d]),
            (1, 0xb1, vec![0x1a]),
        ]
    );
    assert_eq!(radio.tuner_kind(), TunerKind::R820T);
    assert!(!radio.bias_tee_at_start());
}

#[test]
fn an_unreadable_eeprom_does_not_fail_open() {
    let fake = Fake::default();
    fake.fail_reads(0x00a0, 0x0600);
    let radio = open(&fake, Board::Generic);
    assert!(!radio.bias_tee_at_start());
}

#[test]
fn a_programmed_eeprom_asks_for_the_bias_tee() {
    let fake = Fake::default();
    fake.set_eeprom(0, 0x28);
    fake.set_eeprom(1, 0x32);
    fake.set_eeprom(7, 0x00);
    assert!(open(&fake, Board::Generic).bias_tee_at_start());
}

#[test]
fn no_tuner_fails_open() {
    let fake = Fake::answering_at(None);
    assert!(matches!(
        Radio::start(fake, Board::Generic),
        Err(Error::NoTuner)
    ));
}

#[test]
fn a_sample_rate_sets_the_filter_if_and_resampler() {
    let (mut radio, fake) = generic();
    radio.set_sample_rate(2_400_000).unwrap();
    assert_eq!(radio.sample_rate(), 2_400_000);
    assert_eq!(if_writes(&fake), [[0x3b, 0xf7, 0x78]]);
    let demod: Vec<_> = fake
        .demod_writes()
        .into_iter()
        .filter(|(page, reg, _)| *page == 1 && [0x9f, 0xa1, 0x3f, 0x3e].contains(reg))
        .collect();
    assert_eq!(
        demod,
        [
            (1, 0x9f, vec![0x03, 0x00]),
            (1, 0xa1, vec![0x00, 0x00]),
            (1, 0x3f, vec![0x00]),
            (1, 0x3e, vec![0x00]),
        ]
    );
    let last: Vec<_> = fake.demod_writes().into_iter().rev().take(2).collect();
    assert_eq!(last, [(1, 0x01, vec![0x10]), (1, 0x01, vec![0x14])]);
}

#[test]
fn a_bad_sample_rate_touches_nothing() {
    let (mut radio, fake) = generic();
    assert!(matches!(
        radio.set_sample_rate(500_000),
        Err(Error::SampleRate(500_000))
    ));
    assert!(fake.transfers().is_empty());
}

#[test]
fn a_sample_rate_change_retunes_the_center() {
    let (mut radio, fake) = generic();
    radio.set_center(100_000_000).unwrap();
    fake.clear();
    radio.set_sample_rate(2_048_000).unwrap();
    assert_eq!(radio.center_hz(), Some(100_000_000));
    assert!(fake.tuner_writes().iter().any(|(reg, _)| *reg == 0x14));
    assert_eq!(if_writes(&fake), [[0x3c, 0x63, 0x8f], [0x3c, 0x63, 0x8f]]);
}

#[test]
fn tuning_writes_the_pll_then_the_if() {
    let (mut radio, fake) = generic();
    radio.set_center(100_000_000).unwrap();
    assert_eq!(radio.center_hz(), Some(100_000_000));
    assert_eq!(fake.tuner_value(0x14), Some(0x0b));
    assert_eq!(fake.tuner_value(0x16), Some(0x89));
    assert_eq!(fake.tuner_value(0x15), Some(0xf5));
    assert_eq!(if_writes(&fake), [[0x38, 0x11, 0x12]]);
}

#[test]
fn a_failed_tune_forgets_the_center() {
    let (mut radio, _fake) = generic();
    radio.set_center(100_000_000).unwrap();
    assert!(matches!(
        radio.set_center(10_000_000),
        Err(Error::Pll {
            fault: PllFault::NoDivider,
            ..
        })
    ));
    assert_eq!(radio.center_hz(), None);
}

#[test]
fn ppm_writes_the_correction_and_retunes_with_the_corrected_crystals() {
    let (mut radio, fake) = generic();
    radio.set_center(100_000_000).unwrap();
    let nominal = (fake.tuner_value(0x16), fake.tuner_value(0x15));
    fake.clear();
    radio.set_ppm(100).unwrap();
    let first: Vec<_> = fake.demod_writes().into_iter().take(2).collect();
    assert_eq!(first, [(1, 0x3f, vec![0x73]), (1, 0x3e, vec![0x39])]);
    assert_ne!((fake.tuner_value(0x16), fake.tuner_value(0x15)), nominal);
    let word = demod::if_word(3_570_000, 28_802_880).unwrap();
    assert_eq!(if_writes(&fake), [demod::if_bytes(word)]);
    assert_eq!(radio.ppm(), 100);
}

#[test]
fn ppm_out_of_range_is_refused_before_any_write() {
    let (mut radio, fake) = generic();
    assert!(radio.set_ppm(489).is_err());
    assert!(fake.transfers().is_empty());
    assert_eq!(radio.ppm(), 0);
}

#[test]
fn a_bandwidth_change_moves_the_if_and_retunes() {
    let (mut radio, fake) = generic();
    radio.set_sample_rate(2_400_000).unwrap();
    radio.set_center(100_000_000).unwrap();
    fake.clear();
    assert_eq!(radio.set_bandwidth(6_000_000).unwrap(), 3_570_000);
    assert_eq!(if_writes(&fake), [[0x38, 0x11, 0x12], [0x38, 0x11, 0x12]]);
    assert_eq!(fake.tuner_value(0x14), Some(0x0b));
    fake.clear();
    assert_eq!(radio.set_bandwidth(0).unwrap(), 1_815_000);
}

#[test]
fn direct_sampling_bypasses_the_tuner() {
    let (mut radio, fake) = generic();
    radio.set_center(100_000_000).unwrap();
    fake.clear();
    radio.set_direct_sampling(DirectSampling::Q).unwrap();
    assert_eq!(
        radio.center_hz(),
        None,
        "the old center belongs to the tuner"
    );
    assert_eq!(fake.tuner_writes().first(), Some(&(0x06, 0xb1)));
    let demod: Vec<_> = fake
        .demod_writes()
        .into_iter()
        .filter(|(page, reg, _)| !(*page == 1 && *reg == 0x01))
        .collect();
    assert_eq!(
        demod,
        [
            (1, 0xb1, vec![0x1a]),
            (1, 0x15, vec![0x00]),
            (0, 0x08, vec![0x4d]),
            (0, 0x06, vec![0x90]),
        ]
    );
    fake.clear();
    radio.set_center(7_100_000).unwrap();
    assert_eq!(if_writes(&fake), [[0x30, 0x38, 0xe4]]);
    assert!(fake.tuner_writes().is_empty());
    assert!(matches!(
        radio.set_center(14_400_001),
        Err(Error::Invalid(Invalid::DirectCenter(14_400_001)))
    ));
}

#[test]
fn the_i_branch_selects_the_other_adc() {
    let (mut radio, fake) = generic();
    radio.set_direct_sampling(DirectSampling::I).unwrap();
    assert_eq!(fake.demod_writes().last(), Some(&(0, 0x06, vec![0x80])));
}

#[test]
fn a_bypassed_tuner_refuses_gain_and_bandwidth() {
    let (mut radio, _fake) = generic();
    radio.set_direct_sampling(DirectSampling::Q).unwrap();
    let bypassed =
        |outcome: Result<()>| matches!(outcome, Err(Error::Invalid(Invalid::TunerBypassed)));
    assert!(bypassed(radio.set_auto_gain()));
    assert!(bypassed(radio.set_manual_gain(297)));
    assert!(bypassed(radio.measured_gain().map(|_| ())));
    assert!(bypassed(radio.set_bandwidth(1_000_000).map(|_| ())));
}

#[test]
fn a_rate_change_in_direct_sampling_leaves_the_if_alone() {
    let (mut radio, fake) = generic();
    radio.set_direct_sampling(DirectSampling::Q).unwrap();
    radio.set_center(7_100_000).unwrap();
    fake.clear();
    radio.set_sample_rate(2_400_000).unwrap();
    assert!(if_writes(&fake).is_empty());
    assert!(fake.tuner_writes().is_empty());
}

#[test]
fn leaving_direct_sampling_reinits_the_tuner() {
    let (mut radio, fake) = generic();
    radio.set_direct_sampling(DirectSampling::Q).unwrap();
    radio.set_center(7_100_000).unwrap();
    fake.clear();
    radio.set_direct_sampling(DirectSampling::Off).unwrap();
    assert_eq!(radio.center_hz(), None);
    assert_eq!(
        fake.i2c_writes(0x34).first(),
        Some(&vec![0x05, 0x83, 0x32, 0x75, 0xc0, 0x40, 0xd6, 0x6c])
    );
    let demod: Vec<_> = fake
        .demod_writes()
        .into_iter()
        .filter(|(page, reg, _)| !(*page == 1 && *reg == 0x01))
        .collect();
    assert_eq!(
        demod,
        [
            (1, 0xb1, vec![0x1a]),
            (0, 0x08, vec![0x4d]),
            (1, 0x19, vec![0x38]),
            (1, 0x1a, vec![0x11]),
            (1, 0x1b, vec![0x12]),
            (1, 0x15, vec![0x01]),
            (0, 0x06, vec![0x80]),
        ]
    );
}

#[test]
fn the_v4_family_has_no_direct_sampling() {
    for board in [Board::BlogV4, Board::BlogV4Lite] {
        let fake = Fake::default();
        let mut radio = open(&fake, board);
        assert!(matches!(
            radio.set_direct_sampling(DirectSampling::Q),
            Err(Error::Invalid(Invalid::NoDirectSampling))
        ));
        assert!(radio.set_direct_sampling(DirectSampling::Off).is_ok());
    }
}

#[test]
fn a_v4_r828d_runs_on_28_8_mhz() {
    let fake = Fake::answering_at(Some(0x74));
    let mut radio = open(&fake, Board::BlogV4);
    radio.set_center(100_000_000).unwrap();
    let lo = 103_570_000u64;
    assert_eq!(fake.tuner_value(0x16), Some(0x89), "{lo}");
    assert_eq!(fake.tuner_value(0x15), Some(0xf5), "{lo}");
}

#[test]
fn a_generic_r828d_runs_on_16_mhz() {
    let fake = Fake::answering_at(Some(0x74));
    let mut radio = open(&fake, Board::Generic);
    radio.set_center(100_000_000).unwrap();
    assert_eq!(fake.tuner_value(0x16), Some(0x91));
    assert_eq!(fake.tuner_value(0x15), Some(0xec));
}

#[test]
fn dither_off_retunes_with_the_modulator_undithered() {
    let (mut radio, fake) = generic();
    radio.set_center(100_000_000).unwrap();
    radio.set_dither(false).unwrap();
    assert_eq!(fake.tuner_value(0x12).map(|value| value & 0x10), Some(0x10));
}

#[test]
fn gain_goes_through_the_repeater() {
    let (mut radio, fake) = generic();
    radio.set_manual_gain(297).unwrap();
    assert_eq!(fake.tuner_value(0x05).map(|value| value & 0x1f), Some(0x18));
    fake.set_status(3, 0x88);
    assert_eq!(radio.measured_gain().unwrap(), 297);
    radio.set_auto_gain().unwrap();
    assert_eq!(fake.tuner_value(0x05).map(|value| value & 0x10), Some(0x00));
}

#[test]
fn the_bias_tee_is_pin_zero() {
    let (radio, fake) = generic();
    radio.set_bias_tee(true).unwrap();
    assert_eq!(fake.block_writes().last(), Some(&(0x3001, vec![0x01])));
    radio.set_pin(3, true).unwrap();
    assert_eq!(fake.block_writes().last(), Some(&(0x3001, vec![0x09])));
}

#[test]
fn dropping_the_radio_stops_sampling() {
    let (radio, fake) = generic();
    drop(radio);
    assert_eq!(fake.block_writes(), [(0x2148, vec![0x10, 0x02])]);
}

#[test]
fn direct_sampling_modes_spell_exactly() {
    for mode in DirectSampling::MODES {
        assert_eq!(DirectSampling::from_wire(mode.wire_name()), Some(mode));
    }
    for text in ["", "1", "Q", "on", "off "] {
        assert_eq!(DirectSampling::from_wire(text), None, "{text:?}");
    }
    assert_eq!(DirectSampling::default(), DirectSampling::Off);
}

#[test]
fn a_stall_on_the_repeater_names_the_register() {
    let (mut radio, fake) = generic();
    fake.fail_writes(0x0120, 0x0011);
    let error = radio.set_center(100_000_000).unwrap_err();
    assert_eq!(
        error.to_string(),
        "control transfer failed on demod write 1:0x01: endpoint stalled"
    );
    fake.heal();
    assert!(radio.set_center(100_000_000).is_ok());
}
