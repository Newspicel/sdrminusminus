use anyhow::{Context, Result};
use serde::Deserialize;

use super::{Row, Target, report_unmapped, service_of};

pub(super) static TARGET: &Target = &Target {
    id: "gb",
    name: "United Kingdom — Ofcom",
    authority: "Ofcom",
    kind: "regulatory",
};

#[derive(Deserialize)]
struct Document {
    bands: Vec<Band>,
    #[serde(default)]
    footnotes: Vec<Footnote>,
}

#[derive(Deserialize)]
struct Band {
    id: String,
    cat: String,
    lf: f64,
    uf: f64,
    s: String,
}

#[derive(Deserialize)]
struct Footnote {
    id: String,
    cid: String,
    t: String,
}

static SERVICES: &[(&str, &str)] = &[
    ("amateur", "amateur"),
    ("broadcasting", "broadcast"),
    ("aeronautical mobile", "aeronautical"),
    ("aeronautical radionavigation", "aeronautical"),
    ("maritime mobile", "maritime"),
    ("maritime radionavigation", "maritime"),
    ("land mobile", "mobile"),
    ("mobile-satellite", "satellite"),
    ("radionavigation-satellite", "navigation"),
    ("radiodetermination-satellite", "navigation"),
    ("fixed-satellite", "satellite"),
    ("broadcasting-satellite", "satellite"),
    ("meteorological-satellite", "satellite"),
    ("inter-satellite", "satellite"),
    ("space operation", "satellite"),
    ("space research", "science"),
    ("earth exploration", "science"),
    ("radio astronomy", "science"),
    ("meteorological aids", "science"),
    ("standard frequency and time signal", "science"),
    ("radiolocation", "navigation"),
    ("radionavigation", "navigation"),
    ("mobile", "mobile"),
    ("fixed", "other"),
];

pub(super) fn parse(input: &str) -> Result<Vec<Row>> {
    let doc: Document = serde_json::from_str(input).context("fatMapping.json")?;
    let mut unmapped = Vec::new();

    let mut rows: Vec<Row> = doc
        .bands
        .iter()
        .filter(|band| band.uf > band.lf)
        .map(|band| Row {
            primary: band.cat != "s",
            reference: Some(band.id.clone()),
            notes: notes_for(&band.id, &doc.footnotes),
            ..Row::new(
                band.lf,
                band.uf,
                service_of(&band.s, SERVICES, &mut unmapped),
                band.s.clone(),
            )
        })
        .collect();

    rows.sort_by(|a, b| a.start_hz.total_cmp(&b.start_hz));
    report_unmapped("ofcom", &unmapped);
    Ok(rows)
}

fn notes_for(id: &str, footnotes: &[Footnote]) -> Option<String> {
    const LIMIT: usize = 400;
    let joined = footnotes
        .iter()
        .filter(|note| note.id == id)
        .map(|note| format!("{}: {}", note.cid, note.t.trim()))
        .collect::<Vec<_>>()
        .join(" ");
    if joined.is_empty() {
        return None;
    }
    if joined.chars().count() <= LIMIT {
        return Some(joined);
    }
    let cut: String = joined.chars().take(LIMIT).collect();
    let end = cut.rfind(' ').unwrap_or(cut.len());
    Some(format!("{}…", &cut[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../fixtures/bandplan/ofcom-fat-excerpt.json");

    #[test]
    fn reads_ranges_services_and_the_row_id_that_is_its_provenance() {
        let rows = parse(FIXTURE).expect("parse");
        let first = &rows[0];
        assert_eq!(first.start_hz, 8_300.0);
        assert_eq!(first.stop_hz, 9_000.0);
        assert_eq!(first.name, "Meteorological Aids");
        assert_eq!(first.service, "science");
        assert_eq!(first.reference.as_deref(), Some("FREQ_00001"));
    }

    #[test]
    fn keeps_every_service_sharing_one_range() {
        let rows = parse(FIXTURE).expect("parse");
        let shared: Vec<&Row> = rows
            .iter()
            .filter(|row| row.start_hz == 9_000.0 && row.stop_hz == 11_300.0)
            .collect();
        assert_eq!(
            shared.len(),
            2,
            "9–11.3 kHz is met-aids and radionavigation"
        );
        assert!(shared.iter().any(|row| row.service == "science"));
        assert!(shared.iter().any(|row| row.service == "navigation"));
    }

    #[test]
    fn carries_the_primary_secondary_distinction() {
        let rows = parse(FIXTURE).expect("parse");
        assert!(rows.iter().any(|row| row.primary));
        assert!(
            rows.iter().any(|row| !row.primary),
            "the excerpt includes a secondary allocation"
        );
    }

    #[test]
    fn attaches_footnote_text_as_the_note_and_bounds_it() {
        let rows = parse(FIXTURE).expect("parse");
        let noted = rows
            .iter()
            .find(|row| row.reference.as_deref() == Some("FREQ_00001"))
            .expect("first row");
        let notes = noted.notes.as_deref().expect("footnote 5.54A");
        assert!(notes.starts_with("5.54A: "));
        assert!(
            notes.chars().count() <= 401,
            "a note is bounded for a popover"
        );
    }

    #[test]
    fn drops_a_zero_width_band_rather_than_emitting_an_undrawable_block() {
        let rows = parse(r#"{"bands":[{"id":"X","cat":"p","lf":100,"uf":100,"s":"Fixed"}]}"#)
            .expect("parse");
        assert!(rows.is_empty());
    }

    #[test]
    fn rows_come_out_sorted_so_the_loader_can_assume_it() {
        let rows = parse(FIXTURE).expect("parse");
        assert!(rows.windows(2).all(|p| p[1].start_hz >= p[0].start_hz));
    }
}
