use std::{collections::HashSet, sync::Arc};

use sdrmm_wire::{
    decode::{DecodedRecord, DecoderEvent},
    rest::{DecoderLogEntry, DecoderLogQuery},
};

use crate::decoded::{Decoded, time_ms};

const KIND_LABELS: &[(&str, &str)] = &[
    ("call", "Call"),
    ("transmission", "Transmission"),
    ("adsb", "ADS-B"),
    ("ais", "AIS"),
    ("aprs", "APRS"),
    ("pocsag", "POCSAG"),
    ("flex", "FLEX"),
    ("ermes", "ERMES"),
    ("rds", "RDS"),
    ("rtty", "RTTY"),
    ("morse", "Morse"),
    ("cw_skimmer", "CW skimmer"),
    ("selcall", "Selcall"),
    ("navtex", "NAVTEX"),
    ("acars", "ACARS"),
    ("subghz", "Sub-GHz"),
    ("tone", "Tone"),
    ("scrambler", "Scrambler"),
    ("dv", "Digital voice"),
    ("ident", "Signal ID"),
    ("ft8", "FT8"),
    ("ft4", "FT4"),
    ("psk", "PSK"),
    ("wspr", "WSPR"),
    ("broadcast", "Digital broadcast"),
    ("broadcast_data", "Broadcast data"),
    ("radio_clock", "Radio clock"),
    ("gnss", "GNSS lab"),
    ("sstv", "SSTV"),
    ("vor", "VOR"),
    ("ils", "ILS"),
    ("dsc", "DSC"),
    ("inmarsat_stdc", "Inmarsat STD-C"),
    ("inmarsat_aero", "Inmarsat Aero"),
    ("vdl2", "VDL Mode 2"),
    ("hfdl", "HFDL"),
    ("iridium", "Iridium"),
    ("dect", "DECT"),
    ("df", "Bearing"),
    ("df_fix", "Fix"),
    ("radar", "Radar"),
];

#[must_use]
pub fn kind_label(kind: &str) -> String {
    KIND_LABELS
        .iter()
        .find(|(named, _)| *named == kind)
        .map_or_else(|| kind.to_uppercase(), |(_, label)| (*label).to_owned())
}

pub const LIMIT_OPTIONS: [u32; 3] = [100, 500, 2000];
pub const LIVE_ROW_CAP: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogFilter {
    pub q: String,
    pub limit: u32,
}

impl Default for LogFilter {
    fn default() -> Self {
        Self {
            q: String::new(),
            limit: 500,
        }
    }
}

impl LogFilter {
    #[must_use]
    pub fn filtered(&self) -> bool {
        !self.q.trim().is_empty()
    }

    #[must_use]
    pub fn query(&self, sink: &str) -> DecoderLogQuery {
        let q = self.q.trim();
        DecoderLogQuery {
            sink: Some(sink.to_owned()),
            limit: Some(self.limit),
            q: (!q.is_empty()).then(|| q.to_owned()),
            ..DecoderLogQuery::default()
        }
    }

    #[must_use]
    pub fn matches(&self, record: &DecodedRecord, sink: &str) -> bool {
        if !reached_sink(record, sink) {
            return false;
        }
        let q = self.q.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        record.event.summary().to_lowercase().contains(&q)
            || record
                .event
                .station()
                .is_some_and(|station| station.to_lowercase().contains(&q))
    }
}

#[must_use]
pub fn reached_sink(record: &DecodedRecord, sink: &str) -> bool {
    record.sinks.iter().any(|reached| reached == sink)
}

#[must_use]
pub fn query_string(query: &DecoderLogQuery) -> String {
    let mut pairs = url::form_urlencoded::Serializer::new(String::new());
    if let Ok(serde_json::Value::Object(fields)) = serde_json::to_value(query) {
        for (key, value) in fields {
            let text = match value {
                serde_json::Value::String(text) => text,
                serde_json::Value::Null => continue,
                other => other.to_string(),
            };
            pairs.append_pair(&key, &text);
        }
    }
    pairs.finish()
}

#[must_use]
pub fn collect_live(
    decoded: &Decoded,
    filter: &LogFilter,
    sink: &str,
    cap: usize,
) -> Vec<Arc<DecodedRecord>> {
    let mut records: Vec<(i64, Arc<DecodedRecord>)> = decoded
        .all_frames()
        .filter(|record| filter.matches(record, sink))
        .map(|record| (time_ms(&record.at).unwrap_or(0), Arc::clone(record)))
        .collect();
    records.sort_by_key(|record| std::cmp::Reverse(record.0));
    records.truncate(cap);
    records.into_iter().map(|(_, record)| record).collect()
}

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Stored(Arc<DecoderLogEntry>),
    Live(Arc<DecodedRecord>),
}

