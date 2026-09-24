use serde_json::json;

use super::*;

fn parse(parser: &mut PacketParser, frame: &[u8]) -> Vec<StdcPacket> {
    let mut out = Vec::new();
    parser.parse_frame(frame, &mut out);
    out
}

fn parse_padded(packet: &[u8]) -> Vec<StdcPacket> {
    let mut frame = packet.to_vec();
    frame.resize(640, 0);
    parse(&mut PacketParser::new(), &frame)
}

fn find<'a>(packets: &'a [StdcPacket], name: &str) -> &'a StdcPacket {
    packets
        .iter()
        .find(|packet| packet.name == name)
        .unwrap_or_else(|| panic!("{name} missing from {packets:?}"))
}

fn medium(mut body: Vec<u8>) -> Vec<u8> {
    body[1] = body.len() as u8;
    build_packet(&body)
}

fn egc_packet(
    descriptor: u8,
    service: u8,
    continuation: bool,
    priority: u8,
    sequence: (u16, u8),
    presentation: u8,
    text: &[u8],
) -> Vec<u8> {
    let address = vec![0xAB; egc_address_len(service)];
    egc_with_address(
        descriptor,
        service,
        continuation,
        priority,
        sequence,
        presentation,
        &address,
        text,
    )
}

#[allow(clippy::too_many_arguments)]
fn egc_with_address(
    descriptor: u8,
    service: u8,
    continuation: bool,
    priority: u8,
    sequence: (u16, u8),
    presentation: u8,
    address: &[u8],
    text: &[u8],
) -> Vec<u8> {
    let mut body = vec![
        descriptor,
        0,
        service,
        (u8::from(continuation) << 7) | (priority << 5) | 1,
    ];
    body.extend(sequence.0.to_be_bytes());
    body.push(sequence.1);
    body.push(presentation);
    body.extend_from_slice(address);
    body.extend_from_slice(text);
    medium(body)
}

#[test]
fn lcn_message_assembled_on_channel_clear() {
    let second = build_packet(&[&[0xAA, 11, 0x65, 7, 1][..], b" WORLD"].concat());
    let first = build_packet(&[&[0xAA, 10, 0x65, 7, 0][..], b"HELLO"].concat());
    let clear = build_packet(&[0x27, 0x01, 0x02, 0x03, 0x65, 7]);
    let out = parse_padded(&[second, first, clear].concat());
    let message = find(&out, "message");
    assert_eq!(message.text.as_deref(), Some("HELLO WORLD"));
    assert_eq!(message.details["lcn"], 7);
    assert_eq!(message.details["parts"], 2);
    assert_eq!(
        find(&out, "logical-channel-clear").details["mes_id"],
        "010203"
    );
}

#[test]
fn logical_channel_assignment_full_fields() {
    let out = parse_padded(&medium(vec![
        0x83, 0x00, 0xC1, 0x24, 0xBB, 0x44, 0x12, 0x21, 0x28, 0x0A, 0x20, 0xD0, 0x27, 0x48, 0x03,
        0xAA,
    ]));
    let assignment = find(&out, "logical-channel-assignment");
    assert_eq!(assignment.details["mes_id"], "C124BB");
    assert_eq!(assignment.details["sat_les"]["les"], 104);
    assert_eq!(assignment.details["status_bits"], 0x12);
    assert_eq!(assignment.details["lcn"], 0x21);
    assert_eq!(assignment.details["frame_length"], 0x28);
    assert_eq!(assignment.details["duration"], 0x0A);
    assert_eq!(assignment.details["downlink_mhz"], 1531.5);
    assert_eq!(assignment.details["uplink_mhz"], 1636.64);
    assert_eq!(assignment.details["frame_offset"], 0x03);
    assert_eq!(assignment.details["packet_descriptor1"], 0xAA);
}

#[test]
fn les_list_fields_and_stations() {
    let mut body = vec![0xAB, 0x00, 0x05, 0x02];
    body.extend([0x44, 0x01, 0x40, 0x20, 0x20, 0xD0]);
    body.extend([0x82, 0x02, 0x20, 0x00, 0x20, 0xD0]);
    let out = parse_padded(&medium(body));
    let list = find(&out, "les-list");
    assert_eq!(list.details["station_count"], 2);
    assert_eq!(list.details["station_start"], 0x05);
    assert_eq!(list.details["stations"][1]["sat_les"]["region"], "POR");
    assert_eq!(list.details["stations"][1]["sat_les"]["les"], 202);
}

