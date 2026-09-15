use super::{ffi, types::required_string};

/// An ordered list of `key=value` pairs, in the spelling SoapySDR's own markup uses.
///
/// Held in Rust rather than as a borrowed `SoapySDRKwargs`, so that arguments can be parsed,
/// compared and carried between processes on a machine where no SoapySDR runtime is installed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Args {
    pairs: Vec<(String, String)>,
}

impl Args {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        let value = value.into();
        match self.pairs.iter_mut().find(|(found, _)| *found == key) {
            Some((_, existing)) => *existing = value,
            None => self.pairs.push((key, value)),
        }
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(found, _)| found == key)
            .map(|(_, value)| value.as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.pairs
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    pub(crate) unsafe fn from_kwargs(raw: &ffi::Kwargs) -> Self {
        if raw.size == 0 || raw.keys.is_null() || raw.vals.is_null() {
            return Self::new();
        }
        let keys = unsafe { std::slice::from_raw_parts(raw.keys, raw.size) };
        let values = unsafe { std::slice::from_raw_parts(raw.vals, raw.size) };
        Self {
            pairs: keys
                .iter()
                .zip(values)
                .map(|(&key, &value)| unsafe { (required_string(key), required_string(value)) })
                .collect(),
        }
    }
}

impl<'a> IntoIterator for &'a Args {
    type Item = (&'a str, &'a str);
    type IntoIter = Box<dyn Iterator<Item = (&'a str, &'a str)> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

impl From<&str> for Args {
    fn from(markup: &str) -> Self {
        let mut args = Self::new();
        for pair in markup.split(',') {
            if let Some((key, value)) = pair.split_once('=') {
                args.set(key.trim(), value.trim());
            }
        }
        args
    }
}

impl From<()> for Args {
    fn from((): ()) -> Self {
        Self::new()
    }
}

impl std::fmt::Display for Args {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut pairs = self.iter();
        let Some((key, value)) = pairs.next() else {
            return Ok(());
        };
        write!(formatter, "{key}={value}")?;
        for (key, value) in pairs {
            write!(formatter, ", {key}={value}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_parses_into_ordered_pairs() {
        let args = Args::from("driver=rtlsdr, serial=00000001, label=NESDR SMArt v5");
        assert_eq!(args.get("driver"), Some("rtlsdr"));
        assert_eq!(args.get("serial"), Some("00000001"));
        assert_eq!(args.get("label"), Some("NESDR SMArt v5"));
        assert_eq!(args.get("mode"), None);
    }

    #[test]
    fn markup_round_trips_in_the_spelling_soapy_writes() {
        let markup = "driver=rtlsdr, serial=00000001";
        assert_eq!(Args::from(markup).to_string(), markup);
    }

    #[test]
    fn keys_keep_the_order_the_driver_reported() {
        let args = Args::from("driver=example, serial=123456, mode=DT");
        assert_eq!(
            args.iter().map(|(key, _)| key).collect::<Vec<_>>(),
            ["driver", "serial", "mode"]
        );
    }

    #[test]
    fn a_pair_without_a_value_is_skipped() {
        let args = Args::from("driver=rtlsdr, broken, serial=1");
        assert_eq!(args.to_string(), "driver=rtlsdr, serial=1");
    }

    #[test]
    fn setting_a_key_twice_replaces_it_in_place() {
        let mut args = Args::from("driver=rtlsdr, serial=1");
        args.set("serial", "2");
        assert_eq!(args.to_string(), "driver=rtlsdr, serial=2");
    }

    #[test]
    fn an_empty_list_prints_as_nothing() {
        assert_eq!(Args::new().to_string(), "");
        assert_eq!(Args::from(()).to_string(), "");
    }

    #[test]
    fn a_value_containing_an_equals_sign_keeps_it() {
        assert_eq!(
            Args::from("uri=ip:192.168.2.1=x").get("uri"),
            Some("ip:192.168.2.1=x")
        );
    }
}
