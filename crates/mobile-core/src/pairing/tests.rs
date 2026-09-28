use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};

use sdrmm_wire::{
    about::AboutResponse,
    phone::{PHONE_SECRET_BYTES, PairResponse, Phone},
};

use super::*;
use crate::stub_server::{StubIdentity, StubRequest, StubResponse, StubServer};

const CODE: &str = "48210937";
const SERVER_ID: &str = "00112233445566778899aabbccddeeff";
const PHONE_ID: &str = "p0123456789abcdef";

fn about(protocol: u32) -> AboutResponse {
    AboutResponse {
        name: "SDR--".to_owned(),
        version: "0.9.0".to_owned(),
        protocol,
        server_id: SERVER_ID.to_owned(),
        server_name: "Shack".to_owned(),
        license: "AGPL-3.0-or-later".to_owned(),
        license_text: String::new(),
        repository: String::new(),
        components: Vec::new(),
        reveal: false,
    }
}

fn paired(protocol: u32) -> PairResponse {
    PairResponse {
        phone: Phone {
            id: PHONE_ID.to_owned(),
            name: "Pixel".to_owned(),
            platform: PhonePlatform::Android,
            created_at: "2026-09-28T12:00:00Z".to_owned(),
            last_seen: None,
            online: false,
            gps_nodes: Vec::new(),
        },
        token: PhoneToken::new(PHONE_ID.to_owned(), [9; PHONE_SECRET_BYTES]).encode(),
        server_id: SERVER_ID.to_owned(),
        server_name: "Shack".to_owned(),
        protocol,
    }
}

fn offer(host: &str, pin: &str) -> PairOffer {
    PairOffer {
        hosts: vec![host.to_owned()],
        code: CODE.to_owned(),
        fingerprint: Some(pin.to_owned()),
        fingerprint_short: None,
        protocol: API_PROTOCOL,
        server_name: None,
    }
}

fn input(offer: PairOffer) -> PairInput {
    PairInput {
        offer,
        phone_name: "Pixel".to_owned(),
        platform: Platform::Android,
        rebind: None,
        net: Net::new(Platform::Android),
        now_ms: 1_790_000_000_000,
    }
}

async fn pair_stub(reply: StubResponse) -> StubServer {
    StubServer::start(move |request: &StubRequest| match request.path() {
        "/api/about" => StubResponse::json(200, &about(API_PROTOCOL)),
        "/api/phones/pair" => reply.clone(),
        _ => StubResponse::error(404, "no route"),
    })
    .await
}

fn link(code: &str) -> String {
    PairUri {
        hosts: vec!["192.168.1.20:8443".to_owned(), "[fe80::1]:8443".to_owned()],
        code: code.to_owned(),
        pin: "ab".repeat(32),
        protocol: API_PROTOCOL,
        name: Some("Shack".to_owned()),
    }
    .to_uri()
}

fn discovered(fp: &str) -> DiscoveredServer {
    DiscoveredServer {
        name: "SDR-- pi".to_owned(),
        hosts: vec![
            "[fe80::1]:8443".to_owned(),
            "192.168.1.20:8443".to_owned(),
            "not a host".to_owned(),
            "192.168.1.20:8443".to_owned(),
            "10.0.0.2:8443".to_owned(),
        ],
        txt: HashMap::from([
            ("v".to_owned(), "1".to_owned()),
            ("p".to_owned(), "1".to_owned()),
            ("fp".to_owned(), fp.to_owned()),
            ("n".to_owned(), "Shack".to_owned()),
        ]),
    }
}

#[test]
fn a_qr_link_becomes_a_verified_offer() {
    let offer = offer_from_link(&link(CODE)).expect("offer");
    assert_eq!(
        offer,
        PairOffer {
            hosts: vec!["192.168.1.20:8443".to_owned(), "[fe80::1]:8443".to_owned()],
            code: CODE.to_owned(),
            fingerprint: Some("ab".repeat(32)),
            fingerprint_short: None,
            protocol: API_PROTOCOL,
            server_name: Some("Shack".to_owned()),
        }
    );
    assert!(matches!(
        offer_from_link("https://example.org"),
        Err(CoreError::InvalidLink { reason }) if reason == "not a pairing link"
    ));
}

#[tokio::test]
async fn an_8_digit_code_is_required() {
    assert!(matches!(
        offer_from_link(&link("482109")),
        Err(CoreError::InvalidLink { reason }) if reason == "bad code"
    ));
    let short = invalid("Code needs 8 digits");
    assert_eq!(
        offer_from_discovery(&discovered(&"cd".repeat(32)), "123456"),
        Err(short.clone())
    );
    assert_eq!(
        offer_from_discovery(&discovered(&"cd".repeat(32)), "1234567a"),
        Err(short.clone())
    );
    let net = Net::new(Platform::Ios);
    assert_eq!(
        offer_manual("127.0.0.1:1".to_owned(), "1234567".to_owned(), net).await,
        Err(short.clone())
    );
    let mut seven = offer("127.0.0.1:1", &"ab".repeat(32));
    seven.code = "1234567".to_owned();
    assert_eq!(pair(input(seven)).await, Err(short));
}

