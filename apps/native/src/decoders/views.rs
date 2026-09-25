use std::{collections::HashMap, sync::Arc};

use sdrmm_wire::{
    channel::Sideband,
    decode::{
        AdsbMessage, AisMessage, DecodedRecord, DecoderEvent, DectArc, DectCipherState, DvFrame,
        DvMode, DvTrunkProtocol, IdentReport, IdentSignal, Modulation, ProtocolMatch, RdsUpdate,
        VorReading,
    },
    units,
};
use serde_json::Value;

use crate::decoded::{Station, time_ms};

pub type Records = Vec<Arc<DecodedRecord>>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DecoderScope {
    pub device_set: Option<u32>,
    pub channel: Option<u32>,
}

impl DecoderScope {
    #[must_use]
    pub fn holds(self, device_set: u32, channel: u32) -> bool {
        self.device_set.is_none_or(|set| set == device_set)
            && self.channel.is_none_or(|held| held == channel)
    }
}

#[must_use]
pub fn in_scope<'a>(
    records: impl IntoIterator<Item = &'a Arc<DecodedRecord>>,
    scope: DecoderScope,
) -> Records {
    records
        .into_iter()
        .filter(|record| scope.holds(record.device_set, record.channel))
        .cloned()
        .collect()
}

fn at_ms(record: &DecodedRecord) -> i64 {
    time_ms(&record.at).unwrap_or(0)
}

#[must_use]
pub fn vor_of(record: &DecodedRecord) -> Option<&VorReading> {
    match &record.event {
        DecoderEvent::Vor(reading) => Some(reading),
        _ => None,
    }
}

#[must_use]
pub fn latest_vor_readings(records: &[Arc<DecodedRecord>]) -> Records {
    let mut latest: HashMap<String, Arc<DecodedRecord>> = HashMap::new();
    for record in records {
        let Some(reading) = vor_of(record) else {
            continue;
        };
        let key = reading
            .station
            .clone()
            .unwrap_or_else(|| format!("{}:{}", record.device_set, record.channel));
        let newer = latest
            .get(&key)
            .is_none_or(|previous| at_ms(record) > at_ms(previous));
        if newer {
            latest.insert(key, Arc::clone(record));
        }
    }
    let mut readings: Records = latest.into_values().collect();
    readings.sort_by(|a, b| {
        let radial = |record: &DecodedRecord| vor_of(record).map_or(0.0, |r| r.radial_deg);
        radial(a).total_cmp(&radial(b))
    });
    readings
}

#[derive(Clone, Debug, PartialEq)]
pub struct DectStation {
    pub key: String,
    pub rfpi: Option<String>,
    pub arc: Option<DectArc>,
    pub carrier: Option<u8>,
    pub carrier_hz: Option<f64>,
    pub slot_pair: Option<u8>,
    pub authentication: Option<bool>,
    pub ciphering: Option<bool>,
    pub cipher_state: DectCipherState,
    pub handsets: usize,
    pub bursts: u32,
    pub crc_errors: u32,
    pub level_dbfs: f32,
    pub at_ms: i64,
}

