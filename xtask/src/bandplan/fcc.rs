use anyhow::{Result, bail};

use super::{Row, Target};

pub(super) static ITU_R1: &Target = &Target {
    id: "itu-r1",
    name: "ITU Region 1",
    authority: "ITU",
    kind: "world",
};
pub(super) static ITU_R2: &Target = &Target {
    id: "itu-r2",
    name: "ITU Region 2",
    authority: "ITU",
    kind: "world",
};
pub(super) static ITU_R3: &Target = &Target {
    id: "itu-r3",
    name: "ITU Region 3",
    authority: "ITU",
    kind: "world",
};
pub(super) static FCC: &Target = &Target {
    id: "us",
    name: "United States — FCC",
    authority: "FCC",
    kind: "regulatory",
};

#[derive(Clone)]
struct Line {
    x: f64,
    y: f64,
    text: String,
}

struct ColumnLine {
    scale: f64,
    text: String,
}

struct Band {
    start_hz: f64,
    stop_hz: f64,
    label: String,
    content: String,
}

pub(super) struct Mention<'a> {
    start: usize,
    stop: usize,
    pub(super) service: &'static str,
    pub(super) text: &'a str,
}

static SERVICES: &[(&str, &str)] = &[
    ("standard frequency and time signal-satellite", "science"),
    ("standard frequency and time signal", "science"),
    ("aeronautical mobile-satellite (or)", "satellite"),
    ("aeronautical mobile-satellite (r)", "satellite"),
    ("maritime mobile-satellite", "satellite"),
    ("radionavigation-satellite", "navigation"),
    ("radiodetermination-satellite", "navigation"),
    ("earth exploration-satellite", "science"),
    ("broadcasting-satellite", "satellite"),
    ("meteorological-satellite", "satellite"),
    ("mobile except aeronautical mobile (r)", "mobile"),
    ("mobile except aeronautical mobile", "mobile"),
    ("aeronautical mobile (or)", "aeronautical"),
    ("aeronautical mobile (r)", "aeronautical"),
    ("aeronautical radionavigation", "aeronautical"),
    ("maritime radionavigation", "maritime"),
    ("amateur-satellite", "amateur"),
    ("fixed-satellite", "satellite"),
    ("mobile-satellite", "satellite"),
    ("inter-satellite", "satellite"),
    ("radio astronomy", "science"),
    ("meteorological aids", "science"),
    ("space operation", "satellite"),
    ("space research", "science"),
    ("maritime mobile", "maritime"),
    ("land mobile", "mobile"),
    ("radionavigation", "navigation"),
    ("radiolocation", "navigation"),
    ("broadcasting", "broadcast"),
    ("amateur", "amateur"),
    ("mobile", "mobile"),
    ("fixed", "other"),
];

pub(super) fn parse(input: &str) -> Result<Vec<(&'static Target, Vec<Row>)>> {
    let mut columns: [Vec<ColumnLine>; 5] = std::array::from_fn(|_| Vec::new());
    let mut anchors = None;
    let mut scale = None;
    let mut landscape_pages = 0usize;

    for (attributes, body) in elements(input, "page") {
        let width = attribute(attributes, "width").and_then(|value| value.parse::<f64>().ok());
        let height = attribute(attributes, "height").and_then(|value| value.parse::<f64>().ok());
        if !matches!((width, height), (Some(width), Some(height)) if width > height) {
            continue;
        }
        landscape_pages += 1;
        let mut lines = parse_lines(body);
        lines.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
        let page_anchors = anchors_of(&lines);
        let has_header = page_anchors.is_some();
        if let Some(found) = page_anchors {
            anchors = Some(found);
            scale = scale_of(&lines).or(scale);
        }
        let (Some(anchors), Some(scale)) = (anchors, scale) else {
            continue;
        };
        for line in lines {
            if has_header && line.y < 72.0 {
                continue;
            }
            let column = nearest_anchor(line.x, &anchors);
            if column >= columns.len() {
                continue;
            }
            columns[column].push(ColumnLine {
                scale,
                text: line.text,
            });
        }
    }
    if landscape_pages == 0 || columns.iter().all(Vec::is_empty) {
        bail!("no FCC allocation pages found in bbox-layout output");
    }

    let explicit: [Vec<Row>; 5] = columns.map(|lines| rows(&bands(&lines)));
    let region_1 = explicit[0].clone();
    let region_2 = inherit(&region_1, &explicit[1]);
    let region_3 = inherit(&region_2, &explicit[2]);
    let mut united_states = explicit[3].clone();
    united_states.extend(explicit[4].clone());
    sort_and_dedup(&mut united_states);
    if region_1.is_empty() || region_2.is_empty() || region_3.is_empty() || united_states.is_empty()
    {
        bail!("FCC table columns contained no recognised allocations");
    }
    Ok(vec![
        (ITU_R1, region_1),
        (ITU_R2, region_2),
        (ITU_R3, region_3),
        (FCC, united_states),
    ])
}

