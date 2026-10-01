use anyhow::{Context, Result, bail, ensure};

use super::KERNELS;

const RATIO_TOLERANCE: f64 = 0.01;

#[derive(Debug, Default, PartialEq)]
pub struct Measured {
    pub version: Option<String>,
    pub values: Vec<(String, f64)>,
    pub ratios: Vec<(String, f64)>,
}

impl Measured {
    pub fn value(&self, id: &str) -> Option<f64> {
        self.values
            .iter()
            .find(|(kernel, _)| kernel == id)
            .map(|&(_, value)| value)
    }

    fn ratio(&self, id: &str) -> Option<f64> {
        self.ratios
            .iter()
            .find(|(kernel, _)| kernel == id)
            .map(|&(_, value)| value)
    }
}

pub fn parse(text: &str) -> Result<Measured> {
    let mut measured = Measured::default();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            ["version", version] => measured.version = Some((*version).to_owned()),
            ["ratio", id, value] => measured.ratios.push((known(id)?, number(value)?)),
            [id, value] => {
                let id = known(id)?;
                ensure!(measured.value(&id).is_none(), "{id} measured twice");
                measured.values.push((id, number(value)?));
            }
            _ => bail!("unexpected line `{line}`"),
        }
    }
    ensure!(!measured.values.is_empty(), "no kernel was measured");
    Ok(measured)
}

pub fn check_ratios(measured: &Measured) -> Result<()> {
    for kernel in KERNELS {
        let Some(expected) = kernel.ratio else {
            continue;
        };
        if measured.value(kernel.id).is_none() {
            continue;
        }
        let actual = measured
            .ratio(kernel.id)
            .with_context(|| format!("{} reported no output rate", kernel.id))?;
        ensure!(
            (actual / expected - 1.0).abs() <= RATIO_TOLERANCE,
            "{} produced {actual:.6} samples per input, expected {expected:.6}",
            kernel.id
        );
    }
    Ok(())
}

fn known(id: &str) -> Result<String> {
    ensure!(
        KERNELS.iter().any(|kernel| kernel.id == id),
        "unknown kernel `{id}`"
    );
    Ok(id.to_owned())
}

fn number(text: &str) -> Result<f64> {
    let value: f64 = text
        .trim()
        .parse()
        .with_context(|| format!("`{text}` is not a number"))?;
    ensure!(value.is_finite() && value > 0.0, "measured {value}");
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions_values_and_ratios() {
        let measured = parse("version\t1.8.3\nfir\t50.5\nresample\t90\nratio\tresample\t1.0884\n")
            .expect("valid");
        assert_eq!(measured.version.as_deref(), Some("1.8.3"));
        assert_eq!(measured.value("fir"), Some(50.5));
        assert_eq!(measured.value("fm"), None);
        assert!(check_ratios(&measured).is_ok());
    }

    #[test]
    fn refuses_unknown_kernels_and_broken_numbers() {
        assert!(parse("fir2\t1\n").is_err());
        assert!(parse("fir\tNaN\n").is_err());
        assert!(parse("fir\t0\n").is_err());
        assert!(parse("fir\t1\nfir\t2\n").is_err());
        assert!(parse("version\t1\n").is_err());
        assert!(parse("fir 1\n").is_err());
    }

    #[test]
    fn a_wrong_output_rate_is_refused() {
        let wrong = parse("ddc\t100\nratio\tddc\t0.0048\n").expect("valid");
        assert!(check_ratios(&wrong).is_err());
        let missing = parse("ddc\t100\n").expect("valid");
        assert!(check_ratios(&missing).is_err());
    }
}
