use crate::acars::bits::{BitReader, sign_extend};
use serde::Serialize;

fn coordinate(v: u32) -> f64 {
    let r = sign_extend(v, 21) as f64;
    r * (180.0 - 90.0 / 2f64.powi(19)) / 0xF_FFFF as f64
}

fn altitude_ft(v: u32) -> i32 {
    sign_extend(v, 16) * 4
}

fn timestamp_s(v: u32) -> f64 {
    v as f64 * 0.125
}

fn speed(v: u32) -> f64 {
    v as f64 / 2.0
}

fn vert_speed_fpm(v: u32) -> i32 {
    sign_extend(v, 12) * 16
}

fn distance_nm(v: u32) -> f64 {
    v as f64 / 8.0
}

fn heading_deg(v: u32) -> f64 {
    let r = sign_extend(v, 12) as f64;
    let mut h = r * (180.0 - 90.0 / 2f64.powi(10)) / 0x7FF as f64;
    if h < 0.0 {
        h += 360.0;
    }
    h
}

fn wind_dir_deg(v: u32) -> f64 {
    let r = sign_extend(v, 9) as f64;
    let mut d = r * (180.0 - 90.0 / 2f64.powi(7)) / 0xFF as f64;
    if d < 0.0 {
        d += 360.0;
    }
    d
}

