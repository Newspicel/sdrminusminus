use anyhow::{Context, bail};

const DEFAULT_EXTENT: u32 = 4096;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Text(String),
    Number(f64),
    Bool(bool),
}

impl Value {
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }

    #[must_use]
    pub const fn number(&self) -> Option<f64> {
        match self {
            Self::Number(number) => Some(*number),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Unknown,
    Point,
    Line,
    Polygon,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Feature {
    pub shape: Shape,
    pub properties: Vec<(usize, usize)>,
    pub parts: Vec<Vec<(i32, i32)>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub name: String,
    pub extent: u32,
    pub keys: Vec<String>,
    pub values: Vec<Value>,
    pub features: Vec<Feature>,
}

impl Layer {
    #[must_use]
    pub fn get<'a>(&'a self, feature: &Feature, key: &str) -> Option<&'a Value> {
        feature.properties.iter().find_map(|(k, v)| {
            (self.keys.get(*k).map(String::as_str) == Some(key))
                .then(|| self.values.get(*v))
                .flatten()
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tile {
    pub layers: Vec<Layer>,
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

enum Field<'a> {
    Varint(u64),
    Fixed64(u64),
    Bytes(&'a [u8]),
    Fixed32(u32),
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    const fn done(&self) -> bool {
        self.at >= self.bytes.len()
    }

    fn varint(&mut self) -> anyhow::Result<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self
                .bytes
                .get(self.at)
                .context("a varint runs past the end")?;
            self.at += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        bail!("a varint is longer than ten bytes")
    }

    fn take(&mut self, count: usize) -> anyhow::Result<&'a [u8]> {
        let end = self.at.checked_add(count).context("a field is too long")?;
        let slice = self
            .bytes
            .get(self.at..end)
            .context("a field runs past the end")?;
        self.at = end;
        Ok(slice)
    }

    fn field(&mut self) -> anyhow::Result<(u64, Field<'a>)> {
        let key = self.varint()?;
        let field = match key & 7 {
            0 => Field::Varint(self.varint()?),
            1 => Field::Fixed64(u64::from_le_bytes(self.take(8)?.try_into()?)),
            2 => {
                let length = usize::try_from(self.varint()?)?;
                Field::Bytes(self.take(length)?)
            }
            5 => Field::Fixed32(u32::from_le_bytes(self.take(4)?.try_into()?)),
            other => bail!("unknown wire type {other}"),
        };
        Ok((key >> 3, field))
    }
}

fn packed(bytes: &[u8]) -> anyhow::Result<Vec<u32>> {
    let mut reader = Reader::new(bytes);
    let mut out = Vec::new();
    while !reader.done() {
        out.push(u32::try_from(reader.varint()?)?);
    }
    Ok(out)
}

const fn zigzag(value: u32) -> i32 {
    ((value >> 1) as i32) ^ -((value & 1) as i32)
}

pub fn decode(bytes: &[u8]) -> anyhow::Result<Tile> {
    let mut reader = Reader::new(bytes);
    let mut tile = Tile::default();
    while !reader.done() {
        if let (3, Field::Bytes(layer)) = reader.field()? {
            tile.layers.push(decode_layer(layer)?);
        }
    }
    Ok(tile)
}

fn decode_layer(bytes: &[u8]) -> anyhow::Result<Layer> {
    let mut reader = Reader::new(bytes);
    let mut layer = Layer {
        name: String::new(),
        extent: DEFAULT_EXTENT,
        keys: Vec::new(),
        values: Vec::new(),
        features: Vec::new(),
    };
    while !reader.done() {
        match reader.field()? {
            (1, Field::Bytes(name)) => layer.name = String::from_utf8_lossy(name).into_owned(),
            (2, Field::Bytes(feature)) => layer.features.push(decode_feature(feature)?),
            (3, Field::Bytes(key)) => layer.keys.push(String::from_utf8_lossy(key).into_owned()),
            (4, Field::Bytes(value)) => layer.values.push(decode_value(value)?),
            (5, Field::Varint(extent)) => layer.extent = u32::try_from(extent)?.max(1),
            _ => {}
        }
    }
    Ok(layer)
}

fn decode_value(bytes: &[u8]) -> anyhow::Result<Value> {
    let mut reader = Reader::new(bytes);
    let mut value = Value::Bool(false);
    while !reader.done() {
        value = match reader.field()? {
            (1, Field::Bytes(text)) => Value::Text(String::from_utf8_lossy(text).into_owned()),
            (2, Field::Fixed32(bits)) => Value::Number(f64::from(f32::from_bits(bits))),
            (3, Field::Fixed64(bits)) => Value::Number(f64::from_bits(bits)),
            (4 | 5, Field::Varint(number)) => Value::Number(number as i64 as f64),
            (6, Field::Varint(number)) => {
                Value::Number(((number >> 1) as i64 ^ -((number & 1) as i64)) as f64)
            }
            (7, Field::Varint(flag)) => Value::Bool(flag != 0),
            _ => continue,
        };
    }
    Ok(value)
}

fn decode_feature(bytes: &[u8]) -> anyhow::Result<Feature> {
    let mut reader = Reader::new(bytes);
    let mut shape = Shape::Unknown;
    let mut tags = Vec::new();
    let mut geometry = Vec::new();
    while !reader.done() {
        match reader.field()? {
            (2, Field::Bytes(packed_tags)) => tags = packed(packed_tags)?,
            (3, Field::Varint(kind)) => {
                shape = match kind {
                    1 => Shape::Point,
                    2 => Shape::Line,
                    3 => Shape::Polygon,
                    _ => Shape::Unknown,
                };
            }
            (4, Field::Bytes(packed_geometry)) => geometry = packed(packed_geometry)?,
            _ => {}
        }
    }
    let properties = tags
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[key, value]| (*key as usize, *value as usize))
        .collect();
    Ok(Feature {
        shape,
        properties,
        parts: parts(&geometry)?,
    })
}

fn parts(commands: &[u32]) -> anyhow::Result<Vec<Vec<(i32, i32)>>> {
    let mut out: Vec<Vec<(i32, i32)>> = Vec::new();
    let (mut x, mut y) = (0i32, 0i32);
    let mut at = 0;
    while at < commands.len() {
        let command = commands[at];
        at += 1;
        let (id, count) = (command & 7, (command >> 3) as usize);
        match id {
            1 | 2 => {
                for _ in 0..count {
                    let dx = *commands.get(at).context("a move runs past the end")?;
                    let dy = *commands.get(at + 1).context("a move runs past the end")?;
                    at += 2;
                    x = x.wrapping_add(zigzag(dx));
                    y = y.wrapping_add(zigzag(dy));
                    if id == 1 {
                        out.push(Vec::new());
                    }
                    match out.last_mut() {
                        Some(part) => part.push((x, y)),
                        None => bail!("a line starts before its first move"),
                    }
                }
            }
            7 => {
                if let Some(part) = out.last_mut()
                    && let Some(first) = part.first().copied()
                {
                    part.push(first);
                }
            }
            other => bail!("unknown geometry command {other}"),
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn varint(mut value: u64, out: &mut Vec<u8>) {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    pub(crate) fn bytes_field(field: u64, body: &[u8], out: &mut Vec<u8>) {
        varint(field << 3 | 2, out);
        varint(body.len() as u64, out);
        out.extend_from_slice(body);
    }

    fn zig(value: i32) -> u64 {
        u64::from(((value << 1) ^ (value >> 31)) as u32)
    }

    fn command(id: u32, count: u32) -> u64 {
        u64::from(id | count << 3)
    }

    pub(crate) fn feature(shape: u64, tags: &[u64], geometry: &[u64]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut packed_tags = Vec::new();
        for tag in tags {
            varint(*tag, &mut packed_tags);
        }
        bytes_field(2, &packed_tags, &mut out);
        varint(3 << 3, &mut out);
        varint(shape, &mut out);
        let mut packed_geometry = Vec::new();
        for value in geometry {
            varint(*value, &mut packed_geometry);
        }
        bytes_field(4, &packed_geometry, &mut out);
        out
    }

    pub(crate) fn layer(
        name: &str,
        keys: &[&str],
        values: &[&str],
        features: &[Vec<u8>],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        varint(15 << 3, &mut out);
        varint(2, &mut out);
        bytes_field(1, name.as_bytes(), &mut out);
        for feature in features {
            bytes_field(2, feature, &mut out);
        }
        for key in keys {
            bytes_field(3, key.as_bytes(), &mut out);
        }
        for value in values {
            let mut encoded = Vec::new();
            bytes_field(1, value.as_bytes(), &mut encoded);
            bytes_field(4, &encoded, &mut out);
        }
        varint(5 << 3, &mut out);
        varint(4096, &mut out);
        out
    }

    pub(crate) fn tile(layers: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        for layer in layers {
            bytes_field(3, layer, &mut out);
        }
        out
    }

    pub(crate) fn square(left: i32, top: i32, side: i32) -> Vec<u64> {
        vec![
            command(1, 1),
            zig(left),
            zig(top),
            command(2, 3),
            zig(side),
            zig(0),
            zig(0),
            zig(side),
            zig(-side),
            zig(0),
            command(7, 1),
        ]
    }

    pub(crate) fn point(x: i32, y: i32) -> Vec<u64> {
        vec![command(1, 1), zig(x), zig(y)]
    }

    #[test]
    fn a_polygon_is_read_with_its_layer_and_properties() {
        let bytes = tile(&[layer(
            "water",
            &["class"],
            &["ocean"],
            &[feature(3, &[0, 0], &square(10, 20, 100))],
        )]);
        let decoded = decode(&bytes).expect("a tile");
        let water = &decoded.layers[0];
        assert_eq!(water.name, "water");
        assert_eq!(water.extent, 4096);
        let polygon = &water.features[0];
        assert_eq!(polygon.shape, Shape::Polygon);
        assert_eq!(
            polygon.parts,
            vec![vec![(10, 20), (110, 20), (110, 120), (10, 120), (10, 20)]]
        );
        assert_eq!(
            water.get(polygon, "class"),
            Some(&Value::Text("ocean".to_owned()))
        );
        assert_eq!(water.get(polygon, "name"), None);
    }

    #[test]
    fn a_line_with_two_parts_keeps_them_apart() {
        let geometry = vec![
            command(1, 1),
            zig(0),
            zig(0),
            command(2, 1),
            zig(5),
            zig(5),
            command(1, 1),
            zig(10),
            zig(0),
            command(2, 1),
            zig(0),
            zig(-3),
        ];
        let bytes = tile(&[layer("roads", &[], &[], &[feature(2, &[], &geometry)])]);
        let decoded = decode(&bytes).expect("a tile");
        assert_eq!(
            decoded.layers[0].features[0].parts,
            vec![vec![(0, 0), (5, 5)], vec![(15, 5), (15, 2)]]
        );
    }

    #[test]
    fn numbers_come_in_every_encoding() {
        let mut signed = Vec::new();
        varint(6 << 3, &mut signed);
        varint(zig(-7), &mut signed);
        assert_eq!(decode_value(&signed).expect("a value"), Value::Number(-7.0));
        let mut double = Vec::new();
        varint(3 << 3 | 1, &mut double);
        double.extend_from_slice(&2.5f64.to_le_bytes());
        assert_eq!(decode_value(&double).expect("a value"), Value::Number(2.5));
        let mut flag = Vec::new();
        varint(7 << 3, &mut flag);
        varint(1, &mut flag);
        assert_eq!(decode_value(&flag).expect("a value"), Value::Bool(true));
    }

    #[test]
    fn a_truncated_tile_is_refused_rather_than_misread() {
        let bytes = tile(&[layer(
            "water",
            &[],
            &[],
            &[feature(3, &[], &square(0, 0, 4))],
        )]);
        assert!(decode(&bytes[..bytes.len() - 3]).is_err());
        assert!(parts(&[2 | 1 << 3, 2, 2]).is_err());
        assert!(parts(&[1 | 1 << 3, 2]).is_err());
    }
}
