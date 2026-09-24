
use serde_json::{json, Value};

fn xyz(bytes: &[u8], skip: u32) -> Option<(f64, f64, i32, i32, i32, i32)> {
    if bytes.len() < 5 {
        return None;
    }
    let mut val: u64 = 0;
    for &b in &bytes[..5] {
        val = (val << 8) | b as u64;
    }
    let sb = 4 - skip;
    let ext = |f: u64| -> i32 {
        let v = (f & 0xfff) as i32;
        if v > 0x7ff {
            v - 0x1000
        } else {
            v
        }
    };
    let x = ext(val >> (24 + sb));
    let y = ext(val >> (12 + sb));
    let z = ext(val >> sb);
    if x == 0 && y == 0 && z == 0 {
        return None;
    }
    let (xf, yf, zf) = (x as f64, y as f64, z as f64);
    let lat = zf.atan2((xf * xf + yf * yf).sqrt()).to_degrees();
    let lon = yf.atan2(xf).to_degrees();
    let radius_km = (xf * xf + yf * yf + zf * zf).sqrt() * 4.0;
    let alt = (radius_km - 6371.0) as i32;
    if !(-90.0..=90.0).contains(&lat) {
        return None;
    }
    if !(5000.0..=7000.0).contains(&radius_km) {
        return None;
    }
    Some((lat, lon, alt, x, y, z))
}

pub fn extract(data: &[u8], ul: bool) -> Option<Value> {
    if data.len() < 5 {
        return None;
    }
    let msg_type = ((data[0] as u16) << 8) | data[1] as u16;
    let (src, skip): (&[u8], u32) = match msg_type {
        0x0605 if data.len() >= 42 && data[36] == 0x1b => (&data[37..], 0),
        0x7605 if data.len() >= 8 && data[2] == 0x00 && (data[3] & 0xf0) == 0x40 => {
            (&data[3..], 4)
        }
        0x0600
            if ul
                && data.len() >= 24
                && matches!(data[2], 0x10 | 0x40 | 0x70)
                && data[18] == 0x01 =>
        {
            (&data[19..], 0)
        }
        _ => return None,
    };
    let (lat, lon, alt, x, y, z) = xyz(src, skip)?;
    Some(json!({
        "type": "mt-position",
        "msg_type": format!("{msg_type:04x}"),
        "lat": lat,
        "lon": lon,
        "alt_km": alt,
        "x": x,
        "y": y,
        "z": z,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_short_and_zero() {
        assert!(extract(&[0u8; 4], false).is_none());
        let mut d = vec![0u8; 42];
        d[0] = 0x06;
        d[1] = 0x05;
        d[36] = 0x1b;
        assert!(extract(&d, false).is_none());
    }

    #[test]
    fn xyz_unpacks_signed_12bit() {
        let (x, y, z) = (1000i64, 1000i64, 700i64);
        let val: u64 = ((x as u64) << 28) | ((y as u64) << 16) | ((z as u64) << 4);
        let bytes = [
            (val >> 32) as u8,
            (val >> 24) as u8,
            (val >> 16) as u8,
            (val >> 8) as u8,
            val as u8,
        ];
        let (lat, lon, _alt, rx, ry, rz) = xyz(&bytes, 0).expect("valid");
        assert_eq!((rx, ry, rz), (1000, 1000, 700));
        assert!((lat - 26.34).abs() < 0.1, "lat={lat}");
        assert!((lon - 45.0).abs() < 0.1, "lon={lon}");
    }

    #[test]
    fn xyz_negative_components() {
        let neg = ((-300i32) & 0xfff) as u64;
        let val: u64 = (neg << 28) | (1200u64 << 16) | (700u64 << 4);
        let bytes = [
            (val >> 32) as u8,
            (val >> 24) as u8,
            (val >> 16) as u8,
            (val >> 8) as u8,
            val as u8,
        ];
        let (_, _, _, rx, ry, rz) = xyz(&bytes, 0).expect("valid");
        assert_eq!((rx, ry, rz), (-300, 1200, 700));
    }
}
