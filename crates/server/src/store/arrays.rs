use rusqlite::{OptionalExtension, params};
use sdrmm_wire::{ArrayCalRecord, LaneKey, array::MAX_CAL_RECORDS_PER_ARRAY};

use super::{Store, StoreError, now_rfc3339};

const NEAR_MIN_HZ: f64 = 1e6;
const NEAR_FRACTION: f64 = 0.005;
const RATE_TOLERANCE: f64 = 1e-9;

fn lanes_text(lanes: &[LaneKey]) -> String {
    lanes
        .iter()
        .map(|lane| format!("{}#{}", lane.device, lane.stream))
        .collect::<Vec<_>>()
        .join(",")
}

fn near_hz(center_hz: f64) -> f64 {
    NEAR_MIN_HZ.max(NEAR_FRACTION * center_hz.abs())
}

impl Store {
    pub fn put_array_calibration(&self, record: &ArrayCalRecord) -> Result<(), StoreError> {
        let lanes = lanes_text(&record.lanes);
        let body = serde_json::to_string(record)?;
        let keep = i64::try_from(MAX_CAL_RECORDS_PER_ARRAY).unwrap_or(i64::MAX);
        let conn = self.lock();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO array_calibrations \
             (lanes, sample_rate, center_hz, record, saved_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                lanes,
                record.sample_rate,
                record.center_hz,
                body,
                now_rfc3339()
            ],
        )?;
        tx.execute(
            "DELETE FROM array_calibrations WHERE lanes = ?1 \
             AND (sample_rate, center_hz) != (?2, ?3) \
             AND (sample_rate, center_hz) NOT IN \
             (SELECT sample_rate, center_hz FROM array_calibrations \
              WHERE lanes = ?1 AND (sample_rate, center_hz) != (?2, ?3) \
              ORDER BY saved_at DESC LIMIT ?4)",
            params![
                lanes,
                record.sample_rate,
                record.center_hz,
                keep.saturating_sub(1)
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn array_calibration(
        &self,
        lanes: &[LaneKey],
        sample_rate: f64,
        center_hz: f64,
    ) -> Result<Option<ArrayCalRecord>, StoreError> {
        let rate_slack = RATE_TOLERANCE * sample_rate.abs().max(1.0);
        let body: Option<String> = self
            .lock()
            .query_row(
                "SELECT record FROM array_calibrations \
                 WHERE lanes = ?1 AND abs(sample_rate - ?2) <= ?3 AND abs(center_hz - ?4) <= ?5 \
                 ORDER BY abs(center_hz - ?4), saved_at DESC LIMIT 1",
                params![
                    lanes_text(lanes),
                    sample_rate,
                    rate_slack,
                    center_hz,
                    near_hz(center_hz)
                ],
                |row| row.get(0),
            )
            .optional()?;
        body.map(|body| serde_json::from_str(&body))
            .transpose()
            .map_err(StoreError::from)
    }
}

#[cfg(test)]
mod tests;
