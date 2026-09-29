use std::sync::Arc;

use jiff::{SignedDuration, Timestamp};
use sdrmm_wire::{
    API_PROTOCOL, GpsNode, NodeBody, OfferState, PairRequest, PairResponse, PatchGraph, PatchNode,
    PhoneEndpoint, PhonePlatform, PhoneToken, Position, PositionSource,
    phone::{MAX_PHONE_NAME_LEN, PAIR_MAX_FAILURES, key_check, valid_pair_code},
};

use super::{pairing::draw_code, *};

pub(crate) const PIN: &str = "3fa9c2e011b790de5a4c0123456789abcdef0123456789abcdef0123456789ab";
pub(crate) const SERVER: &str = "shack";

pub(crate) fn endpoint() -> PhoneEndpoint {
    PhoneEndpoint {
        port: 8443,
        hosts: vec![
            "192.168.1.20:8443".to_owned(),
            "shack.local:8443".to_owned(),
        ],
        pin: PIN.to_owned(),
        key_check: key_check(PIN),
        dedicated: true,
    }
}

pub(crate) fn request(code: &str) -> PairRequest {
    PairRequest {
        code: code.to_owned(),
        name: "Pixel".to_owned(),
        platform: PhonePlatform::Android,
        protocol: API_PROTOCOL,
        rebind: None,
    }
}

pub(crate) fn pair_one(phones: &Phones) -> PairResponse {
    let offer = phones
        .create_offer(None, &endpoint(), SERVER, Timestamp::now())
        .expect("offer");
    phones
        .pair(&request(&offer.code), "server", SERVER, Timestamp::now())
        .expect("pair")
}

fn phones() -> Phones {
    Phones::new(Arc::new(Store::open(None).expect("in-memory store")))
}

fn token(response: &PairResponse) -> PhoneToken {
    PhoneToken::parse(&response.token).expect("token")
}

fn other_code(code: &str) -> String {
    format!(
        "{:08}",
        (code.parse::<u32>().expect("digits") + 1) % 100_000_000
    )
}

#[test]
fn an_offer_carries_the_link_the_key_and_the_code() {
    let phones = phones();
    let now = Timestamp::now();
    let offer = phones
        .create_offer(Some(" Car "), &endpoint(), SERVER, now)
        .expect("offer");
    assert!(valid_pair_code(&offer.code));
    assert_eq!(offer.key_check, "3FA9 C2E0 11B7 90DE 5A4C");
    let link = sdrmm_wire::PairUri::parse(&offer.uri).expect("pair link");
    assert_eq!(link.code, offer.code);
    assert_eq!(link.pin, PIN);
    assert_eq!(link.hosts, endpoint().hosts);
    assert_eq!(link.name.as_deref(), Some(SERVER));
    let stored = phones.store.offer(&offer.id).expect("read").expect("offer");
    assert_eq!(stored.name.as_deref(), Some("Car"));
    assert_eq!(
        offer.expires_at,
        crate::store::rfc3339(now.checked_add(SignedDuration::from_mins(5)).expect("time"))
    );
}

#[test]
fn an_offer_pairs_once() {
    let phones = phones();
    let offer = phones
        .create_offer(None, &endpoint(), SERVER, Timestamp::now())
        .expect("offer");
    let paired = phones
        .pair(&request(&offer.code), "server", SERVER, Timestamp::now())
        .expect("pair");
    assert_eq!(paired.server_id, "server");
    assert_eq!(paired.phone.name, "Pixel");
    assert!(matches!(
        phones.pair(&request(&offer.code), "server", SERVER, Timestamp::now()),
        Err(PairError::NoOffer)
    ));
    assert_eq!(phones.list(None).expect("list").len(), 1);
}

#[test]
fn a_new_offer_supersedes_the_old() {
    let phones = phones();
    let first = phones
        .create_offer(None, &endpoint(), SERVER, Timestamp::now())
        .expect("offer");
    let second = phones
        .create_offer(None, &endpoint(), SERVER, Timestamp::now())
        .expect("offer");
    if first.code != second.code {
        assert!(matches!(
            phones.pair(&request(&first.code), "server", SERVER, Timestamp::now()),
            Err(PairError::WrongCode { .. })
        ));
    }
    assert_eq!(
        phones
            .store
            .offer(&first.id)
            .expect("read")
            .expect("offer")
            .state,
        OfferState::Superseded
    );
    phones
        .pair(&request(&second.code), "server", SERVER, Timestamp::now())
        .expect("the new code pairs");
}

