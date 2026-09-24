use serde_json::{Value, json};

const REGION_TOLERANCE_DEG: f64 = 35.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OceanRegion {
    AorW,
    AorE,
    Ior,
    Por,
}

impl OceanRegion {
    const ALL: [OceanRegion; 4] = [
        OceanRegion::AorW,
        OceanRegion::AorE,
        OceanRegion::Ior,
        OceanRegion::Por,
    ];

    fn code(self) -> &'static str {
        match self {
            OceanRegion::AorW => "AOR-W",
            OceanRegion::AorE => "AOR-E",
            OceanRegion::Ior => "IOR",
            OceanRegion::Por => "POR",
        }
    }

    fn center_deg(self) -> f64 {
        match self {
            OceanRegion::AorW => -54.0,
            OceanRegion::AorE => -15.5,
            OceanRegion::Ior => 64.0,
            OceanRegion::Por => 178.0,
        }
    }

    pub(super) fn classify(longitude_signed: f64) -> Option<OceanRegion> {
        let mut best: Option<(OceanRegion, f64)> = None;
        for region in Self::ALL {
            let mut distance = (longitude_signed - region.center_deg()).abs();
            if distance > 180.0 {
                distance = 360.0 - distance;
            }
            if distance <= REGION_TOLERANCE_DEG
                && best.is_none_or(|(_, best_distance)| distance < best_distance)
            {
                best = Some((region, distance));
            }
        }
        best.map(|(region, _)| region)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ResolvedSatellite {
    pub satellite_id: u8,
    pub longitude_deg: f64,
    pub longitude_dir: String,
    pub region: Option<OceanRegion>,
}

impl ResolvedSatellite {
    fn longitude_signed(&self) -> f64 {
        if self.longitude_dir == "W" {
            -self.longitude_deg
        } else {
            self.longitude_deg
        }
    }

    fn to_json(&self) -> Value {
        let mut value = json!({
            "satellite_id": self.satellite_id,
            "longitude_deg": self.longitude_deg,
            "longitude_dir": self.longitude_dir,
        });
        if let Some(region) = self.region {
            value["region"] = json!(region.code());
        }
        value
    }
}

#[derive(Debug, Default)]
pub(super) struct SatelliteResolver {
    satellite: Option<ResolvedSatellite>,
    spot_beam: Option<bool>,
    beam_support_seen: bool,
    ges_id: Option<u8>,
}

impl SatelliteResolver {
    pub(super) fn observe(&mut self, su: &Value) {
        match su["su_type"].as_str() {
            Some("satellite-id") => self.observe_satellite(su),
            Some("ges-beam-support") => self.beam_support_seen = true,
            Some("smc-channels") => {
                if let Some(ges_id) = su["ges_id"].as_u64() {
                    self.ges_id = Some(ges_id as u8);
                }
            }
            _ => {}
        }
    }

    fn observe_satellite(&mut self, su: &Value) {
        let mut satellite = ResolvedSatellite {
            satellite_id: su["satellite_id"].as_u64().unwrap_or(0) as u8,
            longitude_deg: su["longitude_deg"].as_f64().unwrap_or(0.0),
            longitude_dir: su["longitude_dir"].as_str().unwrap_or("E").to_owned(),
            region: None,
        };
        satellite.region = OceanRegion::classify(satellite.longitude_signed());
        self.satellite = Some(satellite);
        if let Some(spot) = su["psmc1_spotbeam"].as_bool() {
            self.spot_beam = Some(spot);
        }
    }

    pub(super) fn details(&self) -> Option<Value> {
        let satellite = self.satellite.as_ref()?;
        let mut value = json!({ "resolved_satellite": satellite.to_json() });
        value["beam"] = json!(match self.spot_beam {
            Some(true) => "spot",
            Some(false) => "global",
            None => "unknown",
        });
        if self.beam_support_seen {
            value["ges_beam_support"] = json!(true);
        }
        if let Some(ges_id) = self.ges_id {
            value["resolved_ges_id"] = json!(ges_id);
        }
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inmarsat_aero::su::{parse_p_su, su_with_crc};

    fn satellite_id_su(high: u8, low: u8, longitude: u8, spot: bool) -> Vec<u8> {
        let mut su10 = vec![0u8; 10];
        su10[0] = 0x0C;
        su10[2] = (10u8 << 2) | high;
        su10[3] = low << 4;
        su10[5] = longitude;
        su10[6] = if spot { 0x80 } else { 0x00 } | 0x02;
        su_with_crc(su10)
    }

    #[test]
    fn ocean_region_classifies_classic_slots() {
        assert_eq!(OceanRegion::classify(-54.0), Some(OceanRegion::AorW));
        assert_eq!(OceanRegion::classify(-15.5), Some(OceanRegion::AorE));
        assert_eq!(OceanRegion::classify(64.0), Some(OceanRegion::Ior));
        assert_eq!(OceanRegion::classify(178.0), Some(OceanRegion::Por));
        assert_eq!(OceanRegion::classify(-40.0), Some(OceanRegion::AorW));
        assert_eq!(OceanRegion::classify(98.0), Some(OceanRegion::Ior));
        assert_eq!(OceanRegion::classify(-179.0), Some(OceanRegion::Por));
        assert_eq!(OceanRegion::classify(115.0), None);
        assert_eq!(OceanRegion::AorW.code(), "AOR-W");
    }

    #[test]
    fn resolver_learns_satellite_from_0x0c_broadcast() {
        let mut su10 = vec![0u8; 10];
        su10[0] = 0x0C;
        su10[2] = 0x29;
        su10[3] = 0x40;
        su10[5] = 200;
        su10[6] = 0x01;
        su10[7] = 0x23;
        su10[8] = 0x84;
        su10[9] = 0x56;
        let parsed = parse_p_su(&su_with_crc(su10)).expect("0x0C parses");
        let mut resolver = SatelliteResolver::default();
        resolver.observe(&json!({ "su_type": "log-control" }));
        assert!(resolver.details().is_none());
        resolver.observe(&parsed);
        let satellite = resolver.satellite.clone().expect("resolved");
        assert_eq!(satellite.satellite_id, 20);
        assert_eq!(satellite.longitude_deg, 60.0);
        assert_eq!(satellite.longitude_dir, "W");
        assert_eq!(satellite.region, Some(OceanRegion::AorW));
        let details = resolver.details().expect("details");
        assert_eq!(details["resolved_satellite"]["satellite_id"], 20);
        assert_eq!(details["resolved_satellite"]["region"], "AOR-W");
        assert_eq!(details["beam"], "global");
    }

    #[test]
    fn resolver_reconfigures_and_tracks_beam_support() {
        let mut resolver = SatelliteResolver::default();
        let first = parse_p_su(&satellite_id_su(0, 5, 100, false)).expect("parses");
        resolver.observe(&first);
        let details = resolver.details().expect("details");
        assert_eq!(details["resolved_satellite"]["satellite_id"], 5);
        assert_eq!(details["resolved_satellite"]["longitude_dir"], "E");
        assert_eq!(details["resolved_satellite"]["region"], "POR");
        assert_eq!(details["beam"], "global");
        let mut beam_support = vec![0u8; 10];
        beam_support[0] = 0x07;
        resolver.observe(&parse_p_su(&su_with_crc(beam_support)).expect("parses"));
        let second = parse_p_su(&satellite_id_su(0, 6, 40, true)).expect("parses");
        resolver.observe(&second);
        let details = resolver.details().expect("details");
        assert_eq!(details["resolved_satellite"]["satellite_id"], 6);
        assert_eq!(details["resolved_satellite"]["region"], "IOR");
        assert_eq!(details["beam"], "spot");
        assert_eq!(details["ges_beam_support"], true);
    }
}
