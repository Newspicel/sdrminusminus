use super::*;

fn crc(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes.extend_from_slice(&(!crc16_msb(0x1021, 0xffff, &bytes)).to_be_bytes());
    bytes
}

fn xpad(kind: u8, data: &[u8]) -> Vec<u8> {
    let lengths = [4, 6, 8, 12, 16, 24, 32, 48];
    let index = lengths
        .iter()
        .position(|&length| length >= data.len())
        .unwrap();
    let mut bytes = vec![(index as u8) << 5 | kind, 0];
    bytes.extend_from_slice(data);
    bytes.resize(lengths[index] + 2, 0);
    bytes.reverse();
    bytes
}

#[test]
fn labels_reassemble_out_of_order_check_crc_and_remove() {
    let mut pad = Pad::default();
    let mut events = Vec::new();
    let tail = crc([&[0x24, 0x10][..], b"world"].concat());
    let first = crc([&[0x45, 0xf0][..], b"Hello "].concat());
    for segment in [&tail, &first] {
        pad.process(&xpad(2, segment), [0x20, 2], true, &mut events);
    }
    assert!(matches!(&events[..], [Event::Label(text)] if text == "Hello world"));
    events.clear();
    let mut damaged = first.clone();
    damaged[2] ^= 1;
    pad.process(&xpad(2, &damaged), [0x20, 2], true, &mut events);
    assert!(matches!(
        &events[..],
        [Event::Error("DAB dynamic-label CRC failure")]
    ));
    assert_eq!(pad.bad, 1);
    events.clear();
    pad.process(&xpad(2, &crc(vec![0x11, 0])), [0x20, 2], true, &mut events);
    assert!(matches!(&events[..], [Event::Label(text)] if text.is_empty()));
}

#[test]
fn omitted_ci_continues_only_the_immediately_preceding_subfield() {
    let mut pad = Pad::default();
    let mut events = Vec::new();
    let segment = crc([&[0x6a, 0xf0][..], b"Hello world"].concat());
    let mut first = vec![2];
    first.extend_from_slice(&segment[..3]);
    first.reverse();
    pad.process(&first, [0x10, 2], true, &mut events);
    for block in segment[3..].chunks(4) {
        let mut bytes = block.to_vec();
        bytes.resize(4, 0);
        bytes.reverse();
        pad.process(&bytes, [0x10, 0], true, &mut events);
    }
    assert!(matches!(&events[..], [Event::Label(text)] if text == "Hello world"));
    pad.process(&[], [0, 0], true, &mut events);
    assert!(pad.previous.is_none());
}

fn mot_group(kind: u8, index: u16, last: bool, data: &[u8]) -> Vec<u8> {
    let mut bytes = vec![
        0x70 | kind,
        0,
        ((index >> 8) as u8) | if last { 128 } else { 0 },
        index as u8,
        0x12,
        0x12,
        0x34,
    ];
    bytes.extend_from_slice(&(data.len() as u16).to_be_bytes());
    bytes.extend_from_slice(data);
    crc(bytes)
}

fn mot_header(body_size: usize) -> Vec<u8> {
    let mut bytes = vec![
        (body_size >> 20) as u8,
        (body_size >> 12) as u8,
        (body_size >> 4) as u8,
        (body_size << 4) as u8,
        10,
        0x02,
        0,
    ];
    bytes.extend_from_slice(&[0xcc, 11, 0xf0]);
    bytes.extend_from_slice(b"notice.txt");
    bytes
}

#[test]
fn mot_reassembles_reordered_segments_and_deduplicates_repetition() {
    let mut mot = mot::Mot::default();
    let body = b"This is the broadcast data service.";
    assert!(
        mot.push(&mot_group(4, 1, true, &body[10..]))
            .unwrap()
            .is_none()
    );
    assert!(
        mot.push(&mot_group(3, 0, true, &mot_header(body.len())))
            .unwrap()
            .is_none()
    );
    let object = mot
        .push(&mot_group(4, 0, false, &body[..10]))
        .unwrap()
        .unwrap();
    assert_eq!(object.name, "notice.txt");
    assert_eq!(object.media_type, "text/plain");
    assert_eq!(object.bytes, body);
    assert!(
        mot.push(&mot_group(4, 0, false, &body[..10]))
            .unwrap()
            .is_none()
    );
    assert!(mot.push(&mot_group(4, 0, false, b"different")).is_err());
}

#[test]
fn mot_length_indicator_and_access_unit_route_to_an_object() {
    let mut pad = Pad {
        mot_app: Some(12),
        ..Pad::default()
    };
    let mut events = Vec::new();
    for group in [
        mot_group(3, 0, true, &mot_header(5)),
        mot_group(4, 0, true, b"hello"),
    ] {
        let indicator = crc((group.len() as u16).to_be_bytes().to_vec());
        for (app, field) in [(1, indicator), (12, group)] {
            let mut data = xpad(app, &field);
            data.extend_from_slice(&[0x20, 2]);
            let mut unit = vec![0x81, data.len() as u8];
            unit.extend_from_slice(&data);
            pad.access_unit(&unit, &mut events);
        }
    }
    assert!(matches!(&events[..], [Event::Object(object)] if object.bytes == b"hello"));
    assert_eq!(pad.bad, 0);
    assert_eq!(pad.good, 2);
}

#[test]
fn malformed_pad_lengths_and_mot_crc_are_rejected() {
    let mut pad = Pad::default();
    let mut events = Vec::new();
    pad.process(&[0, 0xe2], [0x20, 2], true, &mut events);
    assert_eq!(pad.bad, 1);
    let mut group = mot_group(4, 0, true, b"payload");
    group[9] ^= 1;
    assert_eq!(
        mot::Mot::default().push(&group),
        Err("MOT data-group CRC failure")
    );
}