#[test]
fn five_wrong_codes_burn_the_offer() {
    let phones = phones();
    let offer = phones
        .create_offer(None, &endpoint(), SERVER, Timestamp::now())
        .expect("offer");
    let wrong = request(&other_code(&offer.code));
    for left in (1..PAIR_MAX_FAILURES).rev() {
        match phones.pair(&wrong, "server", SERVER, Timestamp::now()) {
            Err(PairError::WrongCode { left: said }) => assert_eq!(said, left),
            other => panic!("expected a counted failure, got {other:?}"),
        }
    }
    assert!(matches!(
        phones.pair(&wrong, "server", SERVER, Timestamp::now()),
        Err(PairError::Burned)
    ));
    assert!(matches!(
        phones.pair(&request(&offer.code), "server", SERVER, Timestamp::now()),
        Err(PairError::Burned)
    ));
    let status = phones
        .offer_status(Some(&endpoint()), SERVER, Timestamp::now())
        .expect("status")
        .expect("offer");
    assert_eq!(
        (status.state, status.failures, status.code),
        (OfferState::Burned, PAIR_MAX_FAILURES, None)
    );
}

#[test]
fn an_expired_offer_refuses() {
    let phones = phones();
    let now = Timestamp::now();
    let offer = phones
        .create_offer(None, &endpoint(), SERVER, now)
        .expect("offer");
    let later = now
        .checked_add(SignedDuration::from_secs(301))
        .expect("time");
    assert!(matches!(
        phones.pair(&request(&offer.code), "server", SERVER, later),
        Err(PairError::Expired)
    ));
    assert_eq!(PairError::Expired.to_string(), "Code expired");
    assert_eq!(
        phones
            .store
            .offer(&offer.id)
            .expect("read")
            .expect("offer")
            .state,
        OfferState::Expired
    );
}

#[test]
fn a_live_offer_shows_its_code_and_link_until_used() {
    let phones = phones();
    assert!(
        phones
            .offer_status(None, SERVER, Timestamp::now())
            .expect("status")
            .is_none()
    );
    let offer = phones
        .create_offer(None, &endpoint(), SERVER, Timestamp::now())
        .expect("offer");
    let live = phones
        .offer_status(Some(&endpoint()), SERVER, Timestamp::now())
        .expect("status")
        .expect("offer");
    assert_eq!(live.code.as_deref(), Some(offer.code.as_str()));
    assert_eq!(live.uri.as_deref(), Some(offer.uri.as_str()));
    let paired = pair_with(&phones, &offer.code);
    let used = phones
        .offer_status(Some(&endpoint()), SERVER, Timestamp::now())
        .expect("status")
        .expect("offer");
    assert_eq!(
        used.state,
        OfferState::Used {
            phone: paired.phone.id
        }
    );
    assert_eq!((used.code, used.uri), (None, None));
}

fn pair_with(phones: &Phones, code: &str) -> PairResponse {
    phones
        .pair(&request(code), "server", SERVER, Timestamp::now())
        .expect("pair")
}

#[test]
fn secrets_are_stored_hashed() {
    let phones = phones();
    let paired = pair_one(&phones);
    let token = token(&paired);
    let row = phones.store.phone(&token.phone).expect("row");
    assert_eq!(row.secret_sha256, token::hash(token.secret()));
    assert_ne!(&row.secret_sha256, token.secret());
    assert!(phones.verify(&token));
}

#[test]
fn rebind_keeps_the_id_and_rotates_the_secret() {
    let phones = phones();
    let first = pair_one(&phones);
    let offer = phones
        .create_offer(None, &endpoint(), SERVER, Timestamp::now())
        .expect("offer");
    let again = phones
        .pair(
            &PairRequest {
                rebind: Some(first.token.clone()),
                platform: PhonePlatform::Ios,
                ..request(&offer.code)
            },
            "server",
            SERVER,
            Timestamp::now(),
        )
        .expect("re-pair");
    assert_eq!(again.phone.id, first.phone.id);
    assert_eq!(again.phone.platform, PhonePlatform::Ios);
    assert!(!phones.verify(&token(&first)), "the old key still opens");
    assert!(phones.verify(&token(&again)));
    assert_eq!(phones.list(None).expect("list").len(), 1);
}

#[test]
fn a_rebind_token_that_does_not_verify_pairs_a_new_phone() {
    let phones = phones();
    let first = pair_one(&phones);
    let forged = PhoneToken::new(first.phone.id.clone(), [0; 32]).encode();
    let offer = phones
        .create_offer(None, &endpoint(), SERVER, Timestamp::now())
        .expect("offer");
    let second = phones
        .pair(
            &PairRequest {
                rebind: Some(forged),
                ..request(&offer.code)
            },
            "server",
            SERVER,
            Timestamp::now(),
        )
        .expect("pair");
    assert_ne!(second.phone.id, first.phone.id);
    assert!(phones.verify(&token(&first)));
}

