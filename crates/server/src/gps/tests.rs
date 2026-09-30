use std::time::Instant;

use sdrmm_engine::Engine;
use sdrmm_wire::{
    GpsNode, HeadingSource, PatchNode, Position, WorkspaceSnapshot,
    phone::{PHONE_NOT_PAIRED, PHONE_OFFLINE, PHONE_SILENT},
};
use tokio_serial::UsbPortInfo;

use super::{
    gpsd::{GpsdOutcome, GpsdState},
    nmea::nmea_coordinate,
    *,
};

mod pose;

#[test]
fn parses_checked_gga_and_rmc_sentences() {
    let mut state = NmeaState::default();
    let gga = state
        .parse(
            "$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47",
            Instant::now(),
        )
        .expect("GGA fix");
    assert!((gga.latitude - 48.1173).abs() < 0.000_001);
    assert!((gga.longitude - 11.516_666_7).abs() < 0.000_001);
    assert_eq!(gga.altitude_m, Some(545.4));

    let rmc = state
        .parse(
            "$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A",
            Instant::now(),
        )
        .expect("RMC fix");
    assert!((rmc.speed_mps.expect("speed") - 11.523_545_6).abs() < 0.000_001);
    assert_eq!(rmc.track_deg, Some(84.4));
    assert_eq!(rmc.altitude_m, Some(545.4));
}

#[test]
fn rejects_a_bad_checksum_and_invalid_coordinates() {
    let mut state = NmeaState::default();
    assert!(
        state
            .parse(
                "$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*00",
                Instant::now(),
            )
            .is_none()
    );
    assert!(nmea_coordinate("1260.0", "N", false).is_none());
    assert!(
        state
            .parse(
                "$GPGGA,123519,9100.000,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*4F",
                Instant::now(),
            )
            .is_none()
    );
}

