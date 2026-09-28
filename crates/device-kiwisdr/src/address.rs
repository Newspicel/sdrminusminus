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
        let (password, host) = key.rsplit_once('@').unwrap_or(("", key));
        let host = host.strip_suffix('/').unwrap_or(host);
        Ok(Self {
            endpoint: Endpoint::parse(host, PORT)?,
            password: password.to_string(),
        })
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.password.is_empty() {
            write!(f, "{}@", self.password)?;
        }
        write!(f, "{}", self.endpoint)
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
    fn a_bad_port_is_refused() {
        assert!(Address::parse("pw@kiwi.local:port").is_err());
    }
}
