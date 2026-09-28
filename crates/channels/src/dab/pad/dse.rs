const DATA_STREAM_ELEMENT: u8 = 4;
const ESCAPE_COUNT: u8 = u8::MAX;
const FIXED_PAD_LENGTH: usize = 2;

pub fn pad_field(unit: &[u8]) -> Result<Option<&[u8]>, &'static str> {
    let Some((&first, rest)) = unit.split_first() else {
        return Ok(None);
    };
    if first >> 5 != DATA_STREAM_ELEMENT {
        return Ok(None);
    }
    let (count, rest) = match rest {
        [ESCAPE_COUNT, escape, rest @ ..] => {
            (usize::from(ESCAPE_COUNT) + usize::from(*escape), rest)
        }
        [count, rest @ ..] => (usize::from(*count), rest),
        [] => return Err("Truncated PAD data stream element"),
    };
    let field = rest
        .get(..count)
        .ok_or("Truncated PAD data stream element")?;
    Ok((field.len() >= FIXED_PAD_LENGTH).then_some(field))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leading_data_stream_element_carries_the_pad_field() {
        assert_eq!(
            pad_field(&[0x81, 3, 7, 8, 9, 0xaa]),
            Ok(Some(&[7, 8, 9][..]))
        );
    }

    #[test]
    fn escape_count_extends_the_length() {
        let mut unit = vec![0x80, 255, 5];
        unit.extend(std::iter::repeat_n(1, 260));
        assert_eq!(
            pad_field(&unit).map(|field| field.map(<[u8]>::len)),
            Ok(Some(260))
        );
    }

    #[test]
    fn missing_or_short_element_means_no_pad() {
        assert_eq!(pad_field(&[0x20, 1, 2]), Ok(None));
        assert_eq!(pad_field(&[0x80, 1, 2]), Ok(None));
        assert_eq!(pad_field(&[]), Ok(None));
        assert!(pad_field(&[0x80, 9, 1]).is_err());
    }
}
