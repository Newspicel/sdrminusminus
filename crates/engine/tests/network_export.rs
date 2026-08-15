// Tests may unwrap/expect (AGENTS.md); clippy's test allowance does not cover every helper.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    io::Read,
    net::{TcpListener, UdpSocket},
    sync::Arc,
    time::Duration,
};

use sdrmm_device::DeviceRegistry;
use sdrmm_device_virtual::VirtualDriver;
use sdrmm_engine::Engine;
use sdrmm_wire::{DeviceSettings, NetworkExportSettings, NetworkSampleFormat, NetworkTransport};

const WAIT: Duration = Duration::from_secs(10);

fn engine() -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(10, Box::new(VirtualDriver::new()));
    Engine::with_registry(registry, None)
}

#[test]
fn virtual_device_exports_mtu_safe_ci16_udp() {
    let receiver = UdpSocket::bind("127.0.0.1:0").expect("bind receiver");
    receiver.set_read_timeout(Some(WAIT)).expect("timeout");
    let engine = engine();
    let ds = engine
        .create_device_set("virtual:siggen")
        .expect("virtual set");
    let settings = NetworkExportSettings {
        transport: NetworkTransport::Udp,
        format: NetworkSampleFormat::Ci16Le,
        address: receiver.local_addr().expect("address").to_string(),
    };

    engine
        .start_network_export(ds, "udp".to_owned(), 0, settings.clone())
        .expect("start UDP export");
    let mut datagram = [0u8; 2_048];
    let received = receiver.recv(&mut datagram).expect("IQ datagram");
    assert!(
        received <= 1_400,
        "datagram exceeds the exporter MTU budget"
    );
    assert_eq!(received % settings.format.bytes_per_sample(), 0);
    assert!(
        datagram[..received]
            .as_chunks::<2>()
            .0
            .iter()
            .any(|word| *word != [0, 0]),
        "virtual IQ was encoded as silence"
    );
    let rate_error = engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(2_400_000.0),
                ..DeviceSettings::default()
            },
        )
        .expect_err("raw export pins its sample rate");
    assert!(rate_error.to_string().contains("locked"));
    assert!(
        engine.stop_network_export(ds, "another-node").is_err(),
        "one patch node must not stop another node's export"
    );

    let final_status = engine.stop_network_export(ds, "udp").expect("stop export");
    assert!(final_status.samples > 0);
    assert_eq!(
        final_status.bytes,
        final_status.samples * settings.format.bytes_per_sample() as u64
    );
    assert!(final_status.packets > 0);
    assert_eq!(final_status.error, None);
    engine.remove_device_set(ds).expect("remove set");
}

#[test]
fn virtual_device_exports_an_unframed_cf32_tcp_stream() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let address = listener.local_addr().expect("address");
    let reader = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept exporter");
        stream.set_read_timeout(Some(WAIT)).expect("timeout");
        let mut bytes = [0u8; 16];
        stream.read_exact(&mut bytes).expect("two complex samples");
        bytes
    });
    let engine = engine();
    let ds = engine
        .create_device_set("virtual:siggen")
        .expect("virtual set");
    engine
        .start_network_export(
            ds,
            "tcp".to_owned(),
            0,
            NetworkExportSettings {
                transport: NetworkTransport::Tcp,
                format: NetworkSampleFormat::Cf32Le,
                address: address.to_string(),
            },
        )
        .expect("start TCP export");

    let bytes = reader.join().expect("reader thread");
    let components: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| f32::from_le_bytes(*word))
        .collect();
    assert!(components.iter().all(|value| value.is_finite()));
    assert!(components.iter().any(|value| *value != 0.0));
    let status = engine.stop_network_export(ds, "tcp").expect("stop export");
    assert!(status.bytes >= 16);
    assert_eq!(status.bytes % 8, 0);
    engine.remove_device_set(ds).expect("remove set");
}
