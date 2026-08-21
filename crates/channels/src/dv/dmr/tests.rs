use sdrmm_wire::{DmrSlots, SymbolPlane};

use super::*;
use crate::{
    AUDIO_RATE, ChannelOutputs,
    dv::{testutil::decode, vocoder::testutil::half_rate_frames},
    testgen::dv::dmr as tx,
    testutil::settings,
};

fn channel(slots: DmrSlots) -> DmrChannel {
    DmrChannel::new(
        ChannelCtx {
            input_rate: INPUT_RATE_HZ,
        },
        settings(ChannelParams::Dmr(DmrParams {
            slots,
            ignore_crc: false,
        })),
    )
    .expect("dmr channel")
}

fn put(bits: &mut [bool], at: usize, width: usize, value: u64) {
    for index in 0..width {
        bits[at + index] = value >> (width - index - 1) & 1 == 1;
    }
}

#[test]
fn decodes_a_trellis_coded_data_block() {
    let payload: [bool; 144] = std::array::from_fn(|index| (index * 7 + index / 5) % 3 == 0);
    let iq = tx::rate_three_quarter_data(3, &payload, INPUT_RATE_HZ);

    let frames = decode(&mut channel(DmrSlots::Both), &iq);

    let block = frames
        .iter()
        .find(|frame| frame.opcode.as_deref() == Some("rate 3/4 data"))
        .expect("a rate 3/4 data block");
    assert_eq!(block.color_code, Some(3));
    assert_eq!(block.data.as_deref(), Some(hex(&payload).as_str()));
}

fn hex(bits: &[bool]) -> String {
    bits.chunks(4)
        .map(|nibble| {
            let value = nibble
                .iter()
                .fold(0u8, |acc, bit| acc << 1 | u8::from(*bit));
            char::from_digit(u32::from(value), 16).unwrap_or('0')
        })
        .collect()
}

#[test]
fn decodes_an_absolute_channel_definition_off_the_air() {
    let iq = tx::channel_definition(3, 811, 451_125_000, 456_250_000, INPUT_RATE_HZ);
    let frames = decode(&mut channel(DmrSlots::Both), &iq);

    let defined = frames
        .iter()
        .find(|frame| frame.channel_definition.is_some())
        .expect("an absolute channel definition");
    assert_eq!(
        defined.channel_definition,
        Some(DvChannelDefinition {
            channel: 811,
            tx_hz: 451_125_000,
            rx_hz: 456_250_000,
            color_code: None,
        })
    );
    assert_eq!(defined.channel, Some(811));
    assert_eq!(defined.color_code, Some(3));
    assert_eq!(defined.crc_verified, Some(true));
    assert_eq!(defined.trunk_protocol, Some(DvTrunkProtocol::TierThree));
}

#[test]
fn an_interrupted_multi_block_is_not_read_as_a_definition() {
    let iq = tx::interrupted_channel_definition(3, 811, 451_125_000, 456_250_000, INPUT_RATE_HZ);
    let frames = decode(&mut channel(DmrSlots::Both), &iq);
    assert!(
        frames
            .iter()
            .all(|frame| frame.channel_definition.is_none()),
        "a stale header paired with a later continuation"
    );
}

#[test]
fn a_tier_three_csbk_names_its_protocol() {
    let iq = tx::csbk(3, 0b110001, 505, 2_621_001, INPUT_RATE_HZ);
    let frames = decode(&mut channel(DmrSlots::Both), &iq);
    let grant = frames
        .iter()
        .find(|frame| frame.trunk_protocol.is_some())
        .expect("a tagged grant");
    assert_eq!(grant.trunk_protocol, Some(DvTrunkProtocol::TierThree));
    assert_eq!(grant.crc_verified, Some(true));
}

#[test]
fn tier_three_grant_exposes_channel_slot_and_flags() {
    let mut payload = [false; 96];
    put(&mut payload, 16, 12, 37);
    payload[28] = true;
    payload[29] = true;
    payload[30] = true;
    put(&mut payload, 32, 24, 9_001);
    put(&mut payload, 56, 24, 1_234_567);
    let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
    decode_tier_three_csbk(&mut frame, 0, 0b110001, &payload, false);
    assert_eq!(frame.channel, Some(37));
    assert_eq!(frame.slot, Some(2));
    assert_eq!(frame.late_entry, Some(true));
    assert_eq!(frame.emergency, Some(true));
}

#[test]
fn a_grant_names_the_timeslot_it_hands_out_not_the_one_it_arrived_on() {
    let mut payload = vec![false; 80];
    put(&mut payload, 2, 6, 0b110001);
    put(&mut payload, 16, 12, 37);
    payload[28] = true;
    let iq = tx::csbk_bits(3, &payload, INPUT_RATE_HZ);

    let frames = decode(&mut channel(DmrSlots::Both), &iq);

    let grant = frames
        .iter()
        .find(|frame| frame.channel == Some(37))
        .expect("a channel grant");
    assert_eq!(grant.slot, Some(2));
}

#[test]
fn a_data_grant_is_followed_but_marked_as_data() {
    let mut payload = [false; 96];
    put(&mut payload, 16, 12, 37);
    let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
    decode_tier_three_csbk(&mut frame, 0, 52, &payload, false);
    assert_eq!(frame.channel, Some(37));
    assert_eq!(frame.data.as_deref(), Some("data call"));
}

#[test]
fn a_tier_two_outbound_activation_is_not_read_as_a_grant() {
    let mut payload = [false; 96];
    put(&mut payload, 16, 12, 37);
    let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
    decode_tier_three_csbk(&mut frame, 0, 56, &payload, false);
    assert_eq!(frame.channel, None);
    assert_eq!(frame.trunk_protocol, None);

    let mut tier_three = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
    decode_tier_three_csbk(&mut tier_three, 0, 56, &payload, true);
    assert_eq!(tier_three.channel, Some(37));
}

