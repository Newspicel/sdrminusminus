use sdrmm_dsp::ReedSolomon;

const ROW_DATA_OCTETS: usize = 249;
const ROW_DATA_BITS: usize = ROW_DATA_OCTETS * 8;
const ROW_OCTETS: usize = 255;
const NPAR: usize = 6;
const PRIMITIVE: u16 = 0x187;
const FIRST_ROOT: u8 = 120;
const MAX_TL_BITS: usize = 131_071;
const SOFT_ERASURES: usize = 2;
const DOUBTFUL_RESIDUAL: f32 = 0.20;

pub fn vdl2_rs() -> ReedSolomon {
    ReedSolomon::new(PRIMITIVE, FIRST_ROOT, NPAR)
}

fn checks_for(n: usize) -> usize {
    match n {
        0..=2 => 0,
        3..=30 => 2,
        31..=67 => 4,
        _ => NPAR,
    }
}

pub struct Layout {
    pub rows: Vec<usize>,
    pub checks: Vec<usize>,
    pub total_tx_bits: usize,
}

impl Layout {
    fn transmitted(&self, row: usize, col: usize) -> bool {
        col < self.rows[row] || (ROW_DATA_OCTETS..ROW_DATA_OCTETS + self.checks[row]).contains(&col)
    }
}

pub fn layout(tl_bits: usize) -> Option<Layout> {
    if tl_bits == 0 || tl_bits > MAX_TL_BITS {
        return None;
    }
    let row_count = tl_bits.div_ceil(ROW_DATA_BITS);
    let mut rows = Vec::with_capacity(row_count);
    let mut remaining = tl_bits.div_ceil(8);
    for _ in 0..row_count {
        let n = remaining.min(ROW_DATA_OCTETS);
        rows.push(n);
        remaining -= n;
    }
    let checks: Vec<usize> = rows.iter().map(|&n| checks_for(n)).collect();
    let total_tx_bits = rows.iter().zip(&checks).map(|(d, k)| (d + k) * 8).sum();
    Some(Layout {
        rows,
        checks,
        total_tx_bits,
    })
}

pub(super) fn bits_to_octets(bits: &[u8]) -> Vec<u8> {
    bits.chunks(8)
        .map(|c| c.iter().enumerate().fold(0u8, |b, (i, &v)| b | (v << i)))
        .collect()
}

fn octets_to_bits(octets: &[u8], nbits: usize) -> Vec<u8> {
    octets
        .iter()
        .flat_map(|&o| (0..8).map(move |i| (o >> i) & 1))
        .take(nbits)
        .collect()
}

#[cfg(test)]
pub fn interleave(data_bits: &[u8], rs: &ReedSolomon) -> Option<Vec<u8>> {
    let lay = layout(data_bits.len())?;
    let octets = bits_to_octets(data_bits);
    let mut grid: Vec<Vec<u8>> = Vec::with_capacity(lay.rows.len());
    let mut off = 0;
    for &n in &lay.rows {
        let mut row = vec![0u8; ROW_DATA_OCTETS];
        row[..n].copy_from_slice(&octets[off..off + n]);
        off += n;
        let mut coded = Vec::with_capacity(ROW_OCTETS);
        rs.encode(&row, &mut coded);
        grid.push(coded);
    }
    let mut out_octets = Vec::new();
    for col in 0..ROW_OCTETS {
        for (r, row) in grid.iter().enumerate() {
            if lay.transmitted(r, col) {
                out_octets.push(row[col]);
            }
        }
    }
    Some(octets_to_bits(&out_octets, out_octets.len() * 8))
}

#[cfg(test)]
pub fn deinterleave(tx_bits: &[u8], tl_bits: usize, rs: &ReedSolomon) -> Option<(Vec<u8>, usize)> {
    deinterleave_soft(tx_bits, &[], 0, tl_bits, rs, Erasures::Doubtful)
        .map(|decoded| (decoded.bits, decoded.corrected))
}

pub struct Deinterleaved {
    pub bits: Vec<u8>,
    pub corrected: usize,
    pub soft_assisted: bool,
}

fn octet_confidence(octet_count: usize, sym_conf: &[f32], bit_offset: usize) -> Vec<f32> {
    (0..octet_count)
        .map(|o| {
            let first_sym = (bit_offset + o * 8) / 3;
            let last_sym = (bit_offset + o * 8 + 7) / 3;
            (first_sym..=last_sym)
                .map(|s| sym_conf.get(s).copied().unwrap_or(0.0))
                .fold(0.0f32, f32::max)
        })
        .collect()
}

#[derive(Clone, Copy)]
pub enum Erasures {
    Doubtful,
    Least(usize),
}

struct RowFix {
    corrected: usize,
    soft_assisted: bool,
}