impl Source {
    #[must_use]
    pub fn event(&self) -> &DecoderEvent {
        match self {
            Self::Stored(entry) => &entry.event,
            Self::Live(record) => &record.event,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LogRow {
    pub key: String,
    pub at: String,
    pub kind: String,
    pub station: Option<String>,
    pub summary: String,
    pub freq_hz: f64,
    pub device_set: u32,
    pub channel: u32,
    pub live: bool,
    pub source: Source,
}

impl LogRow {
    fn signature(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}",
            self.at,
            self.kind,
            self.device_set,
            self.channel,
            self.station.as_deref().unwrap_or_default(),
            self.summary
        )
    }
}

#[must_use]
pub fn stored_row(entry: Arc<DecoderLogEntry>) -> LogRow {
    LogRow {
        key: format!("stored:{}", entry.id),
        at: entry.at.clone(),
        kind: entry.kind.clone(),
        station: entry.station.clone(),
        summary: entry.summary.clone(),
        freq_hz: entry.freq_hz,
        device_set: entry.device_set,
        channel: entry.channel,
        live: false,
        source: Source::Stored(entry),
    }
}

#[must_use]
pub fn live_row(record: Arc<DecodedRecord>) -> LogRow {
    let mut row = LogRow {
        key: String::new(),
        at: record.at.clone(),
        kind: record.event.kind().to_owned(),
        station: record.event.station(),
        summary: record.event.summary(),
        freq_hz: record.freq_hz,
        device_set: record.device_set,
        channel: record.channel,
        live: true,
        source: Source::Live(record),
    };
    row.key = format!("live:{}", row.signature());
    row
}

#[must_use]
pub fn build_rows(entries: &[Arc<DecoderLogEntry>], live: Vec<Arc<DecodedRecord>>) -> Vec<LogRow> {
    let stored: Vec<LogRow> = entries.iter().cloned().map(stored_row).collect();
    let mut seen: HashSet<String> = stored.iter().map(LogRow::signature).collect();
    let mut rows = Vec::with_capacity(stored.len() + live.len());
    for record in live {
        let row = live_row(record);
        if seen.insert(row.signature()) {
            rows.push(row);
        }
    }
    rows.extend(stored);
    rows.sort_by_cached_key(|row| std::cmp::Reverse(time_ms(&row.at).unwrap_or(0)));
    rows
}

#[must_use]
pub fn dropped_notice(lost: u64, dropped: u64) -> Option<String> {
    let word = |n: u64| if n == 1 { "frame" } else { "frames" };
    let mut parts = Vec::new();
    if lost > 0 {
        parts.push(format!("{lost} live {} dropped", word(lost)));
    }
    if dropped > 0 {
        parts.push(format!("{dropped} {} never reached the log", word(dropped)));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    At,
    Kind,
    Freq,
    Station,
    Summary,
}

pub const COLUMNS: [(Column, &str, f32); 5] = [
    (Column::At, "Time", 84.0),
    (Column::Kind, "Kind", 96.0),
    (Column::Freq, "Frequency", 116.0),
    (Column::Station, "Station", 152.0),
    (Column::Summary, "Summary", 180.0),
];

pub const MIN_COLUMN_WIDTH: f32 = 56.0;
pub const MAX_COLUMN_WIDTH: f32 = 720.0;
pub const COLUMN_STEP: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColumnWidths(pub [f32; 5]);

impl Default for ColumnWidths {
    fn default() -> Self {
        Self(COLUMNS.map(|(_, _, width)| width))
    }
}

impl ColumnWidths {
    #[must_use]
    pub fn of(self, column: Column) -> f32 {
        self.0[column as usize]
    }

    #[must_use]
    pub fn resized(self, column: Column, px: f32) -> Self {
        let mut next = self;
        next.0[column as usize] = clamp_column_width(px);
        next
    }

    #[must_use]
    pub fn read(raw: Option<&str>) -> Self {
        let mut widths = Self::default();
        let Some(Ok(serde_json::Value::Object(stored))) =
            raw.map(serde_json::from_str::<serde_json::Value>)
        else {
            return widths;
        };
        for (index, (column, _, _)) in COLUMNS.iter().enumerate() {
            let stored = stored
                .get(column_key(*column))
                .and_then(serde_json::Value::as_f64);
            if let Some(px) = stored.filter(|_| *column != Column::Summary) {
                widths.0[index] = clamp_column_width(px as f32);
            }
        }
        widths
    }

    #[must_use]
    pub fn write(self) -> String {
        let fields: serde_json::Map<String, serde_json::Value> = COLUMNS
            .iter()
            .map(|(column, _, _)| (column_key(*column).to_owned(), self.of(*column).into()))
            .collect();
        serde_json::Value::Object(fields).to_string()
    }
}

#[must_use]
pub const fn column_key(column: Column) -> &'static str {
    match column {
        Column::At => "at",
        Column::Kind => "kind",
        Column::Freq => "freq",
        Column::Station => "station",
        Column::Summary => "summary",
    }
}

#[must_use]
pub fn export_path(format: &str, query: &DecoderLogQuery) -> String {
    format!("/api/decoderlog/export/{format}?{}", query_string(query))
}

#[must_use]
pub fn clamp_column_width(px: f32) -> f32 {
    if px.is_finite() {
        px.clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH).round()
    } else {
        MIN_COLUMN_WIDTH
    }
}

#[cfg(test)]
mod tests;
