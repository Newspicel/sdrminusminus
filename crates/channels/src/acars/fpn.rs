use crate::acars::position::{Position, decode_decimal_minutes};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Waypoint {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FlightPlan {
    pub route_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flight_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub company_route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub departure_runway: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub departure_procedure: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arrival_procedure: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approach_procedure: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub waypoints: Vec<Waypoint>,
    pub checksum: String,
}

pub fn parse(text: &str) -> Option<FlightPlan> {
    let cleaned: String = text.chars().filter(|&c| c != '\r' && c != '\n').collect();
    let body = cleaned.strip_prefix("FPN/")?;
    if body.len() < 4 {
        return None;
    }
    let (record, csum) = body.split_at_checked(body.len() - 4)?;
    let record = record.strip_suffix(':').unwrap_or(record);

    let mut fields = record.split(':');
    let header = fields.next()?;

    let mut flight_number = None;
    let mut serial_number = None;
    let mut route_status = None;
    for part in header.split('/') {
        if let Some(fn_) = part.strip_prefix("FN") {
            flight_number = Some(fn_.to_string());
        } else if let Some(sn) = part.strip_prefix("SN") {
            serial_number = Some(sn.split(',').next().unwrap_or(sn).to_string());
        } else if part.starts_with("TS") {
        } else if part == "RI" {
            route_status = Some("Route Inactive");
        } else if part == "RP" {
            route_status = Some("Route Planned");
        }
    }
    let route_status = route_status?.to_string();

    let mut fp = FlightPlan {
        route_status,
        flight_number,
        serial_number,
        origin: None,
        destination: None,
        company_route: None,
        departure_runway: None,
        departure_procedure: None,
        arrival_procedure: None,
        approach_procedure: None,
        waypoints: Vec::new(),
        checksum: format!("0x{}", csum.to_lowercase()),
    };

    let rest: Vec<&str> = fields.collect();
    let mut i = 0;
    while i + 1 < rest.len() {
        let key = rest[i];
        let val = rest[i + 1];
        match key {
            "DA" => fp.origin = Some(val.to_string()),
            "AA" => fp.destination = Some(val.to_string()),
            "CR" => fp.company_route = Some(val.to_string()),
            "R" => fp.departure_runway = Some(val.to_string()),
            "D" => fp.departure_procedure = Some(val.to_string()),
            "A" => fp.arrival_procedure = Some(val.to_string()),
            "AP" => fp.approach_procedure = Some(val.to_string()),
            "F" => fp.waypoints.extend(parse_route(val)),
            _ => {}
        }
        i += 2;
    }

    Some(fp)
}

fn parse_route(route: &str) -> Vec<Waypoint> {
    let mut out = Vec::new();
    for token in route.split('.') {
        if token.is_empty() {
            continue;
        }
        let (name, position) = if let Some((n, coord)) = token.split_once(',') {
            (n.to_string(), decode_decimal_minutes(coord))
        } else {
            (token.to_string(), decode_decimal_minutes(token))
        };
        out.push(Waypoint { name, position });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn landing_route_inactive() {
        let fp = parse(
            "FPN/RI:DA:KEWR:AA:KDFW:CR:EWRDFW01(17L)..SAAME.J6.HVQ.Q68.LITTR..MEEOW..FEWWW:A:SEEVR4.FEWWW:F:VECTOR..DISCO..RIVET:AP:ILS 17L.RIVET:F:TACKEC8B5",
        )
        .expect("FPN parses");
        assert_eq!(fp.route_status, "Route Inactive");
        assert_eq!(fp.origin.as_deref(), Some("KEWR"));
        assert_eq!(fp.destination.as_deref(), Some("KDFW"));
        assert_eq!(
            fp.company_route.as_deref(),
            Some("EWRDFW01(17L)..SAAME.J6.HVQ.Q68.LITTR..MEEOW..FEWWW")
        );
        assert_eq!(fp.arrival_procedure.as_deref(), Some("SEEVR4.FEWWW"));
        assert_eq!(fp.approach_procedure.as_deref(), Some("ILS 17L.RIVET"));
        assert_eq!(fp.checksum, "0xc8b5");
    }

    #[test]
    fn full_flight_with_flight_number_and_coords() {
        let fp = parse(
            "FPN/FNAAL1956/RP:DA:KPHL:AA:KPHX:CR:PHLPHX61:R:27L(26O):D:PHL3:A:EAGUL6.ZUN:AP:ILS26..AIR,N40010W080490.J110.BOWRR..VLA,N39056W089097..STL,N38516W090289..GIBSN,N38430W092244..TYGER,N38410W094050..GCK,N37551W100435..DIXAN,N36169W105573..ZUN,N34579W109093293B",
        )
        .expect("FPN parses");
        assert_eq!(fp.route_status, "Route Planned");
        assert_eq!(fp.flight_number.as_deref(), Some("AAL1956"));
        assert_eq!(fp.origin.as_deref(), Some("KPHL"));
        assert_eq!(fp.destination.as_deref(), Some("KPHX"));
        assert_eq!(fp.company_route.as_deref(), Some("PHLPHX61"));
        assert_eq!(fp.departure_runway.as_deref(), Some("27L(26O)"));
        assert_eq!(fp.departure_procedure.as_deref(), Some("PHL3"));
        assert_eq!(fp.arrival_procedure.as_deref(), Some("EAGUL6.ZUN"));
        assert_eq!(fp.checksum, "0x293b");
        assert!(
            fp.approach_procedure
                .as_deref()
                .unwrap()
                .contains("AIR,N40010W080490")
        );
    }

    #[test]
    fn in_flight_waypoints_decode_coordinates() {
        let fp = parse(
            "FPN/FNUAL1187/RP:DA:KSFO:AA:KPHX:F:KAYEX,N36292W120569..LOSHN,N35509W120000..BOILE,N34253W118016..BLH,N33358W114457DDFB",
        )
        .expect("FPN parses");
        assert_eq!(fp.flight_number.as_deref(), Some("UAL1187"));
        assert_eq!(fp.origin.as_deref(), Some("KSFO"));
        assert_eq!(fp.destination.as_deref(), Some("KPHX"));
        assert_eq!(fp.checksum, "0xddfb");

        let names: Vec<&str> = fp.waypoints.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, ["KAYEX", "LOSHN", "BOILE", "BLH"]);
        let kayex = fp.waypoints[0].position.expect("KAYEX has a position");
        assert!(close(kayex.latitude, 36.487), "lat {}", kayex.latitude);
        assert!(close(kayex.longitude, -120.948), "lon {}", kayex.longitude);
        let blh = fp.waypoints[3].position.expect("BLH has a position");
        assert!(close(blh.latitude, 33.597), "lat {}", blh.latitude);
        assert!(close(blh.longitude, -114.762), "lon {}", blh.longitude);
    }

    #[test]
    fn serial_number_and_route_inactive_with_newlines() {
        let fp = parse(
            "FPN/SN2125/FNQFA780/RI:DA:YPPH:CR:PERMEL001:AA:YMML..MEMUP,S33451E\r\n120525.Y53.WENDY0560",
        )
        .expect("FPN parses");
        assert_eq!(fp.route_status, "Route Inactive");
        assert_eq!(fp.flight_number.as_deref(), Some("QFA780"));
        assert_eq!(fp.serial_number.as_deref(), Some("2125"));
        assert_eq!(fp.origin.as_deref(), Some("YPPH"));
        assert_eq!(fp.company_route.as_deref(), Some("PERMEL001"));
        assert_eq!(fp.checksum, "0x0560");
    }

    #[test]
    fn rejects_non_fpn() {
        assert!(parse("POSN43312W123174,EASON").is_none());
        assert!(parse("#DFB engine data").is_none());
        assert!(parse("FPN/").is_none());
    }
}