fn correct_row(
    row: &mut [u8],
    confidence: &[f32],
    data: usize,
    checks: usize,
    rs: &ReedSolomon,
    erasures: Erasures,
) -> Option<RowFix> {
    let base: Vec<usize> = (ROW_DATA_OCTETS + checks..ROW_OCTETS).collect();
    let budget = NPAR.saturating_sub(base.len());
    let mut ranked: Vec<usize> = (0..ROW_OCTETS)
        .filter(|&col| col < data || (ROW_DATA_OCTETS..ROW_DATA_OCTETS + checks).contains(&col))
        .collect();
    ranked.sort_by(|&a, &b| confidence[b].total_cmp(&confidence[a]));
    if let Erasures::Least(count) = erasures
        && count + 2 <= budget
        && ranked.len() >= count
    {
        let mut attempt = row.to_vec();
        let mut positions = base.clone();
        positions.extend(ranked.iter().take(count).copied());
        let fixed = rs.decode_with_erasures(&mut attempt, &positions)?;
        row.copy_from_slice(&attempt);
        return Some(RowFix {
            corrected: (fixed as usize).saturating_sub(base.len()),
            soft_assisted: true,
        });
    }
    for extra in [0usize, SOFT_ERASURES] {
        if extra > budget {
            break;
        }
        let mut erasures = base.clone();
        if extra > 0 {
            let doubtful = ranked.len() >= extra
                && ranked
                    .iter()
                    .take(extra)
                    .all(|&p| confidence[p] >= DOUBTFUL_RESIDUAL);
            if !doubtful {
                break;
            }
            erasures.extend(ranked.iter().take(extra).copied());
        }
        let mut attempt = row.to_vec();
        if let Some(fixed) = rs.decode_with_erasures(&mut attempt, &erasures) {
            row.copy_from_slice(&attempt);
            return Some(RowFix {
                corrected: (fixed as usize).saturating_sub(base.len()),
                soft_assisted: extra > 0,
            });
        }
    }
    None
}

pub fn deinterleave_soft(
    tx_bits: &[u8],
    sym_conf: &[f32],
    bit_offset: usize,
    tl_bits: usize,
    rs: &ReedSolomon,
    erasures: Erasures,
) -> Option<Deinterleaved> {
    let lay = layout(tl_bits)?;
    if tx_bits.len() < lay.total_tx_bits {
        return None;
    }
    let octets = bits_to_octets(&tx_bits[..lay.total_tx_bits]);
    let octet_conf = octet_confidence(octets.len(), sym_conf, bit_offset);
    let row_count = lay.rows.len();
    let mut grid = vec![[0u8; ROW_OCTETS]; row_count];
    let mut cgrid = vec![[0.0f32; ROW_OCTETS]; row_count];
    let mut it = octets.iter().zip(&octet_conf);
    for col in 0..ROW_OCTETS {
        for r in 0..row_count {
            if lay.transmitted(r, col) {
                let (&o, &cf) = it.next()?;
                grid[r][col] = o;
                cgrid[r][col] = cf;
            }
        }
    }
    let mut corrected = 0usize;
    let mut soft_assisted = false;
    let mut data_octets = Vec::with_capacity(tl_bits.div_ceil(8));
    for (r, row) in grid.iter_mut().enumerate() {
        let n = lay.rows[r];
        let k = lay.checks[r];
        if k > 0 {
            let fix = correct_row(row, &cgrid[r], n, k, rs, erasures)?;
            corrected += fix.corrected;
            soft_assisted |= fix.soft_assisted;
        }
        data_octets.extend_from_slice(&row[..n]);
    }
    Some(Deinterleaved {
        bits: octets_to_bits(&data_octets, tl_bits),
        corrected,
        soft_assisted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern_bits(n: usize, seed: u64) -> Vec<u8> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s & 1) as u8
            })
            .collect()
    }

    #[test]
    fn roundtrip_various_lengths() {
        let rs = vdl2_rs();
        for tl in [10usize, 100, 600, 1500, 1992, 1993, 5000, 8000] {
            let data = pattern_bits(tl, tl as u64 + 1);
            let tx = interleave(&data, &rs).unwrap();
            let lay = layout(tl).unwrap();
            assert_eq!(tx.len(), lay.total_tx_bits, "tl={tl}");
            let (back, fixed) = deinterleave(&tx, tl, &rs).expect("roundtrip");
            assert_eq!(back, data, "tl={tl}");
            assert_eq!(fixed, 0);
        }
    }

    #[test]
    fn corrects_octet_errors() {
        let rs = vdl2_rs();
        let data = pattern_bits(5000, 99);
        let mut tx = interleave(&data, &rs).unwrap();
        for b in &mut tx[1000..1008] {
            *b ^= 1;
        }
        let (back, fixed) = deinterleave(&tx, 5000, &rs).expect("must correct");
        assert_eq!(back, data);
        assert!(fixed >= 1);
    }

    #[test]
    fn shortening_rules() {
        assert_eq!(layout(16).unwrap().checks, vec![0]);
        assert_eq!(layout(17).unwrap().checks, vec![2]);
        assert_eq!(layout(30 * 8).unwrap().checks, vec![2]);
        assert_eq!(layout(31 * 8).unwrap().checks, vec![4]);
        assert_eq!(layout(67 * 8).unwrap().checks, vec![4]);
        assert_eq!(layout(68 * 8).unwrap().checks, vec![6]);
        let l = layout(1992 + 24).unwrap();
        assert_eq!(l.rows, vec![249, 3]);
        assert_eq!(l.checks, vec![6, 2]);
    }
}
