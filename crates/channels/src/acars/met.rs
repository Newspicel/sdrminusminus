use serde::Serialize;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Met {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wind_dir_deg: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wind_speed_kt: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature_c: Option<i16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub true_airspeed_kt: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub altitude_ft: Option<u32>,
}

impl Met {
    fn is_empty(&self) -> bool {
        self.wind_dir_deg.is_none()
            && self.wind_speed_kt.is_none()
            && self.temperature_c.is_none()
            && self.true_airspeed_kt.is_none()
            && self.altitude_ft.is_none()
    }
}

fn parse_temp(s: &str) -> Option<i16> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let normalized = s.replace('M', "-").replace('P', "+");
    normalized.parse::<i16>().ok()
}

fn parse_wind(s: &str) -> (Option<u16>, Option<u16>) {
    let s = s.trim();
    if s.len() < 5 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return (None, None);
    }
    let dir = s[..3].parse::<u16>().ok().filter(|&d| d <= 360);
    let speed = s[3..].parse::<u16>().ok();
    (dir, speed)
}

pub fn decode(label: &str, text: &str) -> Option<Met> {
    if label != "4J" {
        return None;
    }
    let mut m = Met::default();
    for part in text.split('/') {
        if let Some(v) = part
            .strip_prefix("WND ")
            .or_else(|| part.strip_prefix("WND"))
        {
            let (dir, spd) = parse_wind(v);
            m.wind_dir_deg = dir;
            m.wind_speed_kt = spd;
        } else if let Some(v) = part
            .strip_prefix("SAT ")
            .or_else(|| part.strip_prefix("SAT"))
        {
            m.temperature_c = parse_temp(v);
        } else if let Some(v) = part
            .strip_prefix("TAS ")
            .or_else(|| part.strip_prefix("TAS"))
        {
            m.true_airspeed_kt = v.trim().parse().ok();
        } else if let Some(v) = part
            .strip_prefix("ALT ")
            .or_else(|| part.strip_prefix("ALT"))
            && let Ok(fl) = v.trim().parse::<u32>()
        {
            m.altitude_ft = Some(fl * 100);
        }
    }
    if m.is_empty() { None } else { Some(m) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POSWX_4J: &str = "4J01 POSWX 0318/20 ETAD/ETAD .00318S\n\
        /POS N5043.5E01121.8/OVR 0817\n\
        /ALT 270/TFW 1342/TAS 490/SAT -032\n\
        /POS GOVEN /OVR 0835\n\
        /POS DILVI\n\
        /WND 334060/TRB /SKY DCC3";

    #[test]
    fn poswx_met_fields() {
        let m = decode("4J", POSWX_4J).unwrap();
        assert_eq!(m.wind_dir_deg, Some(334));
        assert_eq!(m.wind_speed_kt, Some(60));
        assert_eq!(m.temperature_c, Some(-32));
        assert_eq!(m.true_airspeed_kt, Some(490));
        assert_eq!(m.altitude_ft, Some(27000));
    }

    #[test]
    fn temperature_sign_conventions() {
        assert_eq!(parse_temp("M48"), Some(-48));
        assert_eq!(parse_temp("-032"), Some(-32));
        assert_eq!(parse_temp("P15"), Some(15));
        assert_eq!(parse_temp("020"), Some(20));
        assert_eq!(parse_temp(""), None);
    }

    #[test]
    fn non_4j_is_none() {
        assert!(decode("H1", "#CFBFLR/something").is_none());
        assert!(decode("20", "POSN38160W077075").is_none());
    }

    #[test]
    fn report_without_met_is_none() {
        assert!(decode("4J", "POS/ID91459S,BANKR31,/DC03032024").is_none());
    }
}
