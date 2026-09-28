use sdrmm_dsp::crc16_msb;

use super::*;

const HEADER: u8 = 3;
const BODY: u8 = 4;

fn group(kind: u8, transport_id: u16, number: u16, last: bool, data: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0x70 | kind, 0];
    bytes.extend_from_slice(&(number | if last { 0x8000 } else { 0 }).to_be_bytes());
    bytes.push(0x12);
    bytes.extend_from_slice(&transport_id.to_be_bytes());
    bytes.extend_from_slice(&(0x6000 | data.len() as u16).to_be_bytes());
    bytes.extend_from_slice(data);
    bytes.extend_from_slice(&(!crc16_msb(0x1021, 0xffff, &bytes)).to_be_bytes());
    bytes
}

fn header(body_size: usize, content: u8, subtype: u8, extension: &[u8]) -> Vec<u8> {
    let header_size = 7 + extension.len();
    let word = (body_size as u64) << 28
        | (header_size as u64) << 15
        | u64::from(content) << 9
        | u64::from(subtype);
    [&word.to_be_bytes()[1..], extension].concat()
}

fn named(body_size: usize) -> Vec<u8> {
    header(
        body_size,
        2,
        1,
        &[
            0xcc, 10, 0x40, b'p', b'h', b'o', b't', b'o', b'.', b'j', b'p', b'g',
        ],
    )
}

#[test]
fn body_segments_before_their_header_complete_the_object() {
    let mut mot = Mot::default();
    let body = b"0123456789abcdef";
    assert_eq!(mot.push(&group(BODY, 9, 2, true, &body[12..])), Ok(None));
    assert_eq!(mot.push(&group(BODY, 9, 0, false, &body[..6])), Ok(None));
    assert_eq!(
        mot.push(&group(HEADER, 9, 0, true, &named(body.len()))),
        Ok(None)
    );
    let object = mot.push(&group(BODY, 9, 1, false, &body[6..12]));
    let object = object.unwrap().unwrap();
    assert_eq!(object.name, "photo.jpg");
    assert_eq!(object.media_type, "image/jpeg");
    assert_eq!(object.bytes, body);
}

#[test]
fn header_split_over_segments_is_joined() {
    let mut mot = Mot::default();
    let header = named(2);
    let (head, tail) = header.split_at(5);
    assert_eq!(mot.push(&group(HEADER, 1, 1, true, tail)), Ok(None));
    assert_eq!(mot.push(&group(BODY, 1, 0, true, b"ok")), Ok(None));
    let object = mot.push(&group(HEADER, 1, 0, false, head)).unwrap();
    assert_eq!(object.map(|object| object.bytes), Some(b"ok".to_vec()));
}

#[test]
fn repeated_object_is_delivered_once() {
    let mut delivered = 0;
    let mut mot = Mot::default();
    for _ in 0..3 {
        for bytes in [
            group(HEADER, 4, 0, true, &named(1)),
            group(BODY, 4, 0, true, b"x"),
        ] {
            delivered += usize::from(matches!(mot.push(&bytes), Ok(Some(_))));
        }
    }
    assert_eq!(delivered, 1);
}

#[test]
fn new_transport_id_reports_an_unfinished_object() {
    let mut mot = Mot::default();
    mot.push(&group(HEADER, 1, 0, true, &named(4))).ok();
    assert_eq!(
        mot.push(&group(HEADER, 2, 0, true, &named(1))),
        Err("Incomplete MOT object replaced")
    );
    let object = mot.push(&group(BODY, 2, 0, true, b"y")).unwrap();
    assert!(object.is_some());
    assert_eq!(mot.push(&group(HEADER, 3, 0, true, &named(1))), Ok(None));
}

#[test]
fn damaged_group_or_missing_session_fields_are_rejected() {
    let mut mot = Mot::default();
    let mut damaged = group(BODY, 1, 0, true, b"data");
    damaged[10] ^= 0x01;
    assert_eq!(mot.push(&damaged), Err("MOT data-group CRC failure"));
    assert_eq!(
        mot.push(&[0x04, 0x00, 0xaa]),
        Err("MOT data group without segment number")
    );
    assert_eq!(
        mot.push(&[0x24, 0x00, 0x80, 0x00, 0x00, 0x00]),
        Err("MOT data group without TransportId")
    );
}

#[test]
fn unsupported_group_types_are_ignored() {
    let mut mot = Mot::default();
    for kind in [0, 5, 6, 7] {
        assert_eq!(mot.push(&group(kind, 1, 0, true, b"dir")), Ok(None));
    }
}

#[test]
fn unusable_objects_are_discarded_with_a_reason() {
    let cases: [(Vec<u8>, &str); 4] = [
        (named(3), "MOT body size mismatch"),
        (
            header(2, 0, 0, &[0x51, 1, 0xcc, 2, 0x40, b'z']),
            "Compressed MOT object not supported",
        ),
        (
            header(2, 0, 0, &[0xa3, 0, 0, 0, 0, 0xcc, 2, 0x40, b'z']),
            "Scrambled MOT object not supported",
        ),
        (header(2, 0, 0, &[]), "MOT header without ContentName"),
    ];
    for (header, reason) in cases {
        let mut mot = Mot::default();
        mot.push(&group(HEADER, 1, 0, true, &header)).ok();
        assert_eq!(mot.push(&group(BODY, 1, 0, true, b"ab")), Err(reason));
        assert_eq!(mot.push(&group(BODY, 1, 0, true, b"ab")), Ok(None));
    }
}

#[test]
fn mime_type_parameter_overrides_the_core_type() {
    let mut mot = Mot::default();
    let extension = [&[0xcc, 2, 0x40, b'a', 0xd0, 10][..], b"text/x-dls"].concat();
    mot.push(&group(HEADER, 1, 0, true, &header(1, 1, 0, &extension)))
        .ok();
    let object = mot.push(&group(BODY, 1, 0, true, b"t")).unwrap().unwrap();
    assert_eq!(object.media_type, "text/x-dls");
}
