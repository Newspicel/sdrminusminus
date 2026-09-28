use super::Prefix;

const MAX_SEGMENTS: usize = 8;
const SEGMENT_NUMBER_MASK: u8 = 0x07;

#[derive(Default)]
pub struct Message {
    toggle: Option<bool>,
    charset: u8,
    segments: [Option<Vec<u8>>; MAX_SEGMENTS],
    last: Option<usize>,
}

impl Message {
    pub fn insert(
        &mut self,
        prefix: Prefix,
        field_2: u8,
        data: &[u8],
    ) -> Result<Option<(Vec<u8>, u8)>, &'static str> {
        let index = segment_index(prefix.first, field_2)?;
        if self.toggle != Some(prefix.toggle) || self.conflicts(index, data) {
            *self = Self {
                toggle: Some(prefix.toggle),
                ..Self::default()
            };
        }
        if prefix.first {
            self.charset = field_2;
        }
        if prefix.last {
            self.last = Some(index);
        }
        self.segments[index] = Some(data.to_vec());
        Ok(self.complete().map(|bytes| (bytes, self.charset)))
    }

    fn conflicts(&self, index: usize, data: &[u8]) -> bool {
        self.segments[index]
            .as_deref()
            .is_some_and(|stored| stored != data)
    }

    fn complete(&self) -> Option<Vec<u8>> {
        let last = self.last?;
        self.segments[..=last]
            .iter()
            .map(Option::as_deref)
            .collect::<Option<Vec<_>>>()
            .map(|parts| parts.concat())
    }
}

fn segment_index(first: bool, field_2: u8) -> Result<usize, &'static str> {
    if first {
        return Ok(0);
    }
    match field_2 & SEGMENT_NUMBER_MASK {
        0 => Err("Reserved dynamic-label segment number"),
        number => Ok(usize::from(number)),
    }
}