#[test]
fn a_protocol_mismatch_is_refused() {
    let phones = phones();
    let offer = phones
        .create_offer(None, &endpoint(), SERVER, Timestamp::now())
        .expect("offer");
    let error = phones
        .pair(
            &PairRequest {
                protocol: API_PROTOCOL + 1,
                ..request(&offer.code)
            },
            "server",
            SERVER,
            Timestamp::now(),
        )
        .expect_err("a newer app is refused");
    assert!(matches!(
        error,
        PairError::Protocol { phone, server } if phone == API_PROTOCOL + 1 && server == API_PROTOCOL
    ));
    let status = phones
        .offer_status(None, SERVER, Timestamp::now())
        .expect("status")
        .expect("offer");
    assert_eq!((status.state, status.failures), (OfferState::Live, 0));
}

#[test]
fn a_name_must_fit() {
    let phones = phones();
    let offer = phones
        .create_offer(None, &endpoint(), SERVER, Timestamp::now())
        .expect("offer");
    for name in ["   ".to_owned(), "x".repeat(MAX_PHONE_NAME_LEN + 1)] {
        assert!(matches!(
            phones.pair(
                &PairRequest {
                    name,
                    ..request(&offer.code)
                },
                "server",
                SERVER,
                Timestamp::now()
            ),
            Err(PairError::Name)
        ));
    }
    assert!(matches!(
        phones.create_offer(Some(""), &endpoint(), SERVER, Timestamp::now()),
        Err(PairError::Name)
    ));
    let paired = pair_with(&phones, &offer.code);
    assert!(matches!(
        phones.rename(&paired.phone.id, "\u{7}", None),
        Err(PairError::Name)
    ));
    assert_eq!(
        phones
            .rename(&paired.phone.id, " Car ", None)
            .expect("rename")
            .name,
        "Car"
    );
}

#[test]
fn codes_are_eight_uniform_digits() {
    const DRAWS: usize = 10_000;
    let mut first_digits = [0_usize; 10];
    for _ in 0..DRAWS {
        let code = draw_code(|| token::random::<4>().map(u32::from_le_bytes)).expect("code");
        assert!(valid_pair_code(&code), "{code}");
        let digit = usize::from(code.as_bytes()[0] - b'0');
        first_digits[digit] += 1;
    }
    let expected = DRAWS as f64 / 10.0;
    let sigma = (DRAWS as f64 * 0.1 * 0.9).sqrt();
    for (digit, count) in first_digits.iter().enumerate() {
        assert!(
            (*count as f64 - expected).abs() <= 5.0 * sigma,
            "first digit {digit} drawn {count} times"
        );
    }
}

#[test]
fn draws_above_the_uniform_range_are_thrown_away() {
    let mut draws = [4_200_000_000_u32, u32::MAX, 123].into_iter();
    let code = draw_code(|| Ok(draws.next().expect("a draw"))).expect("code");
    assert_eq!(code, "00000123");
}

#[test]
fn the_server_id_is_stable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("sdrmm.db");
    let first = Store::open(Some(&path)).expect("store").server_id();
    let second = Store::open(Some(&path)).expect("store").server_id();
    assert_eq!(first, second);
    assert_eq!(first.len(), 32);
    assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
}

#[test]
fn sessions_count_sockets_per_phone() {
    let phones = phones();
    let id = pair_one(&phones).phone.id;
    let (first, was_first) = phones.join(&id);
    let (second, again_first) = phones.join(&id);
    assert!(was_first && !again_first);
    assert!(phones.online(&id));
    assert!(!phones.leave(first), "one socket is still open");
    assert!(phones.online(&id));
    assert!(phones.leave(second));
    assert!(!phones.online(&id));
}

#[test]
fn a_socket_of_an_unpaired_phone_is_revoked_at_once() {
    let phones = phones();
    let (guard, _) = phones.join("p0123456789abcdef");
    assert!(*guard.revoked.borrow());
}

#[test]
fn last_seen_is_flushed_to_the_store() {
    let phones = phones();
    let id = pair_one(&phones).phone.id;
    assert!(phones.store.phone(&id).expect("row").last_seen.is_none());
    phones.touch(&id);
    assert!(phones.one(&id, None).expect("phone").last_seen.is_some());
    phones.flush_seen().expect("flush");
    assert!(phones.store.phone(&id).expect("row").last_seen.is_some());
}

#[test]
fn a_listed_phone_names_its_gps_nodes() {
    let phones = phones();
    let id = pair_one(&phones).phone.id;
    let gps = |node: &str, phone: &str| PatchNode {
        id: node.to_owned(),
        body: NodeBody::Gps(GpsNode {
            source: Some(PositionSource::Phone {
                phone: phone.to_owned(),
            }),
        }),
        position: Position { x: 0.0, y: 0.0 },
        size: None,
        label: None,
    };
    let graph = PatchGraph {
        nodes: vec![gps("car", &id), gps("other", "pfedcba9876543210")],
        ..PatchGraph::default()
    };
    let listed = phones.list(Some(&graph)).expect("list");
    assert_eq!(listed[0].gps_nodes, ["car"]);
    assert!(!listed[0].online);
}