#[must_use]
pub fn dect_stations(records: &[Arc<DecodedRecord>]) -> Vec<DectStation> {
    let mut latest: HashMap<String, DectStation> = HashMap::new();
    for record in records {
        let DecoderEvent::Dect(frame) = &record.event else {
            continue;
        };
        let rfpi = frame
            .identity
            .as_ref()
            .map(|identity| identity.rfpi.clone());
        let key = rfpi.clone().unwrap_or_else(|| {
            format!(
                "{}:{}:{}",
                record.device_set,
                record.channel,
                frame.side.label()
            )
        });
        let when = at_ms(record);
        if latest
            .get(&key)
            .is_some_and(|previous| when <= previous.at_ms)
        {
            continue;
        }
        latest.insert(
            key.clone(),
            DectStation {
                key,
                rfpi,
                arc: frame.identity.as_ref().map(|identity| identity.arc),
                carrier: frame.carrier,
                carrier_hz: frame.carrier_hz,
                slot_pair: frame.slot_pair,
                authentication: frame.security.authentication_supported,
                ciphering: frame.security.ciphering_supported,
                cipher_state: frame.security.cipher_state,
                handsets: frame.handsets.len(),
                bursts: frame.bursts,
                crc_errors: frame.crc_errors,
                level_dbfs: frame.level_dbfs,
                at_ms: when,
            },
        );
    }
    let mut stations: Vec<DectStation> = latest.into_values().collect();
    stations.sort_by(|a, b| b.level_dbfs.total_cmp(&a.level_dbfs));
    stations
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VorFix {
    pub lat: f64,
    pub lon: f64,
    pub residual_km: f64,
    pub stations: usize,
}

const EARTH_RADIUS_KM: f64 = 6371.0;

struct Line {
    nx: f64,
    ny: f64,
    projection: f64,
    weight: f64,
}

#[must_use]
pub fn multi_vor_fix(readings: &[&VorReading]) -> Option<VorFix> {
    let usable: Vec<(&VorReading, f64, f64)> = readings
        .iter()
        .filter_map(|reading| {
            let (lat, lon) = (reading.station_lat?, reading.station_lon?);
            (lat.is_finite() && lon.is_finite()).then_some((*reading, lat, lon))
        })
        .collect();
    if usable.len() < 2 {
        return None;
    }
    let count = usable.len() as f64;
    let ref_lat = usable
        .iter()
        .map(|(_, lat, _)| lat.to_radians())
        .sum::<f64>()
        / count;
    let ref_lon = f64::atan2(
        usable
            .iter()
            .map(|(_, _, lon)| lon.to_radians().sin())
            .sum(),
        usable
            .iter()
            .map(|(_, _, lon)| lon.to_radians().cos())
            .sum(),
    );
    let lines: Vec<Line> = usable
        .iter()
        .map(|(reading, lat, lon)| {
            let x = EARTH_RADIUS_KM * normalize(lon.to_radians() - ref_lon) * ref_lat.cos();
            let y = EARTH_RADIUS_KM * (lat.to_radians() - ref_lat);
            let bearing = (reading.radial_deg + reading.magnetic_declination_deg).to_radians();
            let (nx, ny) = (bearing.cos(), -bearing.sin());
            Line {
                nx,
                ny,
                projection: nx * x + ny * y,
                weight: f64::from(reading.confidence).max(0.05).powi(2),
            }
        })
        .collect();
    solve(&lines, ref_lat, ref_lon, usable.len())
}

fn solve(lines: &[Line], ref_lat: f64, ref_lon: f64, stations: usize) -> Option<VorFix> {
    let (mut a00, mut a01, mut a11, mut b0, mut b1) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for line in lines {
        a00 += line.weight * line.nx * line.nx;
        a01 += line.weight * line.nx * line.ny;
        a11 += line.weight * line.ny * line.ny;
        b0 += line.weight * line.nx * line.projection;
        b1 += line.weight * line.ny * line.projection;
    }
    let determinant = a00 * a11 - a01 * a01;
    if determinant.abs() < 1e-8 {
        return None;
    }
    let x = (b0 * a11 - b1 * a01) / determinant;
    let y = (a00 * b1 - a01 * b0) / determinant;
    let error: f64 = lines
        .iter()
        .map(|line| {
            let miss = line.nx * x + line.ny * y - line.projection;
            line.weight * miss * miss
        })
        .sum();
    let weight: f64 = lines.iter().map(|line| line.weight).sum();
    Some(VorFix {
        lat: (ref_lat + y / EARTH_RADIUS_KM).to_degrees(),
        lon: normalize(ref_lon + x / (EARTH_RADIUS_KM * ref_lat.cos())).to_degrees(),
        residual_km: (error / weight).sqrt(),
        stations,
    })
}

fn normalize(angle: f64) -> f64 {
    angle.sin().atan2(angle.cos())
}

pub const TARGET_STALE_MS: i64 = 30_000;
pub const TARGET_MAX_AGE_MS: i64 = 300_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Age {
    Fresh,
    Stale,
    Fading,
}

#[must_use]
pub fn age_class(age_ms: i64) -> Age {
    if age_ms < TARGET_STALE_MS {
        Age::Fresh
    } else if age_ms < TARGET_MAX_AGE_MS / 2 {
        Age::Stale
    } else {
        Age::Fading
    }
}

#[must_use]
pub fn format_age(age_ms: i64) -> String {
    let seconds = (age_ms as f64 / 1000.0).round().max(0.0) as i64;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        format!("{minutes}:{:02}", seconds % 60)
    } else {
        format!("{}h{:02}", minutes / 60, minutes % 60)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TargetRow {
    pub id: String,
    pub label: String,
    pub primary: String,
    pub secondary: String,
    pub position: String,
    pub age_ms: i64,
    pub frames: u64,
}

fn trimmed(text: Option<&String>) -> Option<String> {
    text.map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

#[must_use]
pub fn aircraft_row(station: &Station, now_ms: i64) -> Option<TargetRow> {
    let DecoderEvent::Adsb(m) = &station.event else {
        return None;
    };
    Some(TargetRow {
        id: m.icao.to_uppercase(),
        label: trimmed(m.callsign.as_ref()).unwrap_or_else(|| "-".to_owned()),
        primary: aircraft_altitude(m),
        secondary: join_fields(&[
            format_speed_kt(m.ground_speed_kt),
            format_bearing(m.track_deg),
        ]),
        position: format_position(m.lat, m.lon),
        age_ms: (now_ms - station.last_seen_ms).max(0),
        frames: station.frames,
    })
}

fn aircraft_altitude(m: &AdsbMessage) -> String {
    if m.on_ground == Some(true) {
        "GND".to_owned()
    } else {
        format_altitude_ft(m.altitude_ft.map(f64::from))
    }
}

#[must_use]
pub fn ship_row(station: &Station, now_ms: i64) -> Option<TargetRow> {
    let DecoderEvent::Ais(m) = &station.event else {
        return None;
    };
    Some(TargetRow {
        id: m.mmsi.to_string(),
        label: ship_label(m),
        primary: format_speed_kt(m.sog_kt),
        secondary: join_fields(&[
            format_bearing(m.cog_deg),
            m.destination
                .as_deref()
                .map(str::trim)
                .unwrap_or_default()
                .to_owned(),
        ]),
        position: format_position(m.lat, m.lon),
        age_ms: (now_ms - station.last_seen_ms).max(0),
        frames: station.frames,
    })
}

fn ship_label(m: &AisMessage) -> String {
    trimmed(m.name.as_ref())
        .or_else(|| trimmed(m.call_sign.as_ref()))
        .unwrap_or_else(|| "-".to_owned())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetSort {
    Age,
    Id,
}

#[must_use]
pub fn sort_targets(rows: &[TargetRow], key: TargetSort, descending: bool) -> Vec<TargetRow> {
    let mut sorted = rows.to_vec();
    sorted.sort_by(|a, b| {
        let order = match key {
            TargetSort::Age => a.age_ms.cmp(&b.age_ms),
            TargetSort::Id => a.id.len().cmp(&b.id.len()).then_with(|| a.id.cmp(&b.id)),
        };
        if descending { order.reverse() } else { order }
    });
    sorted
}

#[must_use]
pub fn format_altitude_ft(ft: Option<f64>) -> String {
    ft.map_or_else(
        || "-".to_owned(),
        |ft| {
            let whole = ft.round() as i64;
            let digits = crate::decoders::text::grouped(whole.abs());
            let sign = if whole < 0 { "−" } else { "" };
            format!("{sign}{digits} ft")
        },
    )
}

#[must_use]
pub fn format_speed_kt(kt: Option<f64>) -> String {
    kt.map_or_else(|| "-".to_owned(), |kt| format!("{kt:.0} kt"))
}

#[must_use]
pub fn format_bearing(deg: Option<f64>) -> String {
    deg.map_or_else(String::new, |deg| {
        format!("{}°", (deg.round() as i64).rem_euclid(360))
    })
}

#[must_use]
pub fn format_position(lat: Option<f64>, lon: Option<f64>) -> String {
    crate::decoders::text::position(lat, lon).unwrap_or_else(|| "-".to_owned())
}

#[must_use]
pub fn format_clock(at: &str) -> String {
    let Ok(stamp) = at.parse::<jiff::Timestamp>() else {
        return "--:--:--".to_owned();
    };
    let local = stamp.to_zoned(jiff::tz::TimeZone::system());
    format!(
        "{:02}:{:02}:{:02}",
        local.hour(),
        local.minute(),
        local.second()
    )
}

fn rds_of(record: &DecodedRecord) -> Option<&RdsUpdate> {
    match &record.event {
        DecoderEvent::Rds(update) => Some(update),
        _ => None,
    }
}

#[must_use]
pub fn rds_picture(records: &[Arc<DecodedRecord>]) -> Option<RdsUpdate> {
    let current = same_station(records);
    let mut merged = serde_json::Map::new();
    for update in current.iter().rev() {
        if let Ok(Value::Object(fields)) = serde_json::to_value(update) {
            for (key, value) in fields {
                if !value.is_null() {
                    merged.insert(key, value);
                }
            }
        }
    }
    let newest = current.first()?;
    Some(serde_json::from_value(Value::Object(merged)).unwrap_or_else(|_| (*newest).clone()))
}

fn same_station(records: &[Arc<DecodedRecord>]) -> Vec<&RdsUpdate> {
    let updates: Vec<&RdsUpdate> = records.iter().filter_map(|r| rds_of(r)).collect();
    let Some(newest) = updates.first() else {
        return Vec::new();
    };
    let mut groups = newest.groups;
    let mut end = updates.len();
    for (index, older) in updates.iter().enumerate().skip(1) {
        let restarted = older.groups > groups;
        let retuned = matches!((&older.pi, &newest.pi), (Some(a), Some(b)) if a != b);
        if restarted || retuned {
            end = index;
            break;
        }
        groups = older.groups;
    }
    updates.into_iter().take(end).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RdsGrade {
    NoLock,
    Good,
    Fair,
    Poor,
}

impl RdsGrade {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::NoLock => "no lock",
            Self::Good => "good",
            Self::Fair => "fair",
            Self::Poor => "poor",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RdsQuality {
    pub groups: u64,
    pub block_errors: u64,
    pub error_rate: f64,
    pub grade: RdsGrade,
}

#[must_use]
pub fn rds_quality(update: &RdsUpdate) -> RdsQuality {
    let groups = update.groups;
    let block_errors = update.block_errors;
    let blocks = update.blocks.max(groups * 4 + block_errors);
    let error_rate = if blocks == 0 {
        0.0
    } else {
        block_errors as f64 / blocks as f64
    };
    let grade = if groups == 0 {
        RdsGrade::NoLock
    } else if error_rate < 0.02 {
        RdsGrade::Good
    } else if error_rate < 0.1 {
        RdsGrade::Fair
    } else {
        RdsGrade::Poor
    };
    RdsQuality {
        groups,
        block_errors,
        error_rate,
        grade,
    }
}

#[must_use]
pub fn pty_label(update: &RdsUpdate) -> String {
    match (&update.pty_name, update.pty) {
        (Some(name), _) if !name.is_empty() => name.clone(),
        (_, Some(code)) => format!("PTY {code}"),
        _ => "-".to_owned(),
    }
}

#[must_use]
pub fn format_alt_freqs(hz: &[f64]) -> Vec<String> {
    let mut sorted = hz.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted.into_iter().map(units::hertz).collect()
}

pub const TRANSCRIPT_LIMIT: usize = 20_000;

#[must_use]
pub fn append_transcript(previous: &str, chunk: &str, limit: usize) -> String {
    let joined = format!("{previous}{chunk}");
    let length = joined.chars().count();
    if length <= limit {
        return joined;
    }
    let cut: String = joined.chars().skip(length - limit).collect();
    match cut.find('\n') {
        Some(newline) => cut[newline + 1..].to_owned(),
        None => cut,
    }
}

#[must_use]
pub fn text_of(record: &DecodedRecord) -> Option<&str> {
    match &record.event {
        DecoderEvent::Rtty(text) => Some(&text.text),
        DecoderEvent::Morse(text) => Some(&text.text),
        DecoderEvent::Psk(text) => Some(&text.text),
        _ => None,
    }
}

#[must_use]
pub fn build_transcript(records: &[Arc<DecodedRecord>], limit: usize) -> String {
    records.iter().rev().fold(String::new(), |text, record| {
        text_of(record).map_or(text.clone(), |chunk| append_transcript(&text, chunk, limit))
    })
}

#[must_use]
pub fn latest_wpm(records: &[Arc<DecodedRecord>]) -> Option<f32> {
    match &records.first()?.event {
        DecoderEvent::Morse(morse) => Some(morse.wpm),
        _ => None,
    }
}

pub const CW_SPOT_GROUP_HZ: f32 = 50.0;
pub const CW_SPOT_TEXT_LIMIT: usize = 2_000;

#[derive(Clone, Debug, PartialEq)]
pub struct CwSignalRow {
    pub frequency_hz: f64,
    pub offset_hz: f32,
    pub wpm: f32,
    pub snr_db: f32,
    pub text: String,
}

#[must_use]
pub fn cw_signal_rows(records: &[Arc<DecodedRecord>], limit: usize) -> Vec<CwSignalRow> {
    let mut signals: HashMap<i64, CwSignalRow> = HashMap::new();
    for record in records.iter().rev() {
        let DecoderEvent::CwSkimmer(spot) = &record.event else {
            continue;
        };
        let key = (spot.offset_hz / CW_SPOT_GROUP_HZ).round() as i64;
        let previous = signals
            .get(&key)
            .map(|row| row.text.clone())
            .unwrap_or_default();
        let joined = format!("{previous}{}", spot.text);
        let skip = joined.chars().count().saturating_sub(limit);
        signals.insert(
            key,
            CwSignalRow {
                frequency_hz: record.freq_hz + f64::from(spot.offset_hz),
                offset_hz: spot.offset_hz,
                wpm: spot.wpm,
                snr_db: spot.snr_db,
                text: joined.chars().skip(skip).collect(),
            },
        );
    }
    let mut rows: Vec<CwSignalRow> = signals.into_values().collect();
    rows.sort_by(|a, b| a.offset_hz.total_cmp(&b.offset_hz));
    rows
}

#[must_use]
pub fn is_at_bottom(scroll_top: f32, scroll_height: f32, client_height: f32) -> bool {
    scroll_height - scroll_top - client_height <= 8.0
}

#[must_use]
pub fn tone_label(ctcss_hz: Option<f64>, dcs_code: Option<u16>) -> String {
    join_fields(&[
        ctcss_hz.map_or_else(String::new, |hz| format!("CTCSS {hz:.1} Hz")),
        dcs_code.map_or_else(String::new, |code| format!("DCS {code:03}")),
    ])
}

fn join_fields(fields: &[String]) -> String {
    crate::decoders::text::join(fields.iter().cloned())
}

#[must_use]
pub fn dv_network(frame: &DvFrame) -> String {
    let mut parts = Vec::new();
    if let Some(slot) = frame.slot {
        parts.push(format!("TS{slot}"));
    }
    if let Some(code) = frame.color_code {
        parts.push(match frame.mode {
            DvMode::P25 => format!("NAC {code:03X}"),
            DvMode::Nxdn | DvMode::Dpmr => format!("RAN {code}"),
            _ => format!("CC {code}"),
        });
    }
    parts.join(" ")
}

#[must_use]
pub fn dv_trunking(frame: &DvFrame) -> Option<String> {
    let system = frame.trunk_protocol.map(|protocol| match protocol {
        DvTrunkProtocol::CapacityPlus => "Capacity Plus",
        DvTrunkProtocol::HyteraXpt => "Hytera XPT",
        DvTrunkProtocol::TierThree => "Tier III",
    });
    let role = frame.control_channel.map(|control| {
        if control {
            "control channel"
        } else {
            "traffic channel"
        }
    });
    let parts: Vec<&str> = [system, role].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

#[must_use]
pub fn dv_checksum(frame: &DvFrame) -> Option<String> {
    frame.crc_verified.map(|verified| {
        if verified {
            "verified".to_owned()
        } else {
            "not verified: read on error correction alone".to_owned()
        }
    })
}

#[must_use]
pub fn modulation_label(modulation: Modulation, sideband: Option<Sideband>) -> String {
    let base = modulation.label();
    match sideband {
        Some(sideband) => format!(
            "{base} ({})",
            crate::decoders::text::serde_name(&sideband).to_uppercase()
        ),
        None => base.to_owned(),
    }
}

#[must_use]
pub fn signal_frequency(signal: &IdentSignal) -> String {
    units::hertz(signal.frequency_hz)
}

#[must_use]
pub fn ident_overview(report: &IdentReport) -> Vec<(&'static str, String)> {
    if report.signals.is_empty() {
        vec![(
            "Loudest bin",
            format!("{:.1} dB over the noise floor", report.snr_db),
        )]
    } else {
        Vec::new()
    }
}

#[must_use]
pub fn ident_measurements(signal: &IdentSignal) -> Vec<(&'static str, String)> {
    let mut fields = vec![
        ("Bandwidth", units::hertz(signal.bandwidth_hz)),
        (
            "Off tune",
            format!("{} Hz", signal.center_offset_hz.round()),
        ),
        ("SNR", format!("{:.1} dB", signal.snr_db)),
    ];
    if let Some(rate) = signal.symbol_rate_hz {
        fields.push(("Symbol rate", units::si(rate, "Bd")));
    }
    if let Some(deviation) = signal.deviation_hz {
        fields.push(("Deviation", format!("±{} Hz", deviation.round())));
    }
    if let Some(burst) = signal.burst_ms {
        let period = signal
            .burst_period_ms
            .map_or_else(String::new, |period| format!(" every {period:.1} ms"));
        fields.push(("Bursts", format!("{burst:.2} ms{period}")));
    }
    if let Some(symbol) = signal.ofdm_symbol_us {
        let guard = signal
            .ofdm_guard_us
            .map_or_else(String::new, |guard| format!(", guard {} µs", guard.round()));
        fields.push(("OFDM symbol", format!("{} µs{guard}", symbol.round())));
    }
    if signal.features.duty < 0.99 {
        fields.push((
            "Duty",
            format!("{}%", (f64::from(signal.features.duty) * 100.0).round()),
        ));
    }
    fields
}

#[must_use]
pub fn candidate_score(candidate: &ProtocolMatch) -> String {
    if candidate.confirmed {
        "confirmed".to_owned()
    } else {
        format!("{}%", (f64::from(candidate.score) * 100.0).round())
    }
}

#[cfg(test)]
mod tests;
