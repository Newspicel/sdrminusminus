use sdrmm_dsp::crc16_msb;

use super::*;

const UTF_8: u8 = 0xf0;

fn group(high: u8, low: u8, data: &[u8]) -> Vec<u8> {
    let mut bytes = [&[high, low][..], data].concat();
    bytes.extend_from_slice(&(!crc16_msb(0x1021, 0xffff, &bytes)).to_be_bytes());
    bytes
}

fn characters(toggle: bool, first: bool, last: bool, field_2: u8, text: &[u8]) -> Vec<u8> {
    let flags = u8::from(toggle) << 7 | u8::from(first) << 6 | u8::from(last) << 5;
    group(flags | (text.len() as u8 - 1), field_2, text)
}

#[test]
fn prefix_fields_follow_figure_36() {
    let prefix = Prefix::read([0xe5, 0xf0]);
    assert!(prefix.toggle && prefix.first && prefix.last);
    assert_eq!(
        prefix.control,
        Control::Characters {
            length: 6,
            field_2: 0x0f
        }
    );
    assert_eq!(
        Prefix::read([0x11, 0x00]).control,
        Control::Command(Command::ClearDisplay)
    );
    assert_eq!(Prefix::read([0x12, 0x03]).data_length(), Some(4));
    assert_eq!(Prefix::read([0x1f, 0x00]).data_length(), None);
}

#[test]
fn segments_join_in_segment_number_order() {
    let mut label = Label::default();
    let segments = [
        characters(false, false, true, 0x20, b"three"),
        characters(false, true, false, UTF_8, b"one "),
        characters(false, false, false, 0x10, b"two "),
    ];
    let results: Vec<_> = segments
        .iter()
        .map(|segment| label.push(true, segment))
        .collect();
    assert_eq!(
        results,
        [Ok(None), Ok(None), Ok(Some("one two three".to_owned()))]
    );
}

#[test]
fn group_may_span_several_subfields_and_ignores_padding() {
    let mut label = Label::default();
    let segment = characters(false, true, true, UTF_8, b"Split label");
    let (head, tail) = segment.split_at(5);
    assert_eq!(label.push(true, head), Ok(None));
    let padded = [tail, &[0, 0, 0]].concat();
    assert_eq!(
        label.push(false, &padded),
        Ok(Some("Split label".to_owned()))
    );
    assert_eq!(label.push(false, &[1, 2, 3]), Ok(None));
}

#[test]
fn repetition_is_quiet_and_toggle_starts_a_new_message() {
    let mut label = Label::default();
    let first = characters(false, true, false, UTF_8, b"Old ");
    let second = characters(false, false, true, 0x10, b"text");
    label.push(true, &first).ok();
    assert_eq!(label.push(true, &second), Ok(Some("Old text".to_owned())));
    assert_eq!(label.push(true, &first), Ok(None));
    let fresh = characters(true, false, true, 0x10, b"news");
    assert_eq!(label.push(true, &fresh), Ok(None));
    let head = characters(true, true, false, UTF_8, b"New ");
    assert_eq!(label.push(true, &head), Ok(Some("New news".to_owned())));
}

#[test]
fn clear_command_blanks_the_display() {
    let mut label = Label::default();
    label
        .push(true, &characters(false, true, true, UTF_8, b"Shown"))
        .ok();
    assert_eq!(
        label.push(true, &group(0x91, 0, &[])),
        Ok(Some(String::new()))
    );
    assert_eq!(label.push(true, &group(0x91, 0, &[])), Ok(None));
}

#[test]
fn dl_plus_command_is_consumed_without_changing_the_text() {
    let mut label = Label::default();
    let command = group(0x12, 0x01, &[0xaa, 0xbb]);
    assert_eq!(label.push(true, &command), Ok(None));
}

#[test]
fn damaged_or_reserved_segments_are_rejected() {
    let mut label = Label::default();
    let mut damaged = characters(false, true, true, UTF_8, b"Bad");
    damaged[3] ^= 1;
    assert_eq!(
        label.push(true, &damaged),
        Err("DAB dynamic-label CRC failure")
    );
    assert!(label.push(true, &group(0x1f, 0, &[])).is_err());
    let segment_zero = characters(false, false, true, 0x00, b"x");
    assert!(label.push(true, &segment_zero).is_err());
}

#[test]
fn continuation_without_a_start_is_ignored() {
    let mut label = Label::default();
    let segment = characters(false, true, true, UTF_8, b"Orphan");
    assert_eq!(label.push(false, &segment), Ok(None));
}
