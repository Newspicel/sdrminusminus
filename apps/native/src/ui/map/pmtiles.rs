use std::io::Read;

use anyhow::{Context, bail};

use super::geo::TileId;

pub const HEADER_BYTES: usize = 127;
pub const FIRST_READ: u64 = 16_384;
pub const MAX_DEPTH: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    None,
    Gzip,
    Other(u8),
}

impl Compression {
    const fn of(code: u8) -> Self {
        match code {
            0 | 1 => Self::None,
            2 => Self::Gzip,
            other => Self::Other(other),
        }
    }

    pub fn inflate(self, bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
        match self {
            Self::None => Ok(bytes.to_vec()),
            Self::Gzip => gunzip(bytes),
            Self::Other(code) => bail!("the basemap uses compression {code}, only gzip is read"),
        }
    }
}

pub fn gunzip(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_end(&mut out)
        .context("cannot inflate gzip data")?;
    Ok(out)
}

#[must_use]
pub fn gzipped(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x1f, 0x8b])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub root_offset: u64,
    pub root_length: u64,
    pub leaf_offset: u64,
    pub data_offset: u64,
    pub internal: Compression,
    pub tiles: Compression,
    pub tile_type: u8,
    pub min_zoom: u8,
    pub max_zoom: u8,
}

pub const MVT_TILES: u8 = 1;

fn le64(bytes: &[u8], at: usize) -> anyhow::Result<u64> {
    let slice = bytes.get(at..at + 8).context("the header is too short")?;
    Ok(u64::from_le_bytes(slice.try_into()?))
}

pub fn header(bytes: &[u8]) -> anyhow::Result<Header> {
    if bytes.len() < HEADER_BYTES || &bytes[..7] != b"PMTiles" {
        bail!("not a PMTiles archive");
    }
    if bytes[7] != 3 {
        bail!("PMTiles version {} is not read, only 3", bytes[7]);
    }
    Ok(Header {
        root_offset: le64(bytes, 8)?,
        root_length: le64(bytes, 16)?,
        leaf_offset: le64(bytes, 40)?,
        data_offset: le64(bytes, 56)?,
        internal: Compression::of(bytes[97]),
        tiles: Compression::of(bytes[98]),
        tile_type: bytes[99],
        min_zoom: bytes[100],
        max_zoom: bytes[101],
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub tile_id: u64,
    pub offset: u64,
    pub length: u32,
    pub run_length: u32,
}

fn varint(bytes: &[u8], at: &mut usize) -> anyhow::Result<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*at).context("a directory runs past its end")?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    bail!("a directory varint is too long")
}

pub fn directory(bytes: &[u8]) -> anyhow::Result<Vec<Entry>> {
    let mut at = 0;
    let count = usize::try_from(varint(bytes, &mut at)?)?;
    if count > bytes.len() {
        bail!("a directory claims more entries than it has bytes");
    }
    let mut entries = vec![
        Entry {
            tile_id: 0,
            offset: 0,
            length: 0,
            run_length: 0,
        };
        count
    ];
    let mut last = 0u64;
    for entry in &mut entries {
        last = last
            .checked_add(varint(bytes, &mut at)?)
            .context("tile ids overflow")?;
        entry.tile_id = last;
    }
    for entry in &mut entries {
        entry.run_length = u32::try_from(varint(bytes, &mut at)?)?;
    }
    for entry in &mut entries {
        entry.length = u32::try_from(varint(bytes, &mut at)?)?;
    }
    for index in 0..count {
        let value = varint(bytes, &mut at)?;
        entries[index].offset = if value == 0 && index > 0 {
            entries[index - 1].offset + u64::from(entries[index - 1].length)
        } else {
            value.saturating_sub(1)
        };
    }
    Ok(entries)
}

#[must_use]
pub fn tile_id(tile: TileId) -> u64 {
    let z = u32::from(tile.z);
    let base = ((1u64 << (2 * z)) - 1) / 3;
    let n = 1u64 << z;
    let (mut x, mut y) = (u64::from(tile.x), u64::from(tile.y));
    let mut d = 0u64;
    let mut s = n / 2;
    while s > 0 {
        let rx = u64::from(x & s > 0);
        let ry = u64::from(y & s > 0);
        d += s * s * ((3 * rx) ^ ry);
        if ry == 0 {
            if rx == 1 {
                x = n - 1 - x;
                y = n - 1 - y;
            }
            std::mem::swap(&mut x, &mut y);
        }
        s /= 2;
    }
    base + d
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    Tile { offset: u64, length: u32 },
    Leaf { offset: u64, length: u32 },
    Missing,
}