fn parse_lines(input: &str) -> Vec<Line> {
    elements(input, "line")
        .into_iter()
        .filter_map(|(attributes, body)| {
            let x = attribute(attributes, "xMin")?.parse().ok()?;
            let y = attribute(attributes, "yMin")?.parse().ok()?;
            let text = elements(body, "word")
                .into_iter()
                .map(|(_, word)| unescape(word))
                .collect::<Vec<_>>()
                .join(" ");
            (!text.is_empty()).then_some(Line { x, y, text })
        })
        .collect()
}

fn elements<'a>(input: &'a str, tag: &str) -> Vec<(&'a str, &'a str)> {
    let open = format!("<{tag} ");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(relative) = input[cursor..].find(&open) {
        let start = cursor + relative + open.len();
        let Some(tag_end) = input[start..].find('>').map(|at| start + at) else {
            break;
        };
        let body_start = tag_end + 1;
        let Some(body_end) = input[body_start..].find(&close).map(|at| body_start + at) else {
            break;
        };
        out.push((&input[start..tag_end], &input[body_start..body_end]));
        cursor = body_end + close.len();
    }
    out
}

fn attribute<'a>(attributes: &'a str, name: &str) -> Option<&'a str> {
    let prefix = format!(r#"{name}=""#);
    let start = attributes.find(&prefix)? + prefix.len();
    let stop = attributes[start..].find('"')? + start;
    Some(&attributes[start..stop])
}

fn unescape(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn anchors_of(lines: &[Line]) -> Option<[f64; 6]> {
    let labels = [
        "Region 1 Table",
        "Region 2 Table",
        "Region 3 Table",
        "Federal Table",
        "Non-Federal Table",
        "FCC Rule Part(s)",
    ];
    let mut anchors = [0.0; 6];
    for (at, label) in labels.iter().enumerate() {
        anchors[at] = lines.iter().find(|line| line.text == *label)?.x;
    }
    Some(anchors)
}

fn scale_of(lines: &[Line]) -> Option<f64> {
    lines
        .iter()
        .filter(|line| line.y < 60.0)
        .flat_map(|line| line.text.split_whitespace())
        .find_map(
            |word| match word.trim_matches(|c: char| !c.is_ascii_alphabetic()) {
                "kHz" => Some(1e3),
                "MHz" => Some(1e6),
                "GHz" => Some(1e9),
                _ => None,
            },
        )
}

fn nearest_anchor(x: f64, anchors: &[f64; 6]) -> usize {
    anchors
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (x - **a).abs().total_cmp(&(x - **b).abs()))
        .map_or(anchors.len(), |(at, _)| at)
}

fn bands(lines: &[ColumnLine]) -> Vec<Band> {
    let mut out = Vec::new();
    let mut current: Option<Band> = None;
    for line in lines {
        if let Some((start_hz, stop_hz, label)) = frequency_range(&line.text, line.scale) {
            if let Some(band) = current.take() {
                out.push(band);
            }
            current = Some(Band {
                start_hz,
                stop_hz,
                label,
                content: String::new(),
            });
        } else if let Some(band) = current.as_mut() {
            if !band.content.is_empty() {
                band.content.push(' ');
            }
            band.content.push_str(&line.text);
        }
    }
    if let Some(band) = current {
        out.push(band);
    }
    out
}

