use anyhow::{Result, bail};

use super::{Row, Target, report_unmapped, service_of};

pub(super) static TARGET: &Target = &Target {
    id: "de",
    name: "Germany — BNetzA",
    authority: "Bundesnetzagentur",
    kind: "regulatory",
};

/// German service names, longest-qualified first: "FESTER FUNKDIENST ÜBER SATELLITEN" is a
/// satellite service, not a fixed one, and matching "fester funkdienst" first would lose that.
/// Order matters throughout: the first substring to match wins, so a qualified name has to come
/// before the bare one it contains. Extended from the importer's own "fell through to `other`"
/// report, which is what that report is for.
static SERVICES: &[(&str, &str)] = &[
    ("(nicht zugewiesen)", "other"),
    ("funknachrichten", "mobile"),
    ("erde)", "satellite"),
    // Satellite first: "FESTER FUNKDIENST ÜBER SATELLITEN" is a satellite service, not a fixed
    // one, and matching "fester funkdienst" first would lose that.
    ("über satelliten", "satellite"),
    ("im satellitenfunk", "satellite"),
    ("satellitenfunk", "satellite"),
    ("intersatelliten", "satellite"),
    ("weltraumfernwirk", "satellite"),
    ("weltraum", "satellite"),
    ("amateurfunk", "amateur"),
    ("rundfunk", "broadcast"),
    ("flugnavigationsfunkdienst", "aeronautical"),
    ("flugfunk", "aeronautical"),
    ("flugsicherungsradar", "aeronautical"),
    ("helikopterradar", "aeronautical"),
    ("seefunknavigationsdienst", "maritime"),
    ("seefunk", "maritime"),
    ("binnenschifffahrt", "maritime"),
    ("radioastronomie", "science"),
    ("erderkundung", "science"),
    ("wetterhilfen", "science"),
    ("normalfrequenz", "science"),
    ("fernmessen", "science"),
    ("telemetrie", "science"),
    ("navigationsfunkdienst", "navigation"),
    ("ortungsfunkdienst", "navigation"),
    ("funkortung", "navigation"),
    ("funkbewegungsmelder", "navigation"),
    ("radar", "navigation"),
    ("bos", "mobile"),
    ("betriebsfunk", "mobile"),
    ("bündelfunk", "mobile"),
    ("mobiler landfunkdienst", "mobile"),
    ("mobilfunk", "mobile"),
    ("mobiler funkdienst", "mobile"),
    ("verkehrstelematik", "mobile"),
    ("intelligente verkehrssysteme", "mobile"),
    ("militärische funkanwendungen", "mobile"),
    ("alarmierungszwecke", "mobile"),
    ("srd", "ism"),
    ("geringer reichweite", "ism"),
    ("wlan", "ism"),
    ("mgws", "ism"),
    ("ism", "ism"),
    ("funkmikrofone", "ism"),
    ("hörhilfen", "ism"),
    ("fernsteuerung", "ism"),
    ("demonstrationsfunk", "ism"),
    ("fester funkdienst", "other"),
    ("festfunk", "other"),
];

pub(super) fn parse(input: &str) -> Result<Vec<Row>> {
    let mut rows = Vec::new();
    let mut unmapped = Vec::new();

    for record in records(input) {
        let Some(reference) = field(&record, "Eintrag") else {
            continue;
        };
        let range = field(&record, "Frequenzteilbereich(e)")
            .or_else(|| field(&record, "Frequenzteilbereich"))
            .or_else(|| field(&record, "Frequenzbereich"));
        let Some(range) = range else { continue };
        let service_name = field(&record, "Funkdienst").filter(|name| !name.is_empty());
        let usage = field(&record, "Frequenznutzung").filter(|name| !name.is_empty());
        let Some(name) = service_name.clone().or_else(|| usage.clone()) else {
            continue;
        };

        for (start_hz, stop_hz) in ranges(&range)? {
            rows.push(Row {
                // The Radio Regulations write a primary service in capitals. Rows named from
                // `Frequenznutzung` are not ITU services at all, so they are not primary ones.
                primary: service_name
                    .as_deref()
                    .is_some_and(|name| name == name.to_uppercase()),
                reference: Some(reference.clone()),
                channel_step_hz: field(&record, "Kanalraster").and_then(|v| hertz(&v)),
                notes: notes(&record, usage.as_deref()),
                ..Row::new(
                    start_hz,
                    stop_hz,
                    service_of(&name, SERVICES, &mut unmapped),
                    name.clone(),
                )
            });
        }
    }

    rows.sort_by(|a, b| a.start_hz.total_cmp(&b.start_hz));
    report_unmapped("bnetza", &unmapped);
    if rows.is_empty() {
        bail!("no records parsed — the Frequenzplan's layout has changed");
    }
    Ok(rows)
}

