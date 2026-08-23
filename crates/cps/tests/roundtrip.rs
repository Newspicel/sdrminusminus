#![allow(clippy::unwrap_used, clippy::expect_used)]

use sdrmm_cps::{Image, RadioModel, models};
use sdrmm_wire::cps::{
    Admit, Bandwidth, Channel, ChannelMode, Codeplug, Contact, ContactKind, DmrChannel, FmChannel,
    GroupList, Power, RadioId, ScanList, ScanTarget, TimeSlot, Tone, Zone,
};

fn sample() -> Codeplug {
    let mut codeplug = Codeplug::empty();
    codeplug.radio_ids = vec![RadioId {
        name: "Home".to_owned(),
        number: 2_628_001,
    }];
    codeplug.settings.default_radio_id = Some("Home".to_owned());
    codeplug.contacts = vec![
        Contact {
            name: "Austria".to_owned(),
            kind: ContactKind::Group,
            number: 232,
            ring: false,
        },
        Contact {
            name: "Worldwide".to_owned(),
            kind: ContactKind::Group,
            number: 91,
            ring: false,
        },
        Contact {
            name: "OE1ABC".to_owned(),
            kind: ContactKind::Private,
            number: 2_320_123,
            ring: false,
        },
    ];
    codeplug.group_lists = vec![GroupList {
        name: "Regional".to_owned(),
        contacts: vec!["Austria".to_owned(), "Worldwide".to_owned()],
    }];
    codeplug.channels = vec![
        Channel {
            name: "OE1XUU".to_owned(),
            rx_hz: 438_950_000,
            tx_hz: 431_350_000,
            power: Power::High,
            rx_only: false,
            timeout_s: None,
            scan_list: None,
            mode: ChannelMode::Fm(FmChannel {
                bandwidth: Bandwidth::Wide,
                rx_tone: Some(Tone::Ctcss { decihertz: 1230 }),
                tx_tone: Some(Tone::Ctcss { decihertz: 1230 }),
                squelch: None,
                admit: Admit::ChannelFree,
            }),
        },
        Channel {
            name: "OE1XDS DMR".to_owned(),
            rx_hz: 438_500_000,
            tx_hz: 430_900_000,
            power: Power::Low,
            rx_only: false,
            timeout_s: None,
            scan_list: None,
            mode: ChannelMode::Dmr(DmrChannel {
                color_code: 1,
                time_slot: TimeSlot::Two,
                contact: Some("Austria".to_owned()),
                group_list: Some("Regional".to_owned()),
                radio_id: None,
                admit: Admit::ColorCodeFree,
            }),
        },
        Channel {
            name: "Calling".to_owned(),
            rx_hz: 145_500_000,
            tx_hz: 145_500_000,
            power: Power::High,
            rx_only: false,
            timeout_s: None,
            scan_list: None,
            mode: ChannelMode::Fm(FmChannel {
                bandwidth: Bandwidth::Wide,
                rx_tone: None,
                tx_tone: None,
                squelch: None,
                admit: Admit::Always,
            }),
        },
    ];
    codeplug.zones = vec![Zone {
        name: "Vienna".to_owned(),
        channels_a: vec!["OE1XUU".to_owned(), "OE1XDS DMR".to_owned()],
        channels_b: Vec::new(),
    }];
    codeplug.scan_lists = vec![ScanList {
        name: "Vienna scan".to_owned(),
        channels: vec!["OE1XUU".to_owned(), "Calling".to_owned()],
        primary: Some(ScanTarget::Selected),
        secondary: Some(ScanTarget::Channel {
            name: "Calling".to_owned(),
        }),
        revert: sdrmm_wire::cps::ScanRevert::LastUsed,
        dwell_ms: Some(2900),
        hang_ms: Some(2900),
    }];
    codeplug
}

fn round_trip(model: &dyn RadioModel, source: &Codeplug) -> Codeplug {
    let mut image = model.blank_image();
    let report = model.encode(source, &mut image).expect("encode");
    let unexpected: Vec<_> = report
        .issues
        .iter()
        .filter(|issue| issue.severity == sdrmm_wire::cps::IssueSeverity::Dropped)
        .filter(|issue| {
            !(issue.scope == sdrmm_wire::cps::IssueScope::ScanList
                && model.limits().scan_lists == 0)
        })
        .collect();
    assert!(
        unexpected.is_empty(),
        "{:?} dropped entries: {unexpected:?}",
        model.descriptor().id
    );
    model.decode(&image).expect("decode")
}

