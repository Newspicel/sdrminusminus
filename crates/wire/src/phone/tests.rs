use super::*;

const PIN: &str = "3fa9c2e011b790de5a4c7d2e9b8f60a1c4d3e2f1a0b9c8d7e6f5a4b3c2d1e0f9";

fn uri() -> PairUri {
    PairUri {
        hosts: vec![
            "192.168.1.20:8443".to_owned(),
            "pi.local:8443".to_owned(),
            "[fe80::1]:8443".to_owned(),
        ],
        code: "48210937".to_owned(),
        pin: PIN.to_owned(),
        protocol: 1,
        name: None,
    }
}

fn keys(link: &str) -> Vec<String> {
    url::Url::parse(link)
        .unwrap()
        .query_pairs()
        .map(|(key, _)| key.into_owned())
        .collect()
}

fn refused(link: &str) -> PairUriError {
    match PairUri::parse(link) {
        Err(error) => error,
        Ok(parsed) => panic!("accepted {link} as {parsed:?}"),
    }
}

fn link(query: &str) -> String {
    format!("sdrmm://pair?{query}")
}

fn valid_query() -> String {
    format!("h=pi.local:8443&c=48210937&fp={PIN}&p=1")
}

#[test]
fn pair_uri_round_trips() {
    let text = uri().to_uri();
    assert!(text.starts_with("sdrmm://pair?h="), "{text}");
    let parsed = PairUri::parse(&text).unwrap();
    assert_eq!(parsed, uri());
    assert_eq!(
        parsed.hosts,
        ["192.168.1.20:8443", "pi.local:8443", "[fe80::1]:8443"]
    );
}

#[test]
fn pair_uri_text_keeps_its_key_order_and_encoding() {
    let named = PairUri {
        name: Some("Shack & Co".to_owned()),
        ..uri()
    };
    assert_eq!(
        named.to_uri(),
        format!(
            "sdrmm://pair?h=192.168.1.20%3A8443&h=pi.local%3A8443&h=%5Bfe80%3A%3A1%5D%3A8443\
             &c=48210937&fp={PIN}&p=1&n=Shack+%26+Co"
        )
    );
}

#[test]
fn pair_uri_errors_read_short() {
    for (error, text) in [
        (PairUriError::NotPairing, "not a pairing link"),
        (
            PairUriError::Host("pi.local".to_owned()),
            "bad host pi.local",
        ),
        (PairUriError::Pin, "bad key"),
        (PairUriError::Duplicate("fp"), "fp given twice"),
    ] {
        assert_eq!(error.to_string(), text);
    }
}

#[test]
fn pair_uri_brackets_ipv6() {
    let parsed =
        PairUri::parse(&link(&format!("h=[fe80::1]:8443&c=48210937&fp={PIN}&p=1"))).unwrap();
    assert_eq!(parsed.hosts, ["[fe80::1]:8443"]);
}

#[test]
fn pair_uri_carries_no_token() {
    assert_eq!(keys(&uri().to_uri()), ["h", "h", "h", "c", "fp", "p"]);
}

#[test]
fn pair_uri_carries_an_optional_name() {
    let named = PairUri {
        name: Some("SDR-- Shack & Co".to_owned()),
        ..uri()
    };
    let text = named.to_uri();
    assert_eq!(keys(&text).last().map(String::as_str), Some("n"));
    assert_eq!(PairUri::parse(&text).unwrap(), named);
    assert_eq!(
        PairUri::parse(&link(&format!("{}&n=Shack", valid_query())))
            .unwrap()
            .name
            .as_deref(),
        Some("Shack")
    );
}

#[test]
fn pair_uri_ignores_unknown_keys() {
    let parsed = PairUri::parse(&link(&format!("{}&x=1&v=9", valid_query()))).unwrap();
    assert_eq!(parsed.code, "48210937");
    assert_eq!(parsed.protocol, 1);
}

#[test]
fn pair_uri_accepts_a_trailing_slash_and_drops_repeated_hosts() {
    let parsed = PairUri::parse(&format!(
        "sdrmm://pair/?h=b.local:1&h=a.local:2&h=b.local:1&c=48210937&fp={PIN}&p=7"
    ))
    .unwrap();
    assert_eq!(parsed.hosts, ["b.local:1", "a.local:2"]);
    assert_eq!(parsed.protocol, 7);

    let six = (1..=6)
        .map(|index| format!("h=host{index}.local:8443"))
        .collect::<Vec<_>>()
        .join("&");
    let parsed = PairUri::parse(&link(&format!(
        "{six}&h=host1.local:8443&c=48210937&fp={PIN}&p=1"
    )))
    .unwrap();
    assert_eq!(parsed.hosts.len(), MAX_PAIR_HOSTS);
}

