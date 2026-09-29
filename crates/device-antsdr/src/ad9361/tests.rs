use super::{testing::FakeChip, *};

fn settings() -> Settings {
    Settings {
        rate: 32e6,
        rx_hz: 100e6,
        tx_hz: 100e6,
        bandwidth: MAX_BANDWIDTH,
    }
}

fn ready() -> Ad9361<FakeChip> {
    let mut chip = Ad9361::new(FakeChip::default(), REFERENCE_HZ);
    chip.initialize(settings()).expect("initialized");
    chip.bus().writes.clear();
    chip
}

#[test]
fn initialization_leaves_the_transceiver_streaming_in_duplex() {
    let mut chip = Ad9361::new(FakeChip::default(), REFERENCE_HZ);
    chip.initialize(settings()).expect("initialized");
    assert!((chip.rate() - 32e6).abs() < 1.0);
    assert!((chip.frequency(Direction::Rx) - 100e6).abs() < 1.0);
    let bus = chip.bus();
    assert_eq!(bus.state, 0x0a);
    assert_eq!(bus.writes_to(reg::GAIN_TABLE_ADDRESS), tables::GAIN_SLOTS);
    assert!(bus.wrote(reg::CALIBRATION, 0x02), "RF DC was calibrated");
    assert!(bus.wrote(reg::CALIBRATION, 0x10), "transmit quadrature ran");
    assert_eq!(bus.last(reg::RX_QUAD_TRACKING), Some(0xcf));
    assert_eq!(bus.last(reg::DC_TRACKING), Some(0xad));
    assert_eq!(bus.last(reg::RX_FILTERS).map(|v| v & 0xc0), Some(0x40));
}

#[test]
fn a_part_that_is_not_an_ad9361_is_refused() {
    let mut chip = Ad9361::new(
        FakeChip {
            product_id: 0x42,
            ..FakeChip::default()
        },
        REFERENCE_HZ,
    );
    assert!(chip.initialize(settings()).is_err());
}

#[test]
fn a_synthesizer_that_will_not_lock_is_an_error() {
    let mut chip = Ad9361::new(
        FakeChip {
            synthesizers_lock: false,
            ..FakeChip::default()
        },
        REFERENCE_HZ,
    );
    let error = chip.initialize(settings()).expect_err("unlocked");
    assert!(error.to_string().contains("lock"), "{error}");
}

#[test]
fn a_rate_change_recalibrates_and_returns_to_duplex() {
    let mut chip = ready();
    let rate = chip.set_rate(7.68e6).expect("rate");
    assert!((rate - 7.68e6).abs() < 1.0);
    let bus = chip.bus();
    assert!(bus.wrote(reg::ENSM_CONFIG_1, 0x00), "parked in wait");
    assert_eq!(bus.last(reg::ENSM_CONFIG_1), Some(0x21));
    assert!(bus.writes_to(reg::BBPLL_INTEGER) > 0);
    assert_eq!(bus.last(reg::RX_FILTERS).map(|v| v & 0xc0), Some(0x40));
}

#[test]
fn asking_for_the_same_rate_again_touches_nothing() {
    let mut chip = ready();
    chip.set_rate(32e6).expect("same rate");
    assert!(chip.bus().writes.is_empty());
}

#[test]
fn a_rate_outside_the_converter_is_refused() {
    let mut chip = ready();
    assert!(chip.set_rate(80e6).is_err());
    assert!(chip.set_rate(100e3).is_err());
}

#[test]
fn a_far_retune_recalibrates_and_a_near_one_does_not() {
    let mut chip = ready();
    chip.tune(Direction::Rx, 150e6).expect("near");
    assert!(!chip.bus().wrote(reg::CALIBRATION, 0x02));
    chip.tune(Direction::Rx, 2.4e9).expect("far");
    let bus = chip.bus();
    assert!(bus.wrote(reg::CALIBRATION, 0x02));
    assert_eq!(bus.writes_to(reg::GAIN_TABLE_ADDRESS), tables::GAIN_SLOTS);
    assert_eq!(bus.last(reg::ENSM_CONFIG_1), Some(0x21));
    assert!((chip.frequency(Direction::Rx) - 2.4e9).abs() < 1.0);
}

#[test]
fn a_frequency_outside_the_part_is_refused() {
    let mut chip = ready();
    assert!(chip.tune(Direction::Rx, 10e6).is_err());
    assert!(chip.tune(Direction::Tx, 7e9).is_err());
}

#[test]
fn transmit_attenuation_is_written_in_quarter_decibels() {
    let mut chip = ready();
    assert_eq!(chip.set_tx_attenuation(1, 10.3).expect("set"), 10.25);
    let bus = chip.bus();
    assert_eq!(bus.last(reg::TX2_ATTENUATION_LOW), Some(41));
    assert_eq!(bus.last(reg::TX2_ATTENUATION_HIGH), Some(0));
    assert_eq!(chip.set_tx_attenuation(0, 200.0).expect("clamped"), 89.75);
    assert_eq!(chip.bus().last(reg::TX1_ATTENUATION_HIGH), Some(1));
}

#[test]
fn receive_gain_is_a_whole_gain_table_index() {
    let mut chip = ready();
    assert_eq!(chip.set_rx_gain(0, 40.7).expect("set"), 40.0);
    assert_eq!(chip.bus().last(reg::RX1_MANUAL_GAIN), Some(40));
    assert_eq!(chip.set_rx_gain(1, 99.0).expect("clamped"), MAX_RX_GAIN);
}

#[test]
fn switching_to_agc_loads_its_thresholds_and_back_restores_the_gain() {
    let mut chip = ready();
    chip.set_rx_gain(0, 33.0).expect("gain");
    chip.set_gain_mode(0, GainMode::SlowAttack).expect("agc");
    assert_eq!(chip.bus().last(reg::GAIN_MODE), Some(0xe2));
    assert!(chip.bus().wrote(0x12a, 0x22));
    chip.set_gain_mode(0, GainMode::Manual).expect("manual");
    assert_eq!(chip.bus().last(reg::GAIN_MODE), Some(0xe0));
    assert_eq!(chip.bus().last(reg::RX1_MANUAL_GAIN), Some(33));
    assert_eq!(chip.gain_index(0).expect("readback"), 33.0);
}

#[test]
fn enabling_a_transmitter_calibrates_it_and_keeps_tracking_on() {
    let mut chip = ready();
    chip.set_chains(Chains {
        rx: [true, true],
        tx: [true, true],
    })
    .expect("chains");
    let bus = chip.bus();
    assert!(bus.wrote(reg::CALIBRATION, 0x10));
    assert_eq!(bus.last(reg::RX_FILTERS).map(|v| v & 0xc0), Some(0xc0));
    assert_eq!(bus.last(reg::RX_QUAD_TRACKING), Some(0xcf));
    assert_eq!(bus.last(reg::ENSM_CONFIG_1), Some(0x21));
}

#[test]
fn a_crystal_correction_retunes_both_synthesizers() {
    let mut chip = ready();
    chip.set_reference(REFERENCE_HZ * (1.0 + 2e-6))
        .expect("trim");
    let bus = chip.bus();
    assert!(bus.writes_to(0x233) > 0 && bus.writes_to(0x273) > 0);
}