#[test]
fn aloha_reads_its_parameters_where_the_standard_puts_them() {
    let mut payload = [false; 96];
    put(&mut payload, 19, 3, 5);
    put(&mut payload, 24, 5, 21);
    put(&mut payload, 29, 2, 3);
    put(&mut payload, 31, 4, 9);
    payload[35] = true;
    put(&mut payload, 36, 4, 11);
    put(&mut payload, 56, 24, 4_001);
    let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
    decode_tier_three_csbk(&mut frame, 0, 25, &payload, false);

    assert_eq!(frame.trunk_protocol, Some(DvTrunkProtocol::TierThree));
    assert_eq!(frame.destination, Some(4_001));
    assert_eq!(
        frame.data.as_deref(),
        Some("version 5, mask 21, service 3, wait 9, registration 1, backoff 11")
    );
}

fn on_air_csbk(fid: u8, opcode: u8, body: &[(usize, usize, u64)]) -> Vec<DvFrame> {
    let mut head = [false; 80];
    put(&mut head, 2, 6, u64::from(opcode));
    put(&mut head, 8, 8, u64::from(fid));
    for (at, width, value) in body {
        put(&mut head, *at, *width, *value);
    }
    let iq = tx::csbk_bits(3, &head, INPUT_RATE_HZ);
    decode(&mut channel(DmrSlots::Both), &iq)
}

fn control_frame(fid: u8, opcode: u8, body: &[(usize, usize, u64)]) -> DvFrame {
    on_air_csbk(fid, opcode, body)
        .into_iter()
        .find(|frame| frame.kind == DvFrameKind::Control)
        .expect("a control frame off the air")
}

#[test]
fn a_capacity_max_channel_update_survives_the_whole_chain() {
    let frame = control_frame(0x10, 33, &[(16, 12, 811), (32, 24, 9_001), (56, 24, 9_002)]);

    assert_eq!(frame.trunk_protocol, Some(DvTrunkProtocol::TierThree));
    assert_eq!(frame.channel, Some(811));
    assert_eq!(frame.crc_verified, Some(true));
    assert_eq!(frame.slot_activity.len(), 2);
    assert_eq!(frame.slot_activity[0].destination, Some(9_001));
    assert_eq!(frame.slot_activity[0].logical_channel, Some(811));
    assert_eq!(frame.slot_activity[1].slot, 2);
    assert_eq!(frame.slot_activity[1].destination, Some(9_002));
}

#[test]
fn an_adjacent_site_announcement_survives_the_whole_chain() {
    let frame = control_frame(
        0,
        40,
        &[
            (16, 5, 6),
            (21, 2, 2),
            (23, 4, 12),
            (27, 8, 200),
            (68, 12, 811),
        ],
    );

    assert_eq!(frame.opcode.as_deref(), Some("broadcast, adjacent site"));
    assert_eq!(frame.channel, Some(811));
    assert_eq!(frame.network_id, Some(12));
    assert_eq!(frame.site_id, Some(200));
}

#[test]
fn a_data_channel_grant_reaches_the_trunk_follower_as_a_grant() {
    let frame = control_frame(0, 52, &[(16, 12, 37), (32, 24, 9_001), (56, 24, 4_001)]);

    assert_eq!(frame.trunk_protocol, Some(DvTrunkProtocol::TierThree));
    assert_eq!(frame.channel, Some(37));
    assert_eq!(frame.slot, Some(1));
    assert_eq!(frame.destination, Some(9_001));
    assert_eq!(frame.source, Some(4_001));
    assert_eq!(frame.data.as_deref(), Some("data call"));
}

#[test]
fn opcode_56_means_outbound_activation_until_a_tier_three_csbk_arrives() {
    let alone = control_frame(0, 56, &[(16, 12, 37)]);
    assert_eq!(alone.opcode.as_deref(), Some("BS outbound activation"));
    assert_eq!(alone.channel, None);

    let mut decoder = channel(DmrSlots::Both);
    let mut head = [false; 80];
    put(&mut head, 2, 6, 25);
    decode(&mut decoder, &tx::csbk_bits(3, &head, INPUT_RATE_HZ));

    let mut later = [false; 80];
    put(&mut later, 2, 6, 56);
    put(&mut later, 16, 12, 37);
    let frames = decode(&mut decoder, &tx::csbk_bits(3, &later, INPUT_RATE_HZ));
    let grant = frames
        .iter()
        .find(|frame| frame.kind == DvFrameKind::Control)
        .expect("a control frame");

    assert_eq!(
        grant.opcode.as_deref(),
        Some("talkgroup data channel grant, multiple items")
    );
    assert_eq!(grant.channel, Some(37));
}

#[test]
fn an_off_air_capacity_max_aloha_reads_as_its_site_announced_itself() {
    let bytes = [
        0x99, 0x00, 0x09, 0x00, 0xF6, 0x84, 0x07, 0x00, 0x00, 0x00, 0x00, 0xC8,
    ];
    let mut payload = [false; 96];
    for (i, bit) in payload.iter_mut().enumerate() {
        *bit = bytes[i / 8] >> (7 - i % 8) & 1 == 1;
    }
    assert_eq!(bits_to_u32(&payload, 2, 6), 25, "not an ALOHA");

    let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
    decode_tier_three_csbk(&mut frame, 0, 25, &payload, false);

    assert_eq!(frame.network_id, Some(1));
    assert_eq!(frame.site_id, Some(1));
    assert_eq!(
        frame.data.as_deref(),
        Some("version 2, mask 0, service 0, wait 7, registration 1, backoff 6")
    );
}

