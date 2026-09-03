use std::collections::BTreeMap;

use anyhow::{Result, bail};

use super::{Import, Provision, Row, Target, report_unmapped, service_of};

static TARGET: &Target = &Target {
    id: "de",
    name: "Germany — BNetzA",
    authority: "Bundesnetzagentur",
    kind: "regulatory",
};

static ANNEX_TARGET: &Target = &Target {
    id: "de-sonstige",
    name: "Germany — other applications",
    authority: "Bundesnetzagentur",
    kind: "application",
};

static SERVICES: &[(&str, &str)] = &[
    ("(nicht zugewiesen)", "other"),
    ("funknachrichten", "mobile"),
    ("erde)", "satellite"),
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
    ("tankradare", "ism"),
    ("uwb", "ism"),
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
    ("eisenbahnen", "mobile"),
    ("grubenfunk", "mobile"),
    ("lawinenverschütteten", "mobile"),
    ("srd", "ism"),
    ("geringer reichweite", "ism"),
    ("wlan", "ism"),
    ("mgws", "ism"),
    ("ism", "ism"),
    ("induktive funkanwendungen", "ism"),
    ("infrarot", "ism"),
    ("gesundheitsbereich", "ism"),
    ("funkmikrofone", "ism"),
    ("drahtlose kameras", "ism"),
    ("ortsfeste tonübertragung", "ism"),
    ("hörhilfen", "ism"),
    ("fernsteuerung", "ism"),
    ("demonstrationsfunk", "ism"),
    ("fester funkdienst", "other"),
    ("festfunk", "other"),
];

static LABELS: &[&str] = &[
    "Frequenzteilplan",
    "Eintrag",
    "Stand",
    "Frequenzbereich",
    "Nutzungsbestimmung(en)",
    "Funkdienst",
    "Nutzung",
    "Frequenznutzung",
    "Frequenzteilbereich(e)",
    "Frequenzteilbereich",
    "bedingungen",
];

static RANGE_LABELS: &[&str] = &[
    "Frequenzteilbereich(e)",
    "Frequenzteilbereich",
    "Frequenzbereich",
];

const ANNEX: &str = "Sonstige Funkanwendungen und andere Anwendungen elektromagnetischer Wellen";

const PROVISIONS: &str = "Zitierte Nutzungsbestimmungen";
const ABBREVIATIONS: &str = "Abkürzungsverzeichnis";

struct Record {
    text: String,
    group: Option<String>,
}

pub(super) fn parse(layout: &str, spaced: &str) -> Result<Vec<Import>> {
    let provisions = provisions(layout);
    let cited = citations(spaced);
    let mut plan = Vec::new();
    let mut annex = Vec::new();
    let mut unmapped = Vec::new();
    let mut unknown = Vec::new();

    for record in records(layout) {
        let reference = field(&record.text, "Eintrag").or_else(|| record.group.clone());
        let Some(reference) = reference.filter(|value| !value.is_empty()) else {
            continue;
        };
        let Some(range) = RANGE_LABELS
            .iter()
            .find_map(|label| field(&record.text, label).filter(|value| !value.is_empty()))
        else {
            continue;
        };
        let (service_name, service_cited) = named(&record.text, "Funkdienst");
        let (usage, usage_cited) = named(&record.text, "Frequenznutzung");
        let Some(name) = service_name.clone().or_else(|| usage.clone()) else {
            continue;
        };

        let mut refs: Vec<String> = cited.get(&reference).cloned().unwrap_or_default();
        for found in [service_cited, usage_cited].into_iter().flatten() {
            if !refs.contains(&found) {
                refs.push(found);
            }
        }
        for found in &refs {
            if !provisions.iter().any(|known| known.id == *found) && !unknown.contains(found) {
                unknown.push(found.clone());
            }
        }

        let rows = if record.group.is_some() {
            &mut annex
        } else {
            &mut plan
        };
        for (start_hz, stop_hz) in ranges(&range) {
            rows.push(Row {
                primary: service_name
                    .as_deref()
                    .is_some_and(|name| name == name.to_uppercase()),
                reference: Some(reference.clone()),
                channel_step_hz: field(&record.text, "Kanalraster")
                    .and_then(|value| steps(&value).first().copied()),
                notes: notes(&record.text, usage.as_deref()),
                provisions: refs.clone(),
                ..Row::new(
                    start_hz,
                    stop_hz,
                    service_of(&name, SERVICES, &mut unmapped),
                    name.clone(),
                )
            });
        }
    }

    report_unmapped("bnetza", &unmapped);
    if !unknown.is_empty() {
        println!(
            "bnetza: {} cited Nutzungsbestimmung(en) the Frequenzverordnung extract does not \
             define: {}",
            unknown.len(),
            unknown.join(", ")
        );
    }
    if plan.is_empty() || provisions.is_empty() {
        bail!("no records parsed — the Frequenzplan's layout has changed");
    }
    if annex.is_empty() {
        bail!("the annex of other applications is missing — its layout has changed");
    }

    plan.sort_by(|a, b| a.start_hz.total_cmp(&b.start_hz));
    let mut annex = merged(annex);
    annex.sort_by(|a, b| a.start_hz.total_cmp(&b.start_hz));
    Ok(vec![
        Import {
            target: TARGET,
            rows: plan,
            provisions,
        },
        Import {
            target: ANNEX_TARGET,
            rows: annex,
            provisions: Vec::new(),
        },
    ])
}

