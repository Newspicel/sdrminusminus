use sdrmm_wire::{channel::ChannelParams, decode::DecoderEvent};

use crate::ui::{
    kit_maps::feed::{MAP_KINDS, Station},
    map::{
        Geo,
        overlay::{Dot, Glyph, Label, Mark, Overlay},
    },
};

pub const TARGET_MAX_AGE_MS: i64 = 5 * 60_000;

#[must_use]
pub const fn style(kind: &str) -> (&'static str, u32) {
    match kind.as_bytes() {
        b"adsb" => ("Aircraft", 0x21_b0_b0),
        b"ais" => ("Ships", 0xe0_a4_58),
        _ => ("APRS", 0xb0_7d_e0),
    }
}

const fn glyph(kind: &str) -> Glyph {
    match kind.as_bytes() {
        b"adsb" => Glyph::Plane,
        b"ais" => Glyph::Ship,
        _ => Glyph::Arrow,
    }
}

const fn label_below(kind: &str) -> f64 {
    match kind.as_bytes() {
        b"adsb" => 14.0,
        b"ais" => 12.0,
        _ => 8.0,
    }
}

#[must_use]
pub fn map_kinds_of(kinds: &[String]) -> Vec<&'static str> {
    MAP_KINDS
        .into_iter()
        .filter(|kind| kinds.iter().any(|wired| wired == kind))
        .collect()
}

#[must_use]
pub fn is_stale(last_seen: i64, now: i64, max_age_ms: i64) -> bool {
    last_seen < now - max_age_ms
}

#[must_use]
pub fn position(station: &Station) -> Option<Geo> {
    match &station.event {
        DecoderEvent::Adsb(message) => Geo::checked(message.lat, message.lon),
        DecoderEvent::Ais(message) => Geo::checked(message.lat, message.lon),
        DecoderEvent::Aprs(packet) => Geo::checked(packet.lat, packet.lon),
        _ => None,
    }
}

fn trimmed(value: Option<&str>) -> Option<String> {
    let text = value.unwrap_or("").trim();
    (!text.is_empty()).then(|| text.to_owned())
}

#[must_use]
pub fn label(station: &Station) -> String {
    match &station.event {
        DecoderEvent::Adsb(message) => {
            trimmed(message.callsign.as_deref()).unwrap_or_else(|| message.icao.to_uppercase())
        }
        DecoderEvent::Ais(message) => trimmed(message.name.as_deref())
            .or_else(|| trimmed(message.call_sign.as_deref()))
            .unwrap_or_else(|| message.mmsi.to_string()),
        DecoderEvent::Aprs(packet) => packet.source.clone(),
        _ => station.id.clone(),
    }
}

fn heading_of(deg: Option<u16>) -> Option<f64> {
    deg.filter(|deg| *deg != 511).map(f64::from)
}

fn course_of(deg: Option<f64>) -> Option<f64> {
    deg.filter(|deg| *deg != 360.0)
}

fn bearing(deg: Option<f64>) -> Option<f64> {
    deg.filter(|deg| deg.is_finite())
        .map(|deg| deg.rem_euclid(360.0))
}

#[must_use]
pub fn heading(station: &Station) -> Option<f64> {
    match &station.event {
        DecoderEvent::Adsb(message) => bearing(message.track_deg),
        DecoderEvent::Ais(message) => {
            bearing(heading_of(message.heading_deg)).or_else(|| bearing(course_of(message.cog_deg)))
        }
        DecoderEvent::Aprs(packet) => bearing(packet.course_deg),
        _ => None,
    }
}

fn hemisphere(deg: f64, positive: &str, negative: &str) -> String {
    format!(
        "{:.4}° {}",
        deg.abs(),
        if deg < 0.0 { negative } else { positive }
    )
}

#[must_use]
pub fn format_position(at: Geo) -> String {
    format!(
        "{} {}",
        hemisphere(at.lat, "N", "S"),
        hemisphere(at.lon, "E", "W")
    )
}