fn records(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in input.lines() {
        if line.contains("Frequenzteilplan:") && line.contains("Eintrag:") {
            if let Some(lines) = current.take() {
                out.push(lines.join("\n"));
            }
            current = Some(vec![line]);
        } else if let Some(lines) = current.as_mut() {
            lines.push(line);
        }
    }
    if let Some(lines) = current {
        out.push(lines.join("\n"));
    }
    out
}

fn field(record: &str, label: &str) -> Option<String> {
    let needle = format!("{label}:");
    for (index, line) in record.lines().enumerate() {
        let Some(at) = label_at(line, &needle) else {
            continue;
        };
        let rest = &line[at + needle.len()..];
        let mut value = rest[..next_label(rest).unwrap_or(rest.len())]
            .trim()
            .to_string();

        if line[..at].trim().is_empty() {
            for continuation in record.lines().skip(index + 1) {
                let trimmed = continuation.trim();
                if trimmed.is_empty() || next_label(continuation).is_some() {
                    break;
                }
                value.push(' ');
                value.push_str(trimmed);
            }
        }
        return Some(value.trim().to_string());
    }
    None
}

/// Where `needle` starts in `line`, only where it is a whole label — otherwise looking for
/// `Nutzung:` would find the tail of `Frequenznutzung:` and read the wrong field.
fn label_at(line: &str, needle: &str) -> Option<usize> {
    let mut from = 0usize;
    while let Some(found) = line[from..].find(needle) {
        let at = from + found;
        if at == 0 || line[..at].ends_with(char::is_whitespace) {
            return Some(at);
        }
        from = at + needle.len();
    }
    None
}

/// The offset of the next `Label:` in `text`, i.e. where the current value stops.
fn next_label(text: &str) -> Option<usize> {
    let colon = text.find(':')?;
    let before = &text[..colon];
    Some(before.rfind(char::is_whitespace).map_or(0, |at| at + 1))
}

/// Every `a - b unit` range in a value. Plural because `Frequenzteilbereich(e)` is, and a record
/// that lists two sub-bands means both. Split on `;` only: a comma is a German decimal point.
fn ranges(value: &str) -> Result<Vec<(f64, f64)>> {
    let mut out = Vec::new();
    for part in value.split(';') {
        let Some((lhs, rest)) = part.split_once('-') else {
            continue;
        };
        let Some(unit) = rest.split_whitespace().last().and_then(scale) else {
            continue;
        };
        let (Some(low), Some(high)) = (german(lhs), german(rest)) else {
            continue;
        };
        if high > low {
            out.push((low * unit, high * unit));
        }
    }
    Ok(out)
}

/// German decimal commas: "148,5" is 148.5, and reading it as 1485 would put a broadcast band in
/// the wrong part of the spectrum entirely.
fn german(text: &str) -> Option<f64> {
    let cleaned: String = text
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
        .collect();
    cleaned.replace('.', "").replace(',', ".").parse().ok()
}

fn scale(unit: &str) -> Option<f64> {
    match unit.trim().to_lowercase().as_str() {
        "hz" => Some(1.0),
        "khz" => Some(1e3),
        "mhz" => Some(1e6),
        "ghz" => Some(1e9),
        _ => None,
    }
}