fn merged(rows: Vec<Row>) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::with_capacity(rows.len());
    for row in rows {
        if let Some(seen) = out.iter_mut().find(|seen| {
            seen.reference == row.reference
                && seen.name == row.name
                && seen.start_hz == row.start_hz
                && seen.stop_hz == row.stop_hz
        }) {
            if let (Some(kept), Some(extra)) = (seen.notes.as_mut(), row.notes.as_deref())
                && !kept.contains(extra)
            {
                kept.push(' ');
                kept.push_str(extra);
            }
            continue;
        }
        out.push(row);
    }
    out
}

fn records(input: &str) -> Vec<Record> {
    let mut out = Vec::new();
    let mut current: Option<(Vec<&str>, Option<String>)> = None;
    let mut annex = false;
    let mut group: Option<String> = None;

    for line in input.lines() {
        let trimmed = collapsed(line);
        let trimmed = trimmed.as_str();
        if trimmed == PROVISIONS {
            break;
        }
        if current.is_some() && trimmed.starts_with(ANNEX) {
            annex = true;
            continue;
        }
        if annex && let Some(name) = heading(trimmed) {
            group = Some(name);
            continue;
        }
        let opens = if annex {
            trimmed.starts_with("Frequenznutzung:")
        } else {
            line.contains("Frequenzteilplan:") && line.contains("Eintrag:")
        };
        if opens {
            close(&mut out, current.take());
            current = Some((vec![line], group.clone()));
        } else if let Some((lines, _)) = current.as_mut() {
            lines.push(line);
        }
    }
    close(&mut out, current);
    out
}

fn close(out: &mut Vec<Record>, current: Option<(Vec<&str>, Option<String>)>) {
    if let Some((lines, group)) = current {
        out.push(Record {
            text: lines.join("\n"),
            group,
        });
    }
}

fn heading(line: &str) -> Option<String> {
    let inner = line.strip_prefix("- ")?.strip_suffix(" -")?.trim();
    (!inner.is_empty()).then(|| inner.to_string())
}

fn citations(spaced: &str) -> BTreeMap<String, Vec<String>> {
    records(spaced)
        .into_iter()
        .filter_map(|record| {
            let reference = field(&record.text, "Eintrag")?;
            let cited = field(&record.text, "Nutzungsbestimmung(en)")?;
            Some((
                reference,
                cited.split_whitespace().map(str::to_string).collect(),
            ))
        })
        .collect()
}