fn frequency_range(text: &str, scale: f64) -> Option<(f64, f64, String)> {
    let token = text.split_whitespace().next()?.replace(['–', '—'], "-");
    let (low, high) = token.split_once('-')?;
    let low = low.replace(',', "").parse::<f64>().ok()?;
    let high = high.replace(',', "").parse::<f64>().ok()?;
    (high > low).then(|| {
        (
            (low * scale).round(),
            (high * scale).round(),
            format!("{token} {}", unit(scale)),
        )
    })
}

fn unit(scale: f64) -> &'static str {
    if scale == 1e3 {
        "kHz"
    } else if scale == 1e6 {
        "MHz"
    } else {
        "GHz"
    }
}

fn rows(bands: &[Band]) -> Vec<Row> {
    let mut rows = Vec::new();
    for band in bands {
        let references = references(&band.content);
        for mention in mentions(&band.content) {
            let name = mention.text.to_string();
            let primary = name
                .chars()
                .filter(|c| c.is_alphabetic())
                .all(char::is_uppercase);
            rows.push(Row {
                primary,
                reference: Some(format!("{} — {name}", band.label)),
                notes: (!references.is_empty()).then(|| format!("Footnotes: {references}")),
                ..Row::new(band.start_hz, band.stop_hz, mention.service, name)
            });
        }
        if references
            .split(", ")
            .any(|reference| matches!(reference, "5.138" | "5.150"))
        {
            rows.push(Row {
                reference: Some(format!("{} — ISM", band.label)),
                notes: Some(format!("Footnotes: {references}")),
                ..Row::new(band.start_hz, band.stop_hz, "ism", "ISM".to_string())
            });
        }
    }
    sort_and_dedup(&mut rows);
    rows
}

pub(super) fn mentions(input: &str) -> Vec<Mention<'_>> {
    let lower = input.to_ascii_lowercase();
    let mut found = Vec::new();
    for (name, service) in SERVICES {
        let mut cursor = 0usize;
        while let Some(relative) = lower[cursor..].find(name) {
            let start = cursor + relative;
            let stop = start + name.len();
            found.push(Mention {
                start,
                stop,
                service,
                text: &input[start..stop],
            });
            cursor = stop;
        }
    }
    found.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then((b.stop - b.start).cmp(&(a.stop - a.start)))
    });
    let mut out = Vec::new();
    let mut cursor = 0usize;
    for mention in found {
        if mention.start >= cursor {
            cursor = mention.stop;
            out.push(mention);
        }
    }
    out
}

fn references(input: &str) -> String {
    let mut found = Vec::new();
    for word in input.split_whitespace() {
        let word = word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.');
        let is_itu = word
            .strip_prefix("5.")
            .is_some_and(|rest| rest.chars().next().is_some_and(|c| c.is_ascii_digit()));
        let is_us = ["US", "NG", "G"].iter().any(|prefix| {
            word.strip_prefix(prefix).is_some_and(|rest| {
                !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric())
            })
        });
        if (is_itu || is_us) && !found.contains(&word) {
            found.push(word);
        }
    }
    found.join(", ")
}

fn inherit(left: &[Row], own: &[Row]) -> Vec<Row> {
    let mut out = own.to_vec();
    for row in left {
        let mut cuts = vec![row.start_hz, row.stop_hz];
        for explicit in own {
            if explicit.stop_hz > row.start_hz && explicit.start_hz < row.stop_hz {
                cuts.push(explicit.start_hz.max(row.start_hz));
                cuts.push(explicit.stop_hz.min(row.stop_hz));
            }
        }
        cuts.sort_by(f64::total_cmp);
        cuts.dedup();
        for pair in cuts.windows(2) {
            let middle = pair[0].midpoint(pair[1]);
            if own
                .iter()
                .any(|explicit| explicit.start_hz <= middle && explicit.stop_hz > middle)
            {
                continue;
            }
            let mut inherited = row.clone();
            inherited.start_hz = pair[0];
            inherited.stop_hz = pair[1];
            out.push(inherited);
        }
    }
    sort_and_dedup(&mut out);
    out
}

