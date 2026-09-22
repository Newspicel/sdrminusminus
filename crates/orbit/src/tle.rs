use crate::{OrbitError, time::julian_date_of_year};

const LINE_LEN: usize = 69;

#[derive(Clone, Debug, PartialEq)]
pub struct Tle {
    pub name: Option<String>,
    pub catalog: String,
    pub epoch_jd: f64,
    pub bstar: f64,
    pub inclination_deg: f64,
    pub raan_deg: f64,
    pub eccentricity: f64,
    pub arg_perigee_deg: f64,
    pub mean_anomaly_deg: f64,
    pub mean_motion_rev_per_day: f64,
}

impl Tle {
    pub fn parse(text: &str) -> Result<Self, OrbitError> {
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.trim().is_empty())
            .collect();
        match lines.as_slice() {
            [first, second] => Self::from_lines(None, first, second),
            [name, first, second] => Self::from_lines(Some(name), first, second),
            _ => Err(OrbitError::Tle(
                "expected two element lines, optionally after a name",
            )),
        }
    }

    pub fn parse_many(text: &str) -> Vec<Result<Self, OrbitError>> {
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.trim().is_empty())
            .collect();
        let mut out = Vec::new();
        let mut index = 0;
        while index < lines.len() {
            let named = !lines[index].starts_with("1 ");
            let start = index + usize::from(named);
            let Some(pair) = lines.get(start..start + 2) else {
                out.push(Err(OrbitError::Tle("a set ends before its second line")));
                break;
            };
            let name = named.then(|| lines[index]);
            out.push(Self::from_lines(name, pair[0], pair[1]));
            index = start + 2;
        }
        out
    }

    fn from_lines(name: Option<&str>, first: &str, second: &str) -> Result<Self, OrbitError> {
        let first = checked(first, '1')?;
        let second = checked(second, '2')?;
        let catalog = field(first, 2, 7).trim().to_owned();
        if catalog != field(second, 2, 7).trim() {
            return Err(OrbitError::Tle("the two lines name different satellites"));
        }
        let year = number(field(first, 18, 20))? as i32;
        let year = if year < 57 { 2000 + year } else { 1900 + year };
        let day = number(field(first, 20, 32))?;
        Ok(Self {
            name: name
                .map(|name| name.trim().trim_start_matches("0 ").trim().to_owned())
                .filter(|name| !name.is_empty()),
            catalog,
            epoch_jd: julian_date_of_year(year) + day - 1.0,
            bstar: exponent(field(first, 53, 61))?,
            inclination_deg: number(field(second, 8, 16))?,
            raan_deg: number(field(second, 17, 25))?,
            eccentricity: number(&format!("0.{}", field(second, 26, 33).trim()))?,
            arg_perigee_deg: number(field(second, 34, 42))?,
            mean_anomaly_deg: number(field(second, 43, 51))?,
            mean_motion_rev_per_day: number(field(second, 52, 63))?,
        })
    }

    #[must_use]
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.catalog)
    }
}

fn checked(line: &str, number: char) -> Result<&str, OrbitError> {
    if line.len() < LINE_LEN || !line.is_ascii() {
        return Err(OrbitError::Tle(
            "an element line is shorter than 69 characters",
        ));
    }
    let line = &line[..LINE_LEN];
    if !line.starts_with(number) || line.as_bytes()[1] != b' ' {
        return Err(OrbitError::Tle("element lines are out of order"));
    }
    let sum: u32 = line[..LINE_LEN - 1]
        .chars()
        .map(|c| match c {
            '-' => 1,
            _ => c.to_digit(10).unwrap_or(0),
        })
        .sum();
    let expected = line[LINE_LEN - 1..]
        .chars()
        .next()
        .and_then(|c| c.to_digit(10));
    if expected != Some(sum % 10) {
        return Err(OrbitError::Tle("an element line fails its checksum"));
    }
    Ok(line)
}

fn field(line: &str, from: usize, to: usize) -> &str {
    &line[from..to]
}

fn number(text: &str) -> Result<f64, OrbitError> {
    text.trim()
        .parse()
        .map_err(|_| OrbitError::Tle("an element field is not a number"))
}

fn exponent(text: &str) -> Result<f64, OrbitError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(0.0);
    }
    let split = text
        .rfind(['-', '+'])
        .filter(|&at| at > 0)
        .ok_or(OrbitError::Tle("a drag term has no exponent"))?;
    let (mantissa, power) = text.split_at(split);
    let (sign, digits) = match mantissa.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, mantissa.trim_start_matches('+')),
    };
    let mantissa: f64 = number(&format!("0.{digits}"))?;
    let power: i32 = power
        .parse()
        .map_err(|_| OrbitError::Tle("a drag exponent is not a number"))?;
    Ok(sign * mantissa * 10f64.powi(power))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const ISS: &str = "ISS (ZARYA)
1 25544U 98067A   24001.50000000  .00016717  00000-0  30306-3 0  9999
2 25544  51.6416 247.4627 0006703 130.5360 325.0288 15.50377579432041";

    #[test]
    fn a_named_set_reads_every_field() {
        let tle = Tle::parse(ISS).expect("valid set");
        assert_eq!(tle.label(), "ISS (ZARYA)");
        assert_eq!(tle.catalog, "25544");
        assert!(
            (tle.epoch_jd - 2_460_311.0).abs() < 1e-9,
            "{}",
            tle.epoch_jd
        );
        assert!((tle.bstar - 3.0306e-4).abs() < 1e-12);
        assert!((tle.eccentricity - 0.000_670_3).abs() < 1e-12);
        assert!((tle.mean_motion_rev_per_day - 15.503_775_79).abs() < 1e-9);
    }

    #[test]
    fn a_broken_checksum_is_refused() {
        let broken = ISS.replace("9999\n", "9998\n");
        assert!(Tle::parse(&broken).is_err());
    }

    #[test]
    fn a_catalog_file_yields_every_set() {
        let two = format!("{ISS}\n{}", ISS.replace("ISS (ZARYA)", "COPY"));
        let sets = Tle::parse_many(&two);
        assert_eq!(sets.len(), 2);
        assert_eq!(
            sets[1].as_ref().map(|tle| tle.label().to_owned()).ok(),
            Some("COPY".to_owned())
        );
    }

    #[test]
    fn negative_drag_terms_keep_their_sign() {
        assert!((exponent("-11606-4").unwrap() + 1.1606e-5).abs() < 1e-15);
        assert_eq!(exponent(" 00000+0").unwrap(), 0.0);
    }
}