#[test]
fn every_model_reproduces_what_it_was_given() {
    for model in models().iter() {
        let source = sample();
        let decoded = round_trip(model, &source);
        let id = model.descriptor().id;
        assert_eq!(decoded.contacts, source.contacts, "contacts on {id}");
        assert_eq!(
            decoded.group_lists, source.group_lists,
            "group lists on {id}"
        );
        assert_eq!(
            decoded.channels.len(),
            source.channels.len(),
            "channel count on {id}"
        );
        assert_eq!(
            decoded.channels.iter().map(|c| &c.name).collect::<Vec<_>>(),
            source.channels.iter().map(|c| &c.name).collect::<Vec<_>>(),
            "channel names on {id}"
        );
        assert_eq!(decoded.zones, source.zones, "zones on {id}");
        assert_eq!(
            decoded.radio_ids.first().map(|item| item.number),
            source.radio_ids.first().map(|item| item.number),
            "radio id on {id}"
        );
    }
}

#[test]
fn the_anytone_codec_keeps_every_channel_field() {
    let model = models()
        .get("anytone-d890uv")
        .expect("the D890UV is registered");
    let source = sample();
    let decoded = round_trip(model, &source);
    assert_eq!(decoded.channels, source.channels);
    assert_eq!(decoded.scan_lists, source.scan_lists);
    assert_eq!(decoded.settings.default_radio_id, Some("Home".to_owned()));
}

#[test]
fn the_radtel_codec_keeps_the_fields_that_radio_has() {
    let model = models().get("radtel-rt4d").expect("the RT4D is registered");
    let source = sample();
    let decoded = round_trip(model, &source);
    assert!(decoded.scan_lists.is_empty());
    for (decoded, source) in decoded.channels.iter().zip(source.channels.iter()) {
        assert_eq!(decoded.rx_hz, source.rx_hz);
        assert_eq!(decoded.tx_hz, source.tx_hz);
        assert_eq!(decoded.mode.kind(), source.mode.kind());
    }
}

#[test]
fn a_codeplug_moves_between_two_different_radios() {
    let anytone = models().get("anytone-d890uv").expect("D890UV");
    let radtel = models().get("radtel-rt4d").expect("RT4D");

    let mut image = anytone.blank_image();
    anytone.encode(&sample(), &mut image).expect("encode");
    let read_back = anytone.decode(&image).expect("decode");

    let mut target = radtel.blank_image();
    let report = radtel.encode(&read_back, &mut target).expect("re-encode");
    let landed = radtel.decode(&target).expect("decode on the target");

    assert_eq!(landed.channels.len(), read_back.channels.len());
    assert_eq!(landed.contacts, read_back.contacts);
    assert_eq!(report.target_model, "radtel-rt4d");
    assert_eq!(report.source_model.as_deref(), Some("anytone-d890uv"));
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.scope == sdrmm_wire::cps::IssueScope::ScanList),
        "the RT4D has no scan lists, so the report must say so: {:?}",
        report.issues
    );
}

#[test]
fn the_anytone_report_flags_the_one_field_no_radio_has_confirmed() {
    let model = models().get("anytone-d890uv").expect("D890UV");
    let mut image = model.blank_image();
    let report = model.encode(&sample(), &mut image).expect("encode");
    let shift = report
        .issues
        .iter()
        .find(|issue| issue.field.as_deref() == Some("tx_hz"))
        .expect("a channel with a transmit shift must carry the warning");
    assert_eq!(shift.severity, sdrmm_wire::cps::IssueSeverity::Note);
    assert!(
        shift
            .message
            .contains("never been checked against hardware")
    );

    let mut simplex = sample();
    simplex
        .channels
        .retain(|channel| channel.tx_hz == channel.rx_hz);
    let mut image = model.blank_image();
    let report = model.encode(&simplex, &mut image).expect("encode");
    assert!(
        !report
            .issues
            .iter()
            .any(|issue| issue.field.as_deref() == Some("tx_hz")),
        "a simplex-only codeplug must not carry the warning"
    );
}

#[test]
fn an_empty_image_decodes_to_an_empty_codeplug_rather_than_failing() {
    for model in models().iter() {
        let image = Image::new();
        let decoded = model.decode(&image).expect("decode an unread image");
        assert!(decoded.channels.is_empty());
        assert!(decoded.contacts.is_empty());
    }
}
