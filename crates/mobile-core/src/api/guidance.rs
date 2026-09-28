use crate::records::{LatLon, NavApp};

#[uniffi::export]
pub fn nav_handoff_uri(target: LatLon, app: NavApp) -> String {
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
            nav_handoff_uri(TARGET, NavApp::GoogleMaps),
            "google.navigation:q=52.520008,-13.404954&mode=d"
        );
        assert_eq!(
            nav_handoff_uri(TARGET, NavApp::Chooser),
            "geo:52.520008,-13.404954?q=52.520008,-13.404954(Target)"
        );
        assert_eq!(
            nav_handoff_uri(TARGET, NavApp::Car),
            "geo:52.520008,-13.404954"
        );
    }

    #[test]
    fn negative_zero_prints_as_zero() {
        assert_eq!(
            nav_handoff_uri(
                LatLon {
                    lat: -0.0,
                    lon: 0.0
                },
                NavApp::Car
            ),
            "geo:0.000000,0.000000"
        );
    }
}
