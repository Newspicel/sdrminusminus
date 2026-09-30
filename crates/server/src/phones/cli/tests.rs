use std::net::{Ipv4Addr, TcpListener};

use sdrmm_wire::{
    OfferState, PairUri,
    phone::{key_check, valid_pair_code},
};

use super::*;
use crate::{
    auth::ListenerRole,
    phones::{
        gate::{ListenerRecord, save_listeners},
        tests::{PIN, request},
    },
};

fn phones_record(port: u16) -> ListenerRecord {
    ListenerRecord {
        role: ListenerRole::Phones,
        port,
        bound: Ipv4Addr::UNSPECIFIED.into(),
        pin: Some(PIN.to_owned()),
        stable_key: true,
        names: Vec::new(),
    }
}

fn database(records: &[ListenerRecord]) -> (tempfile::TempDir, PathBuf, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("sdrmm.db");
    let store = Store::open(Some(&path)).expect("store");
    save_listeners(&store, records).expect("write");
    (dir, path, store)
}

fn closed_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port()
}

#[test]
fn cli_offer_needs_a_database() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("sdrmm.db");
    let error = offer_for_cli(&missing, None).expect_err("no database");
    assert!(matches!(&error, CliPairError::NoDatabase(path) if *path == missing));
    assert!(error.to_string().ends_with("start sdrmm first"));
    assert!(!missing.exists(), "the check made a database");
}

#[test]
fn cli_offer_needs_an_endpoint() {
    let loopback = ListenerRecord {
        role: ListenerRole::Main,
        bound: Ipv4Addr::LOCALHOST.into(),
        ..phones_record(8080)
    };
    for records in [Vec::new(), vec![loopback]] {
        let (_dir, path, _store) = database(&records);
        assert!(matches!(
            offer_for_cli(&path, None),
            Err(CliPairError::NoEndpoint)
        ));
    }
    assert_eq!(
        CliPairError::NoEndpoint.to_string(),
        "phones need HTTPS: turn on Allow phones or start with --tls-self-signed"
    );
}

#[test]
fn cli_offer_needs_a_running_listener() {
    let port = closed_port();
    let (_dir, path, store) = database(&[phones_record(port)]);
    match offer_for_cli(&path, None) {
        Err(CliPairError::NotRunning(closed)) => assert_eq!(closed, port),
        other => panic!("expected NotRunning, got {other:?}"),
    }
    assert!(store.latest_offer().expect("read").is_none());
}

#[test]
fn cli_offer_is_accepted_by_the_server() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let (_dir, path, store) = database(&[phones_record(port)]);
    let offer = offer_for_cli(&path, Some("Car")).expect("offer");
    assert!(valid_pair_code(&offer.code));
    assert_eq!(offer.key_check, key_check(PIN));
    assert!(offer.endpoint.dedicated);
    let link = PairUri::parse(&offer.uri).expect("pair link");
    assert_eq!(
        (link.pin.as_str(), link.code.as_str()),
        (PIN, offer.code.as_str())
    );
    assert!(
        link.hosts
            .iter()
            .all(|host| host.ends_with(&format!(":{port}")))
    );
    let server = Phones::new(Arc::new(store));
    let paired = server
        .pair(&request(&offer.code), "server", "shack", Timestamp::now())
        .expect("the server pairs the code the CLI made");
    assert_eq!(
        server
            .offer_status(None, "shack", Timestamp::now())
            .expect("status")
            .expect("offer")
            .state,
        OfferState::Used {
            phone: paired.phone.id
        }
    );
}

#[test]
fn a_bad_name_is_said_plainly() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let (_dir, path, _store) = database(&[phones_record(port)]);
    let error = offer_for_cli(&path, Some(" ")).expect_err("empty name");
    assert_eq!(error.to_string(), "Name must be 1 to 64 characters");
}

#[test]
fn the_caption_groups_the_code_and_lists_hosts() {
    let offer = PairingOffer {
        id: "o1".to_owned(),
        code: "48210937".to_owned(),
        uri: "sdrmm://pair".to_owned(),
        key_check: key_check(PIN),
        expires_at: "not a time".to_owned(),
        endpoint: sdrmm_wire::PhoneEndpoint {
            port: 8443,
            hosts: vec!["192.168.1.20:8443".to_owned(), "pi.local:8443".to_owned()],
            pin: PIN.to_owned(),
            key_check: key_check(PIN),
            dedicated: true,
        },
    };
    assert_eq!(
        caption(&offer),
        "Code 4821 0937\nKey  3FA9 C2E0 11B7 90DE 5A4C\nHost 192.168.1.20:8443, pi.local:8443\nEnds not a time"
    );
    let timed = PairingOffer {
        expires_at: "2026-09-28T12:05:00Z".to_owned(),
        ..offer
    };
    let ends = caption(&timed);
    let clock = ends.rsplit(' ').next().expect("time");
    assert_eq!(clock.len(), 5);
    assert_eq!(clock.as_bytes()[2], b':');
}
