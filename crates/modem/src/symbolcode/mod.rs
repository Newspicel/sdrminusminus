use std::cmp::Ordering;

#[must_use]
pub fn nrz_encode(bits: &[bool]) -> Vec<bool> {
    bits.to_vec()
}

#[must_use]
pub fn nrz_decode(levels: &[bool]) -> Vec<bool> {
    levels.to_vec()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrziConvention {
    TransitionOnZero,
    TransitionOnOne,
}

impl NrziConvention {
    fn toggles(self, bit: bool) -> bool {
        match self {
            Self::TransitionOnZero => !bit,
            Self::TransitionOnOne => bit,
        }
    }
}

#[derive(Clone, Debug)]
pub struct NrziEncoder {
    convention: NrziConvention,
    level: bool,
}

impl NrziEncoder {
    #[must_use]
    pub fn new(convention: NrziConvention) -> Self {
        Self {
            convention,
            level: false,
        }
    }

    pub fn encode(&mut self, bit: bool) -> bool {
        if self.convention.toggles(bit) {
            self.level = !self.level;
        }
        self.level
    }

    pub fn encode_all(&mut self, bits: &[bool]) -> Vec<bool> {
        bits.iter().map(|&b| self.encode(b)).collect()
    }

    pub fn reset(&mut self) {
        self.level = false;
    }
}

#[derive(Clone, Debug)]
pub struct NrziDecoder {
    convention: NrziConvention,
    last: bool,
}

impl NrziDecoder {
    #[must_use]
    pub fn new(convention: NrziConvention) -> Self {
        Self {
            convention,
            last: false,
        }
    }

    pub fn decode(&mut self, level: bool) -> bool {
        let held = level == self.last;
        self.last = level;
        match self.convention {
            NrziConvention::TransitionOnZero => held,
            NrziConvention::TransitionOnOne => !held,
        }
    }

    pub fn decode_all(&mut self, levels: &[bool]) -> Vec<bool> {
        levels.iter().map(|&l| self.decode(l)).collect()
    }

    pub fn reset(&mut self) {
        self.last = false;
    }
}

#[derive(Clone, Debug, Default)]
pub struct DifferentialEncoder {
    last: bool,
}

impl DifferentialEncoder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn encode(&mut self, bit: bool) -> bool {
        self.last ^= bit;
        self.last
    }

    pub fn encode_all(&mut self, bits: &[bool]) -> Vec<bool> {
        bits.iter().map(|&b| self.encode(b)).collect()
    }

    pub fn reset(&mut self) {
        self.last = false;
    }
}

#[derive(Clone, Debug, Default)]
pub struct DifferentialDecoder {
    last: bool,
}

impl DifferentialDecoder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn decode(&mut self, bit: bool) -> bool {
        let out = bit ^ self.last;
        self.last = bit;
        out
    }

    pub fn decode_all(&mut self, bits: &[bool]) -> Vec<bool> {
        bits.iter().map(|&b| self.decode(b)).collect()
    }

    pub fn reset(&mut self) {
        self.last = false;
    }
}

#[derive(Clone, Debug)]
pub struct DifferentialSymbolEncoder {
    m: u32,
    last: u32,
}

impl DifferentialSymbolEncoder {
    #[must_use]
    pub fn new(m: u32) -> Self {
        assert!(m >= 2, "differential alphabet needs at least two symbols");
        Self { m, last: 0 }
    }

    pub fn encode(&mut self, symbol: u32) -> u32 {
        debug_assert!(symbol < self.m, "symbol {symbol} outside 0..{}", self.m);
        self.last = ((u64::from(symbol) + u64::from(self.last)) % u64::from(self.m)) as u32;
        self.last
    }

    pub fn encode_all(&mut self, symbols: &[u32]) -> Vec<u32> {
        symbols.iter().map(|&s| self.encode(s)).collect()
    }

    pub fn reset(&mut self) {
        self.last = 0;
    }
}

#[derive(Clone, Debug)]
pub struct DifferentialSymbolDecoder {
    m: u32,
    last: u32,
}

impl DifferentialSymbolDecoder {
    #[must_use]
    pub fn new(m: u32) -> Self {
        assert!(m >= 2, "differential alphabet needs at least two symbols");
        Self { m, last: 0 }
    }