#[test]
fn the_system_identity_code_splits_by_the_model_it_declares() {
    let identity = |model: u64, net: u64, site: u64, net_bits: usize, site_bits: usize| {
        let mut payload = [false; 96];
        put(&mut payload, 40, 2, model);
        put(&mut payload, 42, net_bits, net);
        put(&mut payload, 42 + net_bits, site_bits, site);
        let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
        decode_tier_three_csbk(&mut frame, 0, 25, &payload, false);
        (frame.network_id, frame.site_id)
    };

    assert_eq!(identity(0, 400, 5, 9, 3), (Some(400), Some(5)));
    assert_eq!(identity(1, 100, 21, 7, 5), (Some(100), Some(21)));
    assert_eq!(identity(2, 12, 200, 4, 8), (Some(12), Some(200)));
    assert_eq!(identity(3, 3, 900, 2, 10), (Some(3), Some(900)));
}

#[test]
fn an_adjacent_site_announcement_names_the_neighbour_control_channel() {
    let mut payload = [false; 96];
    put(&mut payload, 16, 5, 6);
    put(&mut payload, 21, 2, 2);
    put(&mut payload, 23, 4, 12);
    put(&mut payload, 27, 8, 200);
    payload[57] = true;
    put(&mut payload, 68, 12, 811);
    let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
    decode_tier_three_csbk(&mut frame, 0, 40, &payload, false);

    assert_eq!(frame.opcode.as_deref(), Some("broadcast, adjacent site"));
    assert_eq!(frame.channel, Some(811));
    assert_eq!(frame.network_id, Some(12));
    assert_eq!(frame.site_id, Some(200));
    assert!(
        frame
            .data
            .as_deref()
            .is_some_and(|data| data.contains("neighbour TSCC on channel 811"))
    );
}

#[test]
fn a_vote_now_announcement_names_the_channel_to_move_to() {
    let mut payload = [false; 96];
    put(&mut payload, 16, 5, 2);
    put(&mut payload, 68, 12, 42);
    let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
    decode_tier_three_csbk(&mut frame, 0, 40, &payload, false);

    assert_eq!(frame.opcode.as_deref(), Some("broadcast, vote now advice"));
    assert_eq!(frame.channel, Some(42));
}

#[test]
fn an_announce_tscc_names_both_control_channels_of_the_site() {
    let mut payload = [false; 96];
    put(&mut payload, 16, 5, 0);
    put(&mut payload, 25, 4, 3);
    put(&mut payload, 29, 4, 5);
    put(&mut payload, 56, 12, 811);
    put(&mut payload, 68, 12, 812);
    let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
    decode_tier_three_csbk(&mut frame, 0, 40, &payload, false);

    assert_eq!(frame.channel, Some(811));
    assert_eq!(
        frame.data.as_deref(),
        Some("TSCC on channel 811 colour 3, second channel 812 colour 5")
    );
}

#[test]
fn a_move_tscc_names_the_channel_the_radio_must_go_to() {
    let mut payload = [false; 96];
    put(&mut payload, 44, 12, 900);
    put(&mut payload, 56, 24, 4_001);
    let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
    decode_tier_three_csbk(&mut frame, 0, 57, &payload, false);

    assert_eq!(frame.channel, Some(900));
    assert_eq!(frame.destination, Some(4_001));
}

#[test]
fn an_ahoy_names_the_service_it_asks_for() {
    let mut payload = [false; 96];
    put(&mut payload, 28, 4, 14);
    put(&mut payload, 32, 24, 9_001);
    put(&mut payload, 56, 24, 4_001);
    let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
    decode_tier_three_csbk(&mut frame, 0, 28, &payload, false);

    assert_eq!(
        frame.opcode.as_deref(),
        Some("AHOY, registration or radio check")
    );
    assert_eq!(frame.destination, Some(9_001));
    assert_eq!(frame.source, Some(4_001));
}

#[test]
fn a_registration_acknowledgement_names_itself() {
    let mut payload = [false; 96];
    put(&mut payload, 16, 7, 2);
    put(&mut payload, 32, 24, 9_001);
    put(&mut payload, 56, 24, 4_001);
    let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
    decode_tier_three_csbk(&mut frame, 0, 32, &payload, false);

    assert_eq!(
        frame.opcode.as_deref(),
        Some("acknowledge response, registration accepted")
    );
    assert_eq!(frame.source, Some(4_001));
}

#[test]
fn a_capacity_max_channel_update_reports_both_timeslots() {
    let mut payload = [false; 96];
    put(&mut payload, 16, 12, 811);
    put(&mut payload, 32, 24, 9_001);
    put(&mut payload, 56, 24, 9_002);
    let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
    decode_vendor_csbk(&mut frame, 0x10, 33, &payload);

    assert_eq!(frame.trunk_protocol, Some(DvTrunkProtocol::TierThree));
    assert_eq!(frame.channel, Some(811));
    assert_eq!(frame.slot_activity.len(), 2);
    assert_eq!(frame.slot_activity[0].slot, 1);
    assert_eq!(frame.slot_activity[0].destination, Some(9_001));
    assert_eq!(frame.slot_activity[0].logical_channel, Some(811));
    assert_eq!(frame.slot_activity[1].slot, 2);
    assert_eq!(frame.slot_activity[1].destination, Some(9_002));
}

#[test]
fn a_capacity_max_update_skips_a_timeslot_nobody_is_using() {
    let mut payload = [false; 96];
    put(&mut payload, 16, 12, 811);
    put(&mut payload, 32, 24, 9_001);
    let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
    decode_vendor_csbk(&mut frame, 0x10, 33, &payload);

    assert_eq!(frame.slot_activity.len(), 1);
    assert_eq!(frame.slot_activity[0].slot, 1);
}

