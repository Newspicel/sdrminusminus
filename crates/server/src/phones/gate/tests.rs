use std::{net::IpAddr, time::Duration};

use sdrmm_wire::phone::{MAX_PAIR_HOSTS, key_check};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{listener::Cut, *};

const PIN: &str = "3fa9c2e011b790de5a4c0123456789abcdef0123456789abcdef0123456789ab";
const MAIN_PIN: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

fn record(role: ListenerRole, bound: &str, port: u16) -> ListenerRecord {
    ListenerRecord {
        role,
        port,
        bound: bound.parse().expect("address"),
        pin: Some(match role {
            ListenerRole::Phones => PIN.to_owned(),
            ListenerRole::Main => MAIN_PIN.to_owned(),
        }),
        stable_key: true,
        names: Vec::new(),
    }
}

fn lan() -> Vec<String> {
    vec!["192.168.1.20".to_owned(), "10.0.0.5".to_owned()]
}

#[test]
fn endpoint_prefers_the_phone_listener() {
    let main = record(ListenerRole::Main, "0.0.0.0", 8080);
    let phones = record(ListenerRole::Phones, "0.0.0.0", 8443);
    let found = endpoint(&[main.clone(), phones], &lan(), "shack.local").expect("endpoint");
    assert_eq!(
        found,
        PhoneEndpoint {
            port: 8443,
            hosts: vec![
                "192.168.1.20:8443".to_owned(),
                "10.0.0.5:8443".to_owned(),
                "shack.local:8443".to_owned(),
            ],
            pin: PIN.to_owned(),
            key_check: key_check(PIN),
            dedicated: true,
        }
    );
    let alone = endpoint(std::slice::from_ref(&main), &lan(), "shack.local").expect("endpoint");
    assert_eq!((alone.port, alone.dedicated), (8080, false));
    assert_eq!(alone.pin, MAIN_PIN);
}

#[test]
fn a_loopback_main_listener_is_no_endpoint() {
    for bound in ["127.0.0.1", "::1"] {
        let main = record(ListenerRole::Main, bound, 8080);
        assert_eq!(endpoint(&[main], &lan(), "shack.local"), None, "{bound}");
    }
}

#[test]
fn a_main_listener_without_its_own_key_is_no_endpoint() {
    let operator = ListenerRecord {
        stable_key: false,
        ..record(ListenerRole::Main, "0.0.0.0", 443)
    };
    let plain = ListenerRecord {
        pin: None,
        ..record(ListenerRole::Main, "0.0.0.0", 8080)
    };
    for main in [operator, plain] {
        assert_eq!(endpoint(&[main], &lan(), "shack.local"), None);
    }
    assert_eq!(endpoint(&[], &lan(), "shack.local"), None);
}

#[test]
fn hosts_are_capped_and_deduplicated() {
    let many: Vec<String> = (1..=10).map(|last| format!("192.168.1.{last}")).collect();
    let mut repeated = many.clone();
    repeated.insert(1, many[0].clone());
    let phones = record(ListenerRole::Phones, "0.0.0.0", 8443);
    let found = endpoint(&[phones], &repeated, "shack.local").expect("endpoint");
    assert_eq!(found.hosts.len(), MAX_PAIR_HOSTS);
    assert_eq!(found.hosts[0], "192.168.1.1:8443");
    assert_eq!(found.hosts[1], "192.168.1.2:8443");
}

#[test]
fn a_main_listener_names_its_own_hosts_first() {
    let main = ListenerRecord {
        names: vec!["radio.example".to_owned(), "bad name".to_owned()],
        ..record(ListenerRole::Main, "0.0.0.0", 8080)
    };
    let found = endpoint(&[main], &lan(), "shack.local").expect("endpoint");
    assert_eq!(
        found.hosts,
        [
            "radio.example:8080",
            "192.168.1.20:8080",
            "10.0.0.5:8080",
            "shack.local:8080"
        ]
    );
}

#[test]
fn a_bound_address_is_its_own_host() {
    let main = record(ListenerRole::Main, "192.168.1.20", 8080);
    let found = endpoint(&[main], &lan(), "shack.local").expect("endpoint");
    assert_eq!(found.hosts, ["192.168.1.20:8080"]);
    let v6 = record(ListenerRole::Main, "fd00::20", 8080);
    let found = endpoint(&[v6], &lan(), "shack.local").expect("endpoint");
    assert_eq!(found.hosts, ["[fd00::20]:8080"]);
}

#[test]
fn a_wildcard_listener_is_reached_on_loopback_here() {
    let phones = record(ListenerRole::Phones, "0.0.0.0", 8443);
    assert_eq!(
        phones.reachable_here(),
        "127.0.0.1:8443".parse().expect("addr")
    );
    let v6 = record(ListenerRole::Main, "::", 8080);
    assert_eq!(v6.reachable_here(), "[::1]:8080".parse().expect("addr"));
    let bound = record(ListenerRole::Main, "192.168.1.20", 8080);
    assert_eq!(
        bound.reachable_here().ip(),
        "192.168.1.20".parse::<IpAddr>().expect("ip")
    );
}

