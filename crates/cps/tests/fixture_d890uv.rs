#![allow(clippy::unwrap_used, clippy::expect_used)]

use sdrmm_cps::{Image, image::changed_blocks, models};

const FIXTURE: &[u8] = include_bytes!("../../../fixtures/cps/anytone-d890uv-v100.img");

fn fixture() -> Image {
    Image::from_bytes(FIXTURE).expect("the recorded D890UV image parses")
}

#[test]
fn a_recorded_radio_image_decodes_into_the_codeplug_it_was_programmed_with() {
    let model = models().get("anytone-d890uv").expect("D890UV");
    let codeplug = model.decode(&fixture()).expect("decode");
    let counts = codeplug.counts();

    assert_eq!(counts.channels, 35);
    assert_eq!(counts.zones, 4);
    assert_eq!(counts.scan_lists, 1);
    assert_eq!(counts.group_lists, 1);
    assert_eq!(counts.contacts, 1);

    let first = &codeplug.channels[0];
    assert_eq!(first.name, "PMR FM 1");
    assert_eq!(first.rx_hz, 446_006_250);
    assert_eq!(first.tx_hz, 446_006_250);
    assert_eq!(first.mode.kind(), sdrmm_wire::cps::ChannelKind::Fm);
    assert_eq!(first.power, sdrmm_wire::cps::Power::Mid);

    let analogue = codeplug
        .channels
        .iter()
        .find(|channel| channel.name == "MT6 FM")
        .expect("the MT6 analogue channel");
    let sdrmm_wire::cps::ChannelMode::Fm(params) = &analogue.mode else {
        panic!("MT6 FM is an analogue channel");
    };
    assert_eq!(params.bandwidth, sdrmm_wire::cps::Bandwidth::Narrow);
    assert_eq!(
        params.tx_tone,
        Some(sdrmm_wire::cps::Tone::Ctcss { decihertz: 2035 })
    );
    assert_eq!(params.rx_tone, params.tx_tone);
    assert_eq!(analogue.power, sdrmm_wire::cps::Power::High);

    let digital: Vec<_> = codeplug
        .channels
        .iter()
        .filter(|channel| channel.mode.kind() == sdrmm_wire::cps::ChannelKind::Dmr)
        .collect();
    assert_eq!(digital.len(), 18);
    let sdrmm_wire::cps::ChannelMode::Dmr(params) = &digital[0].mode else {
        panic!("a DMR channel carries DMR parameters");
    };
    assert_eq!(params.color_code, 1);
    assert_eq!(params.time_slot, sdrmm_wire::cps::TimeSlot::One);
    assert_eq!(params.contact.as_deref(), Some("MT6"));

    let mt6 = codeplug
        .channels
        .iter()
        .find(|channel| channel.name == "MT6 DMR")
        .expect("the MT6 digital channel");
    let sdrmm_wire::cps::ChannelMode::Dmr(params) = &mt6.mode else {
        panic!("MT6 DMR is a digital channel");
    };
    assert_eq!(params.color_code, 5);
    assert_eq!(params.radio_id.as_deref(), Some("Operator 2"));

    let zone = codeplug
        .zones
        .iter()
        .find(|zone| zone.name == "PMR FM")
        .expect("the PMR FM zone");
    assert_eq!(zone.channels_a.len(), 16);
    assert!(zone.channels_a.iter().all(|name| {
        codeplug
            .channels
            .iter()
            .any(|channel| &channel.name == name)
    }));

    let scan = &codeplug.scan_lists[0];
    assert_eq!(scan.name, "PMR");
    assert_eq!(scan.channels.len(), 32);

    assert_eq!(
        codeplug.settings.default_radio_id.as_deref(),
        Some("Operator 1")
    );
}

#[test]
fn re_encoding_a_recorded_image_changes_no_byte_the_radio_already_holds() {
    let model = models().get("anytone-d890uv").expect("D890UV");
    let image = fixture();
    let codeplug = model.decode(&image).expect("decode");
    let mut written = image.clone();
    let report = model.encode(&codeplug, &mut written).expect("encode");
    assert!(report.is_clean(), "{:?}", report.issues);
    let changed = changed_blocks(&image, &written, 16);
    assert!(
        changed.is_empty(),
        "{} blocks would be rewritten, first at {:#010x}",
        changed.len(),
        changed[0].0
    );
}

#[test]
fn a_recorded_codeplug_survives_the_trip_to_another_radio() {
    let anytone = models().get("anytone-d890uv").expect("D890UV");
    let radtel = models().get("radtel-rt4d").expect("RT4D");
    let source = anytone.decode(&fixture()).expect("decode");

    let mut target = radtel.blank_image();
    let report = radtel.encode(&source, &mut target).expect("encode");
    let landed = radtel.decode(&target).expect("decode");

    assert_eq!(landed.channels.len(), source.channels.len());
    assert_eq!(landed.zones.len(), source.zones.len());
    assert_eq!(report.before.channels, 35);
    assert_eq!(report.after.channels, 35);
    assert!(
        landed
            .channels
            .iter()
            .any(|channel| channel.mode.kind() == sdrmm_wire::cps::ChannelKind::Dmr),
        "the DMR channels must survive the move"
    );
    for (landed, source) in landed.channels.iter().zip(source.channels.iter()) {
        assert_eq!(landed.name, source.name);
        assert_eq!(landed.rx_hz, source.rx_hz);
    }
}