#[test]
fn advantage_mode_reads_the_shorter_talkgroup_fields() {
    let mut payload = [false; 96];
    put(&mut payload, 16, 12, 811);
    put(&mut payload, 32, 10, 501);
    put(&mut payload, 42, 10, 502);
    let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
    decode_vendor_csbk(&mut frame, 0x10, 34, &payload);

    assert_eq!(frame.slot_activity.len(), 2);
    assert_eq!(frame.slot_activity[0].destination, Some(501));
    assert_eq!(frame.slot_activity[1].destination, Some(502));
}

#[test]
fn absolute_channel_definition_uses_125_hz_steps() {
    let mut payload = [false; 96];
    payload[0] = true;
    put(&mut payload, 2, 6, 0b101000);
    put(&mut payload, 22, 12, 811);
    put(&mut payload, 34, 10, 451);
    put(&mut payload, 44, 13, 1000);
    put(&mut payload, 57, 10, 456);
    put(&mut payload, 67, 13, 2000);
    let mut bytes = Vec::new();
    pack_bytes(&payload[..80], &mut bytes);
    put(&mut payload, 80, 16, u64::from(dmr_crc16(&bytes)));
    assert!(valid_mbc_crc(&payload));
    assert_eq!(
        decode_channel_definition(&payload, None),
        Some(DvChannelDefinition {
            channel: 811,
            tx_hz: 451_125_000,
            rx_hz: 456_250_000,
            color_code: None,
        })
    );
}

#[test]
fn ras_mode_marks_an_unverified_block_and_refuses_a_badly_repaired_one() {
    let payload = [false; 96];
    let mut strict = Decoder::new(DmrParams::default());
    assert!(strict.checked_block(&payload, CSBK_MASK, 0).is_none());

    let mut ras = Decoder::new(DmrParams {
        ignore_crc: true,
        ..DmrParams::default()
    });
    let frame = ras.checked_block(&payload, CSBK_MASK, 0).expect("kept");
    assert_eq!(frame.crc_verified, Some(false));
    assert!(
        ras.checked_block(&payload, CSBK_MASK, MAX_UNVERIFIED_REPAIRS)
            .is_some(),
        "a block the forward error correction read back was thrown away"
    );
    assert!(
        ras.checked_block(&payload, CSBK_MASK, MAX_UNVERIFIED_REPAIRS + 1)
            .is_none(),
        "a block that needed more repair than the checks can cover was handed on"
    );
}

fn encoded_tone_sockets() -> [[bool; 216]; 6] {
    let mut sockets = [[false; 216]; 6];
    for (index, air) in half_rate_frames(18).iter().enumerate() {
        let at = index % 3 * VOCODER_FRAME_BITS;
        sockets[index / 3][at..at + VOCODER_FRAME_BITS].copy_from_slice(air);
    }
    sockets
}

fn decode_audio(iq: &[Complex<f32>]) -> (Vec<DvFrame>, Vec<f32>) {
    let mut chan = channel(DmrSlots::Both);
    let mut out = ChannelOutputs::default();
    let mut frames = Vec::new();
    let mut audio = Vec::new();
    let quiet = crate::testutil::complex_noise(0x1157, 0.01, 4 * INPUT_RATE_HZ as usize / 10);
    chan.process(&quiet, &mut out);
    for block in iq.chunks(997) {
        out.reset();
        chan.process(block, &mut out);
        audio.extend_from_slice(&out.audio_pcm);
        for event in out.events.drain(..) {
            let DecoderEvent::Dv(frame) = event else {
                panic!("unexpected event")
            };
            frames.push(frame);
        }
    }
    (frames, audio)
}

#[test]
fn the_symbol_tap_reads_a_clean_call_as_four_well_separated_levels() {
    let call = tx::Call::default();
    let iq = tx::transmission(&call, INPUT_RATE_HZ);
    let mut chan = channel(DmrSlots::Both);
    let mut out = ChannelOutputs::default();
    out.symbols.set_wanted(true);

    let mut seen = 0usize;
    let mut worst = f32::INFINITY;
    for block in iq.chunks(2_048) {
        out.reset();
        chan.process(block, &mut out);
        if out.symbols.symbols.is_empty() {
            continue;
        }
        seen += out.symbols.symbols.len();
        assert_eq!(out.symbols.plane, Some(SymbolPlane::Level));
        assert_eq!(out.symbols.reference.len(), 4);
        assert_eq!(out.symbols.symbol_rate, BAUD);
        worst = worst.min(out.symbols.margin);
    }

    assert!(seen > 700, "only {seen} symbols reached the tap");
    assert!(
        worst > 2.0,
        "a clean transmission left only {worst} of slicing margin"
    );
}

#[test]
fn a_tap_nobody_asked_for_stays_empty_through_a_whole_call() {
    let call = tx::Call::default();
    let iq = tx::transmission(&call, INPUT_RATE_HZ);
    let mut chan = channel(DmrSlots::Both);
    let mut out = ChannelOutputs::default();

    for block in iq.chunks(2_048) {
        out.reset();
        chan.process(block, &mut out);
        assert!(out.symbols.symbols.is_empty());
        assert_eq!(out.symbols.plane, None);
    }
}

