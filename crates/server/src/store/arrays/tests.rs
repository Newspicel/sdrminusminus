use sdrmm_wire::{ArrayCalRecord, CalSourceKind, LaneKey, LaneSolution};

use super::*;

const RATE: f64 = 2_400_000.0;

fn kraken(streams: u32) -> Vec<LaneKey> {
    (0..streams)
        .map(|stream| LaneKey {
            device: "kraken:01/1".to_owned(),
            stream,
        })
        .collect()
}

fn record(lanes: Vec<LaneKey>, center_hz: f64) -> ArrayCalRecord {
    let solution = lanes
        .iter()
        .enumerate()
        .map(|(lane, _)| LaneSolution {
            delay_samples: lane as f64 * 0.25,
            phase_deg: lane as f64 * 10.0,
            gain_db: 0.0,
            coherence: 0.99,
            equaliser: Vec::new(),
        })
        .collect();
    ArrayCalRecord {
        lanes,
        center_hz,
        sample_rate: RATE,
        gain_db: Some(30.0),
        source: CalSourceKind::Noise,
        keeps_phase: true,
        solved_at: "2026-09-29T10:00:00.000Z".to_owned(),
        solution,
    }
}

fn rows(store: &Store) -> i64 {
    store
        .lock()
        .query_row("SELECT COUNT(*) FROM array_calibrations", [], |row| {
            row.get(0)
        })
        .expect("count")
}

#[test]
fn array_calibration_picks_the_nearest_center_within_tolerance() {
    let store = Store::open(None).expect("open");
    for center_hz in [430e6, 433.5e6, 440e6] {
        store
            .put_array_calibration(&record(kraken(5), center_hz))
            .expect("stored");
    }
    store
        .put_array_calibration(&record(kraken(4), 434e6))
        .expect("another array");

    let near = store
        .array_calibration(&kraken(5), RATE, 434e6)
        .expect("read")
        .expect("433.5 MHz is within 2.17 MHz");
    assert_eq!(near.center_hz, 433.5e6);
    assert_eq!(near, record(kraken(5), 433.5e6));

    assert!(
        store
            .array_calibration(&kraken(5), RATE, 436e6)
            .expect("read")
            .is_none(),
        "2.5 MHz is past 0.5 % of 436 MHz"
    );
    let low = store
        .array_calibration(&kraken(5), RATE, 100e6)
        .expect("read");
    assert!(low.is_none(), "nothing within 1 MHz of 100 MHz");
    assert!(
        store
            .array_calibration(&kraken(5), 2_048_000.0, 433.5e6)
            .expect("read")
            .is_none(),
        "another rate is another calibration"
    );
    assert_eq!(
        store
            .array_calibration(&kraken(4), RATE, 433.5e6)
            .expect("read")
            .map(|found| found.lanes.len()),
        Some(4),
        "lanes are the key"
    );

    let mut resolved = record(kraken(5), 433.5e6);
    resolved.solution[1].phase_deg = 42.0;
    store.put_array_calibration(&resolved).expect("replaced");
    assert_eq!(rows(&store), 4, "the same band is solved again, not added");
    assert_eq!(
        store
            .array_calibration(&kraken(5), RATE, 433.6e6)
            .expect("read"),
        Some(resolved)
    );
}

#[test]
fn array_calibration_keeps_at_most_200_rows_per_array() {
    let store = Store::open(None).expect("open");
    store
        .put_array_calibration(&record(kraken(4), 100e6))
        .expect("another array");
    let first = 50e6;
    let step = 10e6;
    for band in 0..210 {
        store
            .put_array_calibration(&record(kraken(5), first + f64::from(band) * step))
            .expect("stored");
    }

    assert_eq!(rows(&store), 201);
    for oldest in 0..10 {
        let center = first + f64::from(oldest) * step;
        assert!(
            store
                .array_calibration(&kraken(5), RATE, center)
                .expect("read")
                .is_none(),
            "{center} was the oldest and went"
        );
    }
    for kept in [10, 11, 209] {
        let center = first + f64::from(kept) * step;
        assert!(
            store
                .array_calibration(&kraken(5), RATE, center)
                .expect("read")
                .is_some(),
            "{center} stays"
        );
    }
    assert!(
        store
            .array_calibration(&kraken(4), RATE, 100e6)
            .expect("read")
            .is_some(),
        "another array keeps its own rows"
    );
}

#[test]
fn a_new_calibration_outlives_a_clock_stepped_back() {
    let store = Store::open(None).expect("open");
    let full = u32::try_from(MAX_CAL_RECORDS_PER_ARRAY).expect("a small limit");
    for band in 0..full {
        store
            .put_array_calibration(&record(kraken(5), 100e6 + f64::from(band) * 10e6))
            .expect("stored");
    }
    store
        .lock()
        .execute(
            "UPDATE array_calibrations SET saved_at = '2099-01-01T00:00:00.000000000Z'",
            [],
        )
        .expect("rows from a clock ahead");

    store
        .put_array_calibration(&record(kraken(5), 50e6))
        .expect("stored");

    assert_eq!(rows(&store), i64::from(full));
    assert!(
        store
            .array_calibration(&kraken(5), RATE, 50e6)
            .expect("read")
            .is_some(),
        "the solution just made is kept"
    );
}
