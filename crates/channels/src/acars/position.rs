use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Position {
    pub latitude: f64,
    pub longitude: f64,
}

fn dir_sign(c: u8) -> Option<f64> {
    match c {
        b'N' | b'E' => Some(1.0),
        b'S' | b'W' => Some(-1.0),
        _ => None,
    }
}

fn split_coord(s: &str) -> Option<(u8, &str, u8, &str)> {
    let b = s.as_bytes();
    if b.len() < 13 {
        return None;
    }
    let lat_dir = b[0];
    let lat_digits = s.get(1..6)?;
    let (lon_dir, lon_digits) = if b[6] == b' ' {
        if b.len() < 14 {
            return None;
        }
        (b[7], s.get(8..14)?)
    } else {
        (b[6], s.get(7..13)?)
    };
    Some((lat_dir, lat_digits, lon_dir, lon_digits))
}

pub fn decode_scaled(s: &str) -> Option<Position> {
    let (lat_dir, lat_digits, lon_dir, lon_digits) = split_coord(s)?;
    let lat_sign = dir_sign(lat_dir)?;
    let lon_sign = dir_sign(lon_dir)?;
    if !(lat_dir == b'N' || lat_dir == b'S') || !(lon_dir == b'W' || lon_dir == b'E') {
        return None;
    }
    let lat: f64 = lat_digits.parse().ok()?;
    let lon: f64 = lon_digits.parse().ok()?;
    Some(Position {
        latitude: (lat / 1000.0) * lat_sign,
        longitude: (lon / 1000.0) * lon_sign,
    })
}

pub fn decode_decimal_minutes(s: &str) -> Option<Position> {
    let (lat_dir, lat_digits, lon_dir, lon_digits) = split_coord(s)?;
    let lat_sign = dir_sign(lat_dir)?;
    let lon_sign = dir_sign(lon_dir)?;
    if !(lat_dir == b'N' || lat_dir == b'S') || !(lon_dir == b'W' || lon_dir == b'E') {
        return None;
    }
    let lat_raw: f64 = lat_digits.parse().ok()?;
    let lon_raw: f64 = lon_digits.parse().ok()?;
    let lat_deg = (lat_raw / 1000.0).trunc();
    let lat_min = (lat_raw % 1000.0) / 10.0;
    let lon_deg = (lon_raw / 1000.0).trunc();
    let lon_min = (lon_raw % 1000.0) / 10.0;
    Some(Position {
        latitude: (lat_deg + lat_min / 60.0) * lat_sign,
        longitude: (lon_deg + lon_min / 60.0) * lon_sign,
    })
}

fn decode_literal_dot(s: &str) -> Option<Position> {
    let b = s.as_bytes();
    if b.is_empty() {
        return None;
    }
    let lat_sign = dir_sign(b[0])?;
    if b[0] != b'N' && b[0] != b'S' {
        return None;
    }
    let lon_pos = s[1..].find(['E', 'W'])? + 1;
    let lat_field = &s[1..lon_pos];
    let lon_sign = dir_sign(b[lon_pos])?;
    let lon_field = &s[lon_pos + 1..];

    let lat = parse_deg_min(lat_field, 2)?;
    let lon = parse_deg_min(lon_field, 3)?;
    Some(Position {
        latitude: lat * lat_sign,
        longitude: lon * lon_sign,
    })
}

fn parse_deg_min(field: &str, deg_digits: usize) -> Option<f64> {
    if field.len() <= deg_digits {
        return None;
    }
    let deg: f64 = field.get(..deg_digits)?.parse().ok()?;
    let min: f64 = field.get(deg_digits..)?.parse().ok()?;
    if min >= 60.0 {
        return None;
    }
    Some(deg + min / 60.0)
}

