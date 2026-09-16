use tracing_subscriber::prelude::*;

use super::*;

fn line(message: &str) -> LogLine {
    LogLine {
        at: "2026-01-01T00:00:00Z".to_string(),
        level: LogLevel::Info,
        target: "test".to_string(),
        message: message.to_string(),
    }
}

#[test]
fn the_ring_keeps_the_newest_lines_and_counts_what_it_dropped() {
    let ring = LogRing::new(3);
    for index in 0..5 {
        ring.push(line(&format!("line {index}")));
    }
    let kept: Vec<String> = ring.lines().into_iter().map(|l| l.message).collect();
    assert_eq!(kept, ["line 2", "line 3", "line 4"]);
    assert_eq!(ring.dropped(), 2);
}

#[test]
fn a_cleared_ring_reports_no_losses() {
    let ring = LogRing::new(1);
    ring.push(line("a"));
    ring.push(line("b"));
    assert_eq!(ring.dropped(), 1);
    ring.clear();
    assert!(ring.lines().is_empty());
    assert_eq!(ring.dropped(), 0);
}

#[test]
fn routable_addresses_are_masked_and_the_loopback_is_kept() {
    assert_eq!(
        redact_text("listening on 192.168.1.20:8080"),
        "listening on <ip>:8080"
    );
    assert_eq!(
        redact_text("bound http://127.0.0.1:8080/api/ws"),
        "bound http://127.0.0.1:8080/api/ws"
    );
    assert_eq!(redact_text("peer 2001:db8::5 left"), "peer <ip> left");
    assert_eq!(
        redact_text("unspecified 0.0.0.0:80"),
        "unspecified 0.0.0.0:80"
    );
}

#[test]
fn version_strings_survive_address_masking() {
    assert_eq!(
        redact_text("SoapySDR 0.8.1 with module 1.2"),
        "SoapySDR 0.8.1 with module 1.2"
    );
}

#[test]
fn a_registered_secret_never_reaches_a_line() {
    let secret = "token-3f9c1e-secret-for-this-test";
    hide_secret(secret);
    let masked = redact_text(&format!("Authorization: Bearer {secret}"));
    assert_eq!(masked, "Authorization: Bearer <redacted>");
    assert!(!masked.contains(secret));
}

#[test]
fn an_empty_secret_is_not_registered() {
    hide_secret("");
    assert_eq!(redact_text("nothing to hide"), "nothing to hide");
}

#[test]
fn the_home_directory_collapses_to_a_tilde() {
    let Some(home) = home_dir() else {
        return;
    };
    assert_eq!(
        redact_text(&format!("{home}/Library/sdrmm.db")),
        "~/Library/sdrmm.db"
    );
}

#[test]
fn position_fields_are_dropped_rather_than_rounded() {
    assert_eq!(redact_field("lat", "48.137154"), "<redacted>");
    assert_eq!(redact_field("longitude", "11.576124"), "<redacted>");
    assert_eq!(redact_field("locator", "JN58td"), "<redacted>");
    assert_eq!(redact_field("device", "rtlsdr-0"), "rtlsdr-0");
}

#[test]
fn a_long_message_is_cut_on_a_character_boundary() {
    let text = "ü".repeat(MAX_LOG_MESSAGE_LEN);
    let cut = truncate(&text, MAX_LOG_MESSAGE_LEN);
    assert!(cut.ends_with('…'));
    assert!(cut.len() <= MAX_LOG_MESSAGE_LEN + '…'.len_utf8());
}

#[test]
fn a_short_message_is_left_alone() {
    assert_eq!(truncate("short", MAX_LOG_MESSAGE_LEN), "short");
}

#[test]
fn the_layer_records_an_event_with_its_fields() {
    let marker = "diagnostics-layer-marker-7b2a";
    let subscriber = tracing_subscriber::registry().with(layer());
    tracing::subscriber::with_default(subscriber, || {
        tracing::warn!(device = "virtual-0", lat = 48.1, "{marker}");
    });
    let recorded = log()
        .lines()
        .into_iter()
        .find(|line| line.message.contains(marker))
        .expect("the layer records the event");
    assert_eq!(recorded.level, LogLevel::Warn);
    assert!(recorded.message.contains("device=virtual-0"));
    assert!(recorded.message.contains("lat=<redacted>"));
    assert!(!recorded.message.contains("48.1"));
}
