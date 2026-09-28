use super::frame::PAGE_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub start: u32,
    pub len: u32,
}

impl Segment {
    const fn new(start: u32, len: u32) -> Self {
        Self { start, len }
    }

    const fn end(self) -> u32 {
        self.start + self.len
    }
}

pub const WRITABLE: [Segment; 9] = [
    Segment::new(0x0_2000, 0x0_0400),
    Segment::new(0x0_4000, 0x0_c000),
    Segment::new(0x1_c000, 0x0_0400),
    Segment::new(0x1_e000, 0x2_0000),
    Segment::new(0x5_e000, 0x3_4000),
    Segment::new(0xc_6000, 0x0_5000),
    Segment::new(0xd_0000, 0x0_3000),
    Segment::new(0xd_6000, 0x0_1000),
    Segment::new(0xf_0000, 0x0_1000),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub segment: u8,
    pub page: u16,
}

pub fn place(addr: u32, len: usize) -> Option<Placement> {
    let page_bytes = PAGE_BYTES as u32;
    let len = u32::try_from(len).ok()?;
    if len == 0 || len > page_bytes {
        return None;
    }
    let (number, segment) = WRITABLE
        .iter()
        .enumerate()
        .find(|(_, segment)| addr >= segment.start && addr < segment.end())?;
    let offset = addr - segment.start;
    if !offset.is_multiple_of(page_bytes) || addr.checked_add(len)? > segment.end() {
        return None;
    }
    Some(Placement {
        segment: u8::try_from(number).ok()?,
        page: u16::try_from(offset / page_bytes).ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_maps_to_its_segment_and_page() {
        assert_eq!(
            place(0x2000, 16),
            Some(Placement {
                segment: 0,
                page: 0
            })
        );
        assert_eq!(
            place(0x4800, PAGE_BYTES),
            Some(Placement {
                segment: 1,
                page: 2
            })
        );
        assert_eq!(
            place(0xf_0c00, 1),
            Some(Placement {
                segment: 8,
                page: 3
            })
        );
    }

    #[test]
    fn gaps_misalignment_and_overruns_have_no_placement() {
        assert_eq!(place(0x0000, 16), None);
        assert_eq!(place(0x1_c400, 16), None);
        assert_eq!(place(0x4010, 16), None);
        assert_eq!(place(0x2000, PAGE_BYTES + 1), None);
        assert_eq!(place(0xf_0c00, 0), None);
    }
}