fn provisions(input: &str) -> Vec<Provision> {
    let mut out: Vec<Provision> = Vec::new();
    let mut inside = false;
    for line in input.lines() {
        let line = line.trim_start_matches('\u{c}');
        let trimmed = line.trim();
        if trimmed == PROVISIONS {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if trimmed == ABBREVIATIONS {
            break;
        }
        if let Some((id, text)) = opens_provision(line) {
            out.push(Provision {
                id: id.to_string(),
                text: text.to_string(),
            });
        } else if let Some(open) = out.last_mut()
            && indent(line) > 0
            && !trimmed.is_empty()
        {
            join(&mut open.text, trimmed);
        }
    }
    for provision in &mut out {
        provision.text = collapsed(&provision.text);
    }
    out
}

fn opens_provision(line: &str) -> Option<(&str, &str)> {
    let (id, rest) = line.split_once(char::is_whitespace)?;
    is_reference(id).then(|| (id, rest.trim()))?;
    let text = rest.trim();
    (!text.is_empty()).then_some((id, text))
}

fn join(text: &mut String, next: &str) {
    match text.strip_suffix('-') {
        Some(head) if next.starts_with(char::is_lowercase) => {
            let kept = head.to_string();
            text.clear();
            text.push_str(&kept);
        }
        Some(_) => {}
        None => text.push(' '),
    }
    text.push_str(next);
}

fn collapsed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn named(record: &str, label: &str) -> (Option<String>, Option<String>) {
    let Some(value) = field(record, label) else {
        return (None, None);
    };
    let (name, cited) = match value.split_once(':') {
        Some((head, rest)) if is_reference(head) => {
            (rest.trim().to_string(), Some(head.trim().to_string()))
        }
        _ => (value.trim().to_string(), None),
    };
    if name.is_empty() {
        return (None, None);
    }
    (Some(name), cited)
}

fn is_reference(text: &str) -> bool {
    let text = text.trim();
    let digits = text
        .strip_prefix('D')
        .unwrap_or(text)
        .trim_end_matches(|c: char| c.is_ascii_uppercase());
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
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
            let column = line[..at].chars().count() + needle.chars().count() + indent(rest);
            for continuation in record.lines().skip(index + 1) {
                let trimmed = continuation.trim();
                if trimmed.is_empty()
                    || indent(continuation) < column
                    || next_label(continuation).is_some()
                {
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

fn indent(line: &str) -> usize {
    line.chars().take_while(|c| c.is_whitespace()).count()
}

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

fn next_label(text: &str) -> Option<usize> {
    LABELS
        .iter()
        .filter_map(|label| label_at(text, &format!("{label}:")))
        .min()
}

fn ranges(value: &str) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for part in value.replace(['–', '—', '−'], "-").split(';') {
        let Some((lhs, rhs)) = part.split_once('-') else {
            continue;
        };
        let Some(high_unit) = unit(rhs) else { continue };
        let low_unit = unit(lhs).unwrap_or(high_unit);
        let (Some(low), Some(high)) = (german(lhs), german(rhs)) else {
            continue;
        };
        let (low, high) = (low * low_unit, high * high_unit);
        if high > low {
            out.push((low, high));
        }
    }
    out
}

fn german(text: &str) -> Option<f64> {
    let cleaned: String = text
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
        .collect();
    if cleaned.contains(',') {
        cleaned.replace('.', "").replace(',', ".").parse().ok()
    } else if grouped(&cleaned) {
        cleaned.replace('.', "").parse().ok()
    } else {
        cleaned.parse().ok()
    }
}

fn grouped(text: &str) -> bool {
    let mut parts = text.split('.');
    parts
        .next()
        .is_some_and(|head| !head.is_empty() && head.len() <= 3)
        && parts.all(|group| group.len() == 3)
}

fn unit(text: &str) -> Option<f64> {
    text.split_whitespace().last().and_then(scale)
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

fn steps(value: &str) -> Vec<f64> {
    let tokens: Vec<&str> = value.split_whitespace().collect();
    tokens
        .windows(2)
        .filter_map(|pair| Some(german(pair[0])? * scale(pair[1])?))
        .collect()
}

fn notes(record: &str, usage: Option<&str>) -> Option<String> {
    let mut note = format!("{}.", usage?);
    if let Some(bandwidth) = field(record, "Kanalbandbreite").filter(|v| !v.is_empty()) {
        note.push_str(&format!(" Kanalbandbreite: {bandwidth}."));
    }
    if let Some(raster) = field(record, "Kanalraster").filter(|v| steps(v).len() > 1) {
        note.push_str(&format!(" Kanalraster: {raster}."));
    }
    Some(note)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../fixtures/bandplan/bnetza-excerpt.txt");
    const SPACED: &str = include_str!("../../../fixtures/bandplan/bnetza-spaced-excerpt.txt");

    fn layers() -> Vec<Import> {
        parse(FIXTURE, SPACED).expect("parse")
    }

    fn plan() -> Vec<Row> {
        layers().swap_remove(0).rows
    }

    fn annex() -> Vec<Row> {
        layers().swap_remove(1).rows
    }

    fn row(rows: &[Row], reference: &str) -> Row {
        rows.iter()
            .find(|row| row.reference.as_deref() == Some(reference))
            .unwrap_or_else(|| panic!("{reference}"))
            .clone()
    }

    #[test]
    fn reads_a_record_into_a_row_with_its_eintrag_as_provenance() {
        let broadcast = row(&plan(), "20001");
        assert_eq!(broadcast.start_hz, 148_500.0);
        assert_eq!(broadcast.stop_hz, 255_000.0);
        assert_eq!(broadcast.name, "RUNDFUNKDIENST");
        assert_eq!(broadcast.service, "broadcast");
        assert!(broadcast.primary, "a capitalised service is primary");
        assert_eq!(broadcast.channel_step_hz, Some(9_000.0));
    }

    #[test]
    fn keeps_every_record_that_shares_one_range() {
        let rows = plan();
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
        let srd = row(&plan(), "27003");
        assert!(srd.name.contains("SRD"));
        assert_eq!(srd.service, "ism");
        assert!(!srd.primary, "a non-ITU usage is not a primary allocation");
        assert_eq!(srd.start_hz, 442_200.0);
        assert_eq!(srd.stop_hz, 450_000.0);
    }

    #[test]
    fn a_usage_that_cites_a_nutzungsbestimmung_keeps_only_its_name() {
        let rows = plan();
        let military = row(&rows, "3001");
        assert_eq!(military.name, "Militärische Funkanwendungen");
        assert_eq!(military.service, "mobile");
        assert_eq!(military.start_hz, 9_000.0);

        let land = row(&rows, "80003");
        assert_eq!(land.name, "Mobiler Landfunkdienst");
        assert_eq!(land.service, "mobile");
        assert_eq!(land.notes.as_deref(), Some("Militärische Funkanwendungen."));
    }

    #[test]
    fn a_decimal_point_where_the_plan_means_a_comma_still_reads_as_one_frequency() {
        let astronomy = row(&plan(), "488005");
        assert_eq!(astronomy.name, "Radioastronomie");
        assert_eq!(astronomy.start_hz, 171_110_000_000.0);
        assert_eq!(astronomy.stop_hz, 171_450_000_000.0);
    }

    #[test]
    fn a_record_without_a_range_to_draw_is_left_out() {
        let rows = plan();
        assert!(
            rows.iter()
                .all(|row| row.reference.as_deref() != Some("511001")),
            "an unbounded Frequenzbereich carries no range to draw"
        );
        assert!(
            rows.iter()
                .all(|row| row.start_hz != 8_300.0 || row.stop_hz != 30_000_000.0),
            "the last record must not swallow the annex through its range fallback"
        );
    }

    #[test]
    fn a_channel_raster_stops_at_the_page_footer_and_reports_its_first_step() {
        let trunked = row(&plan(), "248098");
        assert_eq!(trunked.channel_step_hz, Some(6_250.0));
        assert_eq!(
            trunked.notes.as_deref(),
            Some(
                "Betriebsfunk. Kanalbandbreite: 6,25 kHz / 12,5 kHz / 20 kHz. \
                 Kanalraster: 6,25 kHz / 12,5 kHz / 20 kHz."
            )
        );
    }

    #[test]
    fn the_annex_of_other_applications_is_its_own_layer() {
        let layers = layers();
        assert_eq!(layers[0].target.id, "de");
        assert_eq!(layers[1].target.id, "de-sonstige");
        assert!(layers[1].provisions.is_empty());

        let rows = annex();
        assert!(rows.iter().all(|row| !row.primary));
        assert!(
            plan().iter().all(|row| !row.name.contains("Induktive")),
            "the annex allocates nothing; it belongs to no allocation table"
        );

        let railway: Vec<&Row> = rows
            .iter()
            .filter(|row| row.reference.as_deref() == Some("Funkanwendungen der Eisenbahnen"))
            .collect();
        assert_eq!(railway.len(), 2);
        assert_eq!(
            railway[0].start_hz, 36_000.0,
            "an en dash separates a range"
        );
        assert_eq!(railway[0].stop_hz, 875_000.0);

        let uwb = row(&rows, "UWB - Funkanwendungen");
        assert_eq!(uwb.name, "UWB-Funkanwendungen");
        assert_eq!(uwb.service, "ism");
        assert_eq!(uwb.start_hz, 9_000.0, "each end carries its own unit");
        assert_eq!(uwb.stop_hz, 30_000_000.0);
    }

    #[test]
    fn a_record_carries_the_nutzungsbestimmungen_it_cites() {
        let rows = plan();
        assert_eq!(row(&rows, "20001").provisions, ["2", "5"]);
        assert_eq!(
            row(&rows, "27004").provisions,
            ["1", "2", "5"],
            "the plan prints these as separate numbers, not as one"
        );
        assert_eq!(row(&rows, "3001").provisions, ["D150", "2", "3", "5"]);
        assert_eq!(
            row(&rows, "80003").provisions,
            ["D134", "D136", "2", "3", "5"],
            "the Funkdienst cites D136, which the list already carries"
        );
        assert_eq!(row(&rows, "488005").provisions, ["D149", "5", "31"]);
        assert_eq!(
            row(&annex(), "Induktive Funkanwendungen").provisions,
            ["2"],
            "the annex cites through the name it prints"
        );
    }

    #[test]
    fn reads_the_cited_nutzungsbestimmungen_and_rejoins_their_broken_words() {
        let provisions = layers().swap_remove(0).provisions;
        let text = |id: &str| {
            provisions
                .iter()
                .find(|provision| provision.id == id)
                .unwrap_or_else(|| panic!("{id}"))
                .text
                .clone()
        };
        assert!(text("D54A").starts_with("Die Nutzung des Frequenzbereichs 8,3 – 11,3 kHz"));
        assert!(
            text("D54A").contains("Funkstellen des Navigationsfunkdienstes"),
            "a word broken over two lines is one word again"
        );
        assert!(
            text("D130").starts_with("Die Trägerfrequenzen"),
            "a provision that opens a page is still a provision"
        );
        assert!(
            text("D130").contains("Not- und Sicherheitsverkehr"),
            "a hyphen the sentence needs is kept"
        );
        assert!(
            text("D132A").starts_with("Funkstellen des nichtnavigatorischen"),
            "a long label leaves only one space before its text"
        );
        assert_eq!(text("39"), "nicht genutzt");
        assert!(
            !text("D150").contains("Bundesnetzagentur"),
            "the page footer is not part of the provision"
        );
    }

    #[test]
    fn reads_german_decimal_commas() {
        assert_eq!(german("148,5"), Some(148.5));
        assert_eq!(german("  9 "), Some(9.0));
        assert_eq!(german("12,5 kHz"), Some(12.5));
        assert_eq!(german("30.000"), Some(30_000.0));
        assert_eq!(german("171.11"), Some(171.11));
    }

    #[test]
    fn applies_the_unit_to_both_ends_of_a_range() {
        assert_eq!(ranges("8,3 - 9 kHz"), vec![(8_300.0, 9_000.0)]);
        assert_eq!(
            ranges("27,5 - 10000 MHz"),
            vec![(27_500_000.0, 10_000_000_000.0)]
        );
        assert_eq!(ranges("9 kHz - 30 MHz"), vec![(9_000.0, 30_000_000.0)]);
        assert_eq!(ranges("36 – 875 kHz"), vec![(36_000.0, 875_000.0)]);
        assert!(ranges("siehe Anhang").is_empty());
    }

    #[test]
    fn an_unrecognisable_document_fails_loudly() {
        assert!(parse("this is not the Frequenzplan", "").is_err());
    }

    #[test]
    fn rows_come_out_sorted() {
        for layer in layers() {
            assert!(
                layer
                    .rows
                    .windows(2)
                    .all(|pair| pair[1].start_hz >= pair[0].start_hz)
            );
        }
    }
}
