use std::{collections::HashMap, time::Duration};

use sdrmm_wire::{BroadcastData, DecodedRecord, DecoderEvent, EventOutputTarget};
use tokio::{sync::mpsc, task::JoinHandle};

use super::{Binding, status::Statuses};

struct Entry {
    target: EventOutputTarget,
    sender: mpsc::Sender<Vec<u8>>,
    worker: JoinHandle<()>,
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.worker.abort();
    }
}

pub(super) struct Outputs {
    entries: HashMap<String, Entry>,
    statuses: Statuses,
}

impl Outputs {
    pub fn new(statuses: Statuses) -> Self {
        Self {
            entries: HashMap::new(),
            statuses,
        }
    }

    pub fn configure(&mut self, bindings: &[Binding]) {
        self.entries.retain(|node, entry| {
            bindings
                .iter()
                .any(|binding| binding.node == *node && binding.target == entry.target)
        });
        for binding in bindings {
            let EventOutputTarget::Tunnel {
                interface,
                address,
                prefix,
            } = &binding.target
            else {
                continue;
            };
            if self.entries.contains_key(&binding.node) {
                continue;
            }
            let (sender, mut packets) = mpsc::channel::<Vec<u8>>(256);
            let node = binding.node.clone();
            let statuses = self.statuses.clone();
            let builder = tun_rs::DeviceBuilder::new()
                .name(interface)
                .ipv4(*address, *prefix, None)
                .mtu(65535)
                .enable(true);
            let worker = tokio::spawn(async move {
                let device = match tokio::task::spawn_blocking(move || builder.build_async()).await
                {
                    Ok(Ok(device)) => device,
                    Ok(Err(error)) => {
                        return fail(
                            &statuses,
                            &node,
                            format!("Cannot create interface: {error}"),
                        );
                    }
                    Err(error) => {
                        return fail(&statuses, &node, format!("Interface setup failed: {error}"));
                    }
                };
                while let Some(bytes) = packets.recv().await {
                    match tokio::time::timeout(Duration::from_secs(2), device.send(&bytes)).await {
                        Ok(Ok(written)) if written == bytes.len() => statuses.delivered(&node, 1),
                        Ok(Ok(written)) => fail(
                            &statuses,
                            &node,
                            format!("Packet truncated: {written} of {} bytes", bytes.len()),
                        ),
                        Ok(Err(error)) => fail(&statuses, &node, format!("Write failed: {error}")),
                        Err(_) => fail(&statuses, &node, "Write timed out".to_owned()),
                    }
                }
            });
            self.entries.insert(
                binding.node.clone(),
                Entry {
                    target: binding.target.clone(),
                    sender,
                    worker,
                },
            );
        }
    }

    pub fn push(&self, bindings: &[Binding], record: &DecodedRecord) {
        let DecoderEvent::BroadcastData(data) = &record.event else {
            return;
        };
        for binding in bindings {
            let Some(entry) = self.entries.get(&binding.node) else {
                continue;
            };
            if !record.sinks.contains(&binding.node) {
                continue;
            }
            match ip_packet(data) {
                Ok(bytes) => {
                    if let Err(error) = entry.sender.try_send(bytes.to_vec()) {
                        fail(
                            &self.statuses,
                            &binding.node,
                            format!("Queue unavailable: {error}"),
                        );
                    }
                }
                Err(error) => fail(&self.statuses, &binding.node, error.to_owned()),
            }
        }
    }
}

fn fail(statuses: &Statuses, node: &str, error: String) {
    tracing::error!(output = %node, %error, "broadcast TUN output failed");
    statuses.failed(node, 1, error);
}

