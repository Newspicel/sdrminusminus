use std::net::{IpAddr, Ipv4Addr};

const FALLBACK_LABEL: &str = "sdrmm";
const MAX_LABEL_BYTES: usize = 63;
const VIRTUAL_INTERFACES: [&str; 4] = ["docker", "br-", "veth", "virbr"];

pub(crate) fn lan_addresses() -> Vec<String> {
    match local_ip_address::list_afinet_netifas() {
        Ok(interfaces) => reachable(interfaces),
        Err(error) => {
            tracing::warn!(%error, "could not list the network interfaces");
            Vec::new()
        }
    }
}

pub(crate) fn host_label() -> String {
    label_of(&gethostname::gethostname().to_string_lossy())
}

pub(crate) fn mdns_host() -> String {
    format!("{}.local", host_label())
}

fn reachable(interfaces: impl IntoIterator<Item = (String, IpAddr)>) -> Vec<String> {
    let mut found: Vec<Ipv4Addr> = interfaces
        .into_iter()
        .filter(|(name, _)| !VIRTUAL_INTERFACES.iter().any(|kind| name.starts_with(kind)))
        .filter_map(|(_, address)| match address {
            IpAddr::V4(v4) => Some(v4),
            IpAddr::V6(_) => None,
        })
        .filter(|v4| !v4.is_loopback() && !v4.is_unspecified() && !v4.is_link_local())
        .collect();
    found.sort_by_key(|address| (reach_rank(*address), *address));
    found.dedup();
    found
        .into_iter()
        .map(|address| address.to_string())
        .collect()
}

fn reach_rank(address: Ipv4Addr) -> u8 {
    if address.is_private() {
        0
    } else if is_carrier_grade(address) {
        1
    } else {
        2
    }
}

fn is_carrier_grade(address: Ipv4Addr) -> bool {
    let [first, second, ..] = address.octets();
    first == 100 && (64..128).contains(&second)
}

fn label_of(host: &str) -> String {
    let first = host.split_once('.').map_or(host, |(head, _)| head);
    let mapped: String = first
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = mapped.trim_matches('-');
    let label = trimmed[..trimmed.len().min(MAX_LABEL_BYTES)].trim_end_matches('-');
    if label.is_empty() {
        FALLBACK_LABEL.to_string()
    } else {
        label.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv6Addr;

    use super::*;

    fn on(name: &str, address: IpAddr) -> (String, IpAddr) {
        (name.to_owned(), address)
    }

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(a, b, c, d))
    }

    #[test]
    fn host_labels_are_dns_safe() {
        assert_eq!(label_of("Julians-MacBook-Pro.local"), "julians-macbook-pro");
        assert_eq!(label_of("pi"), "pi");
        assert_eq!(label_of("my_host.lan"), "my-host");
        assert_eq!(label_of("-edge-"), "edge");
        assert_eq!(label_of("Ünïcode"), "n-code");
        assert_eq!(label_of(""), FALLBACK_LABEL);
        assert_eq!(label_of("---.local"), FALLBACK_LABEL);
        assert_eq!(label_of(&"a".repeat(100)).len(), MAX_LABEL_BYTES);
        assert_eq!(label_of(&format!("{}-b", "a".repeat(62))), "a".repeat(62));
    }

    #[test]
    fn this_host_has_a_label() {
        let label = host_label();
        assert!(!label.is_empty() && label.len() <= MAX_LABEL_BYTES);
        assert!(
            label
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        );
        assert_eq!(mdns_host(), format!("{label}.local"));
    }

    #[test]
    fn lan_addresses_skip_loopback_and_link_local() {
        let found = reachable([
            on("lo0", v4(127, 0, 0, 1)),
            on("en0", v4(169, 254, 10, 2)),
            on("en0", v4(0, 0, 0, 0)),
            on("en0", IpAddr::V6(Ipv6Addr::LOCALHOST)),
            on("en0", "fe80::1".parse().expect("v6")),
            on("docker0", v4(172, 17, 0, 1)),
            on("br-9f2c", v4(172, 18, 0, 1)),
            on("veth12", v4(172, 19, 0, 1)),
            on("virbr0", v4(192, 168, 122, 1)),
            on("en0", v4(192, 168, 1, 20)),
            on("en1", v4(192, 168, 1, 20)),
        ]);
        assert_eq!(found, ["192.168.1.20"]);
    }

    #[test]
    fn private_addresses_come_first() {
        let found = reachable([
            on("en2", v4(203, 0, 113, 5)),
            on("utun4", v4(100, 101, 2, 3)),
            on("en0", v4(192, 168, 1, 20)),
            on("en1", v4(10, 0, 0, 5)),
            on("utun5", v4(100, 64, 0, 1)),
            on("en3", v4(100, 128, 0, 1)),
        ]);
        assert_eq!(
            found,
            [
                "10.0.0.5",
                "192.168.1.20",
                "100.64.0.1",
                "100.101.2.3",
                "100.128.0.1",
                "203.0.113.5"
            ]
        );
    }
}