#[must_use]
pub fn find(entries: &[Entry], id: u64) -> Found {
    let at = entries.partition_point(|entry| entry.tile_id <= id);
    let Some(entry) = at.checked_sub(1).and_then(|at| entries.get(at)) else {
        return Found::Missing;
    };
    if entry.run_length == 0 {
        return Found::Leaf {
            offset: entry.offset,
            length: entry.length,
        };
    }
    if id - entry.tile_id < u64::from(entry.run_length) {
        return Found::Tile {
            offset: entry.offset,
            length: entry.length,
        };
    }
    Found::Missing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_are_numbered_along_the_hilbert_curve() {
        let id = |z, x, y| tile_id(TileId { z, x, y });
        assert_eq!(id(0, 0, 0), 0);
        assert_eq!(id(1, 0, 0), 1);
        assert_eq!(id(1, 0, 1), 2);
        assert_eq!(id(1, 1, 1), 3);
        assert_eq!(id(1, 1, 0), 4);
        assert_eq!(id(2, 0, 0), 5);
        let mut seen: Vec<u64> = (0..4)
            .flat_map(|x| (0..4).map(move |y| id(2, x, y)))
            .collect();
        seen.sort_unstable();
        assert_eq!(seen, (5..21).collect::<Vec<_>>());
    }

    fn encode(entries: &[(u64, u32, u32, u64)]) -> Vec<u8> {
        let mut out = Vec::new();
        let push = |value: u64, out: &mut Vec<u8>| crate::ui::map::mvt::tests::varint(value, out);
        push(entries.len() as u64, &mut out);
        let mut last = 0;
        for (id, ..) in entries {
            push(id - last, &mut out);
            last = *id;
        }
        for (_, run, ..) in entries {
            push(u64::from(*run), &mut out);
        }
        for (_, _, length, _) in entries {
            push(u64::from(*length), &mut out);
        }
        for (_, _, _, offset) in entries {
            push(*offset, &mut out);
        }
        out
    }

    #[test]
    fn a_directory_reads_its_offsets_back() {
        let bytes = encode(&[(0, 1, 100, 1), (1, 2, 50, 0), (5, 0, 30, 1001)]);
        let entries = directory(&bytes).expect("a directory");
        assert_eq!(entries[0].offset, 0);
        assert_eq!(entries[1].offset, 100);
        assert_eq!(entries[2].offset, 1000);
        assert_eq!(
            find(&entries, 0),
            Found::Tile {
                offset: 0,
                length: 100
            }
        );
        assert_eq!(
            find(&entries, 2),
            Found::Tile {
                offset: 100,
                length: 50
            }
        );
        assert_eq!(find(&entries, 3), Found::Missing);
        assert_eq!(
            find(&entries, 9),
            Found::Leaf {
                offset: 1000,
                length: 30
            }
        );
        assert_eq!(find(&[], 9), Found::Missing);
    }

    #[test]
    fn a_header_says_where_everything_lives() {
        let mut bytes = vec![0u8; HEADER_BYTES];
        bytes[..7].copy_from_slice(b"PMTiles");
        bytes[7] = 3;
        bytes[8..16].copy_from_slice(&127u64.to_le_bytes());
        bytes[16..24].copy_from_slice(&40u64.to_le_bytes());
        bytes[56..64].copy_from_slice(&9000u64.to_le_bytes());
        bytes[97] = 2;
        bytes[98] = 2;
        bytes[99] = MVT_TILES;
        bytes[101] = 14;
        let read = header(&bytes).expect("a header");
        assert_eq!(read.root_offset, 127);
        assert_eq!(read.root_length, 40);
        assert_eq!(read.data_offset, 9000);
        assert_eq!(read.internal, Compression::Gzip);
        assert_eq!(read.max_zoom, 14);
        bytes[7] = 2;
        assert!(header(&bytes).is_err());
        assert!(header(b"nope").is_err());
    }

    #[test]
    fn gzip_is_recognised_and_inflated() {
        use std::io::Write;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(b"tile").expect("written");
        let packed = encoder.finish().expect("finished");
        assert!(gzipped(&packed));
        assert_eq!(
            Compression::Gzip.inflate(&packed).expect("inflated"),
            b"tile"
        );
        assert!(Compression::Other(3).inflate(&packed).is_err());
    }
}