#[test]
fn decodes_a_call_from_header_to_terminator() {
    let call = tx::Call::default();
    let iq = tx::transmission(&call, INPUT_RATE_HZ);
    let frames = decode(&mut channel(DmrSlots::Both), &iq);

    let header = frames
        .iter()
        .find(|f| f.kind == DvFrameKind::Header)
        .expect("voice LC header");
    assert_eq!(header.mode, DvMode::Dmr);
    assert_eq!(header.slot, Some(1));
    assert_eq!(header.color_code, Some(u16::from(call.color_code)));
    assert_eq!(header.group_call, Some(true));
    assert_eq!(header.destination, Some(call.destination));
    assert_eq!(header.source, Some(call.source));

    let headers: Vec<&DvFrame> = frames
        .iter()
        .filter(|f| f.kind == DvFrameKind::Header)
        .collect();
    assert_eq!(headers.len(), 1, "voice LC header decoded more than once");
    for header in headers {
        assert!(
            header.errors_corrected <= 4,
            "header needed {} corrections: {header:?}",
            header.errors_corrected
        );
    }

    let voice = frames
        .iter()
        .find(|f| f.kind == DvFrameKind::Voice)
        .expect("late entry: no embedded link control survived the superframe");
    assert_eq!(voice.destination, Some(call.destination));
    assert_eq!(voice.source, Some(call.source));
    assert_eq!(voice.color_code, Some(u16::from(call.color_code)));

    let terminator = frames
        .iter()
        .find(|f| f.kind == DvFrameKind::Terminator)
        .expect("terminator with link control");
    assert_eq!(terminator.destination, Some(call.destination));
    assert_eq!(terminator.source, Some(call.source));
    assert_eq!(terminator.color_code, Some(u16::from(call.color_code)));
}

#[test]
fn decodes_voice_to_audio() {
    let call = tx::Call::default();
    let iq = tx::transmission_with_voice(&call, &encoded_tone_sockets(), INPUT_RATE_HZ);
    let (_, audio) = decode_audio(&iq);
    assert!(
        (audio.len() as isize - (18 * 160 * 6) as isize).abs() <= 1,
        "not every vocoder frame decoded: {} samples",
        audio.len()
    );
    assert!(audio.iter().all(|sample| sample.is_finite()));
    assert!(
        audio.iter().all(|sample| sample.abs() < 1.0),
        "presentation gain drove the vocoder into full-scale clipping"
    );
    let settled = &audio[3 * 960..];
    let rms = crate::testutil::rms(settled);
    let (frequency, _) = crate::testutil::dominant_tone(settled, f64::from(AUDIO_RATE));
    assert!(rms > 0.01, "decoded tone is silent: rms {rms}");
    assert!(
        (frequency - 440.0).abs() < 40.0,
        "decoded tone shifted to {frequency} Hz"
    );
}

#[test]
fn encrypted_calls_are_reported_and_muted() {
    let call = tx::Call {
        encrypted: true,
        ..tx::Call::default()
    };
    let iq = tx::transmission(&call, INPUT_RATE_HZ);
    let (frames, audio) = decode_audio(&iq);
    assert!(
        frames
            .iter()
            .filter(|frame| matches!(frame.kind, DvFrameKind::Header | DvFrameKind::Voice))
            .all(|frame| frame.encrypted == Some(true)),
        "privacy was lost in late-entry signalling: {frames:?}"
    );
    assert!(!audio.is_empty());
    assert!(audio.iter().all(|&sample| sample == 0.0));
}

#[test]
fn decodes_a_recorded_call() {
    const FIXTURE: &[u8] = include_bytes!("../../../../../fixtures/dmr_call_48k.sigmf-data");
    let iq: Vec<Complex<f32>> = FIXTURE
        .as_chunks::<8>()
        .0
        .iter()
        .map(|s| {
            Complex::new(
                f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                f32::from_le_bytes([s[4], s[5], s[6], s[7]]),
            )
        })
        .collect();
    let mut chan = channel(DmrSlots::Both);
    let mut filter = channel_filter();
    let mut out = ChannelOutputs::default();
    let mut filtered = Vec::new();
    let mut frames = Vec::new();
    let mut audio = Vec::new();
    for block in iq.chunks(997) {
        filter.process(block, &mut filtered);
        out.reset();
        chan.process(&filtered, &mut out);
        audio.extend_from_slice(&out.audio_pcm);
        for event in out.events.drain(..) {
            let DecoderEvent::Dv(frame) = event else {
                panic!("unexpected event")
            };
            frames.push(frame);
        }
    }

    let calls: Vec<&DvFrame> = frames
        .iter()
        .filter(|f| f.kind == DvFrameKind::Header || f.kind == DvFrameKind::Voice)
        .collect();
    assert!(
        frames.iter().any(|f| f.kind == DvFrameKind::Header),
        "no voice LC header: {frames:?}"
    );
    assert!(
        frames
            .iter()
            .filter(|f| f.kind == DvFrameKind::Voice)
            .count()
            >= 3,
        "late entry recovered fewer than three superframes: {frames:?}"
    );
    for frame in calls {
        assert_eq!(frame.color_code, Some(1));
        assert_eq!(frame.group_call, Some(true));
        assert_eq!(frame.source, Some(12_345_678));
        assert_eq!(frame.destination, Some(12_345_678));
    }
    assert!(
        audio.len() >= 18 * 160 * 6,
        "no complete off-air audio superframe"
    );
    assert!(audio.iter().all(|sample| sample.is_finite()));
    let rms = crate::testutil::rms(&audio);
    let peak = audio
        .iter()
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    assert!(
        rms > 0.001 && peak > 0.01,
        "off-air voice produced no signal: rms {rms}, peak {peak}, frames {frames:?}"
    );
}

fn tier_three_control_fixture() -> Vec<Complex<f32>> {
    const FIXTURE: &[u8] =
        include_bytes!("../../../../../fixtures/dmr_tier3_control_48k.sigmf-data");
    FIXTURE
        .as_chunks::<8>()
        .0
        .iter()
        .map(|s| {
            Complex::new(
                f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                f32::from_le_bytes([s[4], s[5], s[6], s[7]]),
            )
        })
        .collect()
}