#[test]
fn listener_records_roundtrip_through_the_store() {
    let store = Store::open(None).expect("store");
    assert!(stored_listeners(&store).expect("read").is_empty());
    let records = vec![
        record(ListenerRole::Main, "0.0.0.0", 8080),
        record(ListenerRole::Phones, "0.0.0.0", 8443),
    ];
    save_listeners(&store, &records).expect("write");
    assert_eq!(stored_listeners(&store).expect("read"), records);
    store.put_meta(LISTENERS_KEY, "not json").expect("write");
    assert!(matches!(
        stored_listeners(&store),
        Err(StoreError::Corrupt(_))
    ));
}

#[test]
fn port_zero_and_the_main_port_are_refused() {
    let gate = PhoneGate::new(false);
    assert!(!gate.port_allowed(0));
    assert!(gate.port_allowed(8080));
    gate.set_main(MainListener {
        record: record(ListenerRole::Main, "127.0.0.1", 8080),
        own_key: None,
    });
    assert!(!gate.port_allowed(8080));
    assert!(gate.port_allowed(8443));
    assert_eq!(AccessError::Port.to_string(), "Pick another port");
}

#[test]
fn a_stale_mdns_report_is_ignored() {
    let gate = PhoneGate::new(false);
    gate.settled().advert = 2;
    let old = reporter(Arc::downgrade(&gate.settled), 1, Weak::new());
    old(MdnsState::Failed {
        reason: "gone".to_owned(),
    });
    assert_eq!(gate.status().mdns, MdnsState::Off);
    let current = reporter(Arc::downgrade(&gate.settled), 2, Weak::new());
    current(MdnsState::Failed {
        reason: "no multicast".to_owned(),
    });
    assert_eq!(
        gate.status().mdns,
        MdnsState::Failed {
            reason: "no multicast".to_owned()
        }
    );
}

#[tokio::test]
async fn a_cut_stream_ends_reads_and_writes() {
    let (near, mut far) = tokio::io::duplex(64);
    let (cut, uncut) = tokio::sync::watch::channel(());
    let mut stream = Cut::new(near, uncut);
    far.write_all(b"pose").await.expect("write");
    let mut got = [0; 4];
    stream.read_exact(&mut got).await.expect("read");
    assert_eq!(&got, b"pose");
    let pending = tokio::spawn(async move {
        let mut more = [0; 1];
        stream.read(&mut more).await.map(|_| stream)
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    drop(cut);
    let error = tokio::time::timeout(Duration::from_secs(1), pending)
        .await
        .expect("the read ended")
        .expect("task")
        .err()
        .expect("a cut read fails");
    assert_eq!(error.kind(), std::io::ErrorKind::ConnectionAborted);
}

#[tokio::test]
async fn a_cut_websocket_session_ends() {
    use futures::StreamExt;

    let (near, far) = tokio::io::duplex(1024);
    let (cut, uncut) = tokio::sync::watch::channel(());
    let server = tokio::spawn(async move {
        let mut socket = tokio_tungstenite::accept_async(Cut::new(near, uncut))
            .await
            .expect("accept");
        loop {
            match socket.next().await {
                Some(Ok(_)) => {}
                Some(Err(_)) | None => return,
            }
        }
    });
    let (_client, _) = tokio_tungstenite::client_async("ws://phone/api/ws", far)
        .await
        .expect("handshake");
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!server.is_finished());
    drop(cut);
    tokio::time::timeout(Duration::from_secs(1), server)
        .await
        .expect("the session ended")
        .expect("task");
}

fn app_state() -> AppState {
    let mut registry = sdrmm_device::DeviceRegistry::new();
    registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
    AppState::new(
        Engine::with_registry(registry, None),
        Arc::new(Store::open(None).expect("store")),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_kept_wish_is_the_one_that_runs() {
    let state = app_state();
    for round in 0..40 {
        let wishes = [
            PhoneAccess {
                enabled: true,
                port: 9000 + round,
            },
            PhoneAccess {
                enabled: false,
                port: 9100 + round,
            },
        ];
        let applied = wishes.map(|wish| {
            let state = state.clone();
            tokio::spawn(async move { state.gate.apply(&state, wish).await.map(|_| ()) })
        });
        for task in applied {
            task.await.expect("task").expect("apply");
        }
        let kept = stored_access(&state.store).expect("stored");
        assert_eq!(state.gate.status().access, kept, "round {round}");
    }
}

#[tokio::test]
async fn without_a_data_directory_the_wish_is_kept_and_the_reason_shown() {
    let state = app_state();
    let wish = PhoneAccess {
        enabled: true,
        port: 8443,
    };
    let status = state.gate.apply(&state, wish).await.expect("apply");
    assert_eq!(
        status.listener,
        PhoneListenerState::Failed {
            port: 8443,
            reason: NO_DATA_DIR.to_owned()
        }
    );
    assert_eq!(status.endpoint, None);
    assert_eq!(stored_access(&state.store).expect("stored"), wish);
    assert!(
        stored_listeners(&state.store)
            .expect("listeners")
            .is_empty()
    );
}
