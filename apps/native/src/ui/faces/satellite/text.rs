use sdrmm_wire::satellite::{SatellitePass, Transmitter};

const COMPASS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
pub const STALE_ELEMENTS_DAYS: f64 = 7.0;
pub const RANGE_HZ: (f64, f64) = (1e6, 12e9);

#[must_use]
pub fn compass(azimuth_deg: f64) -> &'static str {
    let index = (azimuth_deg.rem_euclid(360.0) / 45.0).round() as usize % COMPASS.len();
    COMPASS[index]
}

#[must_use]
pub fn doppler(hz: f64) -> String {
    let sign = if hz > 0.0 {
        "+"
    } else if hz < 0.0 {
        "\u{2212}"
    } else {
        ""
    };
    let magnitude = hz.abs();
    if magnitude >= 1_000.0 {
        format!("{sign}{:.2} kHz", magnitude / 1_000.0)
    } else {
        format!("{sign}{magnitude:.0} Hz")
    }
}

#[must_use]
pub fn span(seconds: i64) -> String {
    let whole = seconds.max(0);
    let (hours, minutes, rest) = (whole / 3_600, (whole % 3_600) / 60, whole % 60);
    if hours > 0 {
        format!("{hours} h {minutes:02} min")
    } else {
        format!("{minutes}:{rest:02}")
    }
}

#[must_use]
pub fn pass_line(pass: Option<&SatellitePass>, now_s: i64) -> String {
    let Some(pass) = pass else {
        return String::from("none in 48 h");
    };
    let peak = format!("{:.0}°", pass.max_elevation_deg);
    match (pass.aos, pass.los) {
        (Some(aos), _) if aos > now_s => format!("in {}, up to {peak}", span(aos - now_s)),
        (_, Some(los)) => format!("sets in {}, up to {peak}", span(los - now_s)),
        _ => String::from("always up"),
    }
}

#[must_use]
pub fn pasted_elements(text: &str) -> Option<String> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect();
    let first = lines.iter().position(|line| line.starts_with("1 "))?;
    if !lines.get(first + 1)?.starts_with("2 ") {
        return None;
    }
    Some(lines[first.saturating_sub(1)..first + 2].join("\n"))
}

#[must_use]
pub fn transmitter_label(transmitter: &Transmitter) -> String {
    let mhz = transmitter
        .downlink_hz
        .map_or_else(String::new, |hz| format!(" {:.3}", hz / 1e6));
    let mode = transmitter
        .mode
        .as_ref()
        .map_or_else(String::new, |mode| format!(" {mode}"));
    let off = if transmitter.alive { "" } else { " (off)" };
    format!("{}{mode}{mhz}{off}", transmitter.description)
}

#[must_use]
pub fn shown_signals<'a>(
    transmitters: &'a [Transmitter],
    chosen: Option<&str>,
) -> Vec<&'a Transmitter> {
    transmitters
        .iter()
        .filter(|transmitter| {
            transmitter.downlink_hz.is_some()
                && (transmitter.alive || Some(transmitter.id.as_str()) == chosen)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ISS: &str = "ISS (ZARYA)
1 25544U 98067A   24001.50000000  .00016717  00000-0  30306-3 0  9999
2 25544  51.6416 247.4627 0006703 130.5360 325.0288 15.50377579432041";

    fn transmitter(
        id: &str,
        description: &str,
        downlink_hz: Option<f64>,
        alive: bool,
    ) -> Transmitter {
        Transmitter {
            id: id.to_owned(),
            description: description.to_owned(),
            mode: None,
            downlink_hz,
            uplink_hz: None,
            alive,
        }
    }

    #[test]
    fn the_nearest_compass_point_is_named() {
        assert_eq!(compass(0.0), "N");
        assert_eq!(compass(359.0), "N");
        assert_eq!(compass(92.0), "E");
        assert_eq!(compass(-45.0), "NW");
    }

    #[test]
    fn a_doppler_shift_has_its_sign_and_a_readable_unit() {
        assert_eq!(doppler(9_876.0), "+9.88 kHz");
        assert_eq!(doppler(-420.0), "\u{2212}420 Hz");
        assert_eq!(doppler(0.0), "0 Hz");
    }

    #[test]
    fn a_pass_to_come_one_underway_and_one_that_never_ends() {
        let pass = |aos, los, peak| SatellitePass {
            aos,
            los,
            max_elevation_deg: peak,
            max_at: 1_300,
        };
        assert_eq!(
            pass_line(Some(&pass(Some(1_090), Some(1_600), 42.4)), 1_000),
            "in 1:30, up to 42°"
        );
        assert_eq!(
            pass_line(Some(&pass(Some(900), Some(1_600), 12.0)), 1_000),
            "sets in 10:00, up to 12°"
        );
        assert_eq!(pass_line(Some(&pass(None, None, 30.0)), 1_000), "always up");
        assert_eq!(pass_line(None, 1_000), "none in 48 h");
        assert_eq!(span(3_700), "1 h 01 min");
    }

    #[test]
    fn pasted_element_sets_are_recognised_with_or_without_a_name() {
        assert_eq!(pasted_elements(ISS).as_deref(), Some(ISS));
        let unnamed = ISS.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert_eq!(pasted_elements(&unnamed).as_deref(), Some(unnamed.as_str()));
        assert_eq!(pasted_elements("ISS"), None);
    }

    #[test]
    fn a_transmitter_is_labelled_by_what_it_is_and_where_it_sits() {
        let voice = Transmitter {
            mode: Some("FM".to_owned()),
            ..transmitter("a", "FM voice", Some(437_800_000.0), true)
        };
        assert_eq!(transmitter_label(&voice), "FM voice FM 437.800");
        assert_eq!(
            transmitter_label(&transmitter("b", "Beacon", None, false)),
            "Beacon (off)"
        );
    }

    #[test]
    fn live_signals_are_offered_and_a_dead_one_only_while_picked() {
        let signals = [
            transmitter("live", "Voice", Some(437_800_000.0), true),
            transmitter("dead", "Old beacon", Some(145_800_000.0), false),
            transmitter("blank", "No downlink", None, true),
        ];
        let ids = |chosen| {
            shown_signals(&signals, chosen)
                .iter()
                .map(|signal| signal.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(None), ["live"]);
        assert_eq!(ids(Some("dead")), ["live", "dead"]);
    }
}
