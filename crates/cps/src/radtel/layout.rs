use crate::{
    CpsError, Image, Region,
    bits::{is_blank, read_ascii, write_ascii},
};

pub const ERASED: u8 = 0xff;

#[derive(Clone, Copy, Debug)]
pub struct Table {
    pub name: &'static str,
    pub base: u32,
    pub stride: u32,
    pub count: u32,
}

impl Table {
    const fn new(name: &'static str, base: u32, stride: u32, count: u32) -> Self {
        Self {
            name,
            base,
            stride,
            count,
        }
    }

    pub const fn bytes(self) -> u32 {
        self.stride * self.count
    }

    pub const fn region(self) -> Region {
        Region::fixed(self.name, self.base, self.bytes())
    }

    pub fn record(self, image: &Image, index: u32) -> Option<&[u8]> {
        if index >= self.count {
            return None;
        }
        image.get(self.base + index * self.stride, self.stride as usize)
    }

    pub fn records(self, image: &Image) -> impl Iterator<Item = (usize, &[u8])> {
        (0..self.count)
            .map_while(move |index| self.record(image, index))
            .enumerate()
    }

    pub fn whole_mut(self, image: &mut Image) -> Result<&mut [u8], CpsError> {
        image.allocate(self.base, self.bytes(), ERASED);
        image
            .get_mut(self.base, self.bytes() as usize)
            .ok_or(CpsError::MissingRegion {
                addr: self.base,
                len: self.bytes() as usize,
            })
    }

    pub fn fill<T>(
        self,
        image: &mut Image,
        items: &[T],
        mut write: impl FnMut(&mut [u8], &T),
    ) -> Result<(), CpsError> {
        let stride = self.stride as usize;
        for (index, record) in self.whole_mut(image)?.chunks_exact_mut(stride).enumerate() {
            match items.get(index) {
                Some(item) => {
                    claim(record);
                    write(record, item);
                }
                None => release(record),
            }
        }
        Ok(())
    }
}

pub const SETTINGS: Table = Table::new("settings", 0x0_2000, 0x400, 1);
pub const CHANNELS: Table = Table::new("channels", 0x0_4000, 0x30, 1024);
pub const ZONES: Table = Table::new("zones", 0x1_e000, 0x208, 250);
pub const CONTACTS: Table = Table::new("contacts", 0x5_e000, 0x15, 10_000);
pub const GROUP_LISTS: Table = Table::new("group lists", 0xc_6000, 0x50, 250);

pub static REGIONS: [Region; 5] = [
    SETTINGS.region(),
    CHANNELS.region(),
    ZONES.region(),
    CONTACTS.region(),
    GROUP_LISTS.region(),
];

pub fn is_vacant(record: &[u8]) -> bool {
    record.iter().all(|byte| *byte == ERASED)
}

fn claim(record: &mut [u8]) {
    if is_vacant(record) {
        record.fill(0);
    }
}

fn release(record: &mut [u8]) {
    if !is_blank(record) {
        record.fill(ERASED);
    }
}

pub fn text(record: &[u8], at: usize, len: usize) -> String {
    read_ascii(record, at, len, ERASED)
}

pub fn put_text(record: &mut [u8], at: usize, len: usize, value: &str) {
    if let Some(field) = record.get_mut(at..at + len) {
        field.fill(ERASED);
    }
    write_ascii(record, at, value, len, ERASED);
}

#[derive(Clone, Debug, Default)]
pub struct SlotNames(Vec<Option<String>>);

impl SlotNames {
    pub fn remember(&mut self, slot: usize, name: &str) {
        if self.0.len() <= slot {
            self.0.resize(slot + 1, None);
        }
        self.0[slot] = Some(name.to_owned());
    }

    pub fn name(&self, slot: usize) -> Option<String> {
        self.0.get(slot).cloned().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filling_a_table_claims_used_slots_and_erases_the_rest() {
        let mut image = Image::new();
        GROUP_LISTS
            .fill(&mut image, &["A"], |record, name| {
                put_text(record, 0, 14, name);
            })
            .expect("fill");
        let first = GROUP_LISTS.record(&image, 0).expect("first");
        assert_eq!(&first[..2], &[b'A', ERASED]);
        assert_eq!(first[0x10], 0);
        assert!(is_vacant(GROUP_LISTS.record(&image, 1).expect("second")));
        assert_eq!(GROUP_LISTS.records(&image).count(), 250);
    }

    #[test]
    fn an_unread_table_has_no_records() {
        assert_eq!(CONTACTS.records(&Image::new()).count(), 0);
        assert!(CONTACTS.record(&Image::new(), 0).is_none());
    }

    #[test]
    fn slot_names_keep_gaps() {
        let mut names = SlotNames::default();
        names.remember(2, "C");
        assert_eq!(names.name(2).as_deref(), Some("C"));
        assert_eq!(names.name(0), None);
        assert_eq!(names.name(9), None);
    }
}