fn ip_packet(data: &BroadcastData) -> Result<&[u8], &'static str> {
    let bytes = data.bytes.as_slice();
    match data.protocol {
        Some(0x0800) if bytes.len() >= 20 && bytes[0] >> 4 == 4 => {
            let header = usize::from(bytes[0] & 15) * 4;
            if header < 20
                || header > bytes.len()
                || usize::from(u16::from_be_bytes([bytes[2], bytes[3]])) != bytes.len()
            {
                return Err("Invalid IPv4 packet length");
            }
            let mut checksum = bytes[..header]
                .as_chunks::<2>()
                .0
                .iter()
                .fold(0u32, |sum, b| sum + u32::from(u16::from_be_bytes(*b)));
            while checksum >> 16 != 0 {
                checksum = (checksum & 65535) + (checksum >> 16);
            }
            if checksum != 65535 {
                return Err("Invalid IPv4 header checksum");
            }
        }
        Some(0x86dd) if bytes.len() >= 40 && bytes[0] >> 4 == 6 => {
            if 40 + usize::from(u16::from_be_bytes([bytes[4], bytes[5]])) != bytes.len() {
                return Err("Invalid IPv6 packet length");
            }
        }
        _ => return Err("TUN accepts IPv4 and IPv6 datagrams"),
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn data(protocol: u16, bytes: Vec<u8>) -> BroadcastData {
        BroadcastData {
            protocol: Some(protocol),
            label: Vec::new(),
            service_id: None,
            name: String::new(),
            media_type: String::new(),
            bytes,
        }
    }
    #[tokio::test]
    async fn routed_datagrams_reach_a_bounded_writer_without_opening_a_device() {
        let target = EventOutputTarget::Tunnel {
            interface: "test0".to_owned(),
            address: std::net::Ipv4Addr::new(10, 23, 0, 1),
            prefix: 24,
        };
        let binding = Binding {
            node: "out".to_owned(),
            target: target.clone(),
        };
        let (sender, mut receiver) = mpsc::channel(1);
        let statuses = Statuses::default();
        statuses.configure(std::slice::from_ref(&binding));
        let mut outputs = Outputs::new(statuses.clone());
        outputs.entries.insert(
            "out".to_owned(),
            Entry {
                target,
                sender,
                worker: tokio::spawn(std::future::pending()),
            },
        );
        let mut bytes = vec![0; 44];
        bytes[0] = 0x60;
        bytes[5] = 4;
        let elsewhere = DecodedRecord {
            origin: None,
            sinks: vec!["other".to_owned()],
            device_set: 1,
            channel: 2,
            at: "2026-09-16T00:00:00Z".to_owned(),
            freq_hz: 1e9,
            event: DecoderEvent::BroadcastData(data(0x86dd, bytes.clone())),
        };
        outputs.push(std::slice::from_ref(&binding), &elsewhere);
        assert!(receiver.try_recv().is_err());
        let reached = DecodedRecord {
            origin: None,
            sinks: vec!["out".to_owned()],
            ..elsewhere
        };
        outputs.push(std::slice::from_ref(&binding), &reached);
        outputs.push(std::slice::from_ref(&binding), &reached);
        assert_eq!(receiver.recv().await.unwrap(), bytes);
        let status = statuses.get("out").expect("tunnel status");
        assert_eq!(status.failed, 1);
        assert!(
            status
                .error
                .expect("queue error")
                .starts_with("Queue unavailable")
        );
        outputs.configure(&[]);
        assert!(outputs.entries.is_empty());
        assert!(receiver.recv().await.is_none());
    }

    #[test]
    fn only_complete_ip_packets_reach_the_interface() {
        let mut ipv4 = vec![
            0x45, 0, 0, 20, 0, 0, 0, 0, 64, 17, 0, 0, 192, 0, 2, 1, 192, 0, 2, 2,
        ];
        let mut sum = ipv4
            .as_chunks::<2>()
            .0
            .iter()
            .fold(0u32, |sum, b| sum + u32::from(u16::from_be_bytes(*b)));
        while sum >> 16 != 0 {
            sum = (sum & 65535) + (sum >> 16);
        }
        ipv4[10..12].copy_from_slice(&(!(sum as u16)).to_be_bytes());
        assert_eq!(ip_packet(&data(0x800, ipv4.clone())).unwrap(), ipv4);
        ipv4[12] ^= 1;
        assert!(ip_packet(&data(0x800, ipv4)).is_err());
        let mut ipv6 = vec![0u8; 44];
        ipv6[0] = 0x60;
        ipv6[5] = 4;
        assert_eq!(ip_packet(&data(0x86dd, ipv6.clone())).unwrap(), ipv6);
        ipv6.pop();
        assert!(ip_packet(&data(0x86dd, ipv6)).is_err());
        assert!(ip_packet(&data(0x806, vec![0; 28])).is_err());
    }
}
