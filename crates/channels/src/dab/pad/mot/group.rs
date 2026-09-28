use super::super::crc_ok;

const EXTENSION_FLAG: u8 = 0x80;
const CRC_FLAG: u8 = 0x40;
const SEGMENT_FLAG: u8 = 0x20;
const USER_ACCESS_FLAG: u8 = 0x10;
const TYPE_MASK: u8 = 0x0f;
const TRANSPORT_ID_FLAG: u8 = 0x10;
const LENGTH_INDICATOR_MASK: u8 = 0x0f;
const LAST_SEGMENT: u16 = 0x8000;
const SEGMENT_NUMBER_MASK: u16 = 0x7fff;
const SEGMENT_SIZE_MASK: u16 = 0x1fff;
const MOT_HEADER: u8 = 3;
const MOT_BODY: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Header,
    Body,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentNumber {
    pub number: u16,
    pub last: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataGroup<'a> {
    pub kind: Kind,
    pub segment: Option<SegmentNumber>,
    pub transport_id: Option<u16>,
    pub data: &'a [u8],
}

impl<'a> DataGroup<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, &'static str> {
        let Some((&flags, _)) = bytes.split_first() else {
            return Err("Empty MOT data group");
        };
        let fields = checked_fields(bytes, flags & CRC_FLAG != 0)?;
        let rest = fields.get(2..).ok_or("Truncated MOT data group header")?;
        let rest = skip_extension(rest, flags & EXTENSION_FLAG != 0)?;
        let (segment, rest) = segment_field(rest, flags & SEGMENT_FLAG != 0)?;
        let (transport_id, data) = user_access(rest, flags & USER_ACCESS_FLAG != 0)?;
        Ok(Self {
            kind: kind(flags & TYPE_MASK),
            segment,
            transport_id,
            data,
        })
    }
}

pub fn segment_payload(data: &[u8]) -> Result<&[u8], &'static str> {
    let Some((&header, payload)) = data.split_first_chunk::<2>() else {
        return Err("Truncated MOT segmentation header");
    };
    let size = u16::from_be_bytes(header) & SEGMENT_SIZE_MASK;
    if usize::from(size) != payload.len() {
        return Err("MOT segment size mismatch");
    }
    Ok(payload)
}

fn kind(value: u8) -> Kind {
    match value {
        MOT_HEADER => Kind::Header,
        MOT_BODY => Kind::Body,
        _ => Kind::Unsupported,
    }
}

fn checked_fields(bytes: &[u8], has_crc: bool) -> Result<&[u8], &'static str> {
    if !has_crc {
        return Ok(bytes);
    }
    if !crc_ok(bytes) {
        return Err("MOT data-group CRC failure");
    }
    Ok(&bytes[..bytes.len() - 2])
}

fn skip_extension(bytes: &[u8], present: bool) -> Result<&[u8], &'static str> {
    if !present {
        return Ok(bytes);
    }
    bytes
        .get(2..)
        .ok_or("Truncated MOT data group extension field")
}

fn segment_field(
    bytes: &[u8],
    present: bool,
) -> Result<(Option<SegmentNumber>, &[u8]), &'static str> {
    if !present {
        return Ok((None, bytes));
    }
    let Some((&field, rest)) = bytes.split_first_chunk::<2>() else {
        return Err("Truncated MOT session header");
    };
    let field = u16::from_be_bytes(field);
    let segment = SegmentNumber {
        number: field & SEGMENT_NUMBER_MASK,
        last: field & LAST_SEGMENT != 0,
    };
    Ok((Some(segment), rest))
}

fn user_access(bytes: &[u8], present: bool) -> Result<(Option<u16>, &[u8]), &'static str> {
    if !present {
        return Ok((None, bytes));
    }
    let Some((&access, rest)) = bytes.split_first() else {
        return Err("Truncated MOT user access field");
    };
    let length = usize::from(access & LENGTH_INDICATOR_MASK);
    let (field, data) = rest
        .split_at_checked(length)
        .ok_or("Truncated MOT user access field")?;
    if access & TRANSPORT_ID_FLAG == 0 {
        return Ok((None, data));
    }
    let Some((&transport_id, _)) = field.split_first_chunk::<2>() else {
        return Err("MOT TransportId longer than its user access field");
    };
    Ok((Some(u16::from_be_bytes(transport_id)), data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_group_without_optional_fields() {
        let group = DataGroup::parse(&[0x04, 0x00, 0xaa]);
        assert_eq!(
            group,
            Ok(DataGroup {
                kind: Kind::Body,
                segment: None,
                transport_id: None,
                data: &[0xaa],
            })
        );
    }

    #[test]
    fn extension_segment_and_end_user_address_are_skipped() {
        let bytes = [
            0xb3, 0x00, 0xde, 0xad, 0x80, 0x05, 0x14, 0x00, 0x07, 0x01, 0x02, 0x99,
        ];
        let group = DataGroup::parse(&bytes);
        assert_eq!(
            group,
            Ok(DataGroup {
                kind: Kind::Header,
                segment: Some(SegmentNumber {
                    number: 5,
                    last: true
                }),
                transport_id: Some(7),
                data: &[0x99],
            })
        );
    }

    #[test]
    fn truncated_fields_are_errors() {
        assert!(DataGroup::parse(&[]).is_err());
        assert!(DataGroup::parse(&[0x24, 0x00, 0x01]).is_err());
        assert!(DataGroup::parse(&[0x14, 0x00, 0x13, 0x00]).is_err());
        assert!(DataGroup::parse(&[0x14, 0x00, 0x11, 0x00]).is_err());
    }

    #[test]
    fn other_group_types_are_unsupported() {
        for kind in [0x00, 0x05, 0x06, 0x07] {
            assert_eq!(
                DataGroup::parse(&[kind, 0]).map(|group| group.kind),
                Ok(Kind::Unsupported)
            );
        }
    }

    #[test]
    fn segmentation_header_size_must_match_the_segment() {
        assert_eq!(segment_payload(&[0xe0, 0x02, 1, 2]), Ok(&[1, 2][..]));
        assert!(segment_payload(&[0x00, 0x03, 1, 2]).is_err());
        assert!(segment_payload(&[0x00]).is_err());
    }
}
