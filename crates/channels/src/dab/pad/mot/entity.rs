use std::collections::BTreeMap;

use super::group::SegmentNumber;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Conflict;

#[derive(Debug, Default)]
pub struct Entity {
    segments: BTreeMap<u16, Vec<u8>>,
    last: Option<u16>,
}

impl Entity {
    pub fn store(&mut self, segment: SegmentNumber, data: &[u8]) -> Result<(), Conflict> {
        if self.contradicts(segment, data) {
            return Err(Conflict);
        }
        if segment.last {
            self.last = Some(segment.number);
        }
        self.segments
            .entry(segment.number)
            .or_insert_with(|| data.to_vec());
        Ok(())
    }

    pub fn assembled(&self) -> Option<Vec<u8>> {
        let last = self.last?;
        if self.segments.range(..=last).count() != usize::from(last) + 1 {
            return None;
        }
        (0..=last)
            .map(|number| self.segments.get(&number).map(Vec::as_slice))
            .collect::<Option<Vec<_>>>()
            .map(|parts| parts.concat())
    }

    fn contradicts(&self, segment: SegmentNumber, data: &[u8]) -> bool {
        let misplaced_end = match self.last {
            Some(last) if segment.last => last != segment.number,
            Some(last) => segment.number > last,
            None => {
                segment.last
                    && self
                        .segments
                        .last_key_value()
                        .is_some_and(|(&highest, _)| highest > segment.number)
            }
        };
        let changed = self
            .segments
            .get(&segment.number)
            .is_some_and(|stored| stored != data);
        misplaced_end || changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(number: u16, last: bool) -> SegmentNumber {
        SegmentNumber { number, last }
    }

    #[test]
    fn assembles_once_every_segment_up_to_the_last_arrived() {
        let mut entity = Entity::default();
        assert_eq!(entity.store(segment(2, true), b"c"), Ok(()));
        assert_eq!(entity.store(segment(0, false), b"a"), Ok(()));
        assert_eq!(entity.assembled(), None);
        assert_eq!(entity.store(segment(1, false), b"b"), Ok(()));
        assert_eq!(entity.assembled(), Some(b"abc".to_vec()));
    }

    #[test]
    fn identical_repetition_is_accepted() {
        let mut entity = Entity::default();
        entity.store(segment(0, true), b"a").ok();
        assert_eq!(entity.store(segment(0, true), b"a"), Ok(()));
    }

    #[test]
    fn changed_data_or_moved_end_conflicts() {
        let mut entity = Entity::default();
        entity.store(segment(1, true), b"b").ok();
        assert_eq!(entity.store(segment(1, true), b"x"), Err(Conflict));
        assert_eq!(entity.store(segment(2, false), b"c"), Err(Conflict));
        assert_eq!(entity.store(segment(0, true), b"a"), Err(Conflict));
        let mut early = Entity::default();
        early.store(segment(3, false), b"d").ok();
        assert_eq!(early.store(segment(1, true), b"b"), Err(Conflict));
    }
}
