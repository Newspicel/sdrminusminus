use url::Url;

pub const FIELD_ROOT: &str = "/field";
pub const TOKEN_PARAM: &str = "token";

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

fn wrap(address: &str) -> String {
    if address.contains(':') {
        format!("[{address}]")
    } else {
        address.to_owned()
    }
}

#[must_use]
pub fn handoff_origins(origin: &str, lan_addresses: &[String]) -> Vec<String> {
    let parsed = Url::parse(origin).ok();
    let local = parsed
        .as_ref()
        .and_then(Url::host_str)
        .is_some_and(is_loopback);
    let from_lan = lan_addresses.iter().map(|address| match &parsed {
        None => format!("http://{address}"),
        Some(url) => {
            let port = url
                .port()
                .map_or_else(String::new, |port| format!(":{port}"));
            format!("{}://{}{port}", url.scheme(), wrap(address))
        }
    });
    let own = parsed
        .as_ref()
        .map(|url| url.origin().ascii_serialization());
    let ordered: Vec<String> = if local {
        from_lan.chain(own).collect()
    } else {
        own.into_iter().chain(from_lan).collect()
    };
    let mut unique: Vec<String> = Vec::with_capacity(ordered.len());
    for origin in ordered {
        if !unique.contains(&origin) {
            unique.push(origin);
        }
    }
    unique
}

#[must_use]
pub fn handoff_url(origin: &str, token: Option<&str>) -> String {
    let Ok(mut url) = Url::parse(origin).and_then(|base| base.join(FIELD_ROOT)) else {
        return format!("{}{FIELD_ROOT}", origin.trim_end_matches('/'));
    };
    if let Some(token) = token.filter(|token| !token.is_empty()) {
        url.query_pairs_mut().append_pair(TOKEN_PARAM, token);
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lan(addresses: &[&str]) -> Vec<String> {
        addresses
            .iter()
            .map(|address| (*address).to_owned())
            .collect()
    }

    #[test]
    fn puts_the_addresses_a_phone_can_follow_ahead_of_localhost() {
        assert_eq!(
            handoff_origins("http://localhost:8080", &lan(&["192.168.1.10", "10.0.0.4"])),
            [
                "http://192.168.1.10:8080",
                "http://10.0.0.4:8080",
                "http://localhost:8080"
            ]
        );
    }

    #[test]
    fn keeps_the_operators_own_address_first_when_it_already_works() {
        assert_eq!(
            handoff_origins("http://192.168.1.10:8080", &lan(&["192.168.1.10"])),
            ["http://192.168.1.10:8080"]
        );
    }

    #[test]
    fn has_only_the_one_address_when_the_server_reports_none() {
        assert_eq!(
            handoff_origins("http://localhost:8080", &[]),
            ["http://localhost:8080"]
        );
    }

    #[test]
    fn keeps_the_scheme_and_port_and_brackets_a_v6_address() {
        assert_eq!(
            handoff_origins("https://localhost:9443", &lan(&["192.168.1.10"]))[0],
            "https://192.168.1.10:9443"
        );
        assert_eq!(
            handoff_origins("http://127.0.0.1:80", &lan(&["fe80::1"]))[0],
            "http://[fe80::1]"
        );
    }

    #[test]
    fn points_at_field_mode_and_carries_the_token() {
        assert_eq!(
            handoff_url("http://192.168.1.10:8080", None),
            "http://192.168.1.10:8080/field"
        );
        assert_eq!(
            handoff_url("http://192.168.1.10:8080", Some("s3cret")),
            "http://192.168.1.10:8080/field?token=s3cret"
        );
        assert_eq!(
            handoff_url("http://host", Some("a b&c")),
            "http://host/field?token=a+b%26c"
        );
    }
}