#[test]
fn pair_uri_refuses_other_links() {
    let query = valid_query();
    for text in [
        format!("https://pair?{query}"),
        format!("sdrmm://other?{query}"),
        format!("sdrmm://pair/path?{query}"),
        format!("sdrmm://pair:80?{query}"),
        format!("sdrmm://user@pair?{query}"),
        format!("sdrmm:pair?{query}"),
        "not a link".to_owned(),
    ] {
        assert_eq!(refused(&text), PairUriError::NotPairing, "{text}");
    }
}

#[test]
fn pair_uri_refuses_bad_hosts() {
    let rest = format!("c=48210937&fp={PIN}&p=1");
    assert_eq!(refused(&link(&rest)), PairUriError::NoHost);
    let seven = (1..=7)
        .map(|index| format!("h=host{index}.local:8443"))
        .collect::<Vec<_>>()
        .join("&");
    assert_eq!(
        refused(&link(&format!("{seven}&{rest}"))),
        PairUriError::TooManyHosts
    );
    for host in [
        "pi.local",
        "pi.local:0",
        "pi.local:65536",
        "pi.local:08443",
        "pi_local:8443",
        ":8443",
        "[fe80::1%25en0]:8443",
        "[not-v6]:8443",
        "fe80::1:8443",
    ] {
        let decoded = url::form_urlencoded::parse(format!("h={host}").as_bytes())
            .next()
            .map(|(_, value)| value.into_owned())
            .unwrap();
        assert_eq!(
            refused(&link(&format!("h={host}&{rest}"))),
            PairUriError::Host(decoded),
            "{host}"
        );
    }
}

#[test]
fn pair_uri_refuses_bad_fields() {
    let host = "h=pi.local:8443";
    let cases = [
        (format!("{host}&fp={PIN}&p=1"), PairUriError::Code),
        (format!("{host}&c=4821093&fp={PIN}&p=1"), PairUriError::Code),
        (
            format!("{host}&c=4821093a&fp={PIN}&p=1"),
            PairUriError::Code,
        ),
        (format!("{host}&c=48210937&fp=3fa9&p=1"), PairUriError::Pin),
        (
            format!("{host}&c=48210937&fp={}&p=1", PIN.to_ascii_uppercase()),
            PairUriError::Pin,
        ),
        (
            format!("{host}&c=48210937&fp={PIN}&p=one"),
            PairUriError::Protocol,
        ),
        (
            format!("{host}&c=48210937&fp={PIN}&p=%2B1"),
            PairUriError::Protocol,
        ),
        (
            format!("{host}&c=48210937&fp={PIN}"),
            PairUriError::Protocol,
        ),
        (
            format!("{host}&c=48210937&c=48210937&fp={PIN}&p=1"),
            PairUriError::Duplicate("c"),
        ),
        (
            format!("{host}&c=48210937&fp={PIN}&fp={PIN}&p=1"),
            PairUriError::Duplicate("fp"),
        ),
        (
            format!("{host}&c=48210937&fp={PIN}&p=1&p=1"),
            PairUriError::Duplicate("p"),
        ),
        (
            format!("{host}&c=48210937&fp={PIN}&p=1&n=a&n=b"),
            PairUriError::Duplicate("n"),
        ),
        (
            format!("{host}&c=48210937&fp={PIN}&p=1&n="),
            PairUriError::Name,
        ),
        (
            format!("{host}&c=48210937&fp={PIN}&p=1&n={}", "x".repeat(65)),
            PairUriError::Name,
        ),
        (
            format!("{host}&c=48210937&fp={PIN}&p=1&n=a%0Ab"),
            PairUriError::Name,
        ),
    ];
    for (query, error) in cases {
        assert_eq!(refused(&link(&query)), error, "{query}");
    }
}

#[test]
fn phone_token_round_trips_and_redacts() {
    let mut secret = [0_u8; PHONE_SECRET_BYTES];
    for (index, byte) in secret.iter_mut().enumerate() {
        *byte = u8::try_from(index * 7).unwrap();
    }
    let token = PhoneToken::new("p0123456789abcdef".to_owned(), secret);
    let text = token.encode();
    let secret_hex = hex(&secret);
    assert_eq!(text, format!("sdrmm-phone.p0123456789abcdef.{secret_hex}"));
    let parsed = PhoneToken::parse(&text).unwrap();
    assert_eq!(parsed, token);
    assert_eq!(parsed.secret(), &secret);

    let debug = format!("{token:?}");
    assert_eq!(
        debug,
        r#"PhoneToken { phone: "p0123456789abcdef", secret: <redacted> }"#
    );
    assert!(!debug.contains(&secret_hex[..8]));

    for bad in [
        text.replacen("sdrmm-phone.", "sdrmm-token.", 1),
        text[..text.len() - 2].to_owned(),
        format!("{text}00"),
        text.to_ascii_uppercase(),
        format!(
            "sdrmm-phone.p0123456789ABCDEF.{}",
            &text[text.len() - PHONE_SECRET_BYTES * 2..]
        ),
        format!(
            "sdrmm-phone.p0123456789abcdef.{}",
            secret_hex.to_ascii_uppercase()
        ),
        format!("sdrmm-phone.p0123456789abcdef{secret_hex}"),
        format!("sdrmm-phone.q0123456789abcdef.{secret_hex}"),
        String::new(),
    ] {
        assert_eq!(PhoneToken::parse(&bad), None, "{bad}");
    }
}