#[test]
fn login_ack_fields_and_station_list() {
    let mut body = vec![0x92, 0x00, 0x12, 0x34, 0x56, 0x20, 0xD0, 0x77, 0x01];
    body.extend([0x44, 0x01, 0x40, 0x20, 0x20, 0xD0]);
    let out = parse_padded(&medium(body));
    let ack = find(&out, "login-ack");
    assert_eq!(ack.details["les"], "123456");
    assert_eq!(ack.details["downlink_mhz"], 1531.5);
    assert_eq!(ack.details["station_start"], 0x77);
    assert_eq!(ack.details["station_count"], 1);
    assert_eq!(ack.details["stations"][0]["sat_les"]["les"], 104);
}

#[test]
fn individual_poll_text_needs_a_long_packet() {
    let short = parse_padded(&build_packet(&[
        0xA3, 8, 0xC1, 0x24, 0xBB, 0x44, 0x01, 0x03,
    ]));
    let poll = find(&short, "individual-poll");
    assert_eq!(poll.details["mes_id"], "C124BB");
    assert_eq!(poll.text, None);
    let mut body = vec![0xA3, 0x00, 0xC1, 0x24, 0xBB, 0x44, 0, 0, 0, 0, 0, 0, 0];
    body.extend_from_slice(b"POLL ACK SHORT MESSAGE OK");
    let long = parse_padded(&medium(body));
    let poll = find(&long, "individual-poll");
    assert_eq!(poll.details["sat_les"]["region"], "AOR-E");
    assert_eq!(poll.text.as_deref(), Some("POLL ACK SHORT MESSAGE OK"));
}

#[test]
fn confirmation_short_message_text() {
    let mut body = vec![0xA8, 0x00, 0x12, 0x34, 0x56, 0x44, 0, 0, 0, 9, 0];
    body.extend_from_slice(b"CONFIRMED");
    let out = parse_padded(&medium(body));
    let confirmation = find(&out, "confirmation");
    assert_eq!(confirmation.details["mes_id"], "123456");
    assert_eq!(confirmation.details["short_message_len"], 9);
    assert_eq!(confirmation.text.as_deref(), Some("CONFIRMED"));
    let quiet = parse_padded(&medium(vec![
        0xA8, 0x00, 0x12, 0x34, 0x56, 0x44, 0, 0, 0, 0x01, 0, 0,
    ]));
    assert_eq!(find(&quiet, "confirmation").text, None);
}

#[test]
fn ack_request_routing_fields() {
    let out = parse_padded(&build_packet(&[0x08, 0x44, 0x21, 0x27, 0x48, 0x00, 0x00]));
    let request = find(&out, "ack-request");
    assert_eq!(request.details["sat_les"]["les"], 104);
    assert_eq!(request.details["lcn"], 0x21);
    assert_eq!(request.details["uplink_mhz"], 1636.64);
}

#[test]
fn egc_single_packet_message() {
    let frame = egc_packet(
        0xB0,
        0x31,
        false,
        1,
        (777, 1),
        0,
        b"NAVAREA XII WARNING TEST",
    );
    let events = parse(&mut PacketParser::new(), &frame);
    assert_eq!(events.len(), 1);
    let egc = &events[0];
    assert_eq!(egc.name, "egc-message");
    assert_eq!(egc.text.as_deref(), Some("NAVAREA XII WARNING TEST"));
    assert_eq!(egc.details["priority"], "safety");
    assert_eq!(egc.details["service"], "safetynet/navarea-warning");
    assert_eq!(egc.details["msg_seq"], 777);
    assert_eq!(egc.details["header_format"], "single-0xB0");
    assert_eq!(
        egc.details["service_long"],
        "SafetyNET, NAVAREA/METAREA Warning, MET Forecast or Piracy Warning to NAVAREA/METAREA"
    );
}

#[test]
fn egc_multi_packet_assembly() {
    let mut parser = PacketParser::new();
    let first = egc_packet(0xB0, 0x31, true, 0, (42, 1), 0, b"PART ONE ");
    let second = egc_packet(0xB0, 0x31, false, 0, (42, 2), 0, b"PART TWO");
    assert!(parse(&mut parser, &first).is_empty());
    let events = parse(&mut parser, &second);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].text.as_deref(), Some("PART ONE PART TWO"));
    assert_eq!(events[0].details["parts"], 2);
}