    pub fn decode(&mut self, symbol: u32) -> u32 {
        debug_assert!(symbol < self.m, "symbol {symbol} outside 0..{}", self.m);
        let out = ((u64::from(symbol) + u64::from(self.m) - u64::from(self.last % self.m))
            % u64::from(self.m)) as u32;
        self.last = symbol;
        out
    }

    pub fn decode_all(&mut self, symbols: &[u32]) -> Vec<u32> {
        symbols.iter().map(|&s| self.decode(s)).collect()
    }

    pub fn reset(&mut self) {
        self.last = 0;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManchesterConvention {
    Ieee8023,
    GeThomas,
}

impl ManchesterConvention {
    #[must_use]
    pub fn halves(self, bit: bool) -> (bool, bool) {
        match self {
            Self::Ieee8023 => (!bit, bit),
            Self::GeThomas => (bit, !bit),
        }
    }
}

#[must_use]
pub fn manchester_encode(convention: ManchesterConvention, bits: &[bool]) -> Vec<bool> {
    let mut out = Vec::with_capacity(bits.len() * 2);
    for &bit in bits {
        let (first, second) = convention.halves(bit);
        out.push(first);
        out.push(second);
    }
    out
}

#[must_use]
pub fn manchester_decode_pair(
    convention: ManchesterConvention,
    first: bool,
    second: bool,
) -> Option<bool> {
    if first == second {
        return None;
    }
    Some(match convention {
        ManchesterConvention::Ieee8023 => second,
        ManchesterConvention::GeThomas => first,
    })
}

#[must_use]
pub fn manchester_decode(convention: ManchesterConvention, levels: &[bool]) -> PairDecode {
    let mut bits = Vec::with_capacity(levels.len() / 2);
    let mut violations = 0;
    for pair in levels.as_chunks::<2>().0 {
        match manchester_decode_pair(convention, pair[0], pair[1]) {
            Some(bit) => bits.push(bit),
            None => {
                violations += 1;
                bits.push(match convention {
                    ManchesterConvention::Ieee8023 => !pair[0],
                    ManchesterConvention::GeThomas => pair[0],
                });
            }
        }
    }
    let slipped = manchester_violations(levels.get(1..).unwrap_or_default());
    PairDecode {
        alignment: alignment_verdict(violations, slipped),
        bits,
        violations,
    }
}

fn manchester_violations(levels: &[bool]) -> usize {
    levels
        .as_chunks::<2>()
        .0
        .iter()
        .filter(|p| p[0] == p[1])
        .count()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BiphaseVariant {
    Mark,
    Space,
}

impl BiphaseVariant {
    fn mid_transition(self, bit: bool) -> bool {
        match self {
            Self::Mark => bit,
            Self::Space => !bit,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BiphaseEncoder {
    variant: BiphaseVariant,
    level: bool,
}

impl BiphaseEncoder {
    #[must_use]
    pub fn new(variant: BiphaseVariant) -> Self {
        Self {
            variant,
            level: false,
        }
    }

    pub fn encode(&mut self, bit: bool) -> (bool, bool) {
        let first = !self.level;
        let second = if self.variant.mid_transition(bit) {
            !first
        } else {
            first
        };
        self.level = second;
        (first, second)
    }

    pub fn encode_all(&mut self, bits: &[bool]) -> Vec<bool> {
        let mut out = Vec::with_capacity(bits.len() * 2);
        for &bit in bits {
            let (first, second) = self.encode(bit);
            out.push(first);
            out.push(second);
        }
        out
    }

    pub fn reset(&mut self) {
        self.level = false;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BiphasePair {
    pub bit: bool,
    pub boundary_transition: bool,
}

#[derive(Clone, Debug)]
pub struct BiphaseDecoder {
    variant: BiphaseVariant,
    last: Option<bool>,
}

impl BiphaseDecoder {
    #[must_use]
    pub fn new(variant: BiphaseVariant) -> Self {
        Self {
            variant,
            last: None,
        }
    }

    pub fn decode(&mut self, first: bool, second: bool) -> BiphasePair {
        let boundary_transition = self.last.is_none_or(|level| level != first);
        self.last = Some(second);
        BiphasePair {
            bit: self.variant.mid_transition(true) == (first != second),
            boundary_transition,
        }
    }

    pub fn reset(&mut self) {
        self.last = None;
    }
}

#[must_use]
pub fn biphase_decode(variant: BiphaseVariant, levels: &[bool]) -> PairDecode {
    let mut decoder = BiphaseDecoder::new(variant);
    let mut bits = Vec::with_capacity(levels.len() / 2);
    let mut violations = 0;
    for pair in levels.as_chunks::<2>().0 {
        let out = decoder.decode(pair[0], pair[1]);
        bits.push(out.bit);
        violations += usize::from(!out.boundary_transition);
    }
    let slipped = boundary_violations(levels.get(1..).unwrap_or_default());
    PairDecode {
        alignment: alignment_verdict(violations, slipped),
        bits,
        violations,
    }
}

fn boundary_violations(levels: &[bool]) -> usize {
    let mut prev = None;
    let mut violations = 0;
    for pair in levels.as_chunks::<2>().0 {
        violations += usize::from(prev == Some(pair[0]));
        prev = Some(pair[1]);
    }
    violations
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alignment {
    BitEdge,
    MidBit,
    Ambiguous,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairDecode {
    pub bits: Vec<bool>,
    pub violations: usize,
    pub alignment: Alignment,
}

fn alignment_verdict(as_given: usize, slipped: usize) -> Alignment {
    match as_given.cmp(&slipped) {
        Ordering::Less => Alignment::BitEdge,
        Ordering::Greater => Alignment::MidBit,
        Ordering::Equal => Alignment::Ambiguous,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ber::rng::Rng;

    fn random_bits(rng: &mut Rng, len: usize) -> Vec<bool> {
        (0..len).map(|_| rng.next_u64() & 1 == 1).collect()
    }

    fn payloads() -> Vec<Vec<bool>> {
        let mut rng = Rng::new(0x51b0);
        let mut set = vec![
            vec![],
            vec![false],
            vec![true],
            vec![false; 64],
            vec![true; 64],
        ];
        for len in [2, 3, 17, 256, 1001] {
            set.push(random_bits(&mut rng, len));
        }
        set
    }

    #[test]
    fn nrz_is_the_identity_in_both_directions() {
        for payload in payloads() {
            assert_eq!(nrz_encode(&payload), payload);
            assert_eq!(nrz_decode(&nrz_encode(&payload)), payload);
        }
    }

    #[test]
    fn nrzi_round_trips_in_both_conventions() {
        for convention in [
            NrziConvention::TransitionOnZero,
            NrziConvention::TransitionOnOne,
        ] {
            for payload in payloads() {
                let mut encoder = NrziEncoder::new(convention);
                let mut decoder = NrziDecoder::new(convention);
                let levels = encoder.encode_all(&payload);
                assert_eq!(decoder.decode_all(&levels), payload, "{convention:?}");

                encoder.reset();
                decoder.reset();
                assert_eq!(encoder.encode_all(&payload), levels, "{convention:?}");
                assert_eq!(decoder.decode_all(&levels), payload, "{convention:?}");
            }
        }
    }

    #[test]
    fn nrzi_conventions_complement_each_other() {
        let mut rng = Rng::new(0x217);
        let payload = random_bits(&mut rng, 512);
        let inverted: Vec<bool> = payload.iter().map(|&b| !b).collect();
        let on_one = NrziEncoder::new(NrziConvention::TransitionOnOne).encode_all(&payload);
        let on_zero = NrziEncoder::new(NrziConvention::TransitionOnZero).encode_all(&inverted);
        assert_eq!(on_one, on_zero);
        assert_ne!(
            on_one,
            NrziEncoder::new(NrziConvention::TransitionOnZero).encode_all(&payload)
        );
    }

    #[test]
    fn nrzi_transition_on_zero_agrees_with_the_dsp_decoder() {
        let mut rng = Rng::new(0xd5b);
        let levels = random_bits(&mut rng, 4096);
        let mut mine = NrziDecoder::new(NrziConvention::TransitionOnZero);
        let mut theirs = sdrmm_dsp::bits::NrziDecoder::new();
        for &level in &levels {
            assert_eq!(mine.decode(level), theirs.decode(level));
        }

        let payload = random_bits(&mut rng, 2048);
        let mut encoder = NrziEncoder::new(NrziConvention::TransitionOnZero);
        let mut theirs = sdrmm_dsp::bits::NrziDecoder::new();
        let decoded: Vec<bool> = encoder
            .encode_all(&payload)
            .iter()
            .map(|&l| theirs.decode(l))
            .collect();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn differential_round_trips() {
        for payload in payloads() {
            let mut encoder = DifferentialEncoder::new();
            let mut decoder = DifferentialDecoder::new();
            let encoded = encoder.encode_all(&payload);
            assert_eq!(decoder.decode_all(&encoded), payload);

            encoder.reset();
            decoder.reset();
            assert_eq!(encoder.encode_all(&payload), encoded);
            assert_eq!(decoder.decode_all(&encoded), payload);
        }
    }

    #[test]
    fn differential_agrees_with_the_dsp_decoder() {
        let mut rng = Rng::new(0xd1ff);
        let stream = random_bits(&mut rng, 4096);
        let mut mine = DifferentialDecoder::new();
        let mut theirs = sdrmm_dsp::bits::DifferentialDecoder::new();
        for &bit in &stream {
            assert_eq!(mine.decode(bit), theirs.decode(bit));
        }

        let payload = random_bits(&mut rng, 2048);
        let mut encoder = DifferentialEncoder::new();
        let mut theirs = sdrmm_dsp::bits::DifferentialDecoder::new();
        let decoded: Vec<bool> = encoder
            .encode_all(&payload)
            .iter()
            .map(|&b| theirs.decode(b))
            .collect();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn mary_differential_round_trips() {
        let mut rng = Rng::new(0x3a17);
        for m in [2u32, 4, 8, 16, 64] {
            for len in [0usize, 1, 2, 500] {
                let symbols: Vec<u32> = (0..len)
                    .map(|_| (rng.next_u64() % u64::from(m)) as u32)
                    .collect();
                let mut encoder = DifferentialSymbolEncoder::new(m);
                let mut decoder = DifferentialSymbolDecoder::new(m);
                let encoded = encoder.encode_all(&symbols);
                assert_eq!(decoder.decode_all(&encoded), symbols, "M = {m}, len {len}");

                encoder.reset();
                decoder.reset();
                assert_eq!(encoder.encode_all(&symbols), encoded, "M = {m}");
                assert_eq!(decoder.decode_all(&encoded), symbols, "M = {m}");
            }
        }
    }

    #[test]
    fn mary_differential_accumulates_increments() {
        let mut encoder = DifferentialSymbolEncoder::new(4);
        assert_eq!(encoder.encode_all(&[1, 1, 1, 3]), [1, 2, 3, 2]);
        let mut decoder = DifferentialSymbolDecoder::new(4);
        assert_eq!(decoder.decode_all(&[1, 2, 3, 2]), [1, 1, 1, 3]);
    }

    #[test]
    fn binary_differential_is_the_m_equals_2_case() {
        let mut rng = Rng::new(0xb1);
        let payload = random_bits(&mut rng, 1024);
        let symbols: Vec<u32> = payload.iter().map(|&b| u32::from(b)).collect();

        let mut binary = DifferentialEncoder::new();
        let mut mary = DifferentialSymbolEncoder::new(2);
        let from_binary: Vec<u32> = binary
            .encode_all(&payload)
            .iter()
            .map(|&b| u32::from(b))
            .collect();
        assert_eq!(mary.encode_all(&symbols), from_binary);

        let mut binary = DifferentialDecoder::new();
        let mut mary = DifferentialSymbolDecoder::new(2);
        let from_binary: Vec<u32> = binary
            .decode_all(&payload)
            .iter()
            .map(|&b| u32::from(b))
            .collect();
        assert_eq!(mary.decode_all(&symbols), from_binary);
    }

    #[test]
    fn manchester_pair_mapping_matches_the_conventions() {
        use ManchesterConvention::{GeThomas, Ieee8023};
        assert_eq!(manchester_decode_pair(GeThomas, true, false), Some(true));
        assert_eq!(manchester_decode_pair(GeThomas, false, true), Some(false));
        assert_eq!(manchester_decode_pair(Ieee8023, false, true), Some(true));
        assert_eq!(manchester_decode_pair(Ieee8023, true, false), Some(false));
        for convention in [Ieee8023, GeThomas] {
            assert_eq!(manchester_decode_pair(convention, true, true), None);
            assert_eq!(manchester_decode_pair(convention, false, false), None);
        }
    }

    #[test]
    fn manchester_round_trips_in_both_conventions() {
        for convention in [
            ManchesterConvention::Ieee8023,
            ManchesterConvention::GeThomas,
        ] {
            for payload in payloads() {
                let levels = manchester_encode(convention, &payload);
                assert_eq!(levels.len(), payload.len() * 2);
                let decoded = manchester_decode(convention, &levels);
                assert_eq!(decoded.bits, payload, "{convention:?}");
                assert_eq!(decoded.violations, 0, "{convention:?}");
                assert_ne!(decoded.alignment, Alignment::MidBit, "{convention:?}");
                if payload.windows(2).any(|w| w[0] != w[1]) {
                    assert_eq!(decoded.alignment, Alignment::BitEdge, "{convention:?}");
                }
            }
        }
    }

    #[test]
    fn manchester_reports_a_half_bit_slip() {
        let payload: Vec<bool> = (0..64).map(|i| i % 2 == 0).collect();
        for convention in [
            ManchesterConvention::Ieee8023,
            ManchesterConvention::GeThomas,
        ] {
            let levels = manchester_encode(convention, &payload);
            let slipped = manchester_decode(convention, &levels[1..]);
            assert_eq!(slipped.alignment, Alignment::MidBit, "{convention:?}");
            assert!(slipped.violations > 0, "{convention:?}");

            let realigned = manchester_decode(convention, &levels[2..]);
            assert_eq!(realigned.bits, payload[1..]);
            assert_eq!(realigned.violations, 0);
        }
    }

    #[test]
    fn manchester_flags_a_missing_transition_without_shortening_the_output() {
        let payload = vec![true, false, true, true, false];
        let mut levels = manchester_encode(ManchesterConvention::Ieee8023, &payload);
        levels[5] = levels[4];
        let decoded = manchester_decode(ManchesterConvention::Ieee8023, &levels);
        assert_eq!(decoded.violations, 1);
        assert_eq!(decoded.bits.len(), payload.len(), "positions stay aligned");
        assert_eq!(decoded.bits[..2], payload[..2]);
        assert_eq!(decoded.bits[3..], payload[3..]);
    }

    #[test]
    fn ge_thomas_pairs_agree_with_the_dsp_decoder() {
        for first in [false, true] {
            for second in [false, true] {
                assert_eq!(
                    manchester_decode_pair(ManchesterConvention::GeThomas, first, second),
                    sdrmm_dsp::bits::manchester_decode(first, second),
                );
            }
        }

        let mut rng = Rng::new(0xac);
        let payload = random_bits(&mut rng, 2048);
        let levels = manchester_encode(ManchesterConvention::GeThomas, &payload);
        let decoded: Vec<bool> = levels
            .as_chunks::<2>()
            .0
            .iter()
            .filter_map(|p| sdrmm_dsp::bits::manchester_decode(p[0], p[1]))
            .collect();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn biphase_round_trips_in_both_variants() {
        for variant in [BiphaseVariant::Mark, BiphaseVariant::Space] {
            for payload in payloads() {
                let mut encoder = BiphaseEncoder::new(variant);
                let levels = encoder.encode_all(&payload);
                assert_eq!(levels.len(), payload.len() * 2);
                let decoded = biphase_decode(variant, &levels);
                assert_eq!(decoded.bits, payload, "{variant:?}");
                assert_eq!(decoded.violations, 0, "{variant:?}");
                assert_ne!(decoded.alignment, Alignment::MidBit, "{variant:?}");

                encoder.reset();
                assert_eq!(encoder.encode_all(&payload), levels, "{variant:?}");
            }
        }
    }

    #[test]
    fn biphase_transitions_at_every_boundary_and_encodes_data_mid_bit() {
        let mut rng = Rng::new(0xf0);
        let payload = random_bits(&mut rng, 256);
        for variant in [BiphaseVariant::Mark, BiphaseVariant::Space] {
            let mut encoder = BiphaseEncoder::new(variant);
            let levels = encoder.encode_all(&payload);
            for (i, pair) in levels.as_chunks::<2>().0.iter().enumerate() {
                if i > 0 {
                    assert_ne!(levels[2 * i - 1], pair[0], "boundary {i} must transition");
                }
                let expect_mid = match variant {
                    BiphaseVariant::Mark => payload[i],
                    BiphaseVariant::Space => !payload[i],
                };
                assert_eq!(pair[0] != pair[1], expect_mid, "{variant:?} bit {i}");
            }
        }
    }

    #[test]
    fn biphase_is_polarity_free() {
        let mut rng = Rng::new(0x9019);
        let payload = random_bits(&mut rng, 256);
        for variant in [BiphaseVariant::Mark, BiphaseVariant::Space] {
            let levels = BiphaseEncoder::new(variant).encode_all(&payload);
            let inverted: Vec<bool> = levels.iter().map(|&l| !l).collect();
            let decoded = biphase_decode(variant, &inverted);
            assert_eq!(decoded.bits, payload, "{variant:?}");
            assert_eq!(decoded.violations, 0, "{variant:?}");
        }
    }

    #[test]
    fn biphase_reports_a_half_bit_slip() {
        let zeros = vec![false; 64];
        let levels = BiphaseEncoder::new(BiphaseVariant::Mark).encode_all(&zeros);
        let slipped = biphase_decode(BiphaseVariant::Mark, &levels[1..]);
        assert_eq!(slipped.alignment, Alignment::MidBit);
        assert!(slipped.violations > 0);

        let mut rng = Rng::new(0x51ee);
        let payload = random_bits(&mut rng, 256);
        for variant in [BiphaseVariant::Mark, BiphaseVariant::Space] {
            let levels = BiphaseEncoder::new(variant).encode_all(&payload);
            assert_eq!(
                biphase_decode(variant, &levels[1..]).alignment,
                Alignment::MidBit,
                "{variant:?}"
            );
        }
    }

    #[test]
    fn biphase_flags_a_missing_boundary_transition() {
        let zeros = vec![false; 4];
        let mut levels = BiphaseEncoder::new(BiphaseVariant::Mark).encode_all(&zeros);
        levels[2] = !levels[2];
        let decoded = biphase_decode(BiphaseVariant::Mark, &levels);
        assert_eq!(decoded.violations, 1);
        assert_eq!(decoded.bits.len(), zeros.len());
        assert!(decoded.bits[1], "the damaged pair reads as a spurious mark");
        assert_eq!(decoded.bits[2..], zeros[2..], "damage does not propagate");
    }

    #[test]
    fn biphase_streaming_decoder_resets_the_boundary_check() {
        let mut decoder = BiphaseDecoder::new(BiphaseVariant::Mark);
        assert!(decoder.decode(true, true).boundary_transition);
        assert!(!decoder.decode(true, true).boundary_transition);
        decoder.reset();
        assert!(decoder.decode(true, true).boundary_transition);
    }

    #[test]
    fn pair_codes_report_ambiguity_honestly() {
        for levels in [&[] as &[bool], &[true]] {
            for convention in [
                ManchesterConvention::Ieee8023,
                ManchesterConvention::GeThomas,
            ] {
                let decoded = manchester_decode(convention, levels);
                assert!(decoded.bits.is_empty());
                assert_eq!(decoded.violations, 0);
                assert_eq!(decoded.alignment, Alignment::Ambiguous);
            }
            let decoded = biphase_decode(BiphaseVariant::Mark, levels);
            assert!(decoded.bits.is_empty());
            assert_eq!(decoded.alignment, Alignment::Ambiguous);
        }

        let constant = manchester_encode(ManchesterConvention::Ieee8023, &[false; 32]);
        assert_eq!(
            manchester_decode(ManchesterConvention::Ieee8023, &constant).alignment,
            Alignment::Ambiguous
        );
    }
}
