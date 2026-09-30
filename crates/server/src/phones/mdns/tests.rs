use std::time::{Duration, Instant};

use mdns_sd::ServiceEvent;
use sdrmm_wire::phone::key_check;

use super::*;

const PIN: &str = "3fa9c2e011b790de5a4c0123456789abcdef0123456789abcdef0123456789ab";

fn endpoint() -> PhoneEndpoint {
    PhoneEndpoint {
        port: 8443,
        hosts: vec!["192.168.1.20:8443".to_owned()],
        pin: PIN.to_owned(),
        key_check: key_check(PIN),
        dedicated: true,
    }
}

#[test]
fn advert_carries_protocol_id_pin_and_name() {
    let advert = advert("0123456789abcdef0123456789abcdef", "shack", &endpoint());
    assert_eq!(advert.instance, "SDR-- shack");
    assert_eq!(advert.port, 8443);
    assert_eq!(advert.host, format!("{}.local.", crate::net::host_label()));
    assert_eq!(
        advert.txt,
        [
            ("v", "1".to_owned()),
            ("p", API_PROTOCOL.to_string()),
            ("id", "0123456789abcdef0123456789abcdef".to_owned()),
            ("fp", PIN.to_owned()),
            ("n", "shack".to_owned()),
        ]
    );
}

#[test]
fn advert_instance_fits_one_dns_label() {
    let long = advert("id", &"ü".repeat(40), &endpoint());
    assert!(long.instance.len() <= MAX_INSTANCE_BYTES);
    assert!(long.instance.starts_with(INSTANCE_PREFIX));
    assert!(long.instance.ends_with('ü'));
    let spaced = format!("{}  x", "a".repeat(55));
    assert!(!advert("id", &spaced, &endpoint()).instance.ends_with(' '));
}

#[test]
#[ignore = "announces on the local network"]
fn announces_on_the_lan() {
    let id = format!("{:032x}", std::process::id());
    let name = format!("test-{}", std::process::id());
    let advert = advert(&id, &name, &endpoint());
    let advertiser = Advertiser::start(&advert, |_| {}).expect("advertiser");
    let browser = ServiceDaemon::new().expect("browser");
    let found = browser.browse(MDNS_SERVICE_TYPE).expect("browse");
    let deadline = Instant::now() + Duration::from_secs(10);
    let resolved = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match found.recv_timeout(left) {
            Ok(ServiceEvent::ServiceResolved(service))
                if service.get_property_val_str("id") == Some(id.as_str()) =>
            {
                break service;
            }
            Ok(_) => {}
            Err(error) => panic!("no advert for {name}: {error}"),
        }
    };
    assert_eq!(resolved.get_port(), 8443);
    assert_eq!(resolved.get_property_val_str("fp"), Some(PIN));
    assert_eq!(resolved.get_property_val_str("n"), Some(name.as_str()));
    assert!(resolved.get_fullname().starts_with(&advert.instance));
    advertiser.shutdown();
    close(&browser);
}
