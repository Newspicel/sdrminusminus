use num_complex::Complex;
use sdrmm_wire::{
    ChannelParams, DecoderEvent, DectArc, DectBand, DectCapability, DectCipherState, DectParams,
    DectSide, DectSides,
};

use super::{
    DESCRIPTOR, DectChannel,
    burst::INPUT_RATE_HZ,
    identity::Rfpi,
    mac::{self, Tail, a_field_crc_ok, append_r_crc},
};
use crate::{
    ChannelCtx, ChannelRx,
    testgen::dect as sig,
    testutil::{run_events, settings},
};

const CLASS_A_RFPI: u64 = 0x0001_234D_5E6D & ((1 << 40) - 1);

fn channel(params: DectParams) -> DectChannel {
    DectChannel::new(
        ChannelCtx {
            input_rate: INPUT_RATE_HZ,
        },
        settings(ChannelParams::Dect(params)),
    )
    .expect("dect channel")
}

fn frames(events: &[DecoderEvent]) -> Vec<sdrmm_wire::DectFrame> {
    events
        .iter()
        .filter_map(|event| match event {
            DecoderEvent::Dect(frame) => Some(frame.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn descriptor_runs_at_two_samples_per_dect_bit() {
    assert_eq!(DESCRIPTOR.input_rate_hz, 2_304_000.0);
    assert_eq!(DESCRIPTOR.bandwidth_hz, 1_728_000.0);
    assert!(!DESCRIPTOR.has_audio);
}

#[test]
fn carrier_numbers_map_onto_the_published_dect_band_plan() {
    assert_eq!(DectBand::Eu.carriers(), 10);
    assert_eq!(DectBand::Eu.carrier_hz(0), Some(1_897_344_000.0));
    assert_eq!(DectBand::Eu.carrier_hz(9), Some(1_881_792_000.0));
    assert_eq!(DectBand::Eu.carrier_hz(10), None);
    assert_eq!(DectBand::Us.carriers(), 5);
    assert_eq!(DectBand::Us.carrier_hz(0), Some(1_921_536_000.0));
    assert_eq!(DectBand::Us.carrier_hz(4), Some(1_928_448_000.0));
}

#[test]
fn a_class_a_rfpi_splits_into_emc_fpn_and_rpn() {
    let identity = Rfpi::parse(CLASS_A_RFPI).describe();
    assert_eq!(identity.arc, DectArc::A);
    assert_eq!(identity.rfpi, "01234D5E6D");
    assert_eq!(identity.pari, "02469ABCD");
    assert_eq!(identity.emc, Some(0x1234));
    assert_eq!(identity.fpn, Some(0x1ABCD));
    assert_eq!(identity.rpn, 5);
    assert!(!identity.sari_available);
    assert_eq!(identity.multicell, Some(true));
}

#[test]
fn the_e_bit_reports_a_secondary_access_rights_list() {
    let identity = Rfpi::parse(CLASS_A_RFPI | (1 << 39)).describe();
    assert!(identity.sari_available);
}

#[test]
fn a_class_d_rfpi_exposes_the_gsm_operator_code() {
    let bits = (3u64 << 36) | (0x12345 << 16) | (0x42 << 8) | 0x07;
    let identity = Rfpi::parse(bits).describe();
    assert_eq!(identity.arc, DectArc::D);
    assert_eq!(identity.gop, Some(0x12345));
    assert_eq!(identity.mcc, Some(0x123));
    assert_eq!(identity.mnc, Some(0x45));
    assert_eq!(identity.fpn, Some(0x42));
    assert_eq!(identity.rpn, 0x07);
    assert_eq!(identity.multicell, Some(true));
}

#[test]
fn every_ari_class_consumes_the_whole_forty_bit_rfpi() {
    for class in 0..5u64 {
        let parsed = Rfpi::parse(class << 36);
        let rpn_bits = 40 - 1 - parsed.ari_bits as usize;
        assert!(rpn_bits > 0, "class {class} left no room for an RPN");
        assert_eq!(1 + parsed.ari_bits as usize + rpn_bits, 40);
    }
}

#[test]
fn the_r_crc_accepts_its_own_codeword_and_rejects_a_flipped_bit() {
    let a_field = append_r_crc(0x1234_5678_9ABC_0000);
    assert!(a_field_crc_ok(a_field));
    for bit in 0..64 {
        assert!(
            !a_field_crc_ok(a_field ^ (1u64 << bit)),
            "single bit error at {bit} slipped through the R-CRC"
        );
    }
}

#[test]
fn the_a_field_header_decodes_the_spec_tail_codes() {
    let tail = |ta: u8, from_rfp: bool| mac::header(u64::from(ta) << 61, from_rfp).tail;
    assert_eq!(tail(0, true), Tail::Ct { packet: 0 });
    assert_eq!(tail(1, true), Tail::Ct { packet: 1 });
    assert_eq!(tail(2, true), Tail::NtConnectionless);
    assert_eq!(tail(3, true), Tail::Nt);
    assert_eq!(tail(4, true), Tail::Qt);
    assert_eq!(tail(5, true), Tail::Escape);
    assert_eq!(tail(6, true), Tail::Mt);
    assert_eq!(tail(7, true), Tail::Pt);
    assert_eq!(tail(7, false), Tail::MtFirst);
}

#[test]
fn decodes_the_identity_of_a_modulated_dummy_bearer() {
    let station = sig::Station {
        rfpi: CLASS_A_RFPI,
        ..sig::Station::default()
    };
    let iq = sig::dummy_bearer(&station, 9);
    let mut chan = channel(DectParams::default());
    let decoded = frames(&run_events(&mut chan, &iq));
    let identity = decoded
        .iter()
        .find_map(|frame| frame.identity.clone())
        .expect("an Nt burst should carry the RFPI");
    assert_eq!(identity.rfpi, "01234D5E6D");
    assert_eq!(identity.emc, Some(0x1234));
    assert!(decoded.iter().all(|frame| frame.side == DectSide::Rfp));
    assert!(decoded.iter().all(|frame| frame.crc_errors == 0));
}

#[test]
fn reports_the_static_system_information_of_a_base_station() {
    let station = sig::Station {
        rfpi: CLASS_A_RFPI,
        carrier: 4,
        slot_pair: 2,
        rf_carriers: 0x155,
        transceivers: 1,
        pscn: 9,
        ..sig::Station::default()
    };
    let iq = sig::dummy_bearer(&station, 9);
    let mut chan = channel(DectParams::default());
    let decoded = frames(&run_events(&mut chan, &iq));
    let info = decoded
        .iter()
        .find(|frame| frame.carrier.is_some())
        .expect("a Qt static system info burst");
    assert_eq!(info.carrier, Some(4));
    assert_eq!(info.carrier_hz, Some(1_890_432_000.0));
    assert_eq!(info.slot_pair, Some(2));
    assert_eq!(info.rf_carriers, Some(0x155));
    assert_eq!(info.transceivers, Some(2));
    assert_eq!(info.pscn, Some(9));
}

#[test]
fn reports_which_security_a_base_station_advertises() {
    let station = sig::Station {
        rfpi: CLASS_A_RFPI,
        capabilities: sig::capability_bits(&[17, 33, 36, 37, 38]),
        ..sig::Station::default()
    };
    let iq = sig::dummy_bearer(&station, 9);
    let mut chan = channel(DectParams::default());
    let decoded = frames(&run_events(&mut chan, &iq));
    let caps = decoded
        .iter()
        .find(|frame| !frame.capabilities.is_empty())
        .expect("a Qt fixed part capabilities burst");
    assert!(caps.has(DectCapability::StandardAuthentication));
    assert!(caps.has(DectCapability::StandardCiphering));
    assert!(caps.has(DectCapability::LocationRegistration));
    assert!(caps.has(DectCapability::FullSlot));
    assert!(caps.has(DectCapability::GapBasicSpeech));
    assert!(!caps.has(DectCapability::SimServices));
    assert_eq!(caps.security.authentication_supported, Some(true));
    assert_eq!(caps.security.ciphering_supported, Some(true));
}

#[test]
fn a_base_station_without_ciphering_is_reported_as_unprotected() {
    let station = sig::Station {
        rfpi: CLASS_A_RFPI,
        capabilities: sig::capability_bits(&[17, 33]),
        ..sig::Station::default()
    };
    let iq = sig::dummy_bearer(&station, 9);
    let mut chan = channel(DectParams::default());
    let decoded = frames(&run_events(&mut chan, &iq));
    let caps = decoded
        .iter()
        .find(|frame| frame.security.ciphering_supported.is_some())
        .expect("a capabilities burst");
    assert_eq!(caps.security.ciphering_supported, Some(false));
    assert_eq!(caps.security.authentication_supported, Some(false));
    assert_eq!(caps.security.cipher_state, DectCipherState::Clear);
}

#[test]
fn follows_an_encryption_handshake_to_the_active_state() {
    let station = sig::Station {
        rfpi: CLASS_A_RFPI,
        ..sig::Station::default()
    };
    let iq = sig::with_burst(
        &station,
        12,
        7,
        true,
        sig::mt_encryption(0, 2, 0x0ABC, 0x0001_2345),
    );
    let mut chan = channel(DectParams::default());
    let decoded = frames(&run_events(&mut chan, &iq));
    let encrypted = decoded
        .iter()
        .find(|frame| frame.security.cipher_state == DectCipherState::Active)
        .expect("the grant should mark the bearer encrypted");
    assert_eq!(
        encrypted.security.last_command.as_deref(),
        Some("start encryption: grant")
    );
    assert_eq!(encrypted.fmid, Some(0x0ABC));
    assert_eq!(encrypted.pmid, Some(0x0001_2345));
    assert!(encrypted.handsets.contains(&0x0001_2345));
}

#[test]
fn a_stop_encryption_command_clears_the_active_state() {
    let station = sig::Station {
        rfpi: CLASS_A_RFPI,
        ..sig::Station::default()
    };
    let iq = sig::with_burst(&station, 12, 7, true, sig::mt_encryption(1, 2, 0x0ABC, 1));
    let mut chan = channel(DectParams::default());
    let decoded = frames(&run_events(&mut chan, &iq));
    assert!(
        decoded
            .iter()
            .any(|frame| frame.security.cipher_state == DectCipherState::Stopped)
    );
}

#[test]
fn a_corrupt_a_field_is_counted_and_never_reported_as_identity() {
    let station = sig::Station {
        rfpi: CLASS_A_RFPI,
        ..sig::Station::default()
    };
    let broken = sig::nt(CLASS_A_RFPI) ^ 0x0000_0000_00FF_0000;
    let iq = sig::with_burst(&station, 12, 7, true, broken);
    let mut chan = channel(DectParams::default());
    let decoded = frames(&run_events(&mut chan, &iq));
    assert!(decoded.iter().all(|frame| {
        frame
            .identity
            .as_ref()
            .is_none_or(|id| id.rfpi == "01234D5E6D")
    }));
    assert!(
        decoded.last().map(|frame| frame.crc_errors) > Some(0),
        "the damaged burst should be counted, not dropped silently"
    );
}

#[test]
fn survives_noise_at_a_workable_signal_level() {
    let station = sig::Station {
        rfpi: CLASS_A_RFPI,
        ..sig::Station::default()
    };
    let mut iq = sig::dummy_bearer(&station, 9);
    crate::testgen::add_noise(&mut iq, 0x5EED, 0.12);
    let mut chan = channel(DectParams::default());
    let decoded = frames(&run_events(&mut chan, &iq));
    assert!(
        decoded.iter().any(|frame| frame
            .identity
            .as_ref()
            .is_some_and(|id| id.rfpi == "01234D5E6D")),
        "the RFPI should still come through at 0.12 noise amplitude"
    );
}

#[test]
fn two_base_stations_in_different_slots_stay_apart() {
    let first = sig::Station {
        rfpi: CLASS_A_RFPI,
        slot: 2,
        ..sig::Station::default()
    };
    let second = sig::Station {
        rfpi: 0x0055_5555_5555 & ((1 << 40) - 1),
        slot: 9,
        ..sig::Station::default()
    };
    let mut iq = sig::dummy_bearer(&first, 9);
    for (sample, value) in sig::dummy_bearer(&second, 9).iter().enumerate() {
        if value.norm_sqr() > 0.0 {
            iq[sample] = *value;
        }
    }
    let mut chan = channel(DectParams::default());
    let decoded = frames(&run_events(&mut chan, &iq));
    let seen: std::collections::BTreeSet<String> = decoded
        .iter()
        .filter_map(|frame| frame.identity.as_ref().map(|id| id.rfpi.clone()))
        .collect();
    assert_eq!(seen.len(), 2, "expected both base stations, saw {seen:?}");
}

#[test]
fn the_side_filter_drops_the_direction_it_was_told_to_ignore() {
    let station = sig::Station {
        rfpi: CLASS_A_RFPI,
        ..sig::Station::default()
    };
    let iq = sig::dummy_bearer(&station, 9);
    let mut chan = channel(DectParams {
        band: DectBand::Eu,
        sides: DectSides::Pp,
    });
    assert!(frames(&run_events(&mut chan, &iq)).is_empty());
}

#[test]
fn silence_produces_no_frames() {
    let iq = vec![Complex::default(); 200_000];
    let mut chan = channel(DectParams::default());
    assert!(frames(&run_events(&mut chan, &iq)).is_empty());
}

#[test]
fn a_carrier_offset_does_not_stop_the_identity_getting_through() {
    let station = sig::Station {
        rfpi: CLASS_A_RFPI,
        ..sig::Station::default()
    };
    for offset_hz in [-40_000.0, 40_000.0] {
        let mut iq = sig::dummy_bearer(&station, 9);
        crate::testgen::shift(&mut iq, offset_hz, INPUT_RATE_HZ);
        let mut chan = channel(DectParams::default());
        let decoded = frames(&run_events(&mut chan, &iq));
        assert!(
            decoded.iter().any(|frame| frame
                .identity
                .as_ref()
                .is_some_and(|id| id.rfpi == "01234D5E6D")),
            "a {offset_hz} Hz carrier offset lost the RFPI"
        );
    }
}
