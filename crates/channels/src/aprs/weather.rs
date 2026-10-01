use sdrmm_wire::AprsWeather;

const MPH_TO_MS: f32 = 0.447_04;
const KNOT_TO_MS: f32 = 0.514_444;
const HUNDREDTH_INCH_TO_MM: f32 = 0.254;
const INCH_TO_MM: f32 = 25.4;
const TIMESTAMP_LEN: usize = 8;
const WIND_LEN: usize = 7;
const LUMINOSITY_ABOVE_999: i32 = 1_000;

#[derive(Clone, Copy)]
pub(super) enum WindUnit {
    Mph,
    Knots,
}

impl WindUnit {
    fn to_ms(self, value: f64) -> f32 {
        let value = value as f32;
        match self {
            Self::Mph => value * MPH_TO_MS,
            Self::Knots => value * KNOT_TO_MS,
        }
    }
}

pub(super) fn is_station(symbol: &str) -> bool {
    symbol.ends_with('_')
}

pub(super) fn positionless(body: &[u8]) -> Option<AprsWeather> {
    let fields = body.get(TIMESTAMP_LEN..)?;
    let mut weather = AprsWeather::default();
    let (direction, rest) = tagged_value(fields, b'c', 3)?;
    let (speed, rest) = tagged_value(rest, b's', 3)?;
    weather.wind_dir_deg = direction.and_then(|d| u16::try_from(d).ok());
    weather.wind_speed_ms = speed.map(|s| WindUnit::Mph.to_ms(f64::from(s)));
    read_fields(rest, &mut weather);
    reported(weather)
}

pub(super) fn with_position(
    course_deg: Option<f64>,
    speed: Option<f64>,
    unit: WindUnit,
    comment: Option<&str>,
) -> (Option<AprsWeather>, Option<String>) {
    let mut weather = AprsWeather::default();
    weather.wind_dir_deg = course_deg.map(|c| c.round() as u16);
    weather.wind_speed_ms = speed.map(|s| unit.to_ms(s));
    let text = comment.unwrap_or_default().as_bytes();
    let text = match leading_wind(text) {
        Some((direction, speed, rest)) => {
            weather.wind_dir_deg = direction.and_then(|d| u16::try_from(d).ok());
            weather.wind_speed_ms = speed.map(|s| WindUnit::Mph.to_ms(f64::from(s)));
            rest
        }
        None => text,
    };
    let rest = read_fields(text, &mut weather);
    let remaining = String::from_utf8_lossy(rest).trim().to_owned();
    (
        reported(weather),
        (!remaining.is_empty()).then_some(remaining),
    )
}

fn leading_wind(text: &[u8]) -> Option<(Option<i32>, Option<i32>, &[u8])> {
    let field = text.get(..WIND_LEN)?;
    if field.get(3) != Some(&b'/') {
        return None;
    }
    let direction = value(field.get(..3)?)?;
    let speed = value(field.get(4..)?)?;
    Some((direction, speed, text.get(WIND_LEN..).unwrap_or_default()))
}

fn read_fields<'a>(mut rest: &'a [u8], weather: &mut AprsWeather) -> &'a [u8] {
    while let Some((&tag, after)) = rest.split_first() {
        let Some(width) = field_width(tag) else {
            break;
        };
        let Some(reading) = after.get(..width).and_then(value) else {
            break;
        };
        store(tag, reading, weather);
        rest = after.get(width..).unwrap_or_default();
    }
    rest
}

fn field_width(tag: u8) -> Option<usize> {
    match tag {
        b'g' | b't' | b'r' | b'p' | b'P' | b'L' | b'l' | b's' | b'#' => Some(3),
        b'h' => Some(2),
        b'b' => Some(5),
        _ => None,
    }
}

fn store(tag: u8, reading: Option<i32>, weather: &mut AprsWeather) {
    let Some(raw) = reading else {
        return;
    };
    let number = raw as f32;
    match tag {
        b'g' => weather.wind_gust_ms = Some(number * MPH_TO_MS),
        b't' => weather.temperature_c = Some((number - 32.0) * 5.0 / 9.0),
        b'r' => weather.rain_1h_mm = Some(number * HUNDREDTH_INCH_TO_MM),
        b'p' => weather.rain_24h_mm = Some(number * HUNDREDTH_INCH_TO_MM),
        b'P' => weather.rain_midnight_mm = Some(number * HUNDREDTH_INCH_TO_MM),
        b'h' => weather.humidity_pct = Some(if raw == 0 { 100 } else { raw as u8 }),
        b'b' => weather.pressure_hpa = Some(number / 10.0),
        b'L' => weather.luminosity_wm2 = u16::try_from(raw).ok(),
        b'l' => weather.luminosity_wm2 = u16::try_from(raw + LUMINOSITY_ABOVE_999).ok(),
        b's' => weather.snow_24h_mm = Some(number * INCH_TO_MM),
        _ => {}
    }
}

