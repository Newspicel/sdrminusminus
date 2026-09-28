use std::fmt;

use sdrmm_device::{DeviceError, net::Endpoint};

use crate::proto::PORT;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Address {
    pub endpoint: Endpoint,
    pub password: String,
}

impl Address {
    pub(crate) fn parse(key: &str) -> Result<Self, DeviceError> {
        let key = key.trim();
        let (scheme, rest) = key
            .split_once("://")
            .map_or(("", key), |(scheme, rest)| (scheme, rest));
        let (password, host) = rest.rsplit_once('@').unwrap_or(("", rest));
        let host = host.trim_end_matches('/');
        let origin = if scheme.is_empty() {
            host.to_string()
        } else {
            format!("{scheme}://{host}")
        };
        Ok(Self {
            endpoint: Endpoint::parse_web(&origin, PORT)?,
            password: password.to_string(),
        })
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let origin = self.endpoint.to_string();
        match (self.password.is_empty(), origin.split_once("://")) {
            (true, _) => f.write_str(&origin),
            (false, Some((scheme, host))) => write!(f, "{scheme}://{}@{host}", self.password),
            (false, None) => write!(f, "{}@{origin}", self.password),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_host_gets_the_kiwi_port_and_no_password() {
        let address = Address::parse("kiwi.local").expect("parses");
        assert_eq!(address.to_string(), "kiwi.local:8073");
        assert!(address.password.is_empty());
    }

    #[test]
    fn a_password_rides_in_front_of_the_host() {
        let address = Address::parse("se@cr@et@kiwi.local:8074/").expect("parses");
        assert_eq!(address.password, "se@cr@et");
        assert_eq!(address.endpoint.to_string(), "kiwi.local:8074");
        assert_eq!(address.to_string(), "se@cr@et@kiwi.local:8074");
    }

    #[test]
    fn https_keeps_its_scheme_around_the_password() {
        let address = Address::parse("https://pw@kiwi.example/").expect("parses");
        assert!(address.endpoint.secure());
        assert_eq!(address.password, "pw");
        assert_eq!(address.to_string(), "https://pw@kiwi.example:443");
        assert_eq!(
            Address::parse(&address.to_string()).expect("round trips"),
            address
        );
        assert_eq!(
            Address::parse("http://kiwi.example")
                .expect("parses")
                .to_string(),
            "kiwi.example:80"
        );
    }

    #[test]
    fn a_bad_port_is_refused() {
        assert!(Address::parse("pw@kiwi.local:port").is_err());
    }
}
