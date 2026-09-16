use sdrmm_dsp::crc16_msb;

use super::*;

fn packet(address: u16, continuity: u8, flags: u8, data: &[u8]) -> Vec<u8> {
    let length = (data.len() + 5).div_ceil(24) * 24;
    let mut bytes = vec![
        ((length / 24 - 1) as u8) << 6 | (continuity & 3) << 4 | flags << 2 | (address >> 8) as u8,
        address as u8,
        data.len() as u8,
    ];
    bytes.extend_from_slice(data);
    bytes.resize(length - 2, 0);
    bytes.extend_from_slice(&(!crc16_msb(0x1021, 0xffff, &bytes)).to_be_bytes());
    bytes
}

fn config(fec: bool) -> Config {
    Config {
        address: 17,
        kind: 5,
        data_groups: true,
        fec,
    }
}

fn frame(data: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let groups: Vec<_> = data.chunks(19).collect();
    for (i, part) in groups.iter().enumerate() {
        let flags = (u8::from(i == 0) * 2) | u8::from(i + 1 == groups.len());
        bytes.extend(packet(17, i as u8, flags, part));
    }
    while bytes.len() < APPLICATION {
        bytes.extend(packet(0, 0, 3, &[]));
    }
    let rs = ReedSolomon::new(DVB_PRIMITIVE, 0, 16);
    let mut parity = [0u8; 198];
    for row in 0..12 {
        let data: Vec<u8> = (0..188).map(|col| bytes[col * 12 + row]).collect();
        let mut word = Vec::new();
        rs.encode(&data, &mut word);
        for col in 0..16 {
            parity[col * 12 + row] = word[188 + col];
        }
    }
    for (i, field) in parity.as_chunks::<22>().0.iter().enumerate() {
        bytes.extend_from_slice(&[(i as u8) << 2 | 3, 254]);
        bytes.extend_from_slice(field);
    }
    bytes
}

#[test]
fn packet_mode_reassembles_groups_checks_continuity_and_ignores_other_addresses() {
    let mut decoder = PacketData::new(config(false));
    let mut events = Vec::new();
    decoder.push(&packet(17, 0, 2, b"\0\0first "), &mut events);
    decoder.push(&packet(18, 0, 3, b"other"), &mut events);
    decoder.push(&packet(17, 1, 1, b"last"), &mut events);
    assert!(matches!(&events[..], [Event::Object(data)] if data.bytes == b"first last"));
    events.clear();
    decoder.push(&packet(17, 2, 2, b"lost "), &mut events);
    decoder.push(&packet(17, 0, 1, b"last"), &mut events);
    assert!(matches!(
        &events[..],
        [Event::Error("DAB packet continuity gap")]
    ));
}

#[test]
fn packet_fec_repairs_bursts_before_packet_crc_and_preserves_fragment_boundaries() {
    let data: Vec<u8> = (0..200).collect();
    let clean = frame(&data);
    let mut damaged = clean.clone();
    for byte in &mut damaged[96..96 + 96] {
        *byte ^= 0x5a;
    }
    for fragment in [24, 264, 1152, 2472] {
        let mut decoder = PacketData::new(config(true));
        let mut events = Vec::new();
        for chunk in damaged.chunks(fragment) {
            decoder.push(chunk, &mut events);
        }
        assert!(
            matches!(&events[..], [Event::Object(object)] if object.bytes == data[2..]),
            "fragment {fragment}"
        );
        assert!(decoder.pending.is_empty());
    }
}

#[test]
fn uncorrectable_packet_fec_reports_failure_and_recovers_on_the_next_frame() {
    let data = b"\0\0A packet data service";
    let mut damaged = frame(data);
    for byte in &mut damaged[24..24 + 108] {
        *byte ^= 0x27;
    }
    let mut decoder = PacketData::new(config(true));
    let mut events = Vec::new();
    decoder.push(&damaged, &mut events);
    assert!(matches!(
        &events[..],
        [Event::Error("Uncorrectable DAB packet-mode FEC frame")]
    ));
    events.clear();
    decoder.push(&frame(data), &mut events);
    assert!(matches!(&events[..], [Event::Object(object)] if object.bytes == data[2..]));
}

#[test]
fn packet_crc_damage_never_delivers_a_partial_group() {
    let mut bytes = packet(17, 0, 3, b"payload");
    bytes[6] ^= 1;
    let mut decoder = PacketData::new(config(false));
    let mut events = Vec::new();
    decoder.push(&bytes, &mut events);
    assert!(matches!(
        &events[..],
        [Event::Error("DAB packet CRC failure")]
    ));
}

#[test]
fn ip_data_groups_strip_headers_and_reassemble_reordered_segments() {
    let mut decoder = PacketData::new(Config {
        address: 17,
        kind: 59,
        data_groups: true,
        fec: false,
    });
    let mut ip = vec![0u8; 44];
    ip[0] = 0x60;
    ip[5] = 4;
    ip[40..].copy_from_slice(b"test");
    let group = |index: u8, last: bool, part: &[u8]| {
        let mut bytes = vec![0x70, 0, if last { 128 } else { 0 }, index, 0x12, 0x12, 0x34];
        bytes.extend_from_slice(part);
        bytes.extend_from_slice(&(!crc16_msb(0x1021, 0xffff, &bytes)).to_be_bytes());
        bytes
    };
    let mut events = Vec::new();
    decoder.push(&packet(17, 0, 3, &group(1, true, &ip[24..])), &mut events);
    decoder.push(&packet(17, 1, 3, &group(0, false, &ip[..24])), &mut events);
    assert!(
        matches!(&events[..],[Event::Object(data)] if data.protocol==Some(0x86dd) && data.bytes==ip)
    );
}

#[test]
fn packet_boundaries_survive_arbitrary_worker_chunks_and_duplicate_delivery() {
    let bytes = packet(17, 0, 3, b"\0\0fragmented service object");
    for size in [1, 13, 24, 37] {
        let mut decoder = PacketData::new(config(false));
        let mut events = Vec::new();
        for chunk in bytes.chunks(size) {
            decoder.push(chunk, &mut events);
        }
        decoder.push(&bytes, &mut events);
        assert!(
            matches!(&events[..], [Event::Object(data)] if data.bytes == b"fragmented service object")
        );
        assert!(decoder.packet_pending.is_empty());
    }
}
