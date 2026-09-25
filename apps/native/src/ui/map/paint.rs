use zgui::{
    canvas::{Brush, Shape, ShapeBuilder, zgui_color::Color},
    elements::{kurbo, kurbo::Shape as _},
};

use super::{
    geo::TileId,
    mvt::{self, Layer},
};

pub const EXTENT: f64 = 4096.0;
const UNITS_PER_PX: f64 = EXTENT / super::geo::TILE_PX;

#[must_use]
pub fn colour(hex: u32, alpha: f32) -> Color {
    let channel = |shift: u32| ((hex >> shift) & 0xff) as f32 / 255.0;
    Color::srgb(channel(16), channel(8), channel(0), alpha)
}

#[must_use]
pub fn solid(hex: u32, alpha: f32) -> Brush {
    Brush::Solid(colour(hex, alpha))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LabelClass {
    Country,
    City,
    State,
    Town,
    Village,
}

impl LabelClass {
    #[must_use]
    pub const fn css(self) -> &'static str {
        match self {
            Self::Country => "country",
            Self::State => "state",
            Self::City => "city",
            Self::Town => "town",
            Self::Village => "village",
        }
    }

    const fn of(class: &str, z: u8) -> Option<Self> {
        let (label, from, to) = match class.as_bytes() {
            b"country" => (Self::Country, 0, 6),
            b"state" => (Self::State, 4, 9),
            b"city" => (Self::City, 3, 20),
            b"town" => (Self::Town, 7, 20),
            b"village" => (Self::Village, 10, 20),
            _ => return None,
        };
        if z >= from && z <= to {
            Some(label)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TileLabel {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub class: LabelClass,
    pub score: i64,
}

#[derive(Default)]
pub struct Painted {
    pub shapes: Vec<Shape>,
    pub labels: Vec<TileLabel>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Ink {
    Wood,
    Park,
    Landuse,
    Water,
    Waterway,
    Building,
    Minor,
    Secondary,
    Primary,
    Motorway,
    Rail,
    Region,
    Border,
}

struct Look {
    colour: u32,
    alpha: f32,
    fill: bool,
    width_px: f64,
    dash: Option<[f64; 2]>,
}

impl Ink {
    const ALL: [Self; 13] = [
        Self::Wood,
        Self::Park,
        Self::Landuse,
        Self::Water,
        Self::Waterway,
        Self::Building,
        Self::Rail,
        Self::Minor,
        Self::Secondary,
        Self::Primary,
        Self::Motorway,
        Self::Region,
        Self::Border,
    ];

    const fn look(self) -> Look {
        let (colour, alpha, fill, width_px, dash) = match self {
            Self::Wood => (0x1d_27_22, 0.9, true, 0.0, None),
            Self::Park => (0x1c_29_22, 0.9, true, 0.0, None),
            Self::Landuse => (0x22_25_2b, 0.8, true, 0.0, None),
            Self::Water => (0x13_23_33, 1.0, true, 0.0, None),
            Self::Waterway => (0x16_2a_3d, 1.0, false, 0.9, None),
            Self::Building => (0x27_2a_31, 1.0, true, 0.0, None),
            Self::Rail => (0x3a_3e_46, 0.8, false, 0.6, Some([3.0, 2.0])),
            Self::Minor => (0x30_34_3b, 1.0, false, 0.7, None),
            Self::Secondary => (0x3a_3f_48, 1.0, false, 1.1, None),
            Self::Primary => (0x49_4f_5a, 1.0, false, 1.5, None),
            Self::Motorway => (0x5e_57_4b, 1.0, false, 2.0, None),
            Self::Region => (0x4a_4e_57, 0.8, false, 0.7, Some([3.0, 2.0])),
            Self::Border => (0x6a_70_7b, 0.9, false, 1.0, Some([3.0, 2.0])),
        };
        Look {
            colour,
            alpha,
            fill,
            width_px,
            dash,
        }
    }
}

fn text<'a>(layer: &'a Layer, feature: &mvt::Feature, key: &str) -> &'a str {
    layer
        .get(feature, key)
        .and_then(mvt::Value::text)
        .unwrap_or("")
}

fn number(layer: &Layer, feature: &mvt::Feature, key: &str) -> Option<f64> {
    layer.get(feature, key).and_then(mvt::Value::number)
}

fn ink_of(layer: &Layer, feature: &mvt::Feature, z: u8) -> Option<Ink> {
    let class = text(layer, feature, "class");
    Some(match layer.name.as_str() {
        "water" => Ink::Water,
        "waterway" if z >= 8 => Ink::Waterway,
        "landcover" if matches!(class, "wood" | "forest" | "grass" | "farmland") => Ink::Wood,
        "park" => Ink::Park,
        "landuse" if z >= 9 => Ink::Landuse,
        "building" if z >= 13 => Ink::Building,
        "boundary" => {
            let level = number(layer, feature, "admin_level").unwrap_or(10.0);
            let maritime = number(layer, feature, "maritime").unwrap_or(0.0) > 0.0;
            if maritime || level > 4.0 {
                return None;
            }
            if level <= 2.0 {
                Ink::Border
            } else {
                Ink::Region
            }
        }
        "transportation" => road_ink(class, z)?,
        _ => return None,
    })
}

fn road_ink(class: &str, z: u8) -> Option<Ink> {
    match class {
        "motorway" => Some(Ink::Motorway),
        "trunk" | "primary" => (z >= 5).then_some(Ink::Primary),
        "secondary" | "tertiary" => (z >= 8).then_some(Ink::Secondary),
        "minor" | "service" | "street" | "residential" => (z >= 12).then_some(Ink::Minor),
        "rail" => (z >= 10).then_some(Ink::Rail),
        _ => None,
    }
}

fn trace(parts: &[Vec<(i32, i32)>], path: &mut kurbo::BezPath, close: bool, scale: f64) {
    for part in parts {
        let Some((first, rest)) = part.split_first() else {
            continue;
        };
        path.move_to((f64::from(first.0) * scale, f64::from(first.1) * scale));
        for point in rest {
            path.line_to((f64::from(point.0) * scale, f64::from(point.1) * scale));
        }
        if close {
            path.close_path();
        }
    }
}

fn label_of(layer: &Layer, feature: &mvt::Feature, tile: TileId, scale: f64) -> Option<TileLabel> {
    if layer.name != "place" {
        return None;
    }
    let class = LabelClass::of(text(layer, feature, "class"), tile.z)?;
    let name = [
        text(layer, feature, "name:latin"),
        text(layer, feature, "name"),
    ]
    .into_iter()
    .find(|name| !name.is_empty())?;
    let point = feature.parts.first()?.first()?;
    let (px, py) = (f64::from(point.0) * scale, f64::from(point.1) * scale);
    if !(0.0..EXTENT).contains(&px) || !(0.0..EXTENT).contains(&py) {
        return None;
    }
    let count = f64::from(1u32 << tile.z);
    let rank = number(layer, feature, "rank").unwrap_or(10.0) as i64;
    Some(TileLabel {
        text: name.to_owned(),
        x: (f64::from(tile.x) + px / EXTENT) / count,
        y: (f64::from(tile.y) + py / EXTENT) / count,
        class,
        score: class as i64 * 100 + rank,
    })
}

#[must_use]
pub fn paint(tile: &mvt::Tile, id: TileId) -> Painted {
    let mut paths: Vec<(Ink, kurbo::BezPath)> = Ink::ALL
        .iter()
        .map(|ink| (*ink, kurbo::BezPath::new()))
        .collect();
    let mut labels = Vec::new();
    for layer in &tile.layers {
        let scale = EXTENT / f64::from(layer.extent.max(1));
        for feature in &layer.features {
            if feature.shape == mvt::Shape::Point {
                labels.extend(label_of(layer, feature, id, scale));
                continue;
            }
            let Some(ink) = ink_of(layer, feature, id.z) else {
                continue;
            };
            if let Some((_, path)) = paths.iter_mut().find(|(held, _)| *held == ink) {
                trace(
                    &feature.parts,
                    path,
                    feature.shape == mvt::Shape::Polygon && ink.look().fill,
                    scale,
                );
            }
        }
    }
    let clip = kurbo::Rect::new(0.0, 0.0, EXTENT, EXTENT).to_path(0.1);
    let shapes = paths
        .into_iter()
        .filter(|(_, path)| !path.elements().is_empty())
        .map(|(ink, path)| shape(ink, path, &clip))
        .collect();
    Painted { shapes, labels }
}

fn shape(ink: Ink, path: kurbo::BezPath, clip: &kurbo::BezPath) -> Shape {
    let look = ink.look();
    let brush = solid(look.colour, look.alpha);
    let builder = ShapeBuilder::new(path).clipped(clip.clone());
    if look.fill {
        return builder.fill(brush).build();
    }
    let mut stroke = kurbo::Stroke::new(look.width_px * UNITS_PER_PX);
    if let Some([on, off]) = look.dash {
        stroke = stroke
            .with_dashes(0.0, [on * UNITS_PER_PX, off * UNITS_PER_PX])
            .with_caps(kurbo::Cap::Butt);
    }
    builder.stroke_styled(brush, stroke).build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::map::mvt::tests::{feature, layer, point, square, tile};

    #[test]
    fn water_roads_and_places_are_painted() {
        let bytes = tile(&[
            layer("water", &[], &[], &[feature(3, &[], &square(0, 0, 100))]),
            layer(
                "transportation",
                &["class"],
                &["motorway"],
                &[feature(2, &[0, 0], &square(0, 0, 10))],
            ),
            layer(
                "place",
                &["class", "name"],
                &["city", "Berlin"],
                &[feature(1, &[0, 0, 1, 1], &point(2048, 1024))],
            ),
            layer("poi", &[], &[], &[feature(3, &[], &square(0, 0, 10))]),
        ]);
        let decoded = mvt::decode(&bytes).expect("a tile");
        let painted = paint(&decoded, TileId { z: 6, x: 34, y: 21 });
        assert_eq!(painted.shapes.len(), 2);
        assert_eq!(painted.labels.len(), 1);
        let berlin = &painted.labels[0];
        assert_eq!(berlin.text, "Berlin");
        assert_eq!(berlin.class, LabelClass::City);
        assert!((berlin.x - 34.5 / 64.0).abs() < 1e-9);
        assert!((berlin.y - 21.25 / 64.0).abs() < 1e-9);
    }

    #[test]
    fn small_places_wait_for_their_zoom() {
        assert_eq!(LabelClass::of("village", 6), None);
        assert_eq!(LabelClass::of("village", 12), Some(LabelClass::Village));
        assert_eq!(LabelClass::of("country", 9), None);
        assert_eq!(LabelClass::of("hamlet", 14), None);
    }

    #[test]
    fn a_colour_reads_its_hex_channels() {
        assert_eq!(
            colour(0xff_00_80, 1.0),
            Color::srgb(1.0, 0.0, 128.0 / 255.0, 1.0)
        );
    }
}