#[tokio::test]
async fn bounded_lines_reject_oversized_input_before_allocating_it() {
    let mut normal = BufReader::new(&b"$GPGGA,test*00\r\n"[..]);
    assert_eq!(
        read_bounded_line(&mut normal, 64).await.unwrap().as_deref(),
        Some("$GPGGA,test*00")
    );

    let mut oversized = BufReader::new(&b"123456789\n"[..]);
    let error = read_bounded_line(&mut oversized, 8).await.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn serial_discovery_preserves_usb_identity() {
    let device = nmea_device_info(SerialPortInfo {
        port_name: "/dev/ttyACM0".to_owned(),
        port_type: SerialPortType::UsbPort(UsbPortInfo {
            vid: 0x1546,
            pid: 0x01a7,
            serial_number: Some("GPS-1".to_owned()),
            manufacturer: Some("u-blox".to_owned()),
            product: Some("GNSS receiver".to_owned()),
        }),
    });
    assert_eq!(device.path, "/dev/ttyACM0");
    assert_eq!(device.product.as_deref(), Some("GNSS receiver"));
    assert_eq!(device.serial.as_deref(), Some("GPS-1"));
    assert_eq!(
        (device.usb_vid, device.usb_pid),
        (Some(0x1546), Some(0x01a7))
    );
}

#[test]
fn only_openable_ports_are_offered_and_receivers_rank_first() {
    for pseudo in [
        "/dev/cu.Bluetooth-Incoming-Port",
        "/dev/cu.debug-console",
        "/dev/tty.usbmodem11401",
    ] {
        assert!(
            !is_openable_port(&SerialPortInfo {
                port_name: pseudo.to_owned(),
                port_type: SerialPortType::Unknown,
            }),
            "offered {pseudo}"
        );
    }
    assert!(is_openable_port(&SerialPortInfo {
        port_name: "/dev/cu.usbmodem11401".to_owned(),
        port_type: SerialPortType::Unknown,
    }));
    assert!(is_openable_port(&SerialPortInfo {
        port_name: "/dev/ttyUSB0".to_owned(),
        port_type: SerialPortType::Unknown,
    }));

    let port = |product: Option<&str>, vid: Option<u16>| NmeaDeviceInfo {
        path: "/dev/ttyUSB0".to_owned(),
        product: product.map(ToOwned::to_owned),
        manufacturer: None,
        serial: None,
        usb_vid: vid,
        usb_pid: None,
    };
    assert_eq!(receiver_rank(&port(Some("u-blox GNSS receiver"), None)), 0);
    assert_eq!(receiver_rank(&port(None, Some(0x1546))), 0);
    assert_eq!(
        receiver_rank(&port(Some("STM32 Virtual ComPort"), Some(1))),
        1
    );
    assert_eq!(
        receiver_rank(&port(Some("FT232R USB UART"), Some(0x0403))),
        1
    );
    assert_eq!(receiver_rank(&port(None, None)), 2);
}

#[test]
fn the_picker_is_offered_receivers_alone_unless_there_are_none() {
    let named = |path: &str, product: Option<&str>| NmeaDeviceInfo {
        path: path.to_owned(),
        product: product.map(ToOwned::to_owned),
        manufacturer: None,
        serial: None,
        usb_vid: Some(0x1234),
        usb_pid: None,
    };
    let board = named("/dev/cu.usbmodem9A4", Some("STM32 Virtual ComPort"));
    let bare = named("/dev/cu.usbmodem7B2", None);
    let puck = named("/dev/cu.usbmodem11401", Some("u-blox GNSS receiver"));

    assert_eq!(
        offered_ports(vec![board.clone(), bare.clone(), puck.clone()])
            .iter()
            .map(|port| port.path.as_str())
            .collect::<Vec<_>>(),
        ["/dev/cu.usbmodem11401"]
    );

    assert_eq!(offered_ports(vec![board, bare]).len(), 2);
    assert!(offered_ports(Vec::new()).is_empty());
}

#[test]
fn a_screen_that_answers_on_a_serial_port_is_never_offered_as_a_receiver() {
    let monitor = NmeaDeviceInfo {
        path: "/dev/cu.usbmodem306NTQD8F3802".to_owned(),
        product: Some("LG Monitor Controls".to_owned()),
        manufacturer: Some("LG Electronics Inc.".to_owned()),
        serial: Some("306NTQD8F380".to_owned()),
        usb_vid: Some(0x043e),
        usb_pid: Some(0x9a39),
    };
    let keyboard = NmeaDeviceInfo {
        path: "/dev/cu.usbmodem4001".to_owned(),
        product: Some("USB Keyboard".to_owned()),
        manufacturer: None,
        serial: None,
        usb_vid: Some(0x1234),
        usb_pid: None,
    };
    assert!(offered_ports(vec![monitor, keyboard]).is_empty());

    let mapping = NmeaDeviceInfo {
        path: "/dev/cu.usbmodem11401".to_owned(),
        product: Some("Garmin GPSMAP Display".to_owned()),
        manufacturer: None,
        serial: None,
        usb_vid: Some(0x1234),
        usb_pid: None,
    };
    assert_eq!(offered_ports(vec![mapping]).len(), 1);
}

#[test]
fn a_position_typed_in_stands_in_for_a_receiver_that_never_moves() {
    let store = crate::Store::open(None).expect("store");
    let mut snapshot = WorkspaceSnapshot::empty();
    snapshot.graph.nodes.push(PatchNode {
        id: "roof".to_owned(),
        body: NodeBody::Gps(GpsNode {
            source: Some(PositionSource::Fixed {
                lat: 51.5,
                lon: 7.0,
                altitude_m: Some(120.0),
            }),
        }),
        position: Position { x: 0.0, y: 0.0 },
        size: None,
        label: None,
    });
    let workspace_id = store
        .create_workspace("roof", &snapshot)
        .expect("workspace");
    store.activate_workspace(workspace_id).expect("activate");
    let app = crate::AppState::new(Engine::new(None), Arc::new(store));
    app.gps.reconcile(&app);
    let fix = app.gps.fix("roof").expect("a typed-in place is a fix");
    assert!((fix.latitude - 51.5).abs() < 1e-9);
    assert!((fix.longitude - 7.0).abs() < 1e-9);
    assert_eq!(fix.altitude_m, Some(120.0));
    assert!(
        fix.track_deg.is_none(),
        "a receiver that never moves has no course"
    );
}

fn sentence(body: &str) -> String {
    let sum = body.bytes().fold(0_u8, |sum, byte| sum ^ byte);
    format!("${body}*{sum:02X}")
}

const GGA: &str = "GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,";

fn heading_of(fix: &PositionFix) -> (Option<f64>, Option<HeadingSource>) {
    (fix.attitude.heading_deg, fix.attitude.heading_source)
}

#[test]
fn nmea_hdt_and_ths_fill_the_heading() {
    let mut state = NmeaState::default();
    let now = Instant::now();
    assert!(state.parse(&sentence("GPHDT,123.4,T"), now).is_none());
    state.parse(&sentence(GGA), now).expect("fix");
    let hdt = state.parse(&sentence("GPHDT,123.4,T"), now).expect("fix");
    assert_eq!(heading_of(&hdt), (Some(123.4), Some(HeadingSource::Gnss)));
    let ths = state.parse(&sentence("GNTHS,77.0,A"), now).expect("fix");
    assert_eq!(heading_of(&ths), (Some(77.0), Some(HeadingSource::Gnss)));
    let invalid = state.parse(&sentence("GNTHS,77.0,V"), now).expect("fix");
    assert_eq!(heading_of(&invalid), (None, None));
    let compass = state.parse(&sentence("HCHDT,360.0,T"), now).expect("fix");
    assert_eq!(
        heading_of(&compass),
        (Some(0.0), Some(HeadingSource::Compass))
    );
    let gyro = state.parse(&sentence("HEHDT,10.5,T"), now).expect("fix");
    assert_eq!(heading_of(&gyro), (Some(10.5), Some(HeadingSource::Sensor)));
    assert!(state.parse(&sentence("HEHDT,11.0,M"), now).is_none());
    let cleared = state.parse(&sentence("GPHDT,,T"), now).expect("fix");
    assert_eq!(heading_of(&cleared), (None, None));
}

#[test]
fn an_old_nmea_heading_is_dropped() {
    let mut state = NmeaState::default();
    let start = Instant::now();
    state.parse(&sentence(GGA), start).expect("fix");
    state.parse(&sentence("GPHDT,200.0,T"), start).expect("fix");
    let fresh = state
        .parse(&sentence(GGA), start + Duration::from_secs(1))
        .expect("fix");
    assert_eq!(fresh.attitude.heading_deg, Some(200.0));
    let stale = state
        .parse(&sentence(GGA), start + Duration::from_secs(3))
        .expect("fix");
    assert_eq!(stale.attitude.heading_deg, None);
}

#[test]
fn gpsd_att_adds_heading_pitch_and_roll() {
    let mut gpsd = GpsdState::default();
    let start = Instant::now();
    let att = r#"{"class":"ATT","heading":90.0,"pitch":2.0,"roll":-1.0,"mag_st":"N"}"#;
    assert_eq!(gpsd.line(att, start), GpsdOutcome::Nothing);
    let tpv = r#"{"class":"TPV","mode":3,"lat":48.1,"lon":11.5,"time":"2026-09-28T12:00:00Z"}"#;
    let GpsdOutcome::Fix(fix) = gpsd.line(tpv, start) else {
        panic!("a 3D fix is a fix");
    };
    assert_eq!(heading_of(&fix), (Some(90.0), Some(HeadingSource::Compass)));
    assert_eq!(
        (fix.attitude.pitch_deg, fix.attitude.roll_deg),
        (Some(2.0), Some(-1.0))
    );
    let turned = r#"{"class":"ATT","heading":-80.0}"#;
    let GpsdOutcome::Fix(again) = gpsd.line(turned, start) else {
        panic!("an attitude republishes the last fix");
    };
    assert_eq!(
        heading_of(&again),
        (Some(280.0), Some(HeadingSource::Sensor))
    );
    assert_eq!(again.latitude, 48.1);
    let GpsdOutcome::Fix(stale) = gpsd.line(tpv, start + Duration::from_secs(3)) else {
        panic!("a 3D fix is a fix");
    };
    assert_eq!(heading_of(&stale), (None, None));
    let bad = r#"{"class":"ATT","heading":10.0,"pitch":95.0}"#;
    assert_eq!(gpsd.line(bad, start), GpsdOutcome::Nothing);
    assert_eq!(
        gpsd.line(r#"{"class":"TPV","mode":1}"#, start),
        GpsdOutcome::NoFix
    );
    assert_eq!(gpsd.line(turned, start), GpsdOutcome::Nothing);
    assert_eq!(gpsd.line("not json", start), GpsdOutcome::Nothing);
}
