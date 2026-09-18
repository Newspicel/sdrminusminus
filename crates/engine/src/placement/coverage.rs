#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct Coverage(Vec<u64>);

impl Coverage {
    pub(super) fn empty(count: usize) -> Self {
        Self(vec![0; count.div_ceil(64)])
    }

    pub(super) fn insert(&mut self, index: usize) {
        self.0[index / 64] |= 1 << (index % 64);
    }

    pub(super) fn contains(&self, index: usize) -> bool {
        self.0[index / 64] & (1 << (index % 64)) != 0
    }

    pub(super) fn count(&self) -> usize {
        self.0.iter().map(|word| word.count_ones() as usize).sum()
    }

    pub(super) fn union(&self, other: &Self) -> Self {
        Self(self.0.iter().zip(&other.0).map(|(a, b)| a | b).collect())
    }

    pub(super) fn gain(&self, covered: &Self) -> usize {
        self.0
            .iter()
            .zip(&covered.0)
            .map(|(a, b)| (a & !b).count_ones() as usize)
            .sum()
    }

    pub(super) fn subset_of(&self, other: &Self) -> bool {
        self.0.iter().zip(&other.0).all(|(a, b)| a & !b == 0)
    }
}
