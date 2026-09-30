use std::{thread::JoinHandle, time::Duration};

use mdns_sd::{DaemonEvent, Receiver, ServiceDaemon, ServiceInfo};
use sdrmm_wire::{
    API_PROTOCOL, MdnsState, PhoneEndpoint,
    phone::{
        MDNS_SERVICE_TYPE, MDNS_TXT_KEY_NAME, MDNS_TXT_KEY_PIN, MDNS_TXT_KEY_PROTOCOL,
        MDNS_TXT_KEY_SERVER, MDNS_TXT_KEY_VERSION, MDNS_TXT_VERSION,
    },
};

const MAX_INSTANCE_BYTES: usize = 63;
const INSTANCE_PREFIX: &str = "SDR-- ";
const GOODBYE_WAIT: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Advert {
    pub(crate) instance: String,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) txt: Vec<(&'static str, String)>,
}

pub(crate) fn advert(server_id: &str, server_name: &str, endpoint: &PhoneEndpoint) -> Advert {
    Advert {
        instance: cut_to(
            &format!("{INSTANCE_PREFIX}{server_name}"),
            MAX_INSTANCE_BYTES,
        ),
        host: format!("{}.local.", crate::net::host_label()),
        port: endpoint.port,
        txt: vec![
            (MDNS_TXT_KEY_VERSION, MDNS_TXT_VERSION.to_owned()),
            (MDNS_TXT_KEY_PROTOCOL, API_PROTOCOL.to_string()),
            (MDNS_TXT_KEY_SERVER, server_id.to_owned()),
            (MDNS_TXT_KEY_PIN, endpoint.pin.clone()),
            (MDNS_TXT_KEY_NAME, server_name.to_owned()),
        ],
    }
}

fn cut_to(text: &str, max_bytes: usize) -> String {
    let mut end = text.len().min(max_bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].trim_end().to_owned()
}

pub(crate) struct Advertiser {
    daemon: ServiceDaemon,
    fullname: String,
    monitor: JoinHandle<()>,
}

impl Advertiser {
    pub(crate) fn start(
        advert: &Advert,
        report: impl Fn(MdnsState) + Send + 'static,
    ) -> Result<Self, String> {
        let daemon = ServiceDaemon::new().map_err(|error| error.to_string())?;
        match announce(&daemon, advert, report) {
            Ok((fullname, monitor)) => Ok(Self {
                daemon,
                fullname,
                monitor,
            }),
            Err(reason) => {
                close(&daemon);
                Err(reason)
            }
        }
    }

    pub(crate) fn shutdown(self) {
        match self.daemon.unregister(&self.fullname) {
            Ok(done) => {
                if let Err(error) = done.recv_timeout(GOODBYE_WAIT) {
                    tracing::warn!(%error, "mDNS goodbye not confirmed");
                }
            }
            Err(error) => tracing::warn!(%error, "mDNS advert not withdrawn"),
        }
        close(&self.daemon);
        if self.monitor.join().is_err() {
            tracing::warn!("mDNS monitor ended with a panic");
        }
    }
}

fn announce(
    daemon: &ServiceDaemon,
    advert: &Advert,
    report: impl Fn(MdnsState) + Send + 'static,
) -> Result<(String, JoinHandle<()>), String> {
    let events = daemon.monitor().map_err(|error| error.to_string())?;
    let info = ServiceInfo::new(
        MDNS_SERVICE_TYPE,
        &advert.instance,
        &advert.host,
        "",
        advert.port,
        &advert.txt[..],
    )
    .map_err(|error| error.to_string())?
    .enable_addr_auto();
    let fullname = info.get_fullname().to_owned();
    daemon.register(info).map_err(|error| error.to_string())?;
    let monitor = std::thread::Builder::new()
        .name("sdrmm-mdns".to_owned())
        .spawn(move || watch(&events, &report))
        .map_err(|error| error.to_string())?;
    tracing::info!(instance = %advert.instance, port = advert.port, "mDNS advert on");
    Ok((fullname, monitor))
}

fn watch(events: &Receiver<DaemonEvent>, report: &impl Fn(MdnsState)) {
    while let Ok(event) = events.recv() {
        match event {
            DaemonEvent::Error(error) => {
                tracing::warn!(%error, "mDNS failed");
                report(MdnsState::Failed {
                    reason: error.to_string(),
                });
            }
            DaemonEvent::NameChange(change) => {
                tracing::info!(from = %change.original, to = %change.new_name, "mDNS name taken, renamed");
            }
            _ => {}
        }
    }
}

fn close(daemon: &ServiceDaemon) {
    match daemon.shutdown() {
        Ok(done) => {
            if let Err(error) = done.recv_timeout(GOODBYE_WAIT) {
                tracing::warn!(%error, "mDNS daemon stop not confirmed");
            }
        }
        Err(error) => tracing::warn!(%error, "mDNS daemon not stopped"),
    }
}

#[cfg(test)]
mod tests;
