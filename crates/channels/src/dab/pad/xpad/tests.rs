use super::*;

fn transmitted(logical: &[u8]) -> Vec<u8> {
    logical.iter().rev().copied().collect()
}

fn ranges(layout: &Layout) -> Vec<(Option<u8>, Range<usize>)> {
    layout
        .subfields()
        .map(|subfield| (subfield.app_type, subfield.range()))
        .collect()
}

#[test]
fn fixed_pad_signals_size_and_contents_indicator() {
    assert_eq!(
        FixedPad::read([0x10, 2]),
        FixedPad {
            size: Some(Size::Short),
            contents_indicated: true
        }
    );
    assert_eq!(FixedPad::read([0x20, 0]).size, Some(Size::Variable));
    assert_eq!(FixedPad::read([0x30, 0]).size, Some(Size::Reserved));
    assert_eq!(FixedPad::read([0x00, 0]).size, Some(Size::Absent));
    assert_eq!(FixedPad::read([0x60, 2]).size, None);
}

#[test]
fn short_field_holds_one_indicator_and_three_bytes() {
    let field = transmitted(&[0xe2, 1, 2, 3]);
    assert_eq!(
        ranges(&short(&field, true, true).unwrap()),
        [(Some(2), 1..4)]
    );
    assert_eq!(ranges(&short(&field, false, true).unwrap()), [(None, 0..4)]);
    assert!(short(&field[1..], true, true).is_err());
    let mut frame = vec![9, 9];
    frame.extend(&field);
    assert_eq!(
        ranges(&short(&frame, true, false).unwrap()),
        [(Some(2), 1..4)]
    );
}

#[test]
fn short_end_marker_carries_no_data() {
    let field = transmitted(&[0, 0, 0, 0]);
    assert_eq!(short(&field, true, true).unwrap(), Layout::default());
}

#[test]
fn variable_list_of_four_needs_no_end_marker() {
    let mut logical = vec![0x01, 0x2c, 0x42, 0x03];
    logical.resize(4 + 4 + 6 + 8 + 4, 0);
    let layout = variable(&transmitted(&logical), true).unwrap();
    assert_eq!(layout.field_length, 26);
    assert_eq!(
        ranges(&layout),
        [
            (Some(1), 4..8),
            (Some(12), 8..14),
            (Some(2), 14..22),
            (Some(3), 22..26)
        ]
    );
}

#[test]
fn variable_list_stops_at_end_marker() {
    let mut logical = vec![0xe0 | 12, 0xa0];
    logical.resize(2 + 48, 0);
    let layout = variable(&transmitted(&logical), true).unwrap();
    assert_eq!(ranges(&layout), [(Some(12), 2..50)]);
}

#[test]
fn variable_length_must_match_the_carrier() {
    let mut logical = vec![0x62, 0];
    logical.resize(2 + 12, 0);
    let field = transmitted(&logical);
    assert!(variable(&field[1..], true).is_err());
    let mut padded = field.clone();
    padded.insert(0, 0);
    assert!(variable(&padded, true).is_err());
    assert_eq!(variable(&padded, false).unwrap().field_length, 14);
}

#[test]
fn variable_leading_end_marker_means_no_data() {
    assert_eq!(variable(&[0, 0, 0, 0], true).unwrap(), Layout::default());
}

#[test]
fn continuation_reuses_the_previous_field_length() {
    assert_eq!(
        ranges(&continued(&[0; 12], Some(12), true).unwrap()),
        [(None, 0..12)]
    );
    assert!(continued(&[0; 8], Some(12), true).is_err());
    assert_eq!(
        ranges(&continued(&[0; 20], Some(12), false).unwrap()),
        [(None, 0..12)]
    );
    assert_eq!(continued(&[0; 12], None, true).unwrap(), Layout::default());
}

#[test]
fn application_types_follow_table_11() {
    let mot = Some(12);
    assert_eq!(
        Application::classify(1, mot),
        Application::LengthIndicator { start: true }
    );
    assert_eq!(
        Application::classify(2, mot),
        Application::Label { start: true }
    );
    assert_eq!(
        Application::classify(3, mot),
        Application::Label { start: false }
    );
    assert_eq!(
        Application::classify(12, mot),
        Application::DataGroup { start: true }
    );
    assert_eq!(
        Application::classify(13, mot),
        Application::DataGroup { start: false }
    );
    assert_eq!(Application::classify(12, None), Application::Other(12));
    assert_eq!(Application::classify(14, mot), Application::Other(14));
    assert_eq!(
        Application::Label { start: true }.continued(),
        Application::Label { start: false }
    );
}
