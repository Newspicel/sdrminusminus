use std::{
    fmt,
    net::{TcpStream, ToSocketAddrs},
    time::Duration,
};

use sdrmm_device::DeviceError;

pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Endpoint {
    host: String,
    port: u16,
    bracketed: bool,
}

impl Endpoint {
    pub fn parse(key: &str, default_port: u16) -> Result<Self, DeviceError> {
        let refused = |why: &str| DeviceError::NotFound(format!("{key}: {why}"));
        let key = key.trim();
        let (host, port) = match key.strip_prefix('[') {
            Some(rest) => {
                let (inside, after) = rest.split_once(']').ok_or_else(|| refused("unclosed ["))?;
                match after.strip_prefix(':') {
                    Some(port) => (inside, Some(port)),
                    None if after.is_empty() => (inside, None),
                    None => return Err(refused("expected :port after ]")),
                }
            }
            None if key.matches(':').count() > 1 => (key, None),
            None => match key.split_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (key, None),
            },
        };
        if host.is_empty() {
            return Err(refused("no host"));
        }
        let port = match port {
            Some(port) => port
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(|| refused("port must be 1-65535"))?,
            None => default_port,
        };
        Ok(Self {
            bracketed: host.contains(':'),
            host: host.to_ascii_lowercase(),
            port,
        })
    }

    pub(crate) fn connect(&self) -> Result<TcpStream, DeviceError> {
        let addrs = (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|e| DeviceError::NotFound(format!("{self}: {e}")))?;
        let mut last = None;
        for addr in addrs {
            match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                Ok(stream) => {
                    let _ = stream.set_nodelay(true);
                    return Ok(stream);
                }
                Err(e) => last = Some(e),
            }
        }
        Err(DeviceError::Io(match last {
            Some(e) => format!("{self}: {e}"),
            None => format!("{self}: host resolved to no address"),
        }))
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.bracketed {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> String {
        Endpoint::parse(text, 1234).expect("parses").to_string()
    }

    #[test]
    fn a_bare_host_takes_the_default_port() {
        assert_eq!(key("radio.local"), "radio.local:1234");
        assert_eq!(key("192.168.1.5"), "192.168.1.5:1234");
    }

    #[test]
    fn an_explicit_port_wins() {
        assert_eq!(key("192.168.1.5:5555"), "192.168.1.5:5555");
        assert_eq!(key("radio.local:1"), "radio.local:1");
        assert_eq!(key("radio.local:65535"), "radio.local:65535");
    }

    #[test]
    fn ipv6_literals_round_trip_bracketed() {
        assert_eq!(key("[::1]:5555"), "[::1]:5555");
        assert_eq!(key("[::1]"), "[::1]:1234");
        assert_eq!(key("::1"), "[::1]:1234");
        assert_eq!(key("[2001:db8::1]:1234"), "[2001:db8::1]:1234");
        assert_eq!(
            key("[::1]:5555"),
            Endpoint::parse("[::1]:5555", 9999)
                .expect("parses")
                .to_string(),
            "an explicit port ignores the default"
        );
    }

    #[test]
    fn hosts_canonicalize_to_one_key() {
        assert_eq!(key(" Radio.Local:1234 "), "radio.local:1234");
        assert_eq!(key("RADIO.local"), key("radio.local:1234"));
    }

    #[test]
    fn unaddressable_keys_are_refused_by_name() {
        for bad in [
            "",
            ":1234",
            "host:0",
            "host:70000",
            "host:",
            "host:abc",
            "[::1",
        ] {
            assert!(
                matches!(Endpoint::parse(bad, 1234), Err(DeviceError::NotFound(_))),
                "{bad:?} must be refused"
            );
        }
    }
}
