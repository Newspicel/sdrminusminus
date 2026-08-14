use anyhow::{Result, bail};

use super::{Row, Target, fcc, report_unmapped, service_of};

pub(super) static TARGET: &Target = &Target {
    id: "cept",
    name: "CEPT — European Common Allocation",
    authority: "CEPT / ECO",
    kind: "regulatory",
};

static APPLICATIONS: &[(&str, &str)] = &[
    ("amateur", "amateur"),
    ("broadcast", "broadcast"),
    ("dab", "broadcast"),
    ("dvb", "broadcast"),
    ("aeronautical", "aeronautical"),
    ("ils", "aeronautical"),
    ("vdl", "aeronautical"),
    ("vor", "aeronautical"),
    ("maritime", "maritime"),
    ("ais", "maritime"),
    ("amrd", "maritime"),
    ("dsc", "maritime"),
    ("epirb", "maritime"),
    ("navtex", "maritime"),
    ("oceanographic", "maritime"),
    ("satellite", "satellite"),
    ("earth station", "satellite"),
    ("aes", "satellite"),
    ("esim", "satellite"),
    ("esomps", "satellite"),
    ("esv", "satellite"),
    ("feeder link", "satellite"),
    ("hest", "satellite"),
    ("ngso fss", "satellite"),
    ("mss", "satellite"),
    ("vsat", "satellite"),
    ("gnss", "navigation"),
    ("galileo", "navigation"),
    ("glonass", "navigation"),
    ("gps", "navigation"),
    ("navigation", "navigation"),
    ("beacon", "navigation"),
    ("bbdr", "navigation"),
    ("eurobalise", "navigation"),
    ("euroloop", "navigation"),
    ("gbsar", "navigation"),
    ("mbr", "navigation"),
    ("lpr", "navigation"),
    ("radiolocation", "navigation"),
    ("radiodetermination", "navigation"),
    ("radar", "navigation"),
    ("srr", "navigation"),
    ("tlpr", "navigation"),
    ("ttt", "navigation"),
    ("astronomy", "science"),
    ("meteorolog", "science"),
    ("lightning", "science"),
    ("research", "science"),
    ("sensor", "science"),
    ("sondes", "science"),
    ("time signal", "science"),
    ("wind profiler", "science"),
    ("ism", "ism"),
    ("inductive", "ism"),
    ("alarm", "ism"),
    ("lp-ami", "ism"),
    ("mbans", "ism"),
    ("meter reading", "ism"),
    ("model control", "ism"),
    ("rfid", "ism"),
    ("rlan", "ism"),
    ("srd", "ism"),
    ("ulp-", "ism"),
    ("uwb", "ism"),
    ("wideband data", "ism"),
    ("wia", "ism"),
    ("cb radio", "mobile"),
    ("dect", "mobile"),
    ("ald", "mobile"),
    ("als", "mobile"),
    ("audio pmse", "mobile"),
    ("bfwa", "mobile"),
    ("emergency detection", "mobile"),
    ("fwa", "mobile"),
    ("gsm", "mobile"),
    ("its", "mobile"),
    ("m2m", "mobile"),
    ("mca", "mobile"),
    ("mcv", "mobile"),
    ("mfcn", "mobile"),
    ("mobile", "mobile"),
    ("meteor scatter", "mobile"),
    ("on-board communications", "mobile"),
    ("on-site paging", "mobile"),
    ("pmr", "mobile"),
    ("pmse", "mobile"),
    ("ppdr", "mobile"),
    ("radio microphone", "mobile"),
    ("railway", "mobile"),
    ("rmr", "mobile"),
    ("sar (communications)", "mobile"),
    ("tetra", "mobile"),
    ("telemetry", "mobile"),
    ("tracking", "mobile"),
    ("uas", "mobile"),
    ("video pmse", "mobile"),
    ("wireless", "mobile"),
    ("fm sound", "broadcast"),
    ("gbas", "aeronautical"),
    ("altimeter", "aeronautical"),
    ("mls", "aeronautical"),
    ("waic", "aeronautical"),
    ("military", "other"),
    ("defence", "other"),
    ("fixed", "other"),
    ("point-to-point", "other"),
];