fn tagged_value(field: &[u8], tag: u8, width: usize) -> Option<(Option<i32>, &[u8])> {
    let (&found, after) = field.split_first()?;
    if found != tag {
        return None;
    }
    let reading = value(after.get(..width)?)?;
    Some((reading, after.get(width..).unwrap_or_default()))
}

fn value(field: &[u8]) -> Option<Option<i32>> {
    if field.iter().all(|&b| b == b'.' || b == b' ') {
        return Some(None);
    }
    let (negative, digits) = match field.split_first()? {
        (b'-', digits) => (true, digits),
        _ => (false, field),
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let magnitude = digits
        .iter()
        .fold(0i32, |acc, &d| acc * 10 + i32::from(d - b'0'));
    Some(Some(if negative { -magnitude } else { magnitude }))
}

fn reported(weather: AprsWeather) -> Option<AprsWeather> {
    (weather != AprsWeather::default()).then_some(weather)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: Option<f32>, expected: f32) {
        let actual = actual.expect("value present");
        assert!(
            (actual - expected).abs() < 0.05,
            "{actual} against {expected}"
        );
    }

    #[test]
    fn a_positionless_report_reads_every_field() {
        let weather =
            positionless(b"10090556c220s004g005t077r001p002P003h50b09900wRSW").expect("weather");
        assert_eq!(weather.wind_dir_deg, Some(220));
        close(weather.wind_speed_ms, 1.788);
        close(weather.wind_gust_ms, 2.235);
        close(weather.temperature_c, 25.0);
        close(weather.rain_1h_mm, 0.254);
        close(weather.rain_24h_mm, 0.508);
        close(weather.rain_midnight_mm, 0.762);
        assert_eq!(weather.humidity_pct, Some(50));
        close(weather.pressure_hpa, 990.0);
    }

    #[test]
    fn missing_fields_stay_missing() {
        let weather = positionless(b"10090556c...s...g...t-07h00").expect("weather");
        assert_eq!(weather.wind_dir_deg, None);
        assert_eq!(weather.wind_speed_ms, None);
        assert_eq!(weather.wind_gust_ms, None);
        close(weather.temperature_c, -21.67);
        assert_eq!(weather.humidity_pct, Some(100));
        assert!(positionless(b"10090556c...s...g...t...").is_none());
    }

    #[test]
    fn a_positioned_report_takes_wind_from_the_extension_and_keeps_the_rest() {
        let (weather, comment) = with_position(
            Some(220.0),
            Some(4.0),
            WindUnit::Mph,
            Some("g005t077r000p000P000h50b09900L456wRSW"),
        );
        let weather = weather.expect("weather");
        assert_eq!(weather.wind_dir_deg, Some(220));
        close(weather.wind_speed_ms, 1.788);
        close(weather.temperature_c, 25.0);
        assert_eq!(weather.luminosity_wm2, Some(456));
        assert_eq!(comment.as_deref(), Some("wRSW"));
    }

    #[test]
    fn a_wind_extension_with_dots_is_read_from_the_comment() {
        let (weather, comment) = with_position(None, None, WindUnit::Mph, Some(".../...t050l012"));
        let weather = weather.expect("weather");
        assert_eq!(weather.wind_dir_deg, None);
        close(weather.temperature_c, 10.0);
        assert_eq!(weather.luminosity_wm2, Some(1_012));
        assert_eq!(comment, None);
    }

    #[test]
    fn compressed_wind_is_in_knots() {
        let (weather, _) = with_position(Some(88.0), Some(36.2), WindUnit::Knots, Some("t077"));
        close(weather.expect("weather").wind_speed_ms, 18.62);
    }

    #[test]
    fn a_weather_symbol_without_readings_reports_nothing() {
        let (weather, comment) = with_position(None, None, WindUnit::Mph, Some("Station"));
        assert!(weather.is_none());
        assert_eq!(comment.as_deref(), Some("Station"));
    }
}
