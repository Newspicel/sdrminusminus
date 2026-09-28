use crate::records::{LatLon, NavApp};

pub(crate) fn handoff(target: LatLon, app: NavApp) -> String {
    let at = format!("{:.6},{:.6}", target.lat + 0.0, target.lon + 0.0);
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
}
