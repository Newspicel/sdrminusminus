#[derive(Clone, Copy, Debug)]
pub struct PrbsSpec {
    pub stages: u32,
    pub taps: u32,
    pub seed: u32,
}

pub const DAB_DISPERSAL: PrbsSpec = PrbsSpec {
    stages: 9,
    taps: 0x110,
    seed: 0x1FF,
};

pub const DVB_DISPERSAL: PrbsSpec = PrbsSpec {
    stages: 15,
    taps: 0x6000,
    seed: 0x00A9,
};

#[derive(Clone, Debug)]
pub struct Prbs {
    spec: PrbsSpec,
    register: u32,
    mask: u32,
}

impl Prbs {
    #[must_use]
    pub fn new(spec: PrbsSpec) -> Self {
        Self {
            spec,
            register: spec.seed,
            mask: (1 << spec.stages) - 1,
        }
    }

    pub fn reset(&mut self) {
        self.register = self.spec.seed;
    }

    pub fn next_bit(&mut self) -> bool {
        let feedback = (self.register & self.spec.taps).count_ones() % 2;
        self.register = (self.register << 1 | feedback) & self.mask;
        feedback == 1
    }

    pub fn apply_bits(&mut self, bits: &mut [bool]) {
        for bit in bits {
            *bit ^= self.next_bit();
        }
    }

    pub fn apply_bytes(&mut self, data: &mut [u8]) {
        for byte in data {
            let mut scrambled = 0u8;
            for index in (0..8).rev() {
                scrambled |= u8::from(self.next_bit()) << index;
            }
            *byte ^= scrambled;
        }
    }

    pub fn skip_bytes(&mut self, count: usize) {
        for _ in 0..count * 8 {
            self.next_bit();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sequence(spec: PrbsSpec, len: usize) -> Vec<bool> {
        let mut prbs = Prbs::new(spec);
        (0..len).map(|_| prbs.next_bit()).collect()
    }

    fn period(spec: PrbsSpec) -> usize {
        let mut prbs = Prbs::new(spec);
        let mut steps = 0usize;
        loop {
            prbs.next_bit();
            steps += 1;
            if prbs.register == spec.seed {
                return steps;
            }
        }
    }

    #[test]
    fn both_generators_are_maximal_length() {
        assert_eq!(period(DAB_DISPERSAL), 511);
        assert_eq!(period(DVB_DISPERSAL), 32_767);
    }

    #[test]
    fn the_dvb_sequence_starts_with_the_bits_the_seed_implies() {
        let bits = sequence(DVB_DISPERSAL, 8);
        assert_eq!(bits, [false, false, false, false, false, false, true, true]);
    }

    #[test]
    fn the_dab_sequence_starts_with_the_bits_the_all_ones_seed_implies() {
        let bits = sequence(DAB_DISPERSAL, 6);
        assert_eq!(bits, [false, false, false, false, false, true]);
    }

    #[test]
    fn dispersal_is_its_own_inverse() {
        let original: Vec<u8> = (0..188u16).map(|value| (value * 7) as u8).collect();
        let mut data = original.clone();
        Prbs::new(DAB_DISPERSAL).apply_bytes(&mut data);
        assert_ne!(data, original);
        Prbs::new(DAB_DISPERSAL).apply_bytes(&mut data);
        assert_eq!(data, original);
    }

    #[test]
    fn skipping_matches_applying_to_a_scratch_buffer() {
        let mut skipped = Prbs::new(DVB_DISPERSAL);
        skipped.skip_bytes(4);
        let mut applied = Prbs::new(DVB_DISPERSAL);
        applied.apply_bytes(&mut [0u8; 4]);
        assert_eq!(skipped.register, applied.register);
    }

    #[test]
    fn the_dab_sequence_is_balanced_over_one_period() {
        let ones = sequence(DAB_DISPERSAL, 511)
            .into_iter()
            .filter(|&bit| bit)
            .count();
        assert_eq!(ones, 256);
    }
}
