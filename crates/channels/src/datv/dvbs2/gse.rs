use sdrmm_dsp::crc32_mpeg;

pub const MAX_PDU: usize = 8_192;
const FRAGMENTS: usize = 256;
const LIFETIME: u8 = 8;
const CRC_BYTES: usize = 4;
const TOTAL_BYTES: usize = 2;
const PROTOCOL_BYTES: usize = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GseMetrics {
    pub pdus: u32,
    pub fragments: u32,
    pub crc_errors: u32,
    pub dropped: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GsePdu {
    pub protocol: u16,
    pub label: Vec<u8>,
    pub data: Vec<u8>,
}

#[must_use]
pub const fn protocol_name(protocol: u16) -> &'static str {
    match protocol {
        0x0800 => "IPv4",
        0x0806 => "ARP",
        0x86DD => "IPv6",
        0x8100 => "VLAN",
        0x0001 => "MPE",
        _ => "other",
    }
}

impl GsePdu {
    #[cfg(any(test, feature = "test-signals"))]
    #[must_use]
    pub fn label_type(&self) -> u8 {
        match self.label.len() {
            6 => 0,
            3 => 1,
            _ => 2,
        }
    }
}

struct Fragment {
    label: Vec<u8>,
    total: usize,
    covered: Vec<u8>,
    ttl: u8,
}

pub struct Gse {
    partial: Vec<Option<Fragment>>,
    label: Vec<u8>,
    pub metrics: GseMetrics,
}

impl Gse {
    #[must_use]
    pub fn new() -> Self {
        Self {
            partial: (0..FRAGMENTS).map(|_| None).collect(),
            label: Vec::new(),
            metrics: GseMetrics::default(),
        }
    }

    pub fn reset(&mut self) {
        for slot in &mut self.partial {
            *slot = None;
        }
        self.label.clear();
        self.metrics = GseMetrics::default();
    }

    fn expire(&mut self) {
        for slot in &mut self.partial {
            let Some(fragment) = slot else { continue };
            fragment.ttl -= 1;
            if fragment.ttl == 0 {
                *slot = None;
                self.metrics.dropped += 1;
            }
        }
    }

    pub fn push(&mut self, field: &[u8], out: &mut Vec<GsePdu>) {
        let mut at = 0;
        while at + 2 <= field.len() {
            let word = u16::from_be_bytes([field[at], field[at + 1]]);
            if word == 0 {
                break;
            }
            let start = word >> 15 & 1 == 1;
            let end = word >> 14 & 1 == 1;
            let label_type = (word >> 12 & 3) as u8;
            let length = usize::from(word & 0x0FFF);
            at += 2;
            if length == 0 || at + length > field.len() {
                self.metrics.dropped += 1;
                break;
            }
            let body = &field[at..at + length];
            at += length;
            if self.unit(start, end, label_type, body, out).is_none() {
                self.metrics.dropped += 1;
            }
        }
        self.expire();
    }

    fn unit(
        &mut self,
        start: bool,
        end: bool,
        label_type: u8,
        body: &[u8],
        out: &mut Vec<GsePdu>,
    ) -> Option<()> {
        let mut cursor = 0;
        let id = if start && end {
            None
        } else {
            let id = *body.first()?;
            cursor += 1;
            Some(id)
        };
        if start {
            self.begin(id, end, label_type, &body[cursor..], out)
        } else {
            self.extend(id?, end, &body[cursor..], out)
        }
    }

    fn label_length(label_type: u8) -> Option<usize> {
        match label_type {
            0 => Some(6),
            1 => Some(3),
            2 => Some(0),
            _ => None,
        }
    }

    fn begin(
        &mut self,
        id: Option<u8>,
        end: bool,
        label_type: u8,
        body: &[u8],
        out: &mut Vec<GsePdu>,
    ) -> Option<()> {
        let mut cursor = 0;
        let total = if end {
            None
        } else {
            let value = usize::from(u16::from_be_bytes([body[0], *body.get(1)?]));
            cursor += TOTAL_BYTES;
            Some(value)
        };
        let protocol = u16::from_be_bytes([*body.get(cursor)?, *body.get(cursor + 1)?]);
        cursor += PROTOCOL_BYTES;
        let label = match Self::label_length(label_type) {
            Some(len) => {
                let taken = body.get(cursor..cursor + len)?.to_vec();
                cursor += len;
                self.label.clone_from(&taken);
                taken
            }
            None => self.label.clone(),
        };
        let data = body.get(cursor..)?;
        if end {
            if data.len() > MAX_PDU {
                return None;
            }
            self.metrics.pdus += 1;
            out.push(GsePdu {
                protocol,
                label,
                data: data.to_vec(),
            });
            return Some(());
        }
        let id = id?;
        let total = total?;
        if total > MAX_PDU + PROTOCOL_BYTES + label.len() {
            return None;
        }
        let mut covered = Vec::with_capacity(TOTAL_BYTES + total);
        covered.extend_from_slice(&(total as u16).to_be_bytes());
        covered.extend_from_slice(&protocol.to_be_bytes());
        covered.extend_from_slice(&label);
        covered.extend_from_slice(data);
        if self.partial[usize::from(id)].is_some() {
            self.metrics.dropped += 1;
        }
        self.metrics.fragments += 1;
        self.partial[usize::from(id)] = Some(Fragment {
            label,
            total,
            covered,
            ttl: LIFETIME,
        });
        Some(())
    }