#[test]
fn a_discovered_server_is_unverified_and_keeps_its_hosts_ipv4_first() {
    let offer = offer_from_discovery(&discovered(&"cd".repeat(32)), CODE).expect("offer");
    assert_eq!(
        offer.hosts,
        ["192.168.1.20:8443", "10.0.0.2:8443", "[fe80::1]:8443"]
    );
    assert_eq!(offer.server_name.as_deref(), Some("Shack"));
    assert_eq!(offer.protocol, 1);
    assert_eq!(offer.code, CODE);
    let mut nameless = discovered(&"cd".repeat(32));
    nameless.txt.remove("n");
    assert_eq!(
        offer_from_discovery(&nameless, CODE)
            .expect("offer")
            .server_name
            .as_deref(),
        Some("SDR-- pi")
    );
}

#[test]
fn a_discovered_txt_fingerprint_is_used_but_not_trusted() {
    let fp = "cd".repeat(32);
    let offer = offer_from_discovery(&discovered(&fp), CODE).expect("offer");
    assert_eq!(offer.fingerprint.as_deref(), Some(fp.as_str()));
    assert_eq!(
        offer.fingerprint_short.as_deref(),
        Some("CDCD CDCD CDCD CDCD CDCD")
    );
    assert_eq!(
        offer_from_discovery(&discovered("CD"), CODE),
        Err(invalid("bad key"))
    );
    let mut no_protocol = discovered(&fp);
    no_protocol.txt.remove("p");
    assert_eq!(
        offer_from_discovery(&no_protocol, CODE),
        Err(invalid("bad protocol"))
    );
}

#[test]
fn a_manual_host_gets_the_default_port() {
    let cases = [
        ("192.168.1.20", "192.168.1.20:8443"),
        (" 192.168.1.20:9000 ", "192.168.1.20:9000"),
        ("fe80::1", "[fe80::1]:8443"),
        ("[fe80::1]", "[fe80::1]:8443"),
        ("[fe80::1]:9000", "[fe80::1]:9000"),
        ("pi.local", "pi.local:8443"),
    ];
    for (typed, host) in cases {
        assert_eq!(manual_host(typed).as_deref(), Ok(host), "{typed}");
    }
    for bad in ["", "pi local", "pi.local:0", "fe80::1%en0"] {
        assert!(manual_host(bad).is_err(), "{bad}");
    }
}

#[tokio::test]
async fn probe_records_the_key_it_saw_when_the_offer_has_none() {
    let stub = pair_stub(StubResponse::status(500)).await;
    let net = Net::new(Platform::Ios);
    let offer = offer_manual(stub.host(), CODE.to_owned(), net)
        .await
        .expect("probed");
    assert_eq!(offer.fingerprint.as_deref(), Some(stub.pin.as_str()));
    assert_eq!(offer.fingerprint_short, Some(phone::key_check(&stub.pin)));
    assert_eq!(offer.server_name.as_deref(), Some("Shack"));
    assert_eq!(offer.protocol, API_PROTOCOL);
    assert_eq!(offer.hosts, [stub.host()]);
    let requests = stub.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path(), "/api/about");
    assert_eq!(requests[0].header("authorization"), None);
}

#[tokio::test]
async fn an_unreachable_manual_host_names_the_host() {
    let net = Net::new(Platform::Ios);
    assert_eq!(
        offer_manual("127.0.0.1:1".to_owned(), CODE.to_owned(), net).await,
        Err(CoreError::Unreachable {
            hosts: vec!["127.0.0.1:1".to_owned()]
        })
    );
    let android = Net::new(Platform::Android);
    assert_eq!(
        offer_manual("127.0.0.1:1".to_owned(), CODE.to_owned(), android.clone()).await,
        Err(CoreError::LocalNetworkBlocked)
    );
    android.allow_local(true);
    assert!(matches!(
        offer_manual("127.0.0.1:1".to_owned(), CODE.to_owned(), android).await,
        Err(CoreError::Unreachable { .. })
    ));
}

#[tokio::test]
async fn pair_refuses_a_bad_code_or_name_before_any_network() {
    let stub = pair_stub(StubResponse::json(200, &paired(API_PROTOCOL))).await;
    let mut bad_code = offer(&stub.host(), &stub.pin);
    bad_code.code = "1234".to_owned();
    assert_eq!(
        pair(input(bad_code)).await,
        Err(invalid("Code needs 8 digits"))
    );
    let mut bad_name = input(offer(&stub.host(), &stub.pin));
    bad_name.phone_name = "x".repeat(65);
    assert!(matches!(
        pair(bad_name).await,
        Err(CoreError::Refused { .. })
    ));
    let mut no_key = offer(&stub.host(), &stub.pin);
    no_key.fingerprint = None;
    assert_eq!(pair(input(no_key)).await, Err(invalid("No server key")));
    assert_eq!(stub.handshakes(), 0);
}

