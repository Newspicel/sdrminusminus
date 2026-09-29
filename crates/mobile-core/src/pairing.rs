use std::net::IpAddr;

use sdrmm_wire::{
    about::API_PROTOCOL,
    phone::{
        self, DEFAULT_PHONE_PORT, MDNS_TXT_KEY_NAME, MDNS_TXT_KEY_PIN, MDNS_TXT_KEY_PROTOCOL,
        PairRequest, PairUri, PhonePlatform, PhoneToken,
    },
};

use crate::{
    error::CoreError,
    link::{
        DialError, Net, STAGGER, race,
        rest::{RestClient, RestError},
        socket::connect_tls,
        worst,
    },
    records::{DiscoveredServer, PairOffer, Platform},
    tls::{self, Seen, Trust},
    vault::{MAX_SAVED_HOSTS, RECORD_VERSION, ServerRecord},
};

fn invalid(reason: impl Into<String>) -> CoreError {
    CoreError::InvalidLink {
        reason: reason.into(),
    }
}

fn check_code(code: &str) -> Result<(), CoreError> {
    if phone::valid_pair_code(code) {
        Ok(())
    } else {
        Err(invalid("Code needs 8 digits"))
    }
}

pub(crate) fn offer_from_link(link: &str) -> Result<PairOffer, CoreError> {
    let uri = PairUri::parse(link.trim()).map_err(|error| invalid(error.to_string()))?;
    Ok(PairOffer {
        hosts: uri.hosts,
        code: uri.code,
        fingerprint_short: Some(phone::key_check(&uri.pin)),
        fingerprint: Some(uri.pin),
        protocol: uri.protocol,
        server_name: uri.name,
    })
}

pub(crate) fn offer_from_discovery(
    server: &DiscoveredServer,
    code: &str,
) -> Result<PairOffer, CoreError> {
    check_code(code)?;
    let pin = server
        .txt
        .get(MDNS_TXT_KEY_PIN)
        .filter(|pin| phone::valid_pin(pin))
        .ok_or_else(|| invalid("bad key"))?;
    let protocol = server
        .txt
        .get(MDNS_TXT_KEY_PROTOCOL)
        .and_then(|text| text.parse::<u32>().ok())
        .ok_or_else(|| invalid("bad protocol"))?;
    let hosts = discovered_hosts(&server.hosts);
    if hosts.is_empty() {
        return Err(invalid("no host"));
    }
    let name = server
        .txt
        .get(MDNS_TXT_KEY_NAME)
        .cloned()
        .or_else(|| Some(server.name.clone()))
        .filter(|name| !name.trim().is_empty());
    Ok(PairOffer {
        hosts,
        code: code.to_owned(),
        fingerprint: Some(pin.clone()),
        fingerprint_short: Some(phone::key_check(pin)),
        protocol,
        server_name: name,
    })
}

fn discovered_hosts(hosts: &[String]) -> Vec<String> {
    let mut valid: Vec<&String> = hosts
        .iter()
        .filter(|host| phone::valid_pair_host(host))
        .collect();
    valid.sort_by_key(|host| host.starts_with('['));
    let mut out: Vec<String> = Vec::new();
    for host in valid {
        if !out.contains(host) {
            out.push(host.clone());
        }
    }
    out.truncate(MAX_SAVED_HOSTS);
    out
}

pub(crate) fn manual_host(address: &str) -> Result<String, CoreError> {
    let address = address.trim();
    if phone::valid_pair_host(address) {
        return Ok(address.to_owned());
    }
    let bare = address.trim_start_matches('[').trim_end_matches(']');
    let host = match bare.parse::<IpAddr>() {
        Ok(IpAddr::V6(v6)) => format!("[{v6}]:{DEFAULT_PHONE_PORT}"),
        Ok(IpAddr::V4(v4)) => format!("{v4}:{DEFAULT_PHONE_PORT}"),
        Err(_) => format!("{bare}:{DEFAULT_PHONE_PORT}"),
    };
    if phone::valid_pair_host(&host) {
        Ok(host)
    } else {
        Err(invalid(format!("bad host {address}")))
    }
}

pub(crate) async fn offer_manual(
    host: String,
    code: String,
    net: Net,
) -> Result<PairOffer, CoreError> {
    check_code(&code)?;
    let seen = Seen::default();
    let configs = tls::configs(Trust::Recording(seen.clone()))?;
    let rest = RestClient::new(&host, &configs, None)?;
    let about = rest
        .about()
        .await
        .map_err(|error| probe_failure(&net, &host, DialError::from_rest(error)))?;
    let pin = seen
        .get()
        .ok_or_else(|| CoreError::internal("No server key seen"))?;
    Ok(PairOffer {
        hosts: vec![host],
        code,
        fingerprint_short: Some(phone::key_check(&pin)),
        fingerprint: Some(pin),
        protocol: about.protocol,
        server_name: Some(about.server_name),
    })
}