pub(super) fn parse(input: &str) -> Result<Vec<Row>> {
    let main = input
        .trim_start_matches('\u{feff}')
        .split('\u{feff}')
        .next()
        .unwrap_or_default();
    let records = csv(main)?;
    let Some(header) = records.first() else {
        bail!("empty ECA CSV");
    };
    let lower = column(header, "Lower Frequency")?;
    let upper = column(header, "Upper Frequency")?;
    let allocation = column(header, "European Common Allocation and ECA footnotes")?;
    let range_notes = column(header, "ECA frequency range footnotes")?;
    let measure = column(header, "ECC/ERC harmonisation measure")?;
    let application = column(header, "Applications")?;
    let standard = column(header, "Standard")?;
    let source_notes = column(header, "Notes")?;
    let mut rows = Vec::new();
    let mut unmapped = Vec::new();

    for record in &records[1..] {
        if record.len() != header.len() {
            continue;
        }
        let (Some(start_hz), Some(stop_hz)) = (hertz(&record[lower]), hertz(&record[upper])) else {
            continue;
        };
        if stop_hz <= start_hz {
            continue;
        }
        let range = format!("{}–{}", record[lower], record[upper]);
        for mention in fcc::mentions(&record[allocation]) {
            let name = mention.text.to_string();
            rows.push(Row {
                primary: name
                    .chars()
                    .filter(|c| c.is_alphabetic())
                    .all(char::is_uppercase),
                reference: Some(format!("{range} — {name}")),
                notes: note(&[("ECA", &record[range_notes])]),
                ..Row::new(start_hz, stop_hz, mention.service, name)
            });
        }
        let name = record[application].trim();
        if !name.is_empty() && name != "-" {
            rows.push(Row {
                primary: false,
                reference: Some(format!("{range} — {name}")),
                notes: note(&[
                    ("Measure", &record[measure]),
                    ("Standard", &record[standard]),
                    ("Note", &record[source_notes]),
                ]),
                ..Row::new(
                    start_hz,
                    stop_hz,
                    service_of(name, APPLICATIONS, &mut unmapped),
                    name.to_string(),
                )
            });
        }
    }
    fcc::sort_and_dedup(&mut rows);
    report_unmapped("cept", &unmapped);
    if rows.is_empty() {
        bail!("no ECA allocations parsed");
    }
    Ok(rows)
}

fn csv(input: &str) -> Result<Vec<Vec<String>>> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = input.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                field.push('"');
            }
            '"' => quoted = !quoted,
            ';' if !quoted => record.push(std::mem::take(&mut field)),
            '\n' if !quoted => {
                record.push(std::mem::take(&mut field));
                if record.iter().any(|value| !value.is_empty()) {
                    records.push(std::mem::take(&mut record));
                } else {
                    record.clear();
                }
            }
            '\r' if !quoted => {}
            other => field.push(other),
        }
    }
    if quoted {
        bail!("unterminated quoted field in ECA CSV");
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    Ok(records)
}

fn column(header: &[String], name: &str) -> Result<usize> {
    header
        .iter()
        .position(|value| value == name)
        .ok_or_else(|| anyhow::anyhow!("ECA CSV has no {name:?} column"))
}

fn hertz(value: &str) -> Option<f64> {
    let mut parts = value.split_whitespace();
    let number = parts.next()?.replace(',', "").parse::<f64>().ok()?;
    let scale = match parts.next()? {
        "Hz" => 1.0,
        "kHz" => 1e3,
        "MHz" => 1e6,
        "GHz" => 1e9,
        _ => return None,
    };
    Some((number * scale).round())
}

fn note(parts: &[(&str, &str)]) -> Option<String> {
    let joined = parts
        .iter()
        .filter(|(_, value)| !value.trim().is_empty())
        .map(|(label, value)| format!("{label}: {}", value.trim()))
        .collect::<Vec<_>>()
        .join(". ");
    if joined.is_empty() {
        return None;
    }
    let mut chars = joined.chars();
    let bounded: String = chars.by_ref().take(400).collect();
    if chars.next().is_none() {
        return Some(bounded);
    }
    let end = bounded.rfind(' ').unwrap_or(bounded.len());
    Some(format!("{}…", &bounded[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../fixtures/bandplan/cept-eca-excerpt.csv");

    #[test]
    fn reads_allocations_and_applications_from_the_efis_csv() {
        let rows = parse(FIXTURE).expect("parse");
        assert!(
            rows.iter()
                .any(|row| row.name == "METEOROLOGICAL AIDS" && row.primary)
        );
        assert!(
            rows.iter()
                .any(|row| row.name == "RADIONAVIGATION" && row.primary)
        );
        assert!(
            rows.iter()
                .any(|row| row.name == "Inductive applications" && !row.primary)
        );
    }

    #[test]
    fn applies_units_and_preserves_secondary_capitalisation() {
        let rows = parse(FIXTURE).expect("parse");
        let amateur = rows
            .iter()
            .find(|row| row.name == "Amateur")
            .expect("amateur");
        assert_eq!(amateur.start_hz, 135_700.0);
        assert_eq!(amateur.stop_hz, 137_800.0);
        assert!(!amateur.primary);
    }

    #[test]
    fn handles_semicolons_quotes_and_embedded_newlines() {
        let rows = parse(FIXTURE).expect("parse");
        let app = rows
            .iter()
            .find(|row| row.name == "Inductive applications")
            .expect("application");
        let notes = app.notes.as_deref().expect("notes");
        assert!(notes.contains("EN 300 330; EN 303 447"));
        assert!(notes.contains("quoted \"value\""));
        assert!(notes.contains("second line"));
    }

    #[test]
    fn stops_before_the_auxiliary_footnote_tables() {
        let rows = parse(FIXTURE).expect("parse");
        assert!(rows.iter().all(|row| !row.name.contains("Not used")));
    }

    #[test]
    fn rejects_an_unrecognisable_csv() {
        assert!(parse("a;b\n1;2\n").is_err());
    }
}