fn scalar(value: Option<f64>, digits: usize, unit: &str) -> Option<String> {
    value
        .filter(|value| value.is_finite())
        .map(|value| format!("{value:.digits$}{unit}"))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Detail {
    pub kind: &'static str,
    pub id: String,
    pub label: String,
    pub freq_hz: f64,
    pub last_seen: i64,
    pub rows: Vec<(&'static str, String)>,
}

fn kept(entries: Vec<(&'static str, Option<String>)>) -> Vec<(&'static str, String)> {
    entries
        .into_iter()
        .filter_map(|(name, value)| value.map(|value| (name, value)))
        .collect()
}

fn rows(station: &Station) -> Vec<(&'static str, String)> {
    let fix = position(station).map(format_position);
    match &station.event {
        DecoderEvent::Adsb(d) => kept(vec![
            ("ICAO", Some(d.icao.to_uppercase())),
            ("Position", fix),
            ("Altitude", scalar(d.altitude_ft.map(f64::from), 0, " ft")),
            ("Speed", scalar(d.ground_speed_kt, 0, " kt")),
            ("Track", scalar(d.track_deg, 0, "°")),
            ("V/S", scalar(d.vertical_rate_fpm.map(f64::from), 0, " fpm")),
            ("Squawk", trimmed(d.squawk.as_deref())),
            (
                "State",
                (d.on_ground == Some(true)).then(|| "on ground".to_owned()),
            ),
        ]),
        DecoderEvent::Ais(d) => kept(vec![
            ("MMSI", Some(d.mmsi.to_string())),
            ("Position", fix),
            ("SOG", scalar(d.sog_kt, 1, " kt")),
            ("COG", scalar(course_of(d.cog_deg), 0, "°")),
            ("Heading", scalar(heading_of(d.heading_deg), 0, "°")),
            ("Call sign", trimmed(d.call_sign.as_deref())),
            ("Destination", trimmed(d.destination.as_deref())),
        ]),
        DecoderEvent::Aprs(d) => kept(vec![
            ("Source", Some(d.source.clone())),
            ("Position", fix),
            ("Speed", scalar(d.speed_kt, 0, " kt")),
            ("Course", scalar(d.course_deg, 0, "°")),
            ("Altitude", scalar(d.altitude_ft.map(f64::from), 0, " ft")),
            ("Message", trimmed(d.mic_e_message.as_deref())),
            ("Comment", trimmed(d.comment.as_deref())),
        ]),
        _ => Vec::new(),
    }
}

#[must_use]
pub fn detail(station: &Station) -> Detail {
    let mut rows = rows(station);
    rows.push(("Frames", station.frames.to_string()));
    Detail {
        kind: station.kind,
        id: station.id.clone(),
        label: label(station),
        freq_hz: station.freq_hz,
        last_seen: station.last_seen,
        rows,
    }
}

#[must_use]
pub fn pick_key(kind: &str, id: &str) -> String {
    format!("{kind}/{id}")
}

#[must_use]
pub fn overlay(stations: &[&Station], selected: Option<&str>, now: i64) -> (Overlay, usize) {
    let mut out = Overlay::default();
    let mut shown = 0;
    for station in stations {
        if is_stale(station.last_seen, now, TARGET_MAX_AGE_MS) {
            continue;
        }
        let Some(at) = position(station) else {
            continue;
        };
        shown += 1;
        let key = pick_key(station.kind, &station.id);
        let chosen = selected == Some(key.as_str());
        let (_, colour) = style(station.kind);
        out.dots.push(Dot {
            radius: vec![(0.0, if chosen { 6.0 } else { 4.0 })],
            stroke_width: if chosen { 3.0 } else { 1.0 },
            pick: Some(key.clone()),
            ..Dot::plain(at, 4.0, colour)
        });
        if let Some(heading) = heading(station) {
            out.marks.push(Mark {
                at,
                glyph: glyph(station.kind),
                heading,
                colour,
            });
        }
        out.labels.push(Label {
            key,
            at,
            text: label(station),
            colour,
            below_px: label_below(station.kind),
            min_zoom: 0.0,
        });
    }
    (out, shown)
}

#[must_use]
pub fn references(params: &[ChannelParams]) -> Vec<Geo> {
    let mut out: Vec<Geo> = Vec::new();
    for param in params {
        let ChannelParams::Adsb(adsb) = param else {
            continue;
        };
        let Some(at) = Geo::checked(adsb.ref_lat, adsb.ref_lon) else {
            continue;
        };
        if !out.contains(&at) {
            out.push(at);
        }
    }
    out
}

#[must_use]
pub fn reference_marks(positions: &[Geo]) -> Vec<Mark> {
    positions
        .iter()
        .map(|at| Mark {
            at: *at,
            glyph: Glyph::Station,
            heading: 0.0,
            colour: crate::ui::map::ACCENT,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{
        channel::AdsbParams,
        decode::{AdsbMessage, AisMessage, AprsPacket},
    };

    use super::*;

    const NOW: i64 = 1_786_276_800_000;

    fn station(event: DecoderEvent) -> Station {
        Station {
            kind: event.kind(),
            id: "target".to_owned(),
            event,
            last_seen: NOW,
            freq_hz: 1_090_000_000.0,
            frames: 1,
        }
    }

    fn adsb(data: AdsbMessage) -> Station {
        station(DecoderEvent::Adsb(AdsbMessage {
            icao: "3c6444".to_owned(),
            df: 17,
            raw: "8d".to_owned(),
            ..data
        }))
    }

    fn ais(data: AisMessage) -> Station {
        station(DecoderEvent::Ais(AisMessage {
            mmsi: 211_234_560,
            msg_type: 1,
            ais_channel: 'A',
            nmea: "!AIVDM".to_owned(),
            ..data
        }))
    }

    fn aprs(data: AprsPacket) -> Station {
        station(DecoderEvent::Aprs(AprsPacket {
            source: "DL1ABC-9".to_owned(),
            destination: "APRS".to_owned(),
            info: "!".to_owned(),
            tnc2: "DL1ABC-9>APRS:!".to_owned(),
            ..data
        }))
    }

    #[test]
    fn only_decoders_that_report_a_position_are_mapped() {
        let wired = |kinds: &[&str]| {
            kinds
                .iter()
                .map(|kind| (*kind).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(map_kinds_of(&wired(&["adsb", "pocsag", "acars"])), ["adsb"]);
        assert!(map_kinds_of(&wired(&["pocsag", "rtty"])).is_empty());
        assert_eq!(
            map_kinds_of(&wired(&["aprs", "adsb", "aprs"])),
            ["adsb", "aprs"]
        );
        assert_eq!(
            map_kinds_of(&wired(&["ais", "adsb"])),
            map_kinds_of(&wired(&["adsb", "ais"]))
        );
    }

    #[test]
    fn a_target_has_a_position_label_and_heading() {
        let target = adsb(AdsbMessage {
            lat: Some(52.5163),
            lon: Some(13.3777),
            callsign: Some("DLH123 ".to_owned()),
            track_deg: Some(271.5),
            ..AdsbMessage::default()
        });
        assert_eq!(position(&target), Some(Geo::new(52.5163, 13.3777)));
        assert_eq!(label(&target), "DLH123");
        assert_eq!(heading(&target), Some(271.5));
        let still = adsb(AdsbMessage {
            lat: Some(1.0),
            lon: Some(2.0),
            ..AdsbMessage::default()
        });
        assert_eq!(heading(&still), None);
    }

    #[test]
    fn a_target_without_a_fix_is_not_drawn() {
        assert!(position(&adsb(AdsbMessage::default())).is_none());
        assert!(
            position(&adsb(AdsbMessage {
                lat: Some(52.5),
                ..AdsbMessage::default()
            }))
            .is_none()
        );
        assert!(
            position(&ais(AisMessage {
                lat: Some(91.0),
                lon: Some(181.0),
                ..AisMessage::default()
            }))
            .is_none()
        );
        assert!(
            position(&ais(AisMessage {
                lat: Some(0.0),
                lon: Some(0.0),
                ..AisMessage::default()
            }))
            .is_some()
        );
    }

    #[test]
    fn labels_fall_back_through_each_kinds_identities() {
        assert_eq!(
            label(&adsb(AdsbMessage {
                callsign: Some("  ".to_owned()),
                ..AdsbMessage::default()
            })),
            "3C6444"
        );
        assert_eq!(label(&ais(AisMessage::default())), "211234560");
        assert_eq!(
            label(&ais(AisMessage {
                call_sign: Some("DEAB".to_owned()),
                ..AisMessage::default()
            })),
            "DEAB"
        );
        assert_eq!(
            label(&ais(AisMessage {
                name: Some("NORDIC".to_owned()),
                call_sign: Some("DEAB".to_owned()),
                ..AisMessage::default()
            })),
            "NORDIC"
        );
        assert_eq!(label(&aprs(AprsPacket::default())), "DL1ABC-9");
    }

    #[test]
    fn a_vessel_prefers_true_heading_and_ignores_sentinels() {
        assert_eq!(
            heading(&ais(AisMessage {
                heading_deg: Some(90),
                cog_deg: Some(275.0),
                ..AisMessage::default()
            })),
            Some(90.0)
        );
        assert_eq!(
            heading(&ais(AisMessage {
                cog_deg: Some(275.0),
                ..AisMessage::default()
            })),
            Some(275.0)
        );
        assert_eq!(
            heading(&ais(AisMessage {
                heading_deg: Some(511),
                cog_deg: Some(360.0),
                ..AisMessage::default()
            })),
            None
        );
        assert_eq!(
            heading(&ais(AisMessage {
                heading_deg: Some(511),
                cog_deg: Some(12.0),
                ..AisMessage::default()
            })),
            Some(12.0)
        );
    }

    #[test]
    fn headings_wrap_into_a_circle() {
        let course = |deg| {
            heading(&aprs(AprsPacket {
                course_deg: Some(deg),
                ..AprsPacket::default()
            }))
        };
        assert_eq!(course(360.0), Some(0.0));
        assert_eq!(course(-90.0), Some(270.0));
        assert_eq!(course(450.0), Some(90.0));
        assert_eq!(course(f64::NAN), None);
    }

    #[test]
    fn a_target_expires_strictly_after_the_horizon() {
        assert!(!is_stale(
            NOW - TARGET_MAX_AGE_MS + 1,
            NOW,
            TARGET_MAX_AGE_MS
        ));
        assert!(is_stale(
            NOW - TARGET_MAX_AGE_MS - 1,
            NOW,
            TARGET_MAX_AGE_MS
        ));
        assert!(is_stale(NOW - 2_000, NOW, 1_000));
    }

    #[test]
    fn only_fresh_positioned_targets_are_drawn() {
        let fresh = Station {
            id: "fresh".to_owned(),
            ..adsb(AdsbMessage {
                lat: Some(1.0),
                lon: Some(1.0),
                ..AdsbMessage::default()
            })
        };
        let stale = Station {
            id: "stale".to_owned(),
            last_seen: NOW - TARGET_MAX_AGE_MS - 1,
            ..adsb(AdsbMessage {
                lat: Some(2.0),
                lon: Some(2.0),
                ..AdsbMessage::default()
            })
        };
        let unfixed = Station {
            id: "no-fix".to_owned(),
            ..adsb(AdsbMessage::default())
        };
        let (drawn, shown) = overlay(&[&fresh, &stale, &unfixed], None, NOW);
        assert_eq!(shown, 1);
        assert_eq!(drawn.dots[0].pick.as_deref(), Some("adsb/fresh"));
        let (empty, none) = overlay(&[], None, NOW);
        assert_eq!(none, 0);
        assert!(empty.dots.is_empty());
    }

    #[test]
    fn only_adsb_references_are_read_and_shared_antennas_merge() {
        let reference = |lat: Option<f64>, lon: Option<f64>| {
            ChannelParams::Adsb(AdsbParams {
                ref_lat: lat,
                ref_lon: lon,
                ..AdsbParams::default()
            })
        };
        assert_eq!(
            references(&[
                reference(Some(50.7), Some(6.1)),
                reference(Some(50.7), Some(6.1)),
                reference(Some(-33.9), Some(18.4))
            ]),
            [Geo::new(50.7, 6.1), Geo::new(-33.9, 18.4)]
        );
        assert!(
            references(&[
                reference(Some(50.7), None),
                reference(Some(91.0), Some(181.0))
            ])
            .is_empty()
        );
    }

    #[test]
    fn a_detail_lists_only_what_the_target_reported() {
        let target = Station {
            frames: 12,
            ..adsb(AdsbMessage {
                lat: Some(-33.8688),
                lon: Some(151.2093),
                altitude_ft: Some(37_000),
                track_deg: Some(89.4),
                ..AdsbMessage::default()
            })
        };
        let shown = detail(&target);
        let rows: Vec<(&str, &str)> = shown
            .rows
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                ("ICAO", "3C6444"),
                ("Position", "33.8688° S 151.2093° E"),
                ("Altitude", "37000 ft"),
                ("Track", "89°"),
                ("Frames", "12"),
            ]
        );
        let ship = detail(&Station {
            id: "211234560".to_owned(),
            ..ais(AisMessage {
                name: Some("NORDIC".to_owned()),
                ..AisMessage::default()
            })
        });
        assert_eq!(
            (ship.kind, ship.id.as_str(), ship.label.as_str()),
            ("ais", "211234560", "NORDIC")
        );
        assert_eq!(ship.freq_hz, 1_090_000_000.0);
        assert_eq!(ship.last_seen, NOW);
    }
}