#[test]
fn key_check_groups_the_pin() {
    assert_eq!(key_check(PIN), "3FA9 C2E0 11B7 90DE 5A4C");
    assert_eq!(group_code("48210937"), "4821 0937");
    assert_eq!(group_code(""), "");
}

#[test]
fn phone_rules_hold_at_their_edges() {
    assert!(valid_phone_id("p0123456789abcdef"));
    for bad in [
        "p0123456789abcde",
        "p0123456789abcdef0",
        "P0123456789abcdef",
        "p0123456789ABCDEF",
        "x0123456789abcdef",
    ] {
        assert!(!valid_phone_id(bad), "{bad}");
    }
    assert!(valid_phone_name("Pixel 9"));
    assert!(valid_phone_name(&"é".repeat(MAX_PHONE_NAME_LEN)));
    for bad in ["", " Pixel", "Pixel ", "Pi\u{7}xel"] {
        assert!(!valid_phone_name(bad), "{bad:?}");
    }
    assert!(!valid_phone_name(&"a".repeat(MAX_PHONE_NAME_LEN + 1)));
    assert!(valid_pin(PIN));
    assert!(!valid_pin(&PIN[1..]));
    assert!(valid_pair_code("00000000"));
    assert!(!valid_pair_code("0000000"));
    assert!(!valid_pair_code("０0000000"));
    assert!(valid_pair_host("[::1]:1"));
    assert!(valid_pair_host("a:65535"));
    let long = format!("{}:65535", "a".repeat(253));
    assert!(valid_pair_host(&long));
    assert!(!valid_pair_host(&format!("a{long}")));
}

#[test]
fn hex_is_lowercase_both_ways() {
    let bytes = [0x00, 0x0f, 0xa5, 0xff];
    assert_eq!(hex(&bytes), "000fa5ff");
    assert_eq!(unhex("000fa5ff").unwrap(), bytes);
    assert_eq!(unhex(""), Some(Vec::new()));
    assert_eq!(unhex("000FA5FF"), None);
    assert_eq!(unhex("abc"), None);
    assert_eq!(unhex("0g"), None);
}

#[test]
fn phone_states_are_tagged_by_state() {
    assert_eq!(
        serde_json::to_value(OfferState::Used {
            phone: "p0123456789abcdef".to_owned()
        })
        .unwrap(),
        serde_json::json!({"state":"used","phone":"p0123456789abcdef"})
    );
    assert_eq!(
        serde_json::to_value(PhoneListenerState::Failed {
            port: 8443,
            reason: "in use".to_owned()
        })
        .unwrap(),
        serde_json::json!({"state":"failed","port":8443,"reason":"in use"})
    );
    assert_eq!(
        serde_json::to_value(MdnsState::Off).unwrap(),
        serde_json::json!({"state":"off"})
    );
    assert_eq!(
        PhoneAccess::default(),
        PhoneAccess {
            enabled: false,
            port: DEFAULT_PHONE_PORT
        }
    );
    let phone: Phone = serde_json::from_value(serde_json::json!({
        "id": "p0123456789abcdef",
        "name": "Pixel",
        "platform": "android",
        "created_at": "2026-09-28T12:00:00Z",
        "online": false
    }))
    .unwrap();
    assert_eq!(phone.platform, PhonePlatform::Android);
    assert!(phone.gps_nodes.is_empty() && phone.last_seen.is_none());
}

#[cfg(feature = "pin")]
mod pin {
    use super::*;

