//! The UK Frequency Allocation Table, from Ofcom's own JSON.
//!
//! The easy source, and the one that shows what the others are being parsed *into*: it already
//! has a row id, a primary/secondary flag, frequencies in Hz, and footnote text keyed by row.
//!
//! Source: <https://static.ofcom.org.uk/static/spectrum/data/fatMapping.json>, published as open
//! data via data.gov.uk under a permissive grant carried in the document itself.

use anyhow::{Context, Result};
use serde::Deserialize;

use super::{Row, Target, report_unmapped, service_of};

pub(super) static TARGET: &Target = &Target {
    id: "gb",
    name: "United Kingdom — Ofcom",
    authority: "Ofcom",
    kind: "regulatory",
};

/// Ofcom's own field names, kept verbatim so the mapping to ours is visible in one place.
#[derive(Deserialize)]
struct Document {
    bands: Vec<Band>,
    #[serde(default)]
    footnotes: Vec<Footnote>,
}

#[derive(Deserialize)]
struct Band {
    id: String,
    /// `p` primary, `s` secondary.
    cat: String,
    /// Lower and upper frequency, in Hz.
    lf: f64,
    uf: f64,
    /// Service name.
    s: String,
}

#[derive(Deserialize)]
struct Footnote {
    /// The band `id` it belongs to.
    id: String,
    /// The footnote's own identifier, e.g. `5.54A`.
    cid: String,
    t: String,
}

/// Ofcom writes ITU service names in title case, which makes them a short and stable table.
/// Ordered: the first substring to match wins, so the qualified names come before the bare ones.
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
        // A zero-width band is a data artefact, not an allocation, and would resolve into a
        // block nothing can be drawn in.
        .filter(|band| band.uf > band.lf)
        .map(|band| Row {
            // Secondary is the exception and the flag carries real meaning: a secondary service
            // has to accept interference from every primary one.
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

/// Ofcom attaches ITU and UK footnotes per row. They are the only prose the document has, and
/// they are what an operator actually wants to read at a frequency, so they become the note —
/// truncated, because some of them run to a page and a popover is not a document viewer.
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
    // Break on a word so the ellipsis does not land mid-word.
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
        // 8.3–9 kHz in Hz: the document's units are Hz, and reading them as kHz would put every
        // UK allocation a thousand times too high.
        assert_eq!(first.start_hz, 8_300.0);
        assert_eq!(first.stop_hz, 9_000.0);
        assert_eq!(first.name, "Meteorological Aids");
        assert_eq!(first.service, "science");
        assert_eq!(first.reference.as_deref(), Some("FREQ_00001"));
    }

    /// The property that forced the model change: one range, several services, all kept.
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