fn temperature_c(v: u32) -> f64 {
    let r = sign_extend(v, 12) as f64;
    r * (512.0 - 256.0 / 2f64.powi(10)) / 0x7FF as f64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportKind {
    Basic,
    Emergency,
    LateralDeviationChange,
    VerticalRateChange,
    AltitudeRange,
    WaypointChange,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BasicReport {
    pub kind: ReportKind,
    pub lat: f64,
    pub lon: f64,
    pub alt_ft: i32,
    pub timestamp_s: f64,
    pub accuracy: u8,
    pub nav_redundancy_ok: bool,
    pub tcas_ok: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "tag", rename_all = "snake_case")]
pub enum AdscTag {
    Ack {
        contract: u8,
    },
    Nack {
        contract: u8,
        reason: u8,
        reason_text: String,
        ext: Option<u8>,
    },
    Noncompliance {
        contract: u8,
        groups: Vec<NoncomplianceGroup>,
    },
    CancelEmergency,
    Report(BasicReport),
    FlightId {
        id: String,
    },
    PredictedRoute {
        next_lat: f64,
        next_lon: f64,
        next_alt_ft: i32,
        next_eta_s: u32,
        next_next_lat: f64,
        next_next_lon: f64,
        next_next_alt_ft: i32,
    },
    EarthRef {
        true_track_valid: bool,
        true_track_deg: f64,
        ground_speed_kt: f64,
        vert_speed_fpm: i32,
    },
    AirRef {
        true_heading_valid: bool,
        true_heading_deg: f64,
        mach: f64,
        vert_speed_fpm: i32,
    },
    Meteo {
        wind_speed_kt: f64,
        wind_dir_valid: bool,
        wind_dir_deg: f64,
        temperature_c: f64,
    },
    AirframeId {
        icao: String,
    },
    IntermediateProjection {
        distance_nm: f64,
        true_track_valid: bool,
        true_track_deg: f64,
        alt_ft: i32,
        eta_s: u32,
    },
    FixedProjection {
        lat: f64,
        lon: f64,
        alt_ft: i32,
        eta_s: u32,
    },
    DisconnectReason {
        code: u8,
        text: String,
    },
    CancelAllContracts,
    CancelContract {
        contract: u8,
    },
    UplinkCancelEmergency {
        contract: u8,
    },
    ContractRequest {
        kind: ContractKind,
        contract: u8,
        groups: Vec<RequestGroup>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractKind {
    Periodic,
    Event,
    EmergencyPeriodic,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NoncomplianceGroup {
    pub tag: u8,
    pub unrecognized: bool,
    pub group_unavailable: bool,
    pub params: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum RequestGroup {
    LateralDeviationThreshold {
        nm: f64,
    },
    ReportingInterval {
        seconds: u32,
    },
    FlightIdModulus {
        every_n: u8,
    },
    PredictedRouteModulus {
        every_n: u8,
    },
    EarthRefModulus {
        every_n: u8,
    },
    AirRefModulus {
        every_n: u8,
    },
    MeteoModulus {
        every_n: u8,
    },
    AirframeIdModulus {
        every_n: u8,
    },
    VerticalSpeedThreshold {
        fpm: i32,
    },
    AltitudeRange {
        ceiling_ft: i32,
        floor_ft: i32,
    },
    ReportWaypointChanges,
    AircraftIntent {
        modulus: u8,
        projection_time_min: u8,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AdscMessage {
    pub tags: Vec<AdscTag>,
    pub err: bool,
}

impl AdscMessage {
    pub fn summary(&self) -> Option<String> {
        for t in &self.tags {
            if let AdscTag::Report(r) = t {
                return Some(format!(
                    "ADS-C {:?} {:.4} {:.4} {} ft",
                    r.kind, r.lat, r.lon, r.alt_ft
                ));
            }
        }
        self.tags.first().map(|t| format!("ADS-C {}", tag_name(t)))
    }
}

fn tag_name(t: &AdscTag) -> &'static str {
    match t {
        AdscTag::Ack { .. } => "ack",
        AdscTag::Nack { .. } => "nack",
        AdscTag::Noncompliance { .. } => "noncompliance",
        AdscTag::CancelEmergency => "cancel-emergency",
        AdscTag::Report(_) => "report",
        AdscTag::FlightId { .. } => "flight-id",
        AdscTag::PredictedRoute { .. } => "predicted-route",
        AdscTag::EarthRef { .. } => "earth-ref",
        AdscTag::AirRef { .. } => "air-ref",
        AdscTag::Meteo { .. } => "meteo",
        AdscTag::AirframeId { .. } => "airframe-id",
        AdscTag::IntermediateProjection { .. } => "intermediate-projection",
        AdscTag::FixedProjection { .. } => "fixed-projection",
        AdscTag::DisconnectReason { .. } => "disconnect",
        AdscTag::CancelAllContracts => "cancel-all-contracts",
        AdscTag::CancelContract { .. } => "cancel-contract",
        AdscTag::UplinkCancelEmergency { .. } => "cancel-emergency",
        AdscTag::ContractRequest { .. } => "contract-request",
    }
}

pub fn parse(buf: &[u8], downlink: bool, dis: bool) -> AdscMessage {
    let mut msg = AdscMessage {
        tags: Vec::new(),
        err: false,
    };
    if dis {
        match buf.first() {
            Some(&b) => {
                let code = b >> 4;
                let text = match code {
                    0 => "reason not specified",
                    1 => "congestion",
                    2 => "application not available",
                    8 => "normal disconnect",
                    _ => "unknown",
                };
                msg.tags.push(AdscTag::DisconnectReason {
                    code,
                    text: text.to_owned(),
                });
            }
            None => msg.err = true,
        }
        return msg;
    }
    if downlink {
        parse_downlink(buf, &mut msg);
    } else {
        parse_uplink(buf, &mut msg);
    }
    msg
}

fn take<'a>(buf: &mut &'a [u8], len: usize) -> Option<&'a [u8]> {
    if buf.len() < len {
        return None;
    }
    let (head, rest) = buf.split_at(len);
    *buf = rest;
    Some(head)
}

fn parse_basic_report(b: &[u8], kind: ReportKind) -> Option<AdscTag> {
    let mut r = BitReader::new(b);
    let lat = coordinate(r.read(21)?);
    let lon = coordinate(r.read(21)?);
    let alt = altitude_ft(r.read(16)?);
    let ts = timestamp_s(r.read(15)?);
    let flags = r.read(7)?;
    Some(AdscTag::Report(BasicReport {
        kind,
        lat,
        lon,
        alt_ft: alt,
        timestamp_s: ts,
        nav_redundancy_ok: flags & 1 == 1,
        accuracy: ((flags >> 1) & 0x7) as u8,
        tcas_ok: (flags >> 4) & 1 == 1,
    }))
}

fn parse_downlink(mut buf: &[u8], msg: &mut AdscMessage) {
    const NACK_REASONS: [&str; 14] = [
        "unknown",
        "Duplicate group tag",
        "Duplicate reporting interval tag",
        "Event contract request with no data",
        "Improper operational mode tag",
        "Cancel request of a contract which does not exist",
        "Requested contract already exists",
        "Undefined contract request tag",
        "Undefined error",
        "Not enough data in request",
        "Invalid altitude range: low limit >= high limit",
        "Vertical speed threshold is 0",
        "Aircraft intent projection time is 0",
        "Lateral deviation threshold is 0",
    ];

    while let Some((&tag, rest)) = buf.split_first() {
        buf = rest;
        let parsed: Option<AdscTag> = match tag {
            3 => take(&mut buf, 1).map(|b| AdscTag::Ack { contract: b[0] }),
            4 => take(&mut buf, 2).and_then(|b| {
                let reason = b[1];
                if reason > 13 {
                    return None;
                }
                let ext = if matches!(reason, 1 | 2 | 7) {
                    Some(take(&mut buf, 1)?[0])
                } else {
                    None
                };
                Some(AdscTag::Nack {
                    contract: b[0],
                    reason,
                    reason_text: NACK_REASONS[reason as usize].to_owned(),
                    ext,
                })
            }),
            5 => parse_noncompliance(&mut buf),
            6 => Some(AdscTag::CancelEmergency),
            7 => take(&mut buf, 10).and_then(|b| parse_basic_report(b, ReportKind::Basic)),
            9 => take(&mut buf, 10).and_then(|b| parse_basic_report(b, ReportKind::Emergency)),
            10 => take(&mut buf, 10)
                .and_then(|b| parse_basic_report(b, ReportKind::LateralDeviationChange)),
            12 => take(&mut buf, 6).and_then(|b| {
                let mut r = BitReader::new(b);
                let mut id = String::with_capacity(8);
                for _ in 0..8 {
                    let mut c = r.read(6)?;
                    if c & 0x20 == 0 {
                        c += 0x40;
                    }
                    id.push(c as u8 as char);
                }
                Some(AdscTag::FlightId {
                    id: id.trim_end().to_owned(),
                })
            }),
            13 => take(&mut buf, 17).and_then(|b| {
                let mut r = BitReader::new(b);
                Some(AdscTag::PredictedRoute {
                    next_lat: coordinate(r.read(21)?),
                    next_lon: coordinate(r.read(21)?),
                    next_alt_ft: altitude_ft(r.read(16)?),
                    next_eta_s: r.read(14)?,
                    next_next_lat: coordinate(r.read(21)?),
                    next_next_lon: coordinate(r.read(21)?),
                    next_next_alt_ft: altitude_ft(r.read(16)?),
                })
            }),
            14 => take(&mut buf, 5).and_then(|b| {
                let mut r = BitReader::new(b);
                let invalid = r.read(1)? == 1;
                Some(AdscTag::EarthRef {
                    true_track_valid: !invalid,
                    true_track_deg: heading_deg(r.read(12)?),
                    ground_speed_kt: speed(r.read(13)?),
                    vert_speed_fpm: vert_speed_fpm(r.read(12)?),
                })
            }),
            15 => take(&mut buf, 5).and_then(|b| {
                let mut r = BitReader::new(b);
                let invalid = r.read(1)? == 1;
                Some(AdscTag::AirRef {
                    true_heading_valid: !invalid,
                    true_heading_deg: heading_deg(r.read(12)?),
                    mach: speed(r.read(13)?) / 1000.0,
                    vert_speed_fpm: vert_speed_fpm(r.read(12)?),
                })
            }),
            16 => take(&mut buf, 4).and_then(|b| {
                let mut r = BitReader::new(b);
                let ws = speed(r.read(9)?);
                let dir_invalid = r.read(1)? == 1;
                Some(AdscTag::Meteo {
                    wind_speed_kt: ws,
                    wind_dir_valid: !dir_invalid,
                    wind_dir_deg: wind_dir_deg(r.read(9)?),
                    temperature_c: temperature_c(r.read(12)?),
                })
            }),
            17 => take(&mut buf, 3).map(|b| AdscTag::AirframeId {
                icao: format!("{:02X}{:02X}{:02X}", b[0], b[1], b[2]),
            }),
            18 => take(&mut buf, 10)
                .and_then(|b| parse_basic_report(b, ReportKind::VerticalRateChange)),
            19 => take(&mut buf, 10).and_then(|b| parse_basic_report(b, ReportKind::AltitudeRange)),
            20 => {
                take(&mut buf, 10).and_then(|b| parse_basic_report(b, ReportKind::WaypointChange))
            }
            22 => take(&mut buf, 8).and_then(|b| {
                let mut r = BitReader::new(b);
                let d = distance_nm(r.read(16)?);
                let invalid = r.read(1)? == 1;
                Some(AdscTag::IntermediateProjection {
                    distance_nm: d,
                    true_track_valid: !invalid,
                    true_track_deg: heading_deg(r.read(12)?),
                    alt_ft: altitude_ft(r.read(16)?),
                    eta_s: r.read(14)?,
                })
            }),
            23 => take(&mut buf, 9).and_then(|b| {
                let mut r = BitReader::new(b);
                Some(AdscTag::FixedProjection {
                    lat: coordinate(r.read(21)?),
                    lon: coordinate(r.read(21)?),
                    alt_ft: altitude_ft(r.read(16)?),
                    eta_s: r.read(14)?,
                })
            }),
            _ => None,
        };
        match parsed {
            Some(t) => msg.tags.push(t),
            None => {
                msg.err = true;
                return;
            }
        }
    }
}

fn parse_noncompliance(buf: &mut &[u8]) -> Option<AdscTag> {
    let hdr = take(buf, 2)?;
    let contract = hdr[0];
    let group_cnt = hdr[1] as usize;
    let mut groups = Vec::with_capacity(group_cnt);
    for _ in 0..group_cnt {
        let g = take(buf, 2)?;
        let tag = g[0];
        let unrecognized = g[1] & 0x80 != 0;
        let group_unavailable = g[1] & 0x40 != 0;
        let mut params = Vec::new();
        if !unrecognized && !group_unavailable {
            let cnt = (g[1] & 0x0F) as usize;
            let packed = take(buf, cnt.div_ceil(2))?;
            for i in 0..cnt {
                params.push((packed[i / 2] >> (((i + 1) % 2) * 4)) & 0xF);
            }
        }
        groups.push(NoncomplianceGroup {
            tag,
            unrecognized,
            group_unavailable,
            params,
        });
    }
    Some(AdscTag::Noncompliance { contract, groups })
}

fn parse_uplink(mut buf: &[u8], msg: &mut AdscMessage) {
    while let Some((&tag, rest)) = buf.split_first() {
        buf = rest;
        let parsed: Option<AdscTag> = match tag {
            1 => Some(AdscTag::CancelAllContracts),
            2 => take(&mut buf, 1).map(|b| AdscTag::CancelContract { contract: b[0] }),
            6 => take(&mut buf, 1).map(|b| AdscTag::UplinkCancelEmergency { contract: b[0] }),
            7..=9 => {
                let kind = match tag {
                    7 => ContractKind::Periodic,
                    8 => ContractKind::Event,
                    _ => ContractKind::EmergencyPeriodic,
                };
                take(&mut buf, 1).map(|b| AdscTag::ContractRequest {
                    kind,
                    contract: b[0],
                    groups: parse_request_groups(&mut buf),
                })
            }
            _ => None,
        };
        match parsed {
            Some(t) => msg.tags.push(t),
            None => {
                msg.err = true;
                return;
            }
        }
    }
}

fn parse_request_groups(buf: &mut &[u8]) -> Vec<RequestGroup> {
    let mut groups = Vec::new();
    loop {
        let Some((&tag, rest)) = buf.split_first() else {
            return groups;
        };
        let mut b = rest;
        let parsed: Option<RequestGroup> = match tag {
            10 => take(&mut b, 1).map(|x| RequestGroup::LateralDeviationThreshold {
                nm: x[0] as f64 / 8.0,
            }),
            11 => take(&mut b, 1).map(|x| {
                let sf = [0u32, 1, 8, 64][(x[0] >> 6) as usize];
                RequestGroup::ReportingInterval {
                    seconds: sf * ((x[0] & 0x3F) as u32 + 1),
                }
            }),
            12 => take(&mut b, 1).map(|x| RequestGroup::FlightIdModulus { every_n: x[0] }),
            13 => take(&mut b, 1).map(|x| RequestGroup::PredictedRouteModulus { every_n: x[0] }),
            14 => take(&mut b, 1).map(|x| RequestGroup::EarthRefModulus { every_n: x[0] }),
            15 => take(&mut b, 1).map(|x| RequestGroup::AirRefModulus { every_n: x[0] }),
            16 => take(&mut b, 1).map(|x| RequestGroup::MeteoModulus { every_n: x[0] }),
            17 => take(&mut b, 1).map(|x| RequestGroup::AirframeIdModulus { every_n: x[0] }),
            18 => take(&mut b, 1).map(|x| RequestGroup::VerticalSpeedThreshold {
                fpm: (x[0] as i8) as i32 * 64,
            }),
            19 => take(&mut b, 4).map(|x| RequestGroup::AltitudeRange {
                ceiling_ft: altitude_ft(u32::from_be_bytes([0, 0, x[0], x[1]])),
                floor_ft: altitude_ft(u32::from_be_bytes([0, 0, x[2], x[3]])),
            }),
            20 => Some(RequestGroup::ReportWaypointChanges),
            21 => take(&mut b, 2).map(|x| RequestGroup::AircraftIntent {
                modulus: x[0],
                projection_time_min: x[1],
            }),
            _ => None,
        };
        match parsed {
            Some(g) => {
                groups.push(g);
                *buf = b;
            }
            None => return groups,
        }
    }
}
