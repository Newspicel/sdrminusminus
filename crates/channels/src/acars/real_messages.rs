use super::adsc::{AdscTag, ReportKind};
use super::{AcarsApp, decode};

fn adsc_tags(text: &str) -> Vec<AdscTag> {
    let d = decode("B6", text, true);
    let Some(AcarsApp::Adsc { envelope, message }) = d.app else {
        panic!("expected ADS-C for {text}");
    };
    assert!(envelope.crc_ok, "real message must pass CRC: {text}");
    assert!(!message.err, "must parse fully: {text}");
    message.tags
}

fn close(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() < eps
}

#[test]
fn vt_anb_basic_report_with_refs_and_meteo() {
    let tags = adsc_tags(
        "/BOMASAI.ADS.VT-ANB072501A070A988CA73248F0E5DC10200000F5EE1ABC000102B885E0A19F5",
    );
    assert_eq!(tags.len(), 4);

    let AdscTag::Report(r) = &tags[0] else {
        panic!("tag 0: {tags:?}")
    };
    assert_eq!(r.kind, ReportKind::Basic);
    assert!(close(r.lat, 52.0401764, 1e-6), "lat {}", r.lat);
    assert!(close(r.lon, 19.8038864, 1e-6), "lon {}", r.lon);
    assert_eq!(r.alt_ft, 36004);
    assert!(close(r.timestamp_s, 3273.125, 1e-9));
    assert_eq!(r.accuracy, 7);
    assert!(r.nav_redundancy_ok);
    assert!(!r.tcas_ok);

    let AdscTag::EarthRef {
        true_track_deg,
        ground_speed_kt,
        vert_speed_fpm,
        ..
    } = &tags[1]
    else {
        panic!("tag 1: {tags:?}")
    };
    assert!(close(*true_track_deg, 263.7, 0.1));
    assert!(close(*ground_speed_kt, 516.0, 0.01));
    assert_eq!(*vert_speed_fpm, 0);

    let AdscTag::AirRef {
        true_heading_deg,
        mach,
        ..
    } = &tags[2]
    else {
        panic!()
    };
    assert!(close(*true_heading_deg, 266.8, 0.1));
    assert!(close(*mach, 0.8555, 1e-4));

    let AdscTag::Meteo {
        wind_speed_kt,
        wind_dir_deg,
        temperature_c,
        ..
    } = &tags[3]
    else {
        panic!()
    };
    assert!(close(*wind_speed_kt, 43.5, 0.01));
    assert!(close(*wind_dir_deg, 46.4, 0.1));
    assert!(close(*temperature_c, -62.75, 0.01));
}

#[test]
fn a6_pfe_report_with_climb() {
    let tags = adsc_tags(
        "/AUHASMO.ADS.A6-PFE0724D9586A36C92B2DCF1F0E74A8E4807C0F7219AF407C10422E9E08A1C4",
    );
    let AdscTag::Report(r) = &tags[0] else {
        panic!()
    };
    assert!(close(r.lat, 51.8189049, 1e-6));
    assert!(close(r.lon, 18.6704063, 1e-6));
    assert_eq!(r.alt_ft, 37552);
    assert!(r.tcas_ok);
    let AdscTag::EarthRef {
        vert_speed_fpm,
        ground_speed_kt,
        ..
    } = &tags[1]
    else {
        panic!()
    };
    assert_eq!(*vert_speed_fpm, 496);
    assert!(close(*ground_speed_kt, 457.0, 0.01));
}