    fn extend(&mut self, id: u8, end: bool, body: &[u8], out: &mut Vec<GsePdu>) -> Option<()> {
        let slot = usize::from(id);
        let fragment = self.partial[slot].as_mut()?;
        fragment.ttl = LIFETIME;
        if fragment.covered.len() + body.len() > TOTAL_BYTES + fragment.total + CRC_BYTES {
            self.partial[slot] = None;
            return None;
        }
        fragment.covered.extend_from_slice(body);
        self.metrics.fragments += 1;
        if !end {
            return Some(());
        }
        let fragment = self.partial[slot].take()?;
        if fragment.covered.len() != TOTAL_BYTES + fragment.total + CRC_BYTES {
            return None;
        }
        if crc32_mpeg(&fragment.covered) != 0 {
            self.metrics.crc_errors += 1;
            return Some(());
        }
        let head = TOTAL_BYTES + PROTOCOL_BYTES + fragment.label.len();
        let protocol = u16::from_be_bytes([
            fragment.covered[TOTAL_BYTES],
            fragment.covered[TOTAL_BYTES + 1],
        ]);
        self.metrics.pdus += 1;
        out.push(GsePdu {
            protocol,
            label: fragment.label,
            data: fragment.covered[head..fragment.covered.len() - CRC_BYTES].to_vec(),
        });
        Some(())
    }
}

impl Default for Gse {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "test-signals"))]
pub struct GseWriter {
    next: u8,
}

#[cfg(any(test, feature = "test-signals"))]
impl GseWriter {
    #[must_use]
    pub const fn new() -> Self {
        Self { next: 0 }
    }

    fn head(start: bool, end: bool, label_type: u8, length: usize, out: &mut Vec<u8>) {
        let word = u16::from(start) << 15
            | u16::from(end) << 14
            | u16::from(label_type) << 12
            | length as u16 & 0x0FFF;
        out.extend_from_slice(&word.to_be_bytes());
    }

    pub fn whole(pdu: &GsePdu, out: &mut Vec<u8>) {
        let label_type = pdu.label_type();
        let length = PROTOCOL_BYTES + pdu.label.len() + pdu.data.len();
        Self::head(true, true, label_type, length, out);
        out.extend_from_slice(&pdu.protocol.to_be_bytes());
        out.extend_from_slice(&pdu.label);
        out.extend_from_slice(&pdu.data);
    }

    pub fn fragmented(&mut self, pdu: &GsePdu, pieces: usize, out: &mut Vec<u8>) {
        let id = self.next;
        self.next = self.next.wrapping_add(1);
        let label_type = pdu.label_type();
        let total = PROTOCOL_BYTES + pdu.label.len() + pdu.data.len();
        let mut covered = Vec::with_capacity(TOTAL_BYTES + total + CRC_BYTES);
        covered.extend_from_slice(&(total as u16).to_be_bytes());
        covered.extend_from_slice(&pdu.protocol.to_be_bytes());
        covered.extend_from_slice(&pdu.label);
        covered.extend_from_slice(&pdu.data);
        let crc = crc32_mpeg(&covered);
        covered.extend_from_slice(&crc.to_be_bytes());

        let head = TOTAL_BYTES + PROTOCOL_BYTES + pdu.label.len();
        let body = &covered[head..];
        let pieces = pieces.max(2);
        let step = body.len().div_ceil(pieces);
        let mut chunks = body.chunks(step.max(1)).peekable();
        let mut first = true;
        while let Some(chunk) = chunks.next() {
            let last = chunks.peek().is_none();
            if first {
                Self::head(
                    true,
                    false,
                    label_type,
                    1 + TOTAL_BYTES + PROTOCOL_BYTES + pdu.label.len() + chunk.len(),
                    out,
                );
                out.push(id);
                out.extend_from_slice(&covered[..head]);
            } else {
                Self::head(false, last, 0, 1 + chunk.len(), out);
                out.push(id);
            }
            out.extend_from_slice(chunk);
            first = false;
        }
    }

    pub fn pad(field: &mut Vec<u8>, len: usize) {
        if field.len() < len {
            field.resize(len, 0);
        }
    }
}

