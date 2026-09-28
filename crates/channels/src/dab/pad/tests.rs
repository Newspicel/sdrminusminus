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
fn mot_text_subtypes_follow_ts_101_756() {
    for (subtype, media_type) in [
        (0, "text/plain"),
        (1, "text/plain; charset=iso-8859-1"),
        (2, "text/html"),
    ] {
        let mut mot = mot::Mot::default();
        let body = b"text";
        let mut header = mot_header(body.len());
        header[6] = subtype;
        assert!(mot.push(&mot_group(3, 0, true, &header)).unwrap().is_none());
        let object = mot.push(&mot_group(4, 0, true, body)).unwrap().unwrap();
        assert_eq!(object.media_type, media_type);
    }
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

const VARIABLE_CI: [u8; 2] = [0x20, 2];
const VARIABLE_NO_CI: [u8; 2] = [0x20, 0];
const SHORT_CI: [u8; 2] = [0x10, 2];
const SHORT_NO_CI: [u8; 2] = [0x10, 0];

fn field(subfields: &[(u8, &[u8])]) -> Vec<u8> {
    let lengths = [4, 6, 8, 12, 16, 24, 32, 48];
    let mut list = Vec::new();
    let mut data = Vec::new();
    for &(app, bytes) in subfields {
        let index = lengths
            .iter()
            .position(|&length| length >= bytes.len())
            .unwrap();
        list.push((index as u8) << 5 | app);
        let start = data.len();
        data.extend_from_slice(bytes);
        data.resize(start + lengths[index], 0);
    }
    if list.len() < 4 {
        list.push(0);
    }
    list.extend(data);
    list.reverse();
    list
}

fn indicator(group: &[u8]) -> Vec<u8> {
    crc((group.len() as u16).to_be_bytes().to_vec())
}

fn label_segment(text: &[u8]) -> Vec<u8> {
    crc([&[0x60 | (text.len() as u8 - 1), 0][..], text].concat())
}

fn short_frames(app: u8, data: &[u8]) -> Vec<(Vec<u8>, [u8; 2])> {
    let mut frames = vec![([&[app][..], &data[..3.min(data.len())]].concat(), SHORT_CI)];
    for block in data.get(3..).unwrap_or_default().chunks(4) {
        frames.push((block.to_vec(), SHORT_NO_CI));
    }
    frames
        .into_iter()
        .map(|(mut bytes, fpad)| {
            bytes.resize(4, 0);
            bytes.reverse();
            (bytes, fpad)
        })
        .collect()
}

fn objects(events: &[Event]) -> Vec<&[u8]> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Object(object) => Some(&object.bytes[..]),
            _ => None,
        })
        .collect()
}

#[test]
fn several_contents_indicators_share_one_variable_field() {
    let mut pad = Pad {
        mot_app: Some(12),
        ..Pad::default()
    };
    let mut events = Vec::new();
    let header = mot_group(3, 0, true, &mot_header(2));
    let body = mot_group(4, 0, true, b"hi");
    let label = label_segment(b"Now");
    let first = field(&[(1, &indicator(&header)), (12, &header), (2, &label)]);
    pad.process(&first, VARIABLE_CI, true, &mut events);
    let second = field(&[(1, &indicator(&body)), (12, &body)]);
    pad.process(&second, VARIABLE_CI, true, &mut events);
    assert!(matches!(&events[0], Event::Label(text) if text == "Now"));
    assert_eq!(objects(&events), [b"hi"]);
    assert_eq!((pad.good, pad.bad), (3, 0));
}

#[test]
fn short_xpad_carries_length_indicator_then_data_group() {
    let mut pad = Pad {
        mot_app: Some(12),
        ..Pad::default()
    };
    let mut events = Vec::new();
    for group in [
        mot_group(3, 0, true, &mot_header(5)),
        mot_group(4, 0, true, b"short"),
    ] {
        let frames = short_frames(1, &indicator(&group))
            .into_iter()
            .chain(short_frames(12, &group));
        for (bytes, fpad) in frames {
            pad.process(&bytes, fpad, true, &mut events);
        }
    }
    assert_eq!(objects(&events), [b"short"]);
    assert_eq!(pad.bad, 0);
}