pub(super) fn sort_and_dedup(rows: &mut Vec<Row>) {
    rows.sort_by(|a, b| {
        a.start_hz
            .total_cmp(&b.start_hz)
            .then(a.stop_hz.total_cmp(&b.stop_hz))
            .then((b.service == "ism").cmp(&(a.service == "ism")))
            .then(a.name.cmp(&b.name))
            .then(a.service.cmp(b.service))
            .then(a.primary.cmp(&b.primary))
    });
    let mut unique: Vec<Row> = Vec::with_capacity(rows.len());
    for row in rows.drain(..) {
        let duplicate = unique.last_mut().filter(|previous| {
            previous.start_hz == row.start_hz
                && previous.stop_hz == row.stop_hz
                && previous.name == row.name
                && previous.service == row.service
                && previous.primary == row.primary
        });
        if let Some(previous) = duplicate {
            previous.notes = merge_notes(previous.notes.take(), row.notes);
        } else {
            unique.push(row);
        }
    }
    *rows = unique;
}

fn merge_notes(left: Option<String>, right: Option<String>) -> Option<String> {
    match (left, right) {
        (None, right) => right,
        (left, None) => left,
        (Some(left), Some(right)) if left == right => Some(left),
        (Some(left), Some(right))
            if left.starts_with("Footnotes: ") && right.starts_with("Footnotes: ") =>
        {
            let mut references = left[11..].split(", ").collect::<Vec<_>>();
            for reference in right[11..].split(", ") {
                if !references.contains(&reference) {
                    references.push(reference);
                }
            }
            Some(format!("Footnotes: {}", references.join(", ")))
        }
        (Some(left), Some(right)) => {
            let joined = format!("{left}. {right}");
            let mut chars = joined.chars();
            let bounded: String = chars.by_ref().take(400).collect();
            if chars.next().is_none() {
                Some(bounded)
            } else {
                let end = bounded.rfind(' ').unwrap_or(bounded.len());
                Some(format!("{}…", &bounded[..end]))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../fixtures/bandplan/fcc-bbox-excerpt.xhtml");

    #[test]
    fn reads_shifted_headers_and_their_continuation_pages() {
        let layers = parse(FIXTURE).expect("parse");
        let region_1 = &layers[0].1;
        let region_2 = &layers[1].1;
        let region_3 = &layers[2].1;
        assert!(region_1.iter().any(|row| row.start_hz == 86_000.0));
        assert!(region_2.iter().any(|row| row.start_hz == 137_800.0));
        assert!(region_3.iter().any(|row| row.start_hz == 137_800.0));
    }

    #[test]
    fn fills_regions_to_the_right_when_the_pdf_uses_a_merged_cell() {
        let layers = parse(FIXTURE).expect("parse");
        for (_, rows) in &layers[..3] {
            assert!(rows.iter().any(|row| {
                row.start_hz == 8_300.0
                    && row.stop_hz == 9_000.0
                    && row.name == "METEOROLOGICAL AIDS"
            }));
        }
    }

    #[test]
    fn combines_federal_and_non_federal_allocations() {
        let layers = parse(FIXTURE).expect("parse");
        let us = &layers[3].1;
        assert!(us.iter().any(|row| row.name == "FIXED"));
        assert!(us.iter().any(|row| row.name == "Amateur"));
    }

    #[test]
    fn keeps_primary_and_secondary_services_distinct() {
        let layers = parse(FIXTURE).expect("parse");
        let us = &layers[3].1;
        assert!(us.iter().any(|row| row.name == "FIXED" && row.primary));
        assert!(us.iter().any(|row| row.name == "Amateur" && !row.primary));
    }

    #[test]
    fn rejects_non_bbox_input() {
        assert!(parse("not an XHTML document").is_err());
    }

    #[test]
    fn decimal_boundaries_become_exact_hertz() {
        assert_eq!(
            frequency_range("4.063-4.438", 1e6),
            Some((4_063_000.0, 4_438_000.0, "4.063-4.438 MHz".to_string()))
        );
    }
}
