use std::sync::Arc;

use sdrmm_engine::Engine;

use crate::{Config, tls::Tls};

fn test_engine() -> Arc<Engine> {
    let mut registry = sdrmm_device::DeviceRegistry::new();
    registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
    Engine::with_registry(registry, None)
}

#[tokio::test]
async fn a_self_signed_listener_answers_over_https() {
    let dir = tempfile::tempdir().expect("tempdir");
    let engine = test_engine();
    let handle = crate::serve(
        Config {
            bind: "127.0.0.1:0".parse().expect("bind"),
            db_path: None,
            tls: Some(Tls::SelfSigned {
                dir: dir.path().to_path_buf(),
                names: vec!["radio.example".to_owned()],
            }),
            options: crate::ServerOptions::default(),
        },
        engine.clone(),
    )
    .await
    .expect("serve");
    assert_eq!(handle.scheme, "https");

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("client");
    let response = client
        .get(format!("https://{}/api/auth", handle.local_addr))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let plain = reqwest::Client::new()
        .get(format!("http://{}/api/auth", handle.local_addr))
        .send()
        .await;
    assert!(plain.is_err(), "the listener answered plain HTTP");
    engine.shutdown();
}

#[tokio::test]
async fn a_listener_without_tls_stays_plain_http() {
    let engine = test_engine();
    let handle = crate::serve(
        Config {
            bind: "127.0.0.1:0".parse().expect("bind"),
            db_path: None,
            tls: None,
            options: crate::ServerOptions::default(),
        },
        engine.clone(),
    )
    .await
    .expect("serve");
    assert_eq!(handle.scheme, "http");

    let response = reqwest::Client::new()
        .get(format!("http://{}/api/auth", handle.local_addr))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    engine.shutdown();
}

#[tokio::test]
async fn a_certificate_that_cannot_be_read_stops_the_listener() {
    let dir = tempfile::tempdir().expect("tempdir");
    let engine = test_engine();
    let started = crate::serve(
        Config {
            bind: "127.0.0.1:0".parse().expect("bind"),
            db_path: None,
            tls: Some(Tls::Files {
                cert: dir.path().join("absent.pem"),
                key: dir.path().join("absent.key"),
            }),
            options: crate::ServerOptions::default(),
        },
        engine.clone(),
    )
    .await;
    assert!(started.is_err(), "a missing certificate started anyway");
    engine.shutdown();
}

#[tokio::test]
async fn a_minted_cert_pin_equals_the_pair_uri_fp() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tls = Tls::SelfSigned {
        dir: dir.path().to_path_buf(),
        names: vec!["radio.example".to_owned()],
    };
    let engine = test_engine();
    let handle = crate::serve(
        Config {
            bind: "127.0.0.1:0".parse().expect("bind"),
            db_path: None,
            tls: Some(tls.clone()),
            options: crate::ServerOptions::default(),
        },
        engine.clone(),
    )
    .await
    .expect("serve");
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .tls_info(true)
        .build()
        .expect("client");
    let response = client
        .get(format!("https://{}/api/about", handle.local_addr))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let presented = response
        .extensions()
        .get::<reqwest::tls::TlsInfo>()
        .and_then(reqwest::tls::TlsInfo::peer_certificate)
        .expect("peer certificate")
        .to_vec();
    let pin = sdrmm_wire::phone::spki_pin(&presented).expect("pin");
    let served = crate::tls::load(&tls).expect("same material");
    assert_eq!(served.pin, pin);
    let uri = sdrmm_wire::PairUri {
        hosts: vec![format!("{}", handle.local_addr)],
        code: "48210937".to_owned(),
        pin: served.pin.clone(),
        protocol: sdrmm_wire::about::API_PROTOCOL,
        name: None,
    }
    .to_uri();
    let parsed = sdrmm_wire::PairUri::parse(&uri).expect("pair link");
    assert_eq!(parsed.pin, pin);
    engine.shutdown();
}