#[test]
fn interrupted_data_group_resumes_with_continuation_type() {
    let mut pad = Pad {
        mot_app: Some(12),
        ..Pad::default()
    };
    let mut events = Vec::new();
    let header = mot_group(3, 0, true, &mot_header(20));
    let body = mot_group(4, 0, true, b"twenty bytes of body");
    for group in [&header, &body] {
        let (head, tail) = group.split_at(16);
        pad.process(
            &field(&[(1, &indicator(group)), (12, head)]),
            VARIABLE_CI,
            true,
            &mut events,
        );
        pad.process(
            &field(&[(2, &label_segment(b"Hi"))]),
            VARIABLE_CI,
            true,
            &mut events,
        );
        pad.process(&field(&[(13, tail)]), VARIABLE_CI, true, &mut events);
    }
    assert_eq!(objects(&events), [b"twenty bytes of body"]);
    assert!(matches!(&events[0], Event::Label(text) if text == "Hi"));
    assert_eq!(pad.bad, 0);
}

#[test]
fn length_indicator_must_directly_precede_its_data_group() {
    let mut pad = Pad {
        mot_app: Some(12),
        ..Pad::default()
    };
    let mut events = Vec::new();
    let group = mot_group(4, 0, true, b"x");
    let label = label_segment(b"Hi");
    let bytes = field(&[(1, &indicator(&group)), (2, &label), (12, &group)]);
    pad.process(&bytes, VARIABLE_CI, true, &mut events);
    assert!(events.iter().any(|event| matches!(
        event,
        Event::Error("X-PAD MSC data group without length indicator")
    )));
}

#[test]
fn end_marker_only_field_carries_nothing_and_ends_continuation() {
    let mut pad = Pad::default();
    let mut events = Vec::new();
    let segment = label_segment(b"Hello world");
    let (head, tail) = segment.split_at(8);
    pad.process(&field(&[(2, head)]), VARIABLE_CI, true, &mut events);
    pad.process(&[0; 8], VARIABLE_CI, true, &mut events);
    assert!(pad.previous.is_none());
    let mut rest = tail.to_vec();
    rest.resize(8, 0);
    rest.reverse();
    pad.process(&rest, VARIABLE_NO_CI, true, &mut events);
    assert!(events.is_empty());
    assert_eq!(pad.bad, 0);
}

#[test]
fn omitted_contents_indicator_continues_with_previous_field_length() {
    let mut pad = Pad::default();
    let mut events = Vec::new();
    let segment = label_segment(b"Hello world");
    let (head, tail) = segment.split_at(8);
    pad.process(
        &[vec![0xaa; 5], field(&[(2, head)])].concat(),
        VARIABLE_CI,
        false,
        &mut events,
    );
    let mut rest = tail.to_vec();
    rest.resize(10, 0);
    rest.reverse();
    pad.process(
        &[vec![0xbb; 3], rest].concat(),
        VARIABLE_NO_CI,
        false,
        &mut events,
    );
    assert!(matches!(&events[..], [Event::Label(text)] if text == "Hello world"));
}

#[test]
fn reserved_fpad_type_keeps_the_last_xpad_indicator() {
    let mut pad = Pad::default();
    let mut events = Vec::new();
    pad.process(&[], VARIABLE_CI, true, &mut events);
    let bytes = field(&[(2, &label_segment(b"Kept"))]);
    pad.process(&bytes, [0x40, 2], true, &mut events);
    assert!(matches!(&events[..], [Event::Label(text)] if text == "Kept"));
}

#[test]
fn contents_indicator_length_disagreeing_with_dse_is_discarded() {
    let mut pad = Pad::default();
    let mut events = Vec::new();
    let mut bytes = field(&[(2, &label_segment(b"Lost"))]);
    bytes.insert(0, 0);
    pad.process(&bytes, VARIABLE_CI, true, &mut events);
    assert!(matches!(
        &events[..],
        [Event::Error(
            "X-PAD length disagrees with contents indicators"
        )]
    ));
    assert!(pad.previous.is_none());
}