fn probe_failure(net: &Net, host: &str, error: DialError) -> CoreError {
    dial_failure(net.explain(host, error), vec![host.to_owned()])
}

fn dial_failure(error: DialError, hosts: Vec<String>) -> CoreError {
    match error {
        DialError::KeyMismatch { .. } => CoreError::KeyMismatch,
        DialError::Blocked => CoreError::LocalNetworkBlocked,
        DialError::Revoked => CoreError::Revoked,
        DialError::Protocol { server } => CoreError::ProtocolMismatch {
            server,
            app: API_PROTOCOL,
        },
        DialError::Server { status, message } => CoreError::Server { status, message },
        DialError::TimedOut | DialError::Unreachable(_) | DialError::Closed(_) => {
            CoreError::Unreachable { hosts }
        }
    }
}

pub(crate) struct PairInput {
    pub(crate) offer: PairOffer,
    pub(crate) phone_name: String,
    pub(crate) platform: Platform,
    pub(crate) rebind: Option<String>,
    pub(crate) net: Net,
    pub(crate) now_ms: i64,
}

pub(crate) async fn pair(input: PairInput) -> Result<ServerRecord, CoreError> {
    let PairInput {
        offer,
        phone_name,
        platform,
        rebind,
        net,
        now_ms,
    } = input;
    let pin = offer
        .fingerprint
        .clone()
        .filter(|pin| phone::valid_pin(pin))
        .ok_or_else(|| invalid("No server key"))?;
    check_code(&offer.code)?;
    let name = phone_name.trim().to_owned();
    if !phone::valid_phone_name(&name) {
        return Err(CoreError::Refused {
            message: "Name must be 1 to 64 characters".to_owned(),
        });
    }
    if offer.protocol != API_PROTOCOL {
        return Err(CoreError::ProtocolMismatch {
            server: offer.protocol,
            app: API_PROTOCOL,
        });
    }
    let hosts: Vec<String> = offer
        .hosts
        .iter()
        .filter(|host| phone::valid_pair_host(host))
        .cloned()
        .collect();
    if hosts.is_empty() {
        return Err(invalid("no host"));
    }
    let configs = tls::pinned(&pin)?;
    let winner = race(&hosts, STAGGER, |host| {
        let config = configs.ws.clone();
        let net = net.clone();
        async move {
            connect_tls(&host, config)
                .await
                .map(drop)
                .map_err(|error| net.explain(&host, error))
        }
    })
    .await
    .map(|(host, ())| host)
    .map_err(|failures| dial_failure(worst(failures), hosts.clone()))?;
    let rest = RestClient::new(&winner, &configs, None)?;
    let request = PairRequest {
        code: offer.code.clone(),
        name,
        platform: match platform {
            Platform::Ios => PhonePlatform::Ios,
            Platform::Android => PhonePlatform::Android,
        },
        protocol: API_PROTOCOL,
        rebind,
    };
    let response = rest
        .pair(&request)
        .await
        .map_err(|error| pair_error(error, &winner))?;
    if response.protocol != API_PROTOCOL {
        return Err(CoreError::ProtocolMismatch {
            server: response.protocol,
            app: API_PROTOCOL,
        });
    }
    if PhoneToken::parse(&response.token).is_none_or(|token| token.phone != response.phone.id) {
        return Err(CoreError::internal("Server sent a bad phone key"));
    }
    let record = ServerRecord {
        version: RECORD_VERSION,
        server_id: response.server_id,
        server_name: response.server_name,
        phone_id: response.phone.id,
        token: response.token,
        hosts: crate::link::merged(&[winner], &hosts, MAX_SAVED_HOSTS),
        pin,
        paired_at_ms: now_ms,
    };
    record
        .validate()
        .map_err(|error| CoreError::internal(format!("Server sent a bad record: {error}")))?;
    Ok(record)
}

fn pair_error(error: RestError, host: &str) -> CoreError {
    match error {
        RestError::Status { status: 404, .. } => CoreError::CodeExpired,
        RestError::Status { status: 401, .. } => CoreError::WrongCode,
        RestError::Status { status, message } if (400..500).contains(&status) => {
            CoreError::Refused { message }
        }
        other => other.into_core(host),
    }
}

#[cfg(test)]
mod tests;
