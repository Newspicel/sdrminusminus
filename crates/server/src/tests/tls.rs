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

pub(super) fn insecure_client() -> reqwest::Client {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .tls_info(true)
        .build()
        .expect("client")
}

pub(super) async fn presented_pin(client: &reqwest::Client, url: &str) -> String {
    let response = client.get(url).send().await.expect("request");
    assert_eq!(response.status(), reqwest::StatusCode::OK, "{url}");
    let presented = response
        .extensions()
        .get::<reqwest::tls::TlsInfo>()
        .and_then(reqwest::tls::TlsInfo::peer_certificate)
        .expect("peer certificate")
        .to_vec();
    sdrmm_wire::phone::spki_pin(&presented).expect("pin")
}

pub(super) fn free_port() -> u16 {
    std::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, 0))
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port()
}

pub(super) async fn closed(port: u16) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .is_ok()
    {
        assert!(
            std::time::Instant::now() < deadline,
            "port {port} still open"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

pub(super) async fn serve_at(
    bind: &str,
    db: Option<std::path::PathBuf>,
    tls: Option<Tls>,
) -> (crate::ServerHandle, Arc<Engine>) {
    let engine = test_engine();
    let handle = crate::serve(
        Config {
            bind: bind.parse().expect("bind"),
            db_path: db,
            tls,
            options: crate::ServerOptions::default(),
        },
        engine.clone(),
    )
    .await
    .expect("serve");
    (handle, engine)
}

pub(super) async fn allow_phones(
    client: &reqwest::Client,
    base: &str,
    enabled: bool,
    port: u16,
) -> sdrmm_wire::PhoneAccessStatus {
    let response = client
        .put(format!("{base}/api/phones/access"))
        .json(&sdrmm_wire::PhoneAccess { enabled, port })
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    response.json().await.expect("status")
}

async fn phones_listing(client: &reqwest::Client, base: &str) -> sdrmm_wire::PhonesResponse {
    client
        .get(format!("{base}/api/phones"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("phones")
}

fn operator_certificate(dir: &std::path::Path) -> Tls {
    let minted =
        rcgen::generate_simple_self_signed(vec!["radio.example".to_owned()]).expect("certificate");
    let cert = dir.join("operator.pem");
    let key = dir.join("operator.key");
    std::fs::write(&cert, minted.cert.pem()).expect("write cert");
    std::fs::write(&key, minted.signing_key.serialize_pem()).expect("write key");
    Tls::Files { cert, key }
}

#[tokio::test]
async fn dropping_the_handle_closes_the_main_listener() {
    let (handle, engine) = serve_at("127.0.0.1:0", None, None).await;
    let port = handle.local_addr.port();
    assert!(
        tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .is_ok()
    );
    drop(handle);
    closed(port).await;
    engine.shutdown();
}

#[tokio::test]
async fn a_loopback_plain_server_is_no_phone_endpoint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let self_signed = Tls::SelfSigned {
        dir: dir.path().to_path_buf(),
        names: Vec::new(),
    };
    for tls in [None, Some(self_signed)] {
        let (handle, engine) = serve_at("127.0.0.1:0", None, tls).await;
        let base = format!("{}://{}", handle.scheme, handle.local_addr);
        let listing = phones_listing(&insecure_client(), &base).await;
        assert_eq!(listing.access.endpoint, None, "{base}");
        assert_eq!(listing.access.listener, sdrmm_wire::PhoneListenerState::Off);
        engine.shutdown();
    }
}

#[tokio::test]
async fn a_self_signed_lan_server_is_the_phone_endpoint_and_shares_its_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (handle, engine) = serve_at(
        "0.0.0.0:0",
        Some(dir.path().join("sdrmm.db")),
        Some(Tls::SelfSigned {
            dir: dir.path().to_path_buf(),
            names: Vec::new(),
        }),
    )
    .await;
    let base = format!("https://127.0.0.1:{}", handle.local_addr.port());
    let client = insecure_client();
    let main_pin = presented_pin(&client, &format!("{base}/api/about")).await;
    let endpoint = phones_listing(&client, &base)
        .await
        .access
        .endpoint
        .expect("the main listener serves phones");
    assert_eq!(
        (endpoint.port, endpoint.dedicated, endpoint.pin.as_str()),
        (handle.local_addr.port(), false, main_pin.as_str())
    );
    let port = free_port();
    let status = allow_phones(&client, &base, true, port).await;
    let endpoint = status.endpoint.expect("endpoint");
    assert_eq!((endpoint.port, endpoint.dedicated), (port, true));
    assert_eq!(endpoint.pin, main_pin);
    let phone_pin = presented_pin(&client, &format!("https://127.0.0.1:{port}/api/about")).await;
    assert_eq!(phone_pin, main_pin);
    drop(handle);
    engine.shutdown();
}

#[tokio::test]
async fn the_phone_listener_keeps_its_key_when_the_operator_cert_changes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("sdrmm.db");
    let client = insecure_client();
    let port = free_port();

    let (handle, engine) = serve_at(
        "127.0.0.1:0",
        Some(db.clone()),
        Some(operator_certificate(dir.path())),
    )
    .await;
    let base = format!("https://{}", handle.local_addr);
    let operator_pin = presented_pin(&client, &format!("{base}/api/about")).await;
    let status = allow_phones(&client, &base, true, port).await;
    let phone_pin = status.endpoint.expect("endpoint").pin;
    assert_ne!(phone_pin, operator_pin);
    let phone_url = format!("https://127.0.0.1:{port}/api/about");
    assert_eq!(presented_pin(&client, &phone_url).await, phone_pin);
    drop(handle);
    engine.shutdown();
    closed(port).await;

    let (handle, engine) = serve_at(
        "127.0.0.1:0",
        Some(db),
        Some(operator_certificate(dir.path())),
    )
    .await;
    let base = format!("https://{}", handle.local_addr);
    let renewed_pin = presented_pin(&client, &format!("{base}/api/about")).await;
    assert_ne!(renewed_pin, operator_pin, "the operator key did not change");
    assert_eq!(presented_pin(&client, &phone_url).await, phone_pin);
    let listing = phones_listing(&client, &base).await;
    assert_eq!(
        listing.access.listener,
        sdrmm_wire::PhoneListenerState::On { port }
    );
    assert_eq!(listing.access.endpoint.expect("endpoint").pin, phone_pin);
    drop(handle);
    engine.shutdown();
}
