use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use sdrmm_wire::{
    PhoneEndpoint,
    phone::{MAX_PAIR_HOSTS, key_check, valid_pair_host},
};

use crate::auth::ListenerRole;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ListenerRecord {
    pub(crate) role: ListenerRole,
    pub(crate) port: u16,
    pub(crate) bound: IpAddr,
    pub(crate) pin: Option<String>,
    pub(crate) stable_key: bool,
    pub(crate) names: Vec<String>,
}

impl ListenerRecord {
    fn serves_phones(&self) -> bool {
        match self.role {
            ListenerRole::Phones => self.pin.is_some(),
            ListenerRole::Main => {
                self.pin.is_some() && self.stable_key && !self.bound.is_loopback()
            }
        }
    }

    pub(crate) fn reachable_here(&self) -> SocketAddr {
        let ip = match self.bound {
            IpAddr::V4(v4) if v4.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(v6) if v6.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            bound => bound,
        };
        SocketAddr::new(ip, self.port)
    }
}

pub(crate) fn candidate(records: &[ListenerRecord]) -> Option<&ListenerRecord> {
    let of = |role: ListenerRole| {
        records
            .iter()
            .find(|record| record.role == role && record.serves_phones())
    };
    of(ListenerRole::Phones).or_else(|| of(ListenerRole::Main))
}

pub(crate) fn endpoint(
    records: &[ListenerRecord],
    lan: &[String],
    mdns_host: &str,
) -> Option<PhoneEndpoint> {
    let record = candidate(records)?;
    let pin = record.pin.clone()?;
    let hosts = hosts(record, lan, mdns_host);
    if hosts.is_empty() {
        return None;
    }
    Some(PhoneEndpoint {
        port: record.port,
        hosts,
        key_check: key_check(&pin),
        pin,
        dedicated: record.role == ListenerRole::Phones,
    })
}

fn hosts(record: &ListenerRecord, lan: &[String], mdns_host: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if record.role == ListenerRole::Main {
        names.extend(record.names.iter().cloned());
    }
    if record.bound.is_unspecified() {
        names.extend(lan.iter().cloned());
        names.push(mdns_host.to_owned());
    } else {
        names.push(record.bound.to_string());
    }
    let mut hosts: Vec<String> = Vec::new();
    for host in names.iter().map(|name| with_port(name, record.port)) {
        if hosts.len() == MAX_PAIR_HOSTS {
            break;
        }
        if valid_pair_host(&host) && !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    hosts
}

fn with_port(name: &str, port: u16) -> String {
    match name.parse::<IpAddr>() {
        Ok(IpAddr::V6(v6)) => SocketAddr::from((v6, port)).to_string(),
        _ => format!("{name}:{port}"),
    }
}
