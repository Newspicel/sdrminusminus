use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionKind {
    Fixed,
    Sparse { stride: u32, stop_after_empty: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub name: &'static str,
    pub addr: u32,
    pub len: u32,
    pub kind: RegionKind,
}

impl Region {
    #[must_use]
    pub const fn fixed(name: &'static str, addr: u32, len: u32) -> Self {
        Self {
            name,
            addr,
            len,
            kind: RegionKind::Fixed,
        }
    }

    #[must_use]
    pub const fn sparse(
        name: &'static str,
        addr: u32,
        len: u32,
        stride: u32,
        stop_after_empty: u32,
    ) -> Self {
        Self {
            name,
            addr,
            len,
            kind: RegionKind::Sparse {
                stride,
                stop_after_empty,
            },
        }
    }

    #[must_use]
    pub const fn end(&self) -> u32 {
        self.addr.saturating_add(self.len)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Image {
    segments: BTreeMap<u32, Vec<u8>>,
}

impl Image {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allocate(&mut self, addr: u32, len: u32, fill: u8) {
        let len = len as usize;
        if len == 0 || self.get(addr, len).is_some() {
            return;
        }
        let next_start = self
            .segments
            .range(addr.saturating_add(1)..)
            .next()
            .map(|(start, _)| *start);
        if let Some((start, segment)) = self.segments.range_mut(..=addr).next_back() {
            let offset = (addr - *start) as usize;
            let wanted = offset + len;
            let room =
                next_start.is_none_or(|next| u64::from(*start) + wanted as u64 <= u64::from(next));
            if offset <= segment.len() && room {
                segment.resize(wanted, fill);
                return;
            }
        }
        self.segments.insert(addr, vec![fill; len]);
    }

    pub fn put(&mut self, addr: u32, data: &[u8]) {
        let Some((start, segment)) = self.segment_for_mut(addr, data.len()) else {
            self.segments.insert(addr, data.to_vec());
            return;
        };
        let offset = (addr - start) as usize;
        segment[offset..offset + data.len()].copy_from_slice(data);
    }

    #[must_use]
    pub fn get(&self, addr: u32, len: usize) -> Option<&[u8]> {
        let (start, segment) = self.segment_for(addr, len)?;
        let offset = (addr - start) as usize;
        segment.get(offset..offset + len)
    }

    #[must_use]
    pub fn get_mut(&mut self, addr: u32, len: usize) -> Option<&mut [u8]> {
        let (start, segment) = self.segment_for_mut(addr, len)?;
        let offset = (addr - start) as usize;
        segment.get_mut(offset..offset + len)
    }

    pub fn segments(&self) -> impl Iterator<Item = (u32, &[u8])> {
        self.segments
            .iter()
            .map(|(addr, data)| (*addr, data.as_slice()))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.segments.values().map(Vec::len).sum()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len() + self.segments.len() * 8);
        for (addr, data) in &self.segments {
            out.extend_from_slice(&addr.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(data);
        }
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let mut image = Self::new();
        let mut cursor = 0usize;
        while cursor < bytes.len() {
            let header = bytes.get(cursor..cursor + 8)?;
            let addr = u32::from_le_bytes(header[..4].try_into().ok()?);
            let len = u32::from_le_bytes(header[4..].try_into().ok()?) as usize;
            cursor += 8;
            let data = bytes.get(cursor..cursor + len)?;
            image.segments.insert(addr, data.to_vec());
            cursor += len;
        }
        Some(image)
    }

    fn segment_for(&self, addr: u32, len: usize) -> Option<(u32, &Vec<u8>)> {
        let (start, segment) = self.segments.range(..=addr).next_back()?;
        let offset = (addr - start) as usize;
        (offset + len <= segment.len()).then_some((*start, segment))
    }

    fn segment_for_mut(&mut self, addr: u32, len: usize) -> Option<(u32, &mut Vec<u8>)> {
        let (start, segment) = self.segments.range_mut(..=addr).next_back()?;
        let offset = (addr - *start) as usize;
        let start = *start;
        (offset + len <= segment.len()).then_some((start, segment))
    }
}

#[must_use]
pub fn changed_blocks(before: &Image, after: &Image, block: u32) -> Vec<(u32, Vec<u8>)> {
    let mut blocks = Vec::new();
    for (addr, data) in after.segments() {
        let mut offset = 0u32;
        while (offset as usize) < data.len() {
            let take = block.min(data.len() as u32 - offset) as usize;
            let Some(slice) = data.get(offset as usize..offset as usize + take) else {
                break;
            };
            let at = addr + offset;
            if before.get(at, take) != Some(slice) {
                blocks.push((at, slice.to_vec()));
            }
            offset += take as u32;
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocating_inside_a_segment_grows_it_instead_of_shadowing_it() {
        let mut image = Image::new();
        image.allocate(0x100, 32, 0);
        image.allocate(0x110, 64, 0xff);
        assert_eq!(image.segments().count(), 1);
        assert_eq!(image.get(0x100, 0x50).map(<[u8]>::len), Some(0x50));
        assert_eq!(image.get(0x100, 0x51), None);
        image.allocate(0x400, 16, 0x11);
        image.allocate(0x300, 16, 0x22);
        assert_eq!(image.segments().count(), 3);
        assert_eq!(image.get(0x400, 16), Some([0x11; 16].as_slice()));
    }

    #[test]
    fn allocated_segments_answer_reads_and_writes_by_address() {
        let mut image = Image::new();
        image.allocate(0x1000, 32, 0xff);
        assert_eq!(image.get(0x1000, 4), Some([0xff; 4].as_slice()));
        image.put(0x1004, &[1, 2, 3, 4]);
        assert_eq!(image.get(0x1004, 4), Some([1, 2, 3, 4].as_slice()));
        assert_eq!(image.get(0x2000, 1), None);
    }

    #[test]
    fn a_read_that_runs_past_a_segment_is_refused_rather_than_truncated() {
        let mut image = Image::new();
        image.allocate(0, 16, 0);
        assert!(image.get(8, 16).is_none());
        assert!(image.get_mut(8, 8).is_some());
    }

    #[test]
    fn the_wire_form_round_trips_every_segment() {
        let mut image = Image::new();
        image.allocate(0x40, 8, 0xaa);
        image.allocate(0x2000, 4, 0x55);
        let restored = Image::from_bytes(&image.to_bytes()).expect("round trip");
        assert_eq!(restored, image);
    }

    #[test]
    fn only_the_blocks_that_differ_are_reported_for_writing() {
        let mut before = Image::new();
        before.allocate(0x100, 64, 0);
        let mut after = before.clone();
        if let Some(slot) = after.get_mut(0x120, 1) {
            slot[0] = 9;
        }
        let blocks = changed_blocks(&before, &after, 16);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].0, 0x120);
        assert_eq!(blocks[0].1.len(), 16);
    }
}