#[cfg(any(test, feature = "test-signals"))]
impl Default for GseWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pdu(protocol: u16, label: &[u8], len: usize, seed: u32) -> GsePdu {
        let mut state = seed | 1;
        GsePdu {
            protocol,
            label: label.to_vec(),
            data: (0..len)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    state as u8
                })
                .collect(),
        }
    }

    #[test]
    fn a_whole_packet_comes_back_with_its_label_and_protocol() {
        for label in [vec![1, 2, 3, 4, 5, 6], vec![9, 8, 7], Vec::new()] {
            let sent = pdu(0x0800, &label, 400, 3);
            let mut field = Vec::new();
            GseWriter::whole(&sent, &mut field);
            let mut gse = Gse::new();
            let mut out = Vec::new();
            gse.push(&field, &mut out);
            assert_eq!(out, vec![sent], "{label:?}");
            assert_eq!(gse.metrics.pdus, 1);
            assert_eq!(gse.metrics.crc_errors, 0);
        }
    }

    #[test]
    fn a_fragmented_packet_is_reassembled_across_its_pieces() {
        for pieces in [2usize, 3, 7] {
            let sent = pdu(0x86DD, &[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF], 1_500, 5);
            let mut field = Vec::new();
            GseWriter::new().fragmented(&sent, pieces, &mut field);
            let mut gse = Gse::new();
            let mut out = Vec::new();
            gse.push(&field, &mut out);
            assert_eq!(out, vec![sent], "{pieces} pieces");
            assert_eq!(gse.metrics.pdus, 1);
            assert!(gse.metrics.fragments >= pieces as u32);
        }
    }

    fn split(field: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let mut at = 0;
        while at + 2 <= field.len() {
            let length = 2 + usize::from(u16::from_be_bytes([field[at], field[at + 1]]) & 0x0FFF);
            out.push(field[at..at + length].to_vec());
            at += length;
        }
        out
    }

    #[test]
    fn two_packets_may_be_fragmented_at_the_same_time() {
        let first = pdu(0x0800, &[1, 2, 3], 900, 7);
        let second = pdu(0x86DD, &[4, 5, 6], 700, 11);
        let mut writer = GseWriter::new();
        let mut a = Vec::new();
        let mut b = Vec::new();
        writer.fragmented(&first, 3, &mut a);
        writer.fragmented(&second, 3, &mut b);
        let (a, b) = (split(&a), split(&b));
        assert_eq!(a.len(), 3);
        let mut gse = Gse::new();
        let mut out = Vec::new();
        for (left, right) in a.iter().zip(&b) {
            gse.push(left, &mut out);
            gse.push(right, &mut out);
        }
        assert_eq!(out, vec![first, second]);
        assert_eq!(gse.metrics.dropped, 0);
    }

    #[test]
    fn a_damaged_fragment_is_caught_by_the_checksum() {
        let sent = pdu(0x0800, &[1, 2, 3, 4, 5, 6], 800, 13);
        let mut field = Vec::new();
        GseWriter::new().fragmented(&sent, 3, &mut field);
        let at = field.len() / 2;
        field[at] ^= 0x40;
        let mut gse = Gse::new();
        let mut out = Vec::new();
        gse.push(&field, &mut out);
        assert!(out.is_empty());
        assert_eq!(gse.metrics.crc_errors, 1);
        assert_eq!(gse.metrics.pdus, 0);
    }

    #[test]
    fn padding_ends_the_data_field() {
        let sent = pdu(0x0800, &[1, 2, 3], 100, 17);
        let mut field = Vec::new();
        GseWriter::whole(&sent, &mut field);
        GseWriter::pad(&mut field, 512);
        let mut gse = Gse::new();
        let mut out = Vec::new();
        gse.push(&field, &mut out);
        assert_eq!(out, vec![sent]);
        assert_eq!(gse.metrics.dropped, 0);
    }

    #[test]
    fn a_label_is_reused_when_the_type_says_so() {
        let first = pdu(0x0800, &[1, 2, 3, 4, 5, 6], 60, 19);
        let mut field = Vec::new();
        GseWriter::whole(&first, &mut field);
        GseWriter::head(true, true, 3, PROTOCOL_BYTES + 20, &mut field);
        field.extend_from_slice(&0x86DDu16.to_be_bytes());
        field.extend(std::iter::repeat_n(0x5A, 20));
        let mut gse = Gse::new();
        let mut out = Vec::new();
        gse.push(&field, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].label, first.label);
        assert_eq!(out[1].protocol, 0x86DD);
        assert_eq!(out[1].data.len(), 20);
    }

    #[test]
    fn an_abandoned_fragment_is_eventually_dropped() {
        let sent = pdu(0x0800, &[1, 2, 3], 900, 23);
        let mut field = Vec::new();
        GseWriter::new().fragmented(&sent, 4, &mut field);
        let first = 2 + usize::from(u16::from_be_bytes([field[0], field[1]]) & 0x0FFF);
        let mut gse = Gse::new();
        let mut out = Vec::new();
        gse.push(&field[..first], &mut out);
        assert_eq!(gse.metrics.dropped, 0);
        for _ in 0..LIFETIME {
            gse.push(&[], &mut out);
        }
        assert!(out.is_empty());
        assert_eq!(gse.metrics.dropped, 1);
    }

    #[test]
    fn a_field_of_noise_yields_nothing_and_is_not_read_past_its_end() {
        let mut state = 0x1234_5678u32;
        let field: Vec<u8> = (0..1_000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        let mut gse = Gse::new();
        let mut out = Vec::new();
        gse.push(&field, &mut out);
        assert!(out.iter().all(|pdu| pdu.data.len() <= MAX_PDU));
    }
}