fn tolerant_channel() -> DmrChannel {
    DmrChannel::new(
        ChannelCtx {
            input_rate: INPUT_RATE_HZ,
        },
        settings(ChannelParams::Dmr(DmrParams {
            slots: DmrSlots::Both,
            ignore_crc: true,
        })),
    )
    .expect("dmr channel")
}

/// A control channel arrives as a receiver actually meets it: from a standing start, through the
/// channel filter, in the fixed blocks a radio delivers.
#[test]
fn a_live_control_channel_decodes_in_the_blocks_a_radio_delivers() {
    let iq = tier_three_control_fixture();
    let mut chan = tolerant_channel();
    let mut filter = channel_filter();
    let mut filtered = Vec::new();
    let mut out = ChannelOutputs::default();
    let mut frames = Vec::new();
    for block in iq.chunks(1_200) {
        filter.process(block, &mut filtered);
        out.reset();
        chan.process(&filtered, &mut out);
        for event in out.events.drain(..) {
            if let DecoderEvent::Dv(frame) = event {
                frames.push(frame);
            }
        }
    }

    assert!(
        frames.len() > 50,
        "a receiver that opened on a live carrier heard {} bursts",
        frames.len()
    );
    assert!(
        frames
            .iter()
            .any(|frame| frame.channel == Some(22) || frame.channel == Some(42)),
        "no channel grant survived the cold start: {frames:?}"
    );
}

#[test]
fn decodes_a_recorded_tier_three_control_channel() {
    let iq = tier_three_control_fixture();
    let mut chan = DmrChannel::new(
        ChannelCtx {
            input_rate: INPUT_RATE_HZ,
        },
        settings(ChannelParams::Dmr(DmrParams {
            slots: DmrSlots::Both,
            ignore_crc: true,
        })),
    )
    .expect("dmr channel");
    let mut filter = channel_filter();
    let mut filtered = Vec::new();
    filter.process(&iq, &mut filtered);
    let frames = decode(&mut chan, &filtered);

    assert!(
        frames
            .iter()
            .any(|frame| frame.control_channel == Some(true)
                && frame.trunk_protocol == Some(DvTrunkProtocol::TierThree)),
        "the CACH never named the site's control channel: {frames:?}"
    );
    assert!(
        frames
            .iter()
            .filter(|frame| frame.opcode.as_deref() == Some("Capacity Max ALOHA"))
            .count()
            > 10,
        "the site was not recognised as Capacity Max: {frames:?}"
    );

    let grants: Vec<(u16, u8, u32, u32)> = frames
        .iter()
        .filter(|frame| frame.data.as_deref() == Some("data call"))
        .filter_map(|frame| {
            Some((
                frame.channel?,
                frame.slot?,
                frame.source?,
                frame.destination?,
            ))
        })
        .collect();
    assert!(
        grants.contains(&(22, 2, 9_995, 9_999)),
        "the grant for logical channel 22 was lost: {grants:?}"
    );
    assert!(
        grants.contains(&(42, 1, 9_999, 9_995)),
        "the grant for logical channel 42 was lost: {grants:?}"
    );
    for frame in &frames {
        assert_eq!(frame.color_code.unwrap_or(10), 10);
    }
}

#[test]
fn decodes_a_private_call_header() {
    let call = tx::Call {
        group: false,
        destination: 2_621_002,
        ..tx::Call::default()
    };
    let iq = tx::transmission(&call, INPUT_RATE_HZ);
    let frames = decode(&mut channel(DmrSlots::Both), &iq);
    let header = frames
        .iter()
        .find(|f| f.kind == DvFrameKind::Header)
        .expect("voice LC header");
    assert_eq!(header.group_call, Some(false));
    assert_eq!(header.destination, Some(call.destination));
}

#[test]
fn decodes_a_csbk() {
    let iq = tx::csbk(3, 0b111101, 505, 2_621_001, INPUT_RATE_HZ);
    let frames = decode(&mut channel(DmrSlots::Both), &iq);
    let csbk = frames
        .iter()
        .find(|f| f.kind == DvFrameKind::Control)
        .expect("csbk");
    assert_eq!(csbk.color_code, Some(3));
    assert_eq!(csbk.opcode.as_deref(), Some("preamble"));
    assert_eq!(csbk.destination, Some(505));
    assert_eq!(csbk.source, Some(2_621_001));
}

#[test]
fn vendor_csbk_opcode_collision_does_not_invent_addresses() {
    let iq = tx::csbk_with_fid(3, 0x08, 0b111101, 505, 2_621_001, INPUT_RATE_HZ);
    let frames = decode(&mut channel(DmrSlots::Both), &iq);
    let csbk = frames
        .iter()
        .find(|frame| frame.kind == DvFrameKind::Control)
        .expect("vendor CSBK");
    assert_eq!(csbk.vendor, Some(Vendor::Hytera));
    assert_eq!(csbk.manufacturer_id, Some(0x08));
    assert_eq!(csbk.opcode.as_deref(), Some("Hytera CSBK, unparsed"));
    assert_eq!(csbk.destination, None);
    assert_eq!(csbk.source, None);
}

#[test]
fn a_hytera_xpt_csbk_names_its_protocol() {
    let iq = tx::csbk_with_fid(3, 0x68, 0x0A, 505, 2_621_001, INPUT_RATE_HZ);
    let frames = decode(&mut channel(DmrSlots::Both), &iq);
    let csbk = frames
        .iter()
        .find(|frame| frame.kind == DvFrameKind::Control)
        .expect("Hytera XPT CSBK");
    assert_eq!(csbk.trunk_protocol, Some(DvTrunkProtocol::HyteraXpt));
    assert_eq!(csbk.crc_verified, Some(true));
}

