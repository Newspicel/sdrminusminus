use super::content::Category;
use super::*;

fn core(body_size: usize, header_size: usize, content: usize, subtype: usize) -> Vec<u8> {
    let word = (body_size as u64) << 28
        | (header_size as u64) << 15
        | (content as u64) << 9
        | subtype as u64;
    word.to_be_bytes()[1..].to_vec()
}

fn header(body_size: usize, content: usize, subtype: usize, extension: &[u8]) -> Vec<u8> {
    [
        core(body_size, 7 + extension.len(), content, subtype),
        extension.to_vec(),
    ]
    .concat()
}

#[test]
fn core_fields_follow_figure_20() {
    let parsed = Header::parse(&header(0x0abc_def1, 2, 3, &[])).unwrap();
    assert_eq!(parsed.body_size, 0x0abc_def1);
    assert_eq!(
        parsed.content_type,
        ContentType {
            category: Category::Image,
            subtype: 3
        }
    );
    assert_eq!(parsed.media_type(), "image/png");
    assert_eq!(parsed.content_name, None);
}

#[test]
fn every_parameter_length_indicator_is_walked() {
    let long_name = [&[0x40][..], &[b'n'; 200]].concat();
    let mut extension = vec![0x01, 0x45, 0x07, 0x85, 1, 2, 3, 4, 0xcc, 0x80];
    extension.push(long_name.len() as u8);
    extension.extend_from_slice(&long_name);
    extension.extend_from_slice(&[0xd0, 9]);
    extension.extend_from_slice(b"image/gif");
    let parsed = Header::parse(&header(1, 0, 0, &extension)).unwrap();
    assert_eq!(parsed.content_name, Some("n".repeat(200)));
    assert_eq!(parsed.mime_type.as_deref(), Some("image/gif"));
    assert_eq!(parsed.media_type(), "image/gif");
    assert!(!parsed.compressed && !parsed.scrambled);
}

#[test]
fn short_data_field_length_and_latin_1_name() {
    let parsed = Header::parse(&header(1, 1, 0, &[0xcc, 3, 0x40, b'M', 0xfc])).unwrap();
    assert_eq!(parsed.content_name.as_deref(), Some("M\u{fc}"));
    assert_eq!(parsed.media_type(), "text/plain");
}

#[test]
fn compression_and_conditional_access_are_flagged() {
    let parsed = Header::parse(&header(1, 0, 0, &[0x51, 1, 0xa3, 0, 0, 0, 0])).unwrap();
    assert!(parsed.compressed && parsed.scrambled);
}

#[test]
fn inconsistent_headers_are_rejected() {
    let mut bytes = header(1, 0, 0, &[0xcc, 2, 0xf0, b'a']);
    bytes.push(0);
    assert_eq!(Header::parse(&bytes), Err("MOT header size mismatch"));
    assert!(Header::parse(&header(1, 0, 0, &[0xcc, 9, 0xf0])).is_err());
    assert!(Header::parse(&header(1, 0, 0, &[0xcc, 0x80])).is_err());
    assert!(Header::parse(&header(1, 0, 0, &[0x4c, 0x70])).is_err());
    assert!(Header::parse(&[0; 6]).is_err());
}

#[test]
fn unregistered_types_fall_back_to_octet_stream() {
    for (content, subtype) in [(0, 0), (2, 0), (7, 5), (63, 1), (20, 0)] {
        assert_eq!(
            ContentType::new(content, subtype).media_type(),
            "application/octet-stream"
        );
    }
}