pub fn decode(label: &str, text: &str) -> Option<Position> {
    match label {
        "20" => {
            let body = text.strip_prefix("POS")?;
            let first = body.split(',').next()?;
            decode_scaled(first)
        }
        "H1" => {
            let body = text.strip_prefix("POS")?;
            let first = body.split(',').next()?;
            decode_decimal_minutes(first)
        }
        "4J" => decode_4j(text),
        _ => None,
    }
}

fn decode_4j(text: &str) -> Option<Position> {
    for part in text.split('/') {
        if let Some(rest) = part.strip_prefix("PS") {
            let coord = rest.split(',').next()?.trim();
            if let Some(p) = decode_decimal_minutes(coord) {
                return Some(p);
            }
        }
        if let Some(rest) = part.strip_prefix("POS") {
            let coord = rest.split(',').next().unwrap_or(rest).trim();
            if let Some(p) = decode_literal_dot(coord) {
                return Some(p);
            }
            if let Some(p) = decode_decimal_minutes(coord) {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn label_20_pos_scaled() {
        let p = decode(
            "20",
            "POSN38160W077075,,211733,360,OTT,212041,,N42,19689,40,544",
        )
        .unwrap();
        assert!(close(p.latitude, 38.160), "lat {}", p.latitude);
        assert!(close(p.longitude, -77.075), "lon {}", p.longitude);
    }

    #[test]
    fn label_20_pos_east_longitude() {
        let p = decode("20", "POSN32249E045047,,082806,380,DEBNI").unwrap();
        assert!(close(p.latitude, 32.249));
        assert!(close(p.longitude, 45.047));
    }

    #[test]
    fn h1_pos_decimal_minutes() {
        let p = decode(
            "H1",
            "POSN43312W123174,EASON,215754,370,EBINY,220601,ELENN,M48,02216,185/TS215754,0921227A40",
        )
        .unwrap();
        assert!(close(p.latitude, 43.52), "lat {}", p.latitude);
        assert!(close(p.longitude, -123.29), "lon {}", p.longitude);
    }

    #[test]
    fn h1_pos_variant_2() {
        let p = decode(
            "H1",
            "POSN45209W122550,PEGTY,220309,134,MINNE,220424,HISKU,M6,060013,269,366,355K,292K,730A5B",
        )
        .unwrap();
        assert!(close(p.latitude, 45.348), "lat {}", p.latitude);
        assert!(close(p.longitude, -122.917), "lon {}", p.longitude);
    }

    #[test]
    fn label_4j_packed_ps() {
        let p = decode(
            "4J",
            "POS/ID91459S,BANKR31,/DC03032024,142813/MR64,0/ET31539/PSN39277W077359,142800,240,N39300W077110,031430,N38560W077150,M28,27619,MT370/CG311,160,350/FB732/VR329071",
        )
        .unwrap();
        assert!(close(p.latitude, 39.462), "lat {}", p.latitude);
        assert!(close(p.longitude, -77.598), "lon {}", p.longitude);
    }

    #[test]
    fn label_4j_legacy_literal_dot() {
        let p = decode(
            "4J",
            "4J01 POSWX 0318/20 ETAD/ETAD .00318S\n/POS N5043.5E01121.8/OVR 0817",
        )
        .unwrap();
        assert!(close(p.latitude, 50.0 + 43.5 / 60.0), "lat {}", p.latitude);
        assert!(
            close(p.longitude, 11.0 + 21.8 / 60.0),
            "lon {}",
            p.longitude
        );
    }

    #[test]
    fn rejects_non_position() {
        assert!(decode("20", "RST something").is_none());
        assert!(decode("H1", "#DFB engine data").is_none());
        assert!(decode("Q0", "").is_none());
        assert!(decode("4J", "no position here").is_none());
    }

    #[test]
    fn scaled_vs_decimal_minutes_differ() {
        let scaled = decode_scaled("N43312W123174").unwrap();
        let dm = decode_decimal_minutes("N43312W123174").unwrap();
        assert!(close(scaled.latitude, 43.312));
        assert!(close(dm.latitude, 43.52));
    }
}
