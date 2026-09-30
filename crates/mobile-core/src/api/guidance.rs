use crate::{
    guidance::nav_url,
    records::{LatLon, NavApp},
};

#[uniffi::export]
pub fn nav_handoff_uri(target: LatLon, app: NavApp) -> String {
    nav_url::handoff(target, app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_export_uses_the_one_formatter() {
        let target = LatLon {
            lat: 1.5,
            lon: -2.25,
        };
        for app in [NavApp::GoogleMaps, NavApp::Chooser, NavApp::Car] {
            assert_eq!(nav_handoff_uri(target, app), nav_url::handoff(target, app));
        }
    }
}
