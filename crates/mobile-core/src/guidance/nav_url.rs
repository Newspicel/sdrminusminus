use crate::records::{LatLon, NavApp};

const MICRO_DEGREES: f64 = 1e6;

fn micro(deg: f64) -> f64 {
    (deg * MICRO_DEGREES).round() / MICRO_DEGREES + 0.0
}

pub(crate) fn handoff(target: LatLon, app: NavApp) -> String {
    let at = format!("{:.6},{:.6}", micro(target.lat), micro(target.lon));
    match app {
        NavApp::GoogleMaps => format!("google.navigation:q={at}&mode=d"),
        NavApp::Chooser => format!("geo:{at}?q={at}(Target)"),
        NavApp::Car => format!("geo:{at}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: LatLon = LatLon {
        lat: 52.520_008_4,
        lon: -13.404_954,
    };

    #[test]
    fn nav_handoff_uris_use_six_decimals() {
        assert_eq!(
            handoff(TARGET, NavApp::GoogleMaps),
            "google.navigation:q=52.520008,-13.404954&mode=d"
        );
        assert_eq!(
            handoff(TARGET, NavApp::Chooser),
            "geo:52.520008,-13.404954?q=52.520008,-13.404954(Target)"
        );
        assert_eq!(handoff(TARGET, NavApp::Car), "geo:52.520008,-13.404954");
        let sydney = LatLon {
            lat: -33.8688,
            lon: 151.2093,
        };
        assert_eq!(handoff(sydney, NavApp::Car), "geo:-33.868800,151.209300");
    }

    #[test]
    fn negative_zero_prints_as_zero() {
        let zero = LatLon {
            lat: -0.0,
            lon: 0.0,
        };
        assert_eq!(handoff(zero, NavApp::Car), "geo:0.000000,0.000000");
    }

    #[test]
    fn a_tiny_negative_rounds_to_an_unsigned_zero() {
        let equator = LatLon {
            lat: -0.000_000_4,
            lon: -0.000_000_1,
        };
        assert_eq!(handoff(equator, NavApp::Car), "geo:0.000000,0.000000");
        let west = LatLon {
            lat: 51.477_928_4,
            lon: -0.000_000_6,
        };
        assert_eq!(handoff(west, NavApp::Car), "geo:51.477928,-0.000001");
    }
}
