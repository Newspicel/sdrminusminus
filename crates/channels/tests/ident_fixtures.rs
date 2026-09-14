use sdrmm_channels::testgen::ident_fixtures::{Expect, FIXTURES, identify, judge, samples};

fn fixture_bytes(file: &str) -> Vec<u8> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/");
    std::fs::read(format!("{path}{file}")).unwrap_or_else(|e| panic!("{file}: {e}"))
}

#[test]
fn every_recorded_fixture_is_named_as_recorded() {
    let mut failures = Vec::new();
    for fixture in FIXTURES {
        let outcome = identify(fixture, &samples(&fixture_bytes(&fixture.file())));
        if let Err(miss) = judge(fixture, &outcome) {
            failures.push(miss);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn what_lies_beyond_reach_says_why() {
    for fixture in FIXTURES {
        if let Expect::Beyond(why) = &fixture.expect {
            assert!(!why.is_empty(), "{}", fixture.stem);
        }
    }
}