#[test]
fn hb_jnb_waypoint_change_with_predicted_route() {
    let tags = adsc_tags(
        "/CTUE1YA.ADS.HB-JNB1424AB686D9308CA2EBA1D0D24A2C06C1B48CA004A248050667908CA004BF6",
    );
    let AdscTag::Report(r) = &tags[0] else {
        panic!()
    };
    assert_eq!(r.kind, ReportKind::WaypointChange);
    assert!(close(r.lat, 51.5665627, 1e-6));
    assert!(close(r.lon, 19.2610931, 1e-6));
    assert_eq!(r.alt_ft, 36000);

    let AdscTag::PredictedRoute {
        next_lat,
        next_lon,
        next_alt_ft,
        next_eta_s,
        next_next_lat,
        ..
    } = &tags[1]
    else {
        panic!("expected predicted route: {tags:?}")
    };
    assert!(close(*next_lat, 51.5190125, 1e-6));
    assert!(close(*next_lon, 19.0030861, 1e-6));
    assert_eq!(*next_alt_ft, 36000);
    assert_eq!(*next_eta_s, 74);
    assert!(close(*next_next_lat, 51.3298416, 1e-6));
}

#[test]
fn sp_lrh_low_altitude_approach() {
    let tags = adsc_tags(
        "/YQXE2YA.ADS.SP-LRH1424FD087806C0B527769F0D2500B877ED00B5401E2516707755C01340B768",
    );
    let AdscTag::Report(r) = &tags[0] else {
        panic!()
    };
    assert_eq!(r.kind, ReportKind::WaypointChange);
    assert!(close(r.lat, 52.0149422, 1e-6));
    assert!(close(r.lon, 21.0983849, 1e-6));
    assert_eq!(r.alt_ft, 2896);
    let AdscTag::PredictedRoute {
        next_next_alt_ft, ..
    } = &tags[1]
    else {
        panic!()
    };
    assert_eq!(*next_next_alt_ft, 308);
}

#[test]
fn h1_sublabel_then_plain_text() {
    let d = decode("H1", "#DFB/M1 ENGINE DATA", true);
    assert_eq!(d.sublabel.as_deref(), Some("DF"));
    assert_eq!(d.mfi.as_deref(), Some("M1"));
    assert!(d.app.is_none());
}

#[test]
fn q0_link_test_classified() {
    use super::qseries::QKind;
    let d = decode("Q0", "", true);
    let Some(AcarsApp::QSeries(q)) = d.app else {
        panic!("expected Q-series: {:?}", d.app)
    };
    assert_eq!(q.kind, QKind::LinkTest);
    assert_eq!(q.description, "ACARS Link Test");
}

#[test]
fn q2_eta_report_classified() {
    use super::qseries::QKind;
    let d = decode("Q2", "   2002  99/DS KJFK", true);
    let Some(AcarsApp::QSeries(q)) = d.app else {
        panic!("expected Q-series")
    };
    assert_eq!(q.kind, QKind::EtaReport);
    assert_eq!(q.description, "ETA Report");
}

#[test]
fn qf_off_destination_report_classified() {
    let d = decode("QF", "EWR2210ATL", true);
    let Some(AcarsApp::QSeries(q)) = d.app else {
        panic!("expected Q-series")
    };
    assert_eq!(q.description, "OFF Destination Report");
}

#[test]
fn qq_off_report_classified() {
    let d = decode("QQ", "KEWRKSWF20041942", true);
    let Some(AcarsApp::QSeries(q)) = d.app else {
        panic!("expected Q-series")
    };
    assert_eq!(q.description, "OFF Report");
}

#[test]
fn qq_off_report_oooi_fields() {
    let d = decode("QQ", "KEWRKSWF20041942", true);
    let o = d.oooi.expect("QQ carries OOOI");
    assert_eq!(o.depa.as_deref(), Some("KEWR"));
    assert_eq!(o.dsta.as_deref(), Some("KSWF"));
    assert_eq!(o.wloff.as_deref(), Some("2004"));
}

#[test]
fn qq_off_report_with_status_tail() {
    let d = decode(
        "QQ",
        "KEWRKDFW1829OS KDFW /FUL0306/MO 1816/APH 0000000",
        true,
    );
    let o = d.oooi.expect("QQ carries OOOI");
    assert_eq!(o.depa.as_deref(), Some("KEWR"));
    assert_eq!(o.dsta.as_deref(), Some("KDFW"));
    assert_eq!(o.wloff.as_deref(), Some("1829"));
}