    const FIRST: &str = concat!(
        "3082018430820129a003020102020101300a06082a8648ce3d04030230133111300f06035504030c0870692e",
        "6c6f63616c301e170d3236303932383137333134315a170d3336303932353137333134315a30133111300f06",
        "035504030c0870692e6c6f63616c3059301306072a8648ce3d020106082a8648ce3d0301070342000475005f",
        "cd7a62632be4c6e1a64ebd80d5b48a26bf5bf001f763d2cb88471708a47f92275a2a4e70513808b530685c5a",
        "4624b05dbf12eead632f82c28be24cde13a36e306c301d0603551d0e04160414beb9f7f4fab4ca6e0b7399b9",
        "61463589c9083c37301f0603551d23041830168014beb9f7f4fab4ca6e0b7399b961463589c9083c37300f06",
        "03551d130101ff040530030101ff30190603551d1104123010820870692e6c6f63616c8704c0a80114300a06",
        "082a8648ce3d040302034900304602210087151dd6447b405af467223948acc5125f85a952f226c3bce6ccc3",
        "5310700ffa022100a3a382a3745dc15de1a4efee7f2f5955a46f832dab3bd84b8589284d3b3985fa",
    );
    const REMINTED: &str = concat!(
        "3082018230820129a003020102020102300a06082a8648ce3d04030230133111300f06035504030c0870692e",
        "6c6f63616c301e170d3236303932383137333134315a170d3336303932353137333134315a30133111300f06",
        "035504030c0870692e6c6f63616c3059301306072a8648ce3d020106082a8648ce3d0301070342000475005f",
        "cd7a62632be4c6e1a64ebd80d5b48a26bf5bf001f763d2cb88471708a47f92275a2a4e70513808b530685c5a",
        "4624b05dbf12eead632f82c28be24cde13a36e306c301d0603551d0e04160414beb9f7f4fab4ca6e0b7399b9",
        "61463589c9083c37301f0603551d23041830168014beb9f7f4fab4ca6e0b7399b961463589c9083c37300f06",
        "03551d130101ff040530030101ff30190603551d1104123010820870692e6c6f63616c8704c0a80115300a06",
        "082a8648ce3d040302034700304402206b1f7a03f310f212e34a2089de8a175763e40abe10d14d140ea336b8",
        "d1ba4414022031043d37fffd319fc1f7a90168a2f1daf2a9872bc46baff3c8fe191d005e4ae6",
    );
    const OTHER_KEY: &str = concat!(
        "3082018430820129a003020102020101300a06082a8648ce3d04030230133111300f06035504030c0870692e",
        "6c6f63616c301e170d3236303932383137333134315a170d3336303932353137333134315a30133111300f06",
        "035504030c0870692e6c6f63616c3059301306072a8648ce3d020106082a8648ce3d030107034200045b29dc",
        "4cae8687ba6401e2f2ff4880ea700132e8de62e92c749fb96d3210db3019fe1539a6be2c6c621ab8a761bd70",
        "a5a581e8e2161dde544a9bba9b40ff5b64a36e306c301d0603551d0e041604142464eb788cfd63550c4b7a88",
        "6b0888cac8fd7e3f301f0603551d230418301680142464eb788cfd63550c4b7a886b0888cac8fd7e3f300f06",
        "03551d130101ff040530030101ff30190603551d1104123010820870692e6c6f63616c8704c0a80114300a06",
        "082a8648ce3d0403020349003046022100c58ed3a1383f46202ff7d79c859a05b40b161a8e06742a31546531",
        "22635b94b5022100a81f46d4c7e06edfab1f3ea4b227cc13d16e5fd5f22038e85113b27a16ae4ad2",
    );
    const FIRST_SPKI_SHA256: &str =
        "821420b7ba9aa593f32b49e4cfd4998f69f6f8cfe4eebfed2755021dbb1482aa";
    const FIRST_CERT_SHA256: &str =
        "64a4c50c4f3d94ad80ce91b530152436479bef901826ccdfd2701c86ed769ddc";
    const OTHER_SPKI_SHA256: &str =
        "f223f4fd5f0e4f24c5fd5f493d7c3b262322de3e28309afcfc123b5284af021e";

    fn pin_of(cert: &str) -> Result<String, PinError> {
        spki_pin(&unhex(cert).unwrap())
    }

    #[test]
    fn spki_pin_hashes_the_subject_public_key_info() {
        let pin = pin_of(FIRST).unwrap();
        assert_eq!(pin, FIRST_SPKI_SHA256);
        assert!(valid_pin(&pin));
        assert_ne!(pin, FIRST_CERT_SHA256);
    }

    #[test]
    fn spki_pin_survives_a_remint_with_the_same_key() {
        assert_ne!(FIRST, REMINTED);
        assert_eq!(pin_of(REMINTED).unwrap(), FIRST_SPKI_SHA256);
        assert_eq!(pin_of(OTHER_KEY).unwrap(), OTHER_SPKI_SHA256);
    }

    #[test]
    fn spki_pin_refuses_what_is_no_certificate() {
        assert_eq!(spki_pin(&[]), Err(PinError::Parse));
        assert_eq!(spki_pin(b"not a certificate"), Err(PinError::Parse));
        let cut = unhex(FIRST).unwrap();
        assert_eq!(spki_pin(&cut[..cut.len() - 1]), Err(PinError::Parse));
    }
}
