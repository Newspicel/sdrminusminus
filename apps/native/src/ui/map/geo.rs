use std::f64::consts::PI;

pub const EARTH_RADIUS_KM: f64 = 6371.0;
pub const TILE_PX: f64 = 512.0;
pub const MIN_ZOOM: f64 = 0.0;
pub const MAX_ZOOM: f64 = 19.0;
const MAX_LAT: f64 = 85.051_128_78;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Geo {
    pub lat: f64,
    pub lon: f64,
}

impl Geo {
    #[must_use]
    pub const fn new(lat: f64, lon: f64) -> Self {
        Self { lat, lon }
    }

    #[must_use]
    pub fn checked(lat: Option<f64>, lon: Option<f64>) -> Option<Self> {
        let (lat, lon) = (lat?, lon?);
        (lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0)
            .then_some(Self { lat, lon })
    }
}

#[must_use]
pub fn mercator(at: Geo) -> (f64, f64) {
    let lat = at.lat.clamp(-MAX_LAT, MAX_LAT).to_radians();
    let x = (at.lon + 180.0) / 360.0;
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / PI) / 2.0;
    (x, y)
}

#[cfg(test)]
#[must_use]
pub fn unmercator(x: f64, y: f64) -> Geo {
    let lon = x * 360.0 - 180.0;
    let lat = (PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees();
    Geo { lat, lon }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
    pub width: f64,
    pub height: f64,
}

impl Default for View {
    fn default() -> Self {
        Self {
            x: 0.5,
            y: 0.5,
            zoom: 1.0,
            width: 0.0,
            height: 0.0,
        }
        .centred(Geo::new(25.0, 0.0), 1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileId {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

impl TileId {
    #[must_use]
    pub const fn parent(self) -> Option<Self> {
        if self.z == 0 {
            return None;
        }
        Some(Self {
            z: self.z - 1,
            x: self.x / 2,
            y: self.y / 2,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TilePlace {
    pub id: TileId,
    pub column: i64,
    pub left: f64,
    pub top: f64,
    pub size: f64,
}

impl View {
    #[must_use]
    pub fn world(&self) -> f64 {
        TILE_PX * self.zoom.exp2()
    }

    #[must_use]
    pub fn centred(mut self, at: Geo, zoom: f64) -> Self {
        let (x, y) = mercator(at);
        self.x = x;
        self.y = y;
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self
    }

    #[must_use]
    pub fn screen_of_unit(&self, x: f64, y: f64) -> (f64, f64) {
        let world = self.world();
        (
            (x - self.x) * world + self.width / 2.0,
            (y - self.y) * world + self.height / 2.0,
        )
    }

    #[must_use]
    pub fn screen(&self, at: Geo) -> (f64, f64) {
        let (x, y) = mercator(at);
        let near = x - (x - self.x).round();
        self.screen_of_unit(near, y)
    }

    #[must_use]
    pub fn line(&self, points: &[Geo]) -> Vec<(f64, f64)> {
        let mut out = Vec::with_capacity(points.len());
        let mut previous: Option<f64> = None;
        for point in points {
            let (x, y) = mercator(*point);
            let anchor = previous.unwrap_or(self.x);
            let near = x - (x - anchor).round();
            previous = Some(near);
            out.push(self.screen_of_unit(near, y));
        }
        out
    }

    #[cfg(test)]
    #[must_use]
    pub fn geo(&self, sx: f64, sy: f64) -> Geo {
        let world = self.world();
        let x = self.x + (sx - self.width / 2.0) / world;
        let y = self.y + (sy - self.height / 2.0) / world;
        unmercator(x.rem_euclid(1.0), y.clamp(0.0, 1.0))
    }

    #[must_use]
    pub fn panned(mut self, dx: f64, dy: f64) -> Self {
        let world = self.world();
        self.x = (self.x - dx / world).rem_euclid(1.0);
        self.y = (self.y - dy / world).clamp(0.0, 1.0);
        self
    }

    #[must_use]
    pub fn zoomed_at(mut self, sx: f64, sy: f64, zoom: f64) -> Self {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let before = self.world();
        let ux = self.x + (sx - self.width / 2.0) / before;
        let uy = self.y + (sy - self.height / 2.0) / before;
        self.zoom = zoom;
        let after = self.world();
        self.x = (ux - (sx - self.width / 2.0) / after).rem_euclid(1.0);
        self.y = (uy - (sy - self.height / 2.0) / after).clamp(0.0, 1.0);
        self
    }

    #[must_use]
    pub fn fitted(mut self, points: &[Geo], padding: f64, max_zoom: f64) -> Self {
        let Some(bounds) = trail_bounds(points) else {
            return self;
        };
        let (west, north) = mercator(Geo::new(bounds.north, bounds.west));
        let (east, south) = mercator(Geo::new(bounds.south, bounds.east));
        let span_x = (east - west).max(0.0);
        let span_y = (south - north).max(0.0);
        let room_x = (self.width - 2.0 * padding).max(1.0);
        let room_y = (self.height - 2.0 * padding).max(1.0);
        let fit = |room: f64, span: f64| {
            if span > 0.0 {
                (room / (span * TILE_PX)).log2()
            } else {
                f64::INFINITY
            }
        };
        let zoom = fit(room_x, span_x).min(fit(room_y, span_y)).min(max_zoom);
        self.x = ((west + east) / 2.0).rem_euclid(1.0);
        self.y = (north + south) / 2.0;
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self
    }

    #[must_use]
    pub fn tiles(&self, max_source_zoom: u8) -> Vec<TilePlace> {
        if self.width <= 0.0 || self.height <= 0.0 {
            return Vec::new();
        }
        let z = self.zoom.floor().clamp(0.0, f64::from(max_source_zoom)) as u8;
        let count = 1_i64 << z;
        let world = self.world();
        let size = world / count as f64;
        let origin_x = self.width / 2.0 - self.x * world;
        let origin_y = self.height / 2.0 - self.y * world;
        let first_column = ((0.0 - origin_x) / size).floor() as i64;
        let last_column = ((self.width - origin_x) / size).floor() as i64;
        let first_row = (((0.0 - origin_y) / size).floor() as i64).max(0);
        let last_row = (((self.height - origin_y) / size).floor() as i64).min(count - 1);
        let mut out = Vec::new();
        for row in first_row..=last_row {
            for column in first_column..=last_column {
                out.push(TilePlace {
                    id: TileId {
                        z,
                        x: column.rem_euclid(count) as u32,
                        y: row as u32,
                    },
                    column,
                    left: origin_x + column as f64 * size,
                    top: origin_y + row as f64 * size,
                    size,
                });
            }
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

#[must_use]
pub fn unwrap_trail(points: &[Geo]) -> Vec<Geo> {
    let mut out = Vec::with_capacity(points.len());
    let mut previous = points.first().map_or(0.0, |point| point.lon);
    for point in points {
        let lon = point.lon + 360.0 * ((previous - point.lon) / 360.0).round();
        out.push(Geo::new(point.lat, lon));
        previous = lon;
    }
    out
}

#[must_use]
pub fn trail_bounds(points: &[Geo]) -> Option<Bounds> {
    let unwrapped = unwrap_trail(points);
    let first = unwrapped.first()?;
    let mut bounds = Bounds {
        west: first.lon,
        south: first.lat,
        east: first.lon,
        north: first.lat,
    };
    for point in &unwrapped {
        bounds.west = bounds.west.min(point.lon);
        bounds.east = bounds.east.max(point.lon);
        bounds.south = bounds.south.min(point.lat);
        bounds.north = bounds.north.max(point.lat);
    }
    Some(bounds)
}

#[must_use]
pub fn great_circle_km(from: Geo, to: Geo) -> f64 {
    let (lat1, lon1) = (from.lat.to_radians(), from.lon.to_radians());
    let (lat2, lon2) = (to.lat.to_radians(), to.lon.to_radians());
    let sin_lat = ((lat2 - lat1) / 2.0).sin();
    let sin_lon = ((lon2 - lon1) / 2.0).sin();
    let a = sin_lat * sin_lat + lat1.cos() * lat2.cos() * sin_lon * sin_lon;
    2.0 * EARTH_RADIUS_KM * a.sqrt().min(1.0).asin()
}

#[must_use]
pub fn bearing_deg(from: Geo, to: Geo) -> f64 {
    let (lat1, lon1) = (from.lat.to_radians(), from.lon.to_radians());
    let (lat2, lon2) = (to.lat.to_radians(), to.lon.to_radians());
    let d_lon = lon2 - lon1;
    let y = d_lon.sin() * lat2.cos();
    let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * d_lon.cos();
    (y.atan2(x).to_degrees() + 360.0) % 360.0
}

#[must_use]
pub fn along_great_circle(from: Geo, to: Geo, fraction: f64) -> Geo {
    let d = great_circle_km(from, to) / EARTH_RADIUS_KM;
    if d < 1e-9 {
        return from;
    }
    let (lat1, lon1) = (from.lat.to_radians(), from.lon.to_radians());
    let (lat2, lon2) = (to.lat.to_radians(), to.lon.to_radians());
    let a = ((1.0 - fraction) * d).sin() / d.sin();
    let b = (fraction * d).sin() / d.sin();
    let x = a * lat1.cos() * lon1.cos() + b * lat2.cos() * lon2.cos();
    let y = a * lat1.cos() * lon1.sin() + b * lat2.cos() * lon2.sin();
    let z = a * lat1.sin() + b * lat2.sin();
    Geo::new(z.atan2(x.hypot(y)).to_degrees(), y.atan2(x).to_degrees())
}

pub const LINE_STEPS: usize = 32;

#[must_use]
pub fn great_circle_line(from: Geo, to: Geo) -> Vec<Geo> {
    (0..=LINE_STEPS)
        .map(|step| along_great_circle(from, to, step as f64 / LINE_STEPS as f64))
        .collect()
}

#[must_use]
pub fn destination(from: Geo, degrees: f64, distance_m: f64) -> Geo {
    let bearing = degrees.to_radians();
    let angular = distance_m / (EARTH_RADIUS_KM * 1_000.0);
    let phi = from.lat.to_radians();
    let lambda = from.lon.to_radians();
    let sin_phi = phi.sin() * angular.cos() + phi.cos() * angular.sin() * bearing.cos();
    let phi2 = sin_phi.clamp(-1.0, 1.0).asin();
    let lambda2 = lambda
        + (bearing.sin() * angular.sin() * phi.cos()).atan2(angular.cos() - phi.sin() * sin_phi);
    Geo::new(
        phi2.to_degrees(),
        (lambda2.to_degrees() + 540.0) % 360.0 - 180.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, digits: i32) -> bool {
        (a - b).abs() < 0.5 * 10f64.powi(-digits)
    }

    #[test]
    fn route_segments_stay_local_across_the_antimeridian() {
        let unwrapped = unwrap_trail(&[Geo::new(10.0, 179.8), Geo::new(11.0, -179.9)]);
        assert!(close(unwrapped[1].lon, 180.1, 9));
        assert!(close(unwrapped[0].lon, 179.8, 9));
    }

    #[test]
    fn bounds_keep_an_antimeridian_crossing_local() {
        let bounds = trail_bounds(&[
            Geo::new(10.0, 179.8),
            Geo::new(11.0, -179.9),
            Geo::new(12.0, -179.7),
        ])
        .expect("bounds");
        assert!(close(bounds.west, 179.8, 9));
        assert!(close(bounds.east, 180.3, 9));
        assert!(close(bounds.south, 10.0, 9));
        assert!(close(bounds.north, 12.0, 9));
    }

    #[test]
    fn ordinary_bounds_are_preserved() {
        let bounds = trail_bounds(&[Geo::new(52.1, 13.2), Geo::new(52.6, 13.5)]).expect("bounds");
        assert_eq!(
            bounds,
            Bounds {
                west: 13.2,
                south: 52.1,
                east: 13.5,
                north: 52.6
            }
        );
        assert!(trail_bounds(&[]).is_none());
    }

    #[test]
    fn mercator_round_trips() {
        for at in [
            Geo::new(52.5, 13.4),
            Geo::new(-33.9, 151.2),
            Geo::new(0.0, 0.0),
        ] {
            let (x, y) = mercator(at);
            let back = unmercator(x, y);
            assert!(close(back.lat, at.lat, 9) && close(back.lon, at.lon, 9));
        }
        assert_eq!(mercator(Geo::new(0.0, 0.0)), (0.5, 0.5));
    }

    #[test]
    fn the_centre_of_a_view_is_the_middle_of_the_screen() {
        let view = View {
            width: 400.0,
            height: 300.0,
            ..View::default()
        }
        .centred(Geo::new(52.5, 13.4), 8.0);
        let (sx, sy) = view.screen(Geo::new(52.5, 13.4));
        assert!(close(sx, 200.0, 6) && close(sy, 150.0, 6));
        let back = view.geo(sx, sy);
        assert!(close(back.lat, 52.5, 6) && close(back.lon, 13.4, 6));
    }

    #[test]
    fn zooming_keeps_the_point_under_the_pointer() {
        let view = View {
            width: 400.0,
            height: 300.0,
            ..View::default()
        }
        .centred(Geo::new(50.0, 8.0), 6.0);
        let under = view.geo(100.0, 80.0);
        let zoomed = view.zoomed_at(100.0, 80.0, 9.5);
        let (sx, sy) = zoomed.screen(under);
        assert!(close(sx, 100.0, 6) && close(sy, 80.0, 6));
    }

    #[test]
    fn panning_moves_the_map_with_the_pointer() {
        let view = View {
            width: 400.0,
            height: 300.0,
            ..View::default()
        }
        .centred(Geo::new(50.0, 8.0), 6.0);
        let at = Geo::new(50.5, 8.5);
        let (sx, sy) = view.screen(at);
        let (mx, my) = view.panned(30.0, -20.0).screen(at);
        assert!(close(mx - sx, 30.0, 6) && close(my - sy, -20.0, 6));
    }

    #[test]
    fn fitting_frames_every_point_within_the_padding() {
        let points = [Geo::new(52.1, 13.2), Geo::new(52.6, 13.9)];
        let view = View {
            width: 400.0,
            height: 300.0,
            ..View::default()
        }
        .fitted(&points, 56.0, 14.0);
        for point in points {
            let (sx, sy) = view.screen(point);
            assert!((55.9..=344.1).contains(&sx), "{sx}");
            assert!((55.9..=244.1).contains(&sy), "{sy}");
        }
        let single = View {
            width: 400.0,
            height: 300.0,
            ..View::default()
        }
        .fitted(&[Geo::new(1.0, 2.0)], 56.0, 9.0);
        assert!(close(single.zoom, 9.0, 9));
    }

    #[test]
    fn the_tiles_cover_the_screen_and_wrap_around_the_world() {
        let view = View {
            width: 600.0,
            height: 400.0,
            ..View::default()
        }
        .centred(Geo::new(0.0, 179.9), 1.0);
        let tiles = view.tiles(14);
        assert!(
            tiles
                .iter()
                .all(|tile| tile.id.z == 1 && tile.id.x < 2 && tile.id.y < 2)
        );
        assert!(tiles.iter().any(|tile| tile.column == 2 && tile.id.x == 0));
        let left = tiles
            .iter()
            .map(|tile| tile.left)
            .fold(f64::INFINITY, f64::min);
        let right = tiles
            .iter()
            .map(|tile| tile.left + tile.size)
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(left <= 0.0 && right >= 600.0);
        let deep = view.centred(Geo::new(50.0, 8.0), 17.3).tiles(14);
        assert!(deep.iter().all(|tile| tile.id.z == 14));
        assert!(deep.first().is_some_and(|tile| tile.size > TILE_PX * 8.0));
    }

    #[test]
    fn a_tile_has_the_parent_that_contains_it() {
        let tile = TileId { z: 3, x: 5, y: 2 };
        assert_eq!(tile.parent(), Some(TileId { z: 2, x: 2, y: 1 }));
        assert_eq!(TileId { z: 0, x: 0, y: 0 }.parent(), None);
    }

    #[test]
    fn a_line_is_drawn_continuously_across_the_antimeridian() {
        let view = View {
            width: 400.0,
            height: 300.0,
            ..View::default()
        }
        .centred(Geo::new(0.0, 180.0), 3.0);
        let line = view.line(&[Geo::new(0.0, 170.0), Geo::new(0.0, -170.0)]);
        assert!(line[1].0 > line[0].0);
        assert!(line[1].0 - line[0].0 < view.world() / 2.0);
    }

    #[test]
    fn great_circles_measure_the_earth() {
        assert!(close(
            great_circle_km(Geo::new(0.0, 0.0), Geo::new(0.0, 90.0)),
            PI / 2.0 * EARTH_RADIUS_KM,
            3
        ));
        assert!(close(
            great_circle_km(Geo::new(-90.0, 0.0), Geo::new(90.0, 0.0)),
            PI * EARTH_RADIUS_KM,
            3
        ));
        let tokyo = great_circle_km(Geo::new(52.5, 13.0), Geo::new(35.68, 139.77));
        assert!((8_900.0..9_000.0).contains(&tokyo));
    }

    #[test]
    fn bearings_point_along_the_meridian_and_the_equator() {
        assert!(close(
            bearing_deg(Geo::new(0.0, 0.0), Geo::new(10.0, 0.0)),
            0.0,
            6
        ));
        assert!(close(
            bearing_deg(Geo::new(0.0, 0.0), Geo::new(0.0, 10.0)),
            90.0,
            6
        ));
        assert!(close(
            bearing_deg(Geo::new(0.0, 0.0), Geo::new(-10.0, 0.0)),
            180.0,
            6
        ));
    }

    #[test]
    fn a_path_is_halved_at_its_midpoint() {
        let (from, to) = (Geo::new(0.0, 0.0), Geo::new(0.0, 80.0));
        assert!(close(along_great_circle(from, to, 0.5).lon, 40.0, 6));
        assert!(close(along_great_circle(from, to, 0.0).lon, 0.0, 6));
        assert_eq!(along_great_circle(from, from, 0.5), from);
    }

    #[test]
    fn a_great_circle_line_stays_on_the_sphere() {
        let line = great_circle_line(Geo::new(0.0, 0.0), Geo::new(0.0, 80.0));
        assert_eq!(line[0], Geo::new(0.0, 0.0));
        assert!(close(line[line.len() - 1].lon, 80.0, 6));
        assert!(close(line[line.len() / 2].lon, 40.0, 6));
        let still = great_circle_line(Geo::new(10.0, 20.0), Geo::new(10.0, 20.0));
        assert!(still.iter().all(|point| *point == Geo::new(10.0, 20.0)));
    }

    #[test]
    fn a_destination_lies_along_its_bearing() {
        let home = Geo::new(51.5, 7.0);
        let north = destination(home, 0.0, 1_000.0);
        assert!(close(north.lon, home.lon, 6));
        assert!(north.lat > home.lat);
        assert!(destination(home, 90.0, 1_000.0).lon > home.lon);
        assert!(close(great_circle_km(home, north), 1.0, 6));
    }

    #[test]
    fn a_sentinel_position_is_no_position() {
        assert!(Geo::checked(Some(91.0), Some(181.0)).is_none());
        assert!(Geo::checked(Some(52.5), None).is_none());
        assert!(Geo::checked(Some(f64::NAN), Some(1.0)).is_none());
        assert_eq!(Geo::checked(Some(0.0), Some(0.0)), Some(Geo::new(0.0, 0.0)));
    }
}