/// A value like "9 kHz" or "12,5 kHz".
fn hertz(value: &str) -> Option<f64> {
    let unit = value.split_whitespace().last().and_then(scale)?;
    german(value).map(|number| number * unit)
}

/// What the record says the band is used for, which is the closest thing it has to a note. The
/// conditions prose is often a page of licence text, so only the usage line travels.
fn notes(record: &str, usage: Option<&str>) -> Option<String> {
    let usage = usage?;
    let bandwidth = field(record, "Kanalbandbreite")
        .map(|value| format!(" Kanalbandbreite: {value}."))
        .unwrap_or_default();
    Some(format!("{usage}.{bandwidth}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../fixtures/bandplan/bnetza-excerpt.txt");

    #[test]
    fn reads_a_record_into_a_row_with_its_eintrag_as_provenance() {
        let rows = parse(FIXTURE).expect("parse");
        let broadcast = rows
            .iter()
            .find(|row| row.reference.as_deref() == Some("20001"))
            .expect("Eintrag 20001");
        assert_eq!(broadcast.start_hz, 148_500.0);
        assert_eq!(broadcast.stop_hz, 255_000.0);
        assert_eq!(broadcast.name, "RUNDFUNKDIENST");
        assert_eq!(broadcast.service, "broadcast");
        assert!(broadcast.primary, "a capitalised service is primary");
        assert_eq!(broadcast.channel_step_hz, Some(9_000.0));
    }

    /// The case the whole model change was forced by: BNetzA gives 435–472 kHz to three
    /// different services in three records.
    #[test]
    fn keeps_every_record_that_shares_one_range() {
        let rows = parse(FIXTURE).expect("parse");
        let shared: Vec<&Row> = rows
            .iter()
            .filter(|row| row.start_hz == 435_000.0 && row.stop_hz == 472_000.0)
            .collect();
        assert_eq!(
            shared.len(),
            2,
            "aeronautical and maritime share 435–472 kHz"
        );
        assert!(shared.iter().any(|row| row.service == "aeronautical"));
        assert!(shared.iter().any(|row| row.service == "maritime"));
    }

    #[test]
    fn a_record_with_no_funkdienst_is_named_by_its_usage() {
        let rows = parse(FIXTURE).expect("parse");
        let srd = rows
            .iter()
            .find(|row| row.reference.as_deref() == Some("27003"))
            .expect("Eintrag 27003");
        assert!(srd.name.contains("SRD"));
        assert_eq!(srd.service, "ism");
        assert!(!srd.primary, "a non-ITU usage is not a primary allocation");
        assert_eq!(srd.start_hz, 442_200.0);
        assert_eq!(srd.stop_hz, 450_000.0);
    }

    #[test]
    fn reads_german_decimal_commas() {
        assert_eq!(german("148,5"), Some(148.5));
        assert_eq!(german("  9 "), Some(9.0));
        assert_eq!(german("12,5 kHz"), Some(12.5));
        assert_eq!(german("30.000"), Some(30_000.0));
    }

    #[test]
    fn applies_the_unit_to_both_ends_of_a_range() {
        assert_eq!(ranges("8,3 - 9 kHz").unwrap(), vec![(8_300.0, 9_000.0)]);
        assert_eq!(
            ranges("27,5 - 10000 MHz").unwrap(),
            vec![(27_500_000.0, 10_000_000_000.0)]
        );
        // Not a range: refused rather than guessed at.
        assert!(ranges("siehe Anhang").unwrap().is_empty());
    }

    #[test]
    fn an_unrecognisable_document_fails_loudly() {
        assert!(parse("this is not the Frequenzplan").is_err());
    }

    #[test]
    fn rows_come_out_sorted() {
        let rows = parse(FIXTURE).expect("parse");
        assert!(rows.windows(2).all(|p| p[1].start_hz >= p[0].start_hz));
    }
}