#[tokio::test]
async fn pair_maps_status_codes_to_errors() {
    let cases = [
        (
            StubResponse::error(401, "Wrong code, 3 tries left"),
            CoreError::WrongCode,
        ),
        (
            StubResponse::error(409, "Offer burned"),
            CoreError::Refused {
                message: "Offer burned".to_owned(),
            },
        ),
        (
            StubResponse::error(400, "Name must be 1 to 64 characters"),
            CoreError::Refused {
                message: "Name must be 1 to 64 characters".to_owned(),
            },
        ),
        (
            StubResponse::error(429, "Too many tries, make a new code")
                .with_header("retry-after", "30")
                .after(std::time::Duration::from_millis(20)),
            CoreError::Refused {
                message: "Too many tries, make a new code".to_owned(),
            },
        ),
        (
            StubResponse::status(502),
            CoreError::Server {
                status: 502,
                message: "no error body".to_owned(),
            },
        ),
    ];
    for (reply, expected) in cases {
        let stub = pair_stub(reply).await;
        assert_eq!(
            pair(input(offer(&stub.host(), &stub.pin))).await,
            Err(expected)
        );
    }
}

#[tokio::test]
async fn expired_offer_is_code_expired() {
    let stub = pair_stub(StubResponse::error(404, "Code expired")).await;
    assert_eq!(
        pair(input(offer(&stub.host(), &stub.pin))).await,
        Err(CoreError::CodeExpired)
    );
}

#[tokio::test]
async fn pair_refuses_a_protocol_mismatch() {
    let stub = pair_stub(StubResponse::json(200, &paired(API_PROTOCOL + 1))).await;
    let mut newer = offer(&stub.host(), &stub.pin);
    newer.protocol = API_PROTOCOL + 1;
    assert_eq!(
        pair(input(newer)).await,
        Err(CoreError::ProtocolMismatch {
            server: API_PROTOCOL + 1,
            app: API_PROTOCOL
        })
    );
    assert_eq!(stub.handshakes(), 0);
    assert_eq!(
        pair(input(offer(&stub.host(), &stub.pin))).await,
        Err(CoreError::ProtocolMismatch {
            server: API_PROTOCOL + 1,
            app: API_PROTOCOL
        })
    );
}

#[tokio::test]
async fn pin_mismatch_is_key_mismatch_and_sends_nothing() {
    let stub = pair_stub(StubResponse::json(200, &paired(API_PROTOCOL))).await;
    let other = StubIdentity::generate();
    assert_eq!(
        pair(input(offer(&stub.host(), &other.pin))).await,
        Err(CoreError::KeyMismatch)
    );
    assert!(stub.requests().is_empty());
}

#[tokio::test]
async fn pair_builds_a_record_with_the_winning_host_first() {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let stub = StubServer::start({
        let bodies = bodies.clone();
        move |request: &StubRequest| {
            bodies
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(request.body.clone());
            StubResponse::json(200, &paired(API_PROTOCOL))
        }
    })
    .await;
    let mut two = offer("127.0.0.1:1", &stub.pin);
    two.hosts.push(stub.host());
    let mut request = input(two);
    request.rebind = Some("sdrmm-phone.old".to_owned());
    let record = pair(request).await.expect("paired");
    assert_eq!(record.hosts, vec![stub.host(), "127.0.0.1:1".to_owned()]);
    assert_eq!(record.pin, stub.pin);
    assert_eq!(record.server_id, SERVER_ID);
    assert_eq!(record.phone_id, PHONE_ID);
    assert_eq!(record.token, paired(API_PROTOCOL).token);
    assert_eq!(record.paired_at_ms, 1_790_000_000_000);
    let bodies = bodies
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let sent: serde_json::Value = serde_json::from_slice(&bodies[0]).expect("json");
    assert_eq!(
        sent,
        serde_json::json!({
            "code": CODE,
            "name": "Pixel",
            "platform": "android",
            "protocol": API_PROTOCOL,
            "rebind": "sdrmm-phone.old"
        })
    );
}

#[tokio::test]
async fn a_bad_phone_key_from_the_server_is_refused() {
    let mut reply = paired(API_PROTOCOL);
    reply.token = PhoneToken::new("pfedcba9876543210".to_owned(), [1; PHONE_SECRET_BYTES]).encode();
    let stub = pair_stub(StubResponse::json(200, &reply)).await;
    assert_eq!(
        pair(input(offer(&stub.host(), &stub.pin))).await,
        Err(CoreError::internal("Server sent a bad phone key"))
    );
}