#[test]
fn egc_double_header_pair() {
    let mut parser = PacketParser::new();
    let header = egc_packet(0xB1, 0x31, true, 1, (200, 1), 0, b"DOUBLE ");
    let tail = egc_packet(0xB2, 0x31, false, 1, (200, 1), 0, b"HEADER");
    assert!(parse(&mut parser, &header).is_empty());
    let events = parse(&mut parser, &tail);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].text.as_deref(), Some("DOUBLE HEADER"));
    assert_eq!(events[0].details["header_format"], "double-0xB1+0xB2");
    assert_eq!(events[0].descriptor, 0xB1);
}

#[test]
fn multiple_packets_one_frame_with_padding() {
    let mut frame = build_packet(&[0x7D, 1, 0x03, 0xE8, 0, 0, 1, 0x10, 0, 0, 0, 0]);
    frame.extend(egc_packet(
        0xB0,
        0x00,
        false,
        3,
        (9, 1),
        0,
        b"DISTRESS RELAY",
    ));
    frame.extend([0u8; 32]);
    let events = parse(&mut PacketParser::new(), &frame);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].name, "bulletin-board");
    assert_eq!(events[1].details["priority"], "distress");
}

#[test]
fn bulletin_board_full_fields() {
    let out = parse_padded(&build_packet(&[
        0x7D, 0x01, 0x17, 0x63, 0x08, 0x30, 0x28, 0x44, 0x60, 0x40, 0x00, 0x05,
    ]));
    let board = find(&out, "bulletin-board");
    assert_eq!(board.details["frame_number"], 5987);
    assert_eq!(board.details["utc_time"], "14:22:07");
    assert_eq!(board.details["signalling_channel"], 2);
    assert_eq!(board.details["count"], 6);
    assert_eq!(board.details["channel_type"], 1);
    assert_eq!(board.details["channel_type_name"], "NCS");
    assert_eq!(board.details["local"], 2);
    assert_eq!(board.details["sat_les"]["les"], 104);
    assert_eq!(board.details["status"]["operational"], true);
    assert_eq!(board.details["services"], json!(["SafetyNet"]));
    assert_eq!(board.details["random_interval"], 5);
}

#[test]
fn signalling_channel_surfaces_services_and_tdm_slots() {
    let out = parse_padded(&build_packet(&[
        0x6C, 0xB4, 0x27, 0x48, 0x02, 0x00, 0x08, 0x00, 0x08, 0x00, 0x02,
    ]));
    let channel = find(&out, "signalling-channel");
    assert_eq!(channel.details["uplink_mhz"], 1636.64);
    assert_eq!(
        channel.details["services"],
        json!([
            "MaritimeDistressAlerting",
            "InmarsatC",
            "StoreFwd",
            "FullDuplex"
        ])
    );
    assert_eq!(channel.details["tdm_slots"][3], 2);
}

#[test]
fn egc_message_carries_decoded_circular_geometry() {
    let address = [0x14, 0x0E, 0x42, 0x81, 0x2C, 0x00, 0x00];
    let frame = egc_with_address(0xB0, 0x14, false, 3, (1, 1), 0, &address, b"DISTRESS RELAY");
    let events = parse(&mut PacketParser::new(), &frame);
    let geometry = &events[0].details["area"]["geometry"];
    assert_eq!(events[0].details["area"]["shape"], "circular");
    assert_eq!(geometry["center"]["lat_deg"], 14);
    assert_eq!(geometry["center"]["lon_deg"], -66);
    assert_eq!(geometry["radius_nm"], 300);
}

#[test]
fn egc_presentation_6_decodes_ita2() {
    let frame = egc_packet(0xB0, 0x00, false, 3, (1, 1), 6, &[0x05, 0x18, 0x05]);
    let events = parse(&mut PacketParser::new(), &frame);
    assert_eq!(events[0].text.as_deref(), Some("SOS"));
    assert_eq!(events[0].details["encoding"], "ita2");
}

#[test]
fn bad_checksum_packet_skipped() {
    let mut frame = egc_packet(0xB0, 0x31, false, 1, (5, 1), 0, b"GOOD");
    let last = frame.len() - 1;
    frame[last] ^= 0xFF;
    assert!(parse(&mut PacketParser::new(), &frame).is_empty());
}

#[test]
fn truncated_packets_never_panic() {
    for descriptor in [0xAA, 0xAB, 0x92] {
        for length in 3..12u8 {
            let mut body = vec![descriptor, length - 2];
            body.resize(usize::from(length) - 2, 0x92);
            let mut frame = build_packet(&body);
            frame.resize(640, 0);
            let _ = parse(&mut PacketParser::new(), &frame);
        }
    }
}