#[test]
fn non_oooi_label_has_no_oooi() {
    let d = decode(
        "B6",
        "/BOMASAI.ADS.VT-ANB072501A070A988CA73248F0E5DC10200000F5EE1ABC000102B885E0A19F5",
        true,
    );
    assert!(d.oooi.is_none());
}

fn close3(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn label_20_position_report() {
    let d = decode(
        "20",
        "POSN38160W077075,,211733,360,OTT,212041,,N42,19689,40,544",
        true,
    );
    let p = d.position.expect("20/POS carries a position");
    assert!(close3(p.latitude, 38.160), "lat {}", p.latitude);
    assert!(close3(p.longitude, -77.075), "lon {}", p.longitude);
}

#[test]
fn h1_position_report_decimal_minutes() {
    let d = decode(
        "H1",
        "POSN43312W123174,EASON,215754,370,EBINY,220601,ELENN,M48,02216,185/TS215754,0921227A40",
        true,
    );
    let p = d.position.expect("H1 POS carries a position");
    assert!(close3(p.latitude, 43.52), "lat {}", p.latitude);
    assert!(close3(p.longitude, -123.29), "lon {}", p.longitude);
}

#[test]
fn h1_cfb_flr_realtime_failure() {
    use super::cfb::CfbKind;
    let d = decode("H1", "#CFBFLR/FR19121418400034433406TCAS (1SG)", true);
    assert_eq!(d.sublabel.as_deref(), Some("CF"));
    let Some(AcarsApp::Cfb(c)) = d.app else {
        panic!("expected CFB: {:?}", d.app)
    };
    assert_eq!(c.subtype, "FLR");
    assert_eq!(c.kind, CfbKind::RealtimeFailure);
    assert_eq!(c.description, "Realtime failure");
}

#[test]
fn h1_cfb_apm_report() {
    use super::cfb::CfbKind;
    let d = decode("H1", "#CFBAPM_REPORT_A_20200805180631S.CSV", true);
    let Some(AcarsApp::Cfb(c)) = d.app else {
        panic!("expected CFB")
    };
    assert_eq!(c.subtype, "APM_REPORT");
    assert_eq!(c.kind, CfbKind::ApmReport);
}

#[test]
fn h1_cfb_wrn_and_mpf_and_dotted() {
    use super::cfb::CfbKind;
    let d = decode("H1", "#CFBWRN/WN19121418390034000006NAV TCAS FAULT", true);
    let Some(AcarsApp::Cfb(c)) = d.app else {
        panic!()
    };
    assert_eq!(c.kind, CfbKind::Warning);

    let d = decode("H1", "#CFBMPF/               /AN.N660AW/FIAAL652", true);
    let Some(AcarsApp::Cfb(c)) = d.app else {
        panic!()
    };
    assert_eq!(c.kind, CfbKind::MaintenancePlanning);

    let d = decode(
        "H1",
        "#CFB.1/FLR/FR1602082254 27513406ADR1 X2,ADR3X,ADR2X",
        true,
    );
    let Some(AcarsApp::Cfb(c)) = d.app else {
        panic!()
    };
    assert_eq!(c.kind, CfbKind::FailureRecord);
}

#[test]
fn label_4j_position_report() {
    let d = decode(
        "4J",
        "POS/ID91459S,BANKR31,/DC03032024,142813/MR64,0/ET31539/PSN39277W077359,142800,240,N39300W077110,031430,N38560W077150,M28,27619,MT370/CG311,160,350/FB732/VR329071",
        true,
    );
    let p = d.position.expect("4J carries a position");
    assert!(close3(p.latitude, 39.462), "lat {}", p.latitude);
    assert!(close3(p.longitude, -77.598), "lon {}", p.longitude);
}

#[test]
fn label_4j_poswx_met_fields() {
    let text = "4J01 POSWX 0318/20 ETAD/ETAD .00318S\n\
        /POS N5043.5E01121.8/OVR 0817\n\
        /ALT 270/TFW 1342/TAS 490/SAT -032\n\
        /POS GOVEN /OVR 0835\n\
        /POS DILVI\n\
        /WND 334060/TRB /SKY DCC3";
    let d = decode("4J", text, true);
    let m = d.met.expect("4J POSWX carries met fields");
    assert_eq!(m.wind_dir_deg, Some(334));
    assert_eq!(m.wind_speed_kt, Some(60));
    assert_eq!(m.temperature_c, Some(-32));
    assert_eq!(m.true_airspeed_kt, Some(490));
    assert_eq!(m.altitude_ft, Some(27000));
}

#[test]
fn aklcdya_real_cpdlc_contact_with_unit_and_frequency() {
    let app = super::arinc622::parse(
        "/AKLCDYA.AT1.9M-MTB215B659D84995674293583561CB9906744E9AF40F9EB",
        false,
    )
    .expect("parses ARINC-622 envelope");
    let AcarsApp::Cpdlc {
        envelope, message, ..
    } = app
    else {
        panic!("expected CPDLC: {app:?}");
    };
    assert!(envelope.crc_ok, "real off-air message must pass CRC");
    let m = message.expect("CPDLC body decodes");
    assert_eq!(m.msg_id, 2);
    assert_eq!(m.timestamp.as_deref(), Some("22:54:22"));
    assert_eq!(m.element, "uM118PositionICAOunitnameFrequency");
    assert_eq!(m.args, vec!["LUNBI", "AUCKLAND control", "123.900 MHz"]);
    assert_eq!(m.text, "AT LUNBI CONTACT AUCKLAND control 123.900 MHz");
}

#[test]
fn h1_fpn_flight_plan_with_waypoints() {
    let d = decode(
        "H1",
        "FPN/FNUAL1187/RP:DA:KSFO:AA:KPHX:F:KAYEX,N36292W120569..LOSHN,N35509W120000..BOILE,N34253W118016..BLH,N33358W114457DDFB",
        true,
    );
    let Some(AcarsApp::FlightPlan(fp)) = d.app else {
        panic!("expected FlightPlan: {:?}", d.app);
    };
    assert_eq!(fp.route_status, "Route Planned");
    assert_eq!(fp.flight_number.as_deref(), Some("UAL1187"));
    assert_eq!(fp.origin.as_deref(), Some("KSFO"));
    assert_eq!(fp.destination.as_deref(), Some("KPHX"));
    assert_eq!(fp.checksum, "0xddfb");
    let names: Vec<&str> = fp.waypoints.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(names, ["KAYEX", "LOSHN", "BOILE", "BLH"]);
    let kayex = fp.waypoints[0].position.expect("KAYEX position");
    assert!(close3(kayex.latitude, 36.487), "lat {}", kayex.latitude);
    assert!(close3(kayex.longitude, -120.948), "lon {}", kayex.longitude);
}

#[test]
fn label_5z_txt_telex() {
    let d = decode("5Z", "/TXT\r\nDID U GET THE TIMES", true);
    let Some(AcarsApp::Airline5z(a)) = d.app else {
        panic!("expected 5Z: {:?}", d.app);
    };
    use super::airline5z::Airline5z;
    assert_eq!(
        a,
        Airline5z::Text {
            text: "DID U GET THE TIMES".into()
        }
    );
}

#[test]
fn label_5z_b3_departure_clearance() {
    let d = decode("5Z", "/B3 DCAORD 14 R27C", true);
    let Some(AcarsApp::Airline5z(a)) = d.app else {
        panic!("expected 5Z: {:?}", d.app);
    };
    use super::airline5z::Airline5z;
    let Airline5z::Typed {
        message_type,
        origin,
        destination,
        arrival_runway,
        day,
        ..
    } = a
    else {
        panic!("expected typed");
    };
    assert_eq!(message_type, "B3");
    assert_eq!(origin.as_deref(), Some("DCA"));
    assert_eq!(destination.as_deref(), Some("ORD"));
    assert_eq!(arrival_runway.as_deref(), Some("27C"));
    assert_eq!(day, Some(14));
}