#[test]
fn vendor_dispatch_decodes_connect_plus_and_capacity_plus_fields() {
    let mut connect = [false; 96];
    write_bits(&mut connect, 16, 24, 151_015);
    write_bits(&mut connect, 40, 24, 1_216);
    write_bits(&mut connect, 64, 4, 2);
    connect[68] = true;
    write_bits(&mut connect, 72, 8, 2);
    let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
    decode_vendor_csbk(&mut frame, 0x06, 0x03, &connect);
    assert_eq!(frame.source, Some(151_015));
    assert_eq!(frame.destination, Some(1_216));
    assert_eq!(frame.channel, Some(2));
    assert_eq!(frame.group_call, Some(true));
    assert!(
        frame
            .data
            .as_deref()
            .is_some_and(|data| data.contains("TS 2"))
    );

    let mut capacity = [false; 96];
    write_bits(&mut capacity, 16, 2, 3);
    write_bits(&mut capacity, 20, 4, 7);
    let mut frame = DvFrame::new(DvMode::Dmr, DvFrameKind::Control);
    decode_vendor_csbk(&mut frame, 0x10, 0x3E, &capacity);
    assert_eq!(frame.rest_channel, Some(7));
}

#[test]
fn decodes_gps_info_link_control() {
    let mut decoder = Decoder::new(DmrParams::default());
    let mut lc = [false; 72];
    write_bits(&mut lc, 2, 6, 8);
    write_bits(&mut lc, 8, 8, 0);
    write_bits(&mut lc, 20, 3, 1);
    write_bits(&mut lc, 23, 25, ((12.5 / 360.0) * 2f64.powi(25)) as u32);
    write_bits(
        &mut lc,
        48,
        24,
        ((-33.75 / 180.0) * 2f64.powi(24)) as i32 as u32,
    );
    let frame = decoder.decode_lc(0, &lc);
    assert!((frame.lon.expect("longitude") - 12.5).abs() < 0.000_1);
    assert!((frame.lat.expect("latitude") + 33.75).abs() < 0.000_1);
    assert_eq!(frame.position_error_m, Some(20));
}

#[test]
fn reassembles_utf8_talker_alias() {
    let mut decoder = Decoder::new(DmrParams::default());
    let alias = b"SCANNER-ALIAS";
    let mut stream = Vec::new();
    for byte in alias {
        stream.extend((0..8).rev().map(|bit| byte >> bit & 1 == 1));
    }
    stream.resize(49 + 3 * 56, false);
    let mut completed = None;
    for flco in 4u8..=7 {
        let mut lc = [false; 72];
        write_bits(&mut lc, 2, 6, u32::from(flco));
        if flco == 4 {
            write_bits(&mut lc, 16, 2, 2);
            write_bits(&mut lc, 18, 5, alias.len() as u32);
            lc[24..72].copy_from_slice(&stream[..48]);
        } else {
            let start = 48 + usize::from(flco - 5) * 56;
            lc[16..72].copy_from_slice(&stream[start..start + 56]);
        }
        completed = decoder.decode_lc(0, &lc).talker_alias.or(completed);
    }
    assert_eq!(completed.as_deref(), Some("SCANNER-ALIAS"));
}

fn write_bits(target: &mut [bool], offset: usize, len: usize, value: u32) {
    for (index, bit) in target[offset..offset + len].iter_mut().enumerate() {
        *bit = value >> (len - 1 - index) & 1 == 1;
    }
}

#[test]
fn csbk_opcodes_are_the_specs_six_bit_binary_values() {
    for (opcode, name) in [
        (0b000100, "unit-to-unit voice service request"),
        (0b000101, "unit-to-unit voice service answer response"),
        (0b000111, "channel timing"),
        (0b100110, "negative acknowledge response"),
        (0b111000, "BS outbound activation"),
        (0b111101, "preamble"),
        (0b011001, "ALOHA"),
        (0b101111, "protect"),
        (0b110000, "private voice channel grant"),
        (0b110001, "talkgroup voice channel grant"),
    ] {
        assert_eq!(
            csbk_opcode_name(0, opcode, false),
            name,
            "opcode {opcode:06b}"
        );
    }
    assert_eq!(
        csbk_opcode_name(0, 56, true),
        "talkgroup data channel grant, multiple items",
        "opcode 56 kept its Tier II meaning on a Tier III control channel"
    );
}

#[test]
fn the_slot_filter_selects_what_is_reported() {
    let iq = tx::transmission(&tx::Call::default(), INPUT_RATE_HZ);
    assert!(!decode(&mut channel(DmrSlots::One), &iq).is_empty());
    assert!(decode(&mut channel(DmrSlots::Two), &iq).is_empty());
}

#[test]
fn repeater_cach_activates_the_slot_filter() {
    let call = tx::Call::default();
    let iq = tx::repeater_transmission(&call, 2, INPUT_RATE_HZ);
    assert!(decode(&mut channel(DmrSlots::One), &iq).is_empty());
    let frames = decode(&mut channel(DmrSlots::Two), &iq);
    assert!(!frames.is_empty());
    assert!(frames.iter().all(|frame| frame.slot == Some(2)));
}

#[test]
fn concurrent_repeater_slots_keep_independent_call_state() {
    let first = tx::Call {
        destination: 101,
        source: 1_000_001,
        ..tx::Call::default()
    };
    let second = tx::Call {
        destination: 202,
        source: 2_000_002,
        ..tx::Call::default()
    };
    let iq = tx::dual_slot_transmission(&first, &second, INPUT_RATE_HZ);
    let frames = decode(&mut channel(DmrSlots::Both), &iq);
    for (slot, source, destination) in [
        (1, first.source, first.destination),
        (2, second.source, second.destination),
    ] {
        let call = frames
            .iter()
            .find(|frame| frame.slot == Some(slot) && frame.source == Some(source))
            .unwrap_or_else(|| panic!("missing slot {slot} call: {frames:?}"));
        assert_eq!(call.destination, Some(destination));
    }
}

