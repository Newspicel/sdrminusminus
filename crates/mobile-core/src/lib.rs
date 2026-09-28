uniffi::setup_scaffolding!();

mod api;
mod colormap;
mod error;
mod events;
mod guidance;
mod link;
mod logging;
mod missions;
mod notices;
mod pairing;
mod pose;
mod records;
mod runtime;
mod ticks;
mod tls;
mod vault;

use sdrmm_wire::geo;

pub use api::{MobileCore, nav_handoff_uri};
pub use error::{CoreError, VaultError};
pub use events::CoreEvent;
pub use logging::{LogLevel, LogListener};
pub use missions::views::{
    DfOverlay, DfState, DfView, EstimateView, GuidanceKind, GuidanceView, HeatBand, HuntView,
    Mission, MissionCommand, MissionControl, MissionKind, MissionsView, NavPoint, RadarTrack,
    RadarView, Ray, RetargetNotice, RetargetReason, RgbaImage, Station, SurveyPoint, SurveyView,
    SweepPhase, SweepView, TargetMode, Trend, WorkspaceRef,
};
pub use records::{
    AlignHint, AlignState, CoreAbout, CoreConfig, DiscoveredServer, HeadingMode, HeadingSample,
    HeadingSourceKind, LatLon, LicenseEntry, LinkState, LocationSample, MagAccuracy, MotionFrame,
    MotionSample, Mount, NavApp, Notice, NoticeLevel, PairOffer, Platform, PoseSettings, PoseView,
    RefusalKind, SavedServer,
};
pub use vault::SecretVault;

#[uniffi::export]
pub fn geo_distance_m(from: LatLon, to: LatLon) -> f64 {
    geo::distance_m(from.into(), to.into())
}

#[uniffi::export]
pub fn geo_bearing_deg(from: LatLon, to: LatLon) -> f64 {
    geo::bearing_deg(from.into(), to.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geo_exports_match_wire_geo() {
        let berlin = LatLon {
            lat: 52.52,
            lon: 13.405,
        };
        let paris = LatLon {
            lat: 48.8566,
            lon: 2.3522,
        };
        assert_eq!(
            geo_distance_m(berlin, paris),
            geo::distance_m(berlin.into(), paris.into())
        );
        assert!((geo_distance_m(berlin, paris) - 877_500.0).abs() < 1_000.0);
        assert_eq!(
            geo_bearing_deg(berlin, paris),
            geo::bearing_deg(berlin.into(), paris.into())
        );
        let east = LatLon { lat: 0.0, lon: 1.0 };
        let origin = LatLon { lat: 0.0, lon: 0.0 };
        assert!((geo_bearing_deg(origin, east) - 90.0).abs() < 1e-9);
        assert_eq!(geo_bearing_deg(origin, LatLon { lat: 1.0, lon: 0.0 }), 0.0);
    }
}
