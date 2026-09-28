use super::*;

const ID: &str = "p0123456789abcdef";
const OTHER: &str = "pfedcba9876543210";

fn store() -> Store {
    Store::open(None).expect("in-memory store")
}

fn at(minutes: i64) -> String {
    rfc3339(
        "2026-09-28T12:00:00Z"
            .parse::<jiff::Timestamp>()
            .expect("time")
            .saturating_add(jiff::SignedDuration::from_mins(minutes))
            .expect("in range"),
    )
}

fn offer(id: &str, created_minutes: i64) -> OfferRow {
    OfferRow {
        id: id.to_owned(),
        code: "48210937".to_owned(),
        name: None,
        created_at: at(created_minutes),
        expires_at: at(created_minutes + 5),
        failures: 0,
        state: OfferState::Live,
    }
}

fn phone(id: &str) -> PhoneRow {
    PhoneRow {
        id: id.to_owned(),
        name: "Pixel".to_owned(),
        platform: PhonePlatform::Android,
        secret_sha256: [7; 32],
        created_at: at(0),
        last_seen: None,
    }
}

#[test]
fn a_paired_phone_is_listed_renamed_and_deleted() {
    let store = store();
    store.open_offer(&offer("o1", 0)).expect("offer");
    store
        .pair_with_offer("o1", &PairWrite::New(phone(ID)))
        .expect("pair");
    assert_eq!(store.phones().expect("phones"), [phone(ID)]);
    store.rename_phone(ID, "Car").expect("rename");
    assert_eq!(store.phone(ID).expect("phone").name, "Car");
    store.delete_phone(ID).expect("delete");
    assert!(store.phones().expect("phones").is_empty());
    assert!(matches!(
        store.rename_phone(ID, "x"),
        Err(StoreError::PhoneNotFound(id)) if id == ID
    ));
    assert!(matches!(
        store.delete_phone(ID),
        Err(StoreError::PhoneNotFound(_))
    ));
    assert!(matches!(store.phone(ID), Err(StoreError::PhoneNotFound(_))));
}

#[test]
fn an_offer_is_used_once() {
    let store = store();
    store.open_offer(&offer("o1", 0)).expect("offer");
    store
        .pair_with_offer("o1", &PairWrite::New(phone(ID)))
        .expect("pair");
    assert!(matches!(
        store.pair_with_offer("o1", &PairWrite::New(phone(OTHER))),
        Err(StoreError::OfferGone)
    ));
    assert_eq!(store.phones().expect("phones").len(), 1);
    assert_eq!(
        store.latest_offer().expect("latest").expect("offer").state,
        OfferState::Used {
            phone: ID.to_owned()
        }
    );
}

#[test]
fn a_new_offer_supersedes_the_live_one_and_prunes_old_ones() {
    let store = store();
    store
        .open_offer(&offer("ancient", -3 * 24 * 60))
        .expect("old");
    store.open_offer(&offer("o1", 0)).expect("first");
    store.open_offer(&offer("o2", 1)).expect("second");
    let live = store.live_offer(&at(2)).expect("live").expect("an offer");
    assert_eq!(live.id, "o2");
    let count: i64 = store
        .lock()
        .query_row("SELECT COUNT(*) FROM phone_offers", [], |row| row.get(0))
        .expect("count");
    assert_eq!(count, 2, "the day-old offer was kept");
    let first: String = store
        .lock()
        .query_row(
            "SELECT state FROM phone_offers WHERE id = 'o1'",
            [],
            |row| row.get(0),
        )
        .expect("state");
    assert_eq!(first, "superseded");
}

#[test]
fn a_due_offer_expires_when_asked_for() {
    let store = store();
    store.open_offer(&offer("o1", 0)).expect("offer");
    assert!(store.live_offer(&at(4)).expect("live").is_some());
    assert!(store.live_offer(&at(5)).expect("live").is_none());
    assert_eq!(
        store.latest_offer().expect("latest").expect("offer").state,
        OfferState::Expired
    );
}

#[test]
fn failures_count_down_and_then_burn() {
    let store = store();
    store.open_offer(&offer("o1", 0)).expect("offer");
    for left in (1..PAIR_MAX_FAILURES).rev() {
        assert_eq!(
            store.fail_offer("o1").expect("fail"),
            OfferFailure::Counted { left }
        );
    }
    assert_eq!(store.fail_offer("o1").expect("fail"), OfferFailure::Burned);
    assert!(store.live_offer(&at(1)).expect("live").is_none());
    assert!(matches!(store.fail_offer("o1"), Err(StoreError::OfferGone)));
    let latest = store.latest_offer().expect("latest").expect("offer");
    assert_eq!(
        (latest.state, latest.failures),
        (OfferState::Burned, PAIR_MAX_FAILURES)
    );
}

#[test]
fn rotation_keeps_the_id_and_the_first_pairing_time() {
    let store = store();
    store.open_offer(&offer("o1", 0)).expect("offer");
    store
        .pair_with_offer("o1", &PairWrite::New(phone(ID)))
        .expect("pair");
    store.open_offer(&offer("o2", 10)).expect("offer");
    store
        .pair_with_offer(
            "o2",
            &PairWrite::Rotate {
                id: ID.to_owned(),
                name: "iPhone".to_owned(),
                platform: PhonePlatform::Ios,
                secret_sha256: [9; 32],
            },
        )
        .expect("rotate");
    let rotated = store.phone(ID).expect("phone");
    assert_eq!(rotated.created_at, at(0));
    assert_eq!(rotated.secret_sha256, [9; 32]);
    assert_eq!(
        (rotated.name.as_str(), rotated.platform),
        ("iPhone", PhonePlatform::Ios)
    );
}

#[test]
fn last_seen_only_moves_forward() {
    let store = store();
    store.open_offer(&offer("o1", 0)).expect("offer");
    store
        .pair_with_offer("o1", &PairWrite::New(phone(ID)))
        .expect("pair");
    store
        .touch_phones(&[(ID.to_owned(), at(20))])
        .expect("touch");
    store
        .touch_phones(&[(ID.to_owned(), at(10)), (OTHER.to_owned(), at(30))])
        .expect("touch");
    assert_eq!(store.phone(ID).expect("phone").last_seen, Some(at(20)));
}

#[test]
fn cancelling_says_whether_a_code_was_open() {
    let store = store();
    assert!(!store.cancel_offer().expect("cancel"));
    store.open_offer(&offer("o1", 0)).expect("offer");
    assert!(store.cancel_offer().expect("cancel"));
    assert_eq!(
        store.latest_offer().expect("latest").expect("offer").state,
        OfferState::Cancelled
    );
    assert!(!store.cancel_offer().expect("cancel"));
}

#[test]
fn a_corrupt_phone_row_is_an_error_not_a_guess() {
    let store = store();
    store
        .lock()
        .execute(
            "INSERT INTO phones (id, name, platform, secret_sha256, created_at)
             VALUES (?1, 'x', 'palm', x'00', '2026-09-28T12:00:00Z')",
            params![ID],
        )
        .expect("insert");
    assert!(matches!(store.phones(), Err(StoreError::Db(_))));
}