#[test]
fn concurrent_repeater_slots_yield_one_call_worth_of_audio() {
    let first = tx::Call {
        destination: 101,
        source: 1_000_001,
        ..tx::Call::default()
    };
    let second = tx::Call {
        destination: 202,
        source: 2_000_002,
        ..tx::Call::default()
    };
    let iq = tx::dual_slot_transmission(&first, &second, INPUT_RATE_HZ);
    let (frames, audio) = decode_audio(&iq);
    assert!(
        frames.iter().any(|frame| frame.slot == Some(2)),
        "the unheard slot stopped being reported"
    );
    let voice_frames = 18;
    assert!(
        (audio.len() as isize - (voice_frames * 960) as isize).abs() <= 2,
        "audio from both slots ran together: {} samples for {voice_frames} vocoder frames",
        audio.len()
    );
}

#[test]
fn a_slot_that_stops_hands_the_speaker_over() {
    let first = tx::Call::default();
    let second = tx::Call {
        destination: 202,
        source: 2_000_002,
        ..tx::Call::default()
    };
    let mut iq = tx::repeater_transmission(&first, 1, INPUT_RATE_HZ);
    iq.extend(tx::repeater_transmission(&second, 2, INPUT_RATE_HZ));
    let (_, audio) = decode_audio(&iq);
    let voice_frames = 36;
    assert!(
        (audio.len() as isize - (voice_frames * 960) as isize).abs() <= 4,
        "the second call never reached the speaker: {} samples",
        audio.len()
    );
}

#[test]
fn a_simplex_call_does_not_invent_a_timeslot() {
    let call = tx::Call::default();
    let iq = tx::simplex_transmission(&call, INPUT_RATE_HZ);
    let frames = decode(&mut channel(DmrSlots::Both), &iq);
    assert!(!frames.is_empty(), "an MS-sourced call decoded to nothing");
    assert!(
        frames.iter().all(|frame| frame.slot.is_none()),
        "guard time was read as a CACH and became a timeslot: {:?}",
        frames.iter().map(|f| f.slot).collect::<Vec<_>>()
    );
}

#[test]
fn a_simplex_call_keeps_every_burst_on_the_speaker() {
    let call = tx::Call::default();
    let iq = tx::simplex_transmission(&call, INPUT_RATE_HZ);
    let (_, audio) = decode_audio(&iq);
    let voice_frames = 18;
    assert!(
        (audio.len() as isize - (voice_frames * 960) as isize).abs() <= 2,
        "an invented timeslot took bursts off the speaker: {} samples",
        audio.len()
    );
}

#[test]
fn short_lc_reports_activity_on_both_slots() {
    let mut message = [false; 36];
    write_bits(&mut message, 0, 4, 1);
    write_bits(&mut message, 4, 4, 8);
    write_bits(&mut message, 8, 4, 10);
    write_bits(&mut message, 12, 8, 0xA5);
    write_bits(&mut message, 20, 8, 0x5A);
    let crc = crc8_dmr(&message[..28]);
    write_bits(&mut message, 28, 8, u32::from(crc));

    let mut matrix = [[false; 17]; 4];
    for row in 0..3 {
        matrix[row][..12].copy_from_slice(&message[row * 12..(row + 1) * 12]);
        ParityCode::HAMMING_17_12.encode(&mut matrix[row]);
    }
    for column in 0..17 {
        matrix[3][column] = matrix[..3]
            .iter()
            .fold(false, |parity, row| parity ^ row[column]);
    }
    let transmitted: Vec<bool> = (0..17)
        .flat_map(|column| (0..4).map(move |row| matrix[row][column]))
        .collect();
    let mut decoder = ShortLc::default();
    let mut frame = None;
    for (index, lcss) in [1, 3, 3, 2].into_iter().enumerate() {
        let payload: [bool; 17] = transmitted[index * 17..(index + 1) * 17]
            .try_into()
            .expect("CACH row");
        frame = decoder.push(lcss, payload).or(frame);
    }
    let frame = frame.expect("decoded Short LC");
    assert_eq!(frame.slot_activity.len(), 2);
    assert_eq!(frame.slot_activity[0].activity, "group voice");
    assert_eq!(frame.slot_activity[0].destination_hash, Some(0xA5));
    assert_eq!(frame.slot_activity[1].activity, "individual data");
    assert_eq!(frame.slot_activity[1].destination_hash, Some(0x5A));
}

#[test]
fn noise_decodes_to_nothing() {
    let noise = crate::testutil::complex_noise(9, 0.5, 400_000);
    assert!(decode(&mut channel(DmrSlots::Both), &noise).is_empty());
}

#[test]
fn retuning_forgets_the_call_in_progress() {
    let call = tx::Call::default();
    let iq = tx::transmission(&call, INPUT_RATE_HZ);
    let mut chan = channel(DmrSlots::Both);
    let mut out = ChannelOutputs::default();
    chan.process(&iq[..iq.len() / 2], &mut out);
    chan.retuned();
    out.reset();
    chan.process(&iq[iq.len() / 2..], &mut out);
    let frames: Vec<&DvFrame> = out
        .events
        .iter()
        .map(|event| {
            let DecoderEvent::Dv(frame) = event else {
                panic!("unexpected event")
            };
            frame
        })
        .collect();
    assert!(
        frames.iter().all(|f| f.kind != DvFrameKind::Voice),
        "an embedded link control was assembled across the retune: {frames:?}"
    );
}
