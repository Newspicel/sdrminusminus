use sdrmm_wire::propagation::IonosondeStation;

use super::model::Cell;
use crate::ui::map::geo::{Geo, great_circle_km};

pub const FORECAST_RADIUS_KM: f64 = 3_000.0;
pub const FORECAST_MIN_STATIONS: usize = 2;
const NEAR_KM: f64 = 25.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Forecast {
    pub muf3000_mhz: f64,
    pub stations: usize,
    pub nearest_km: f64,
    pub nearest: String,
}

#[must_use]
pub fn forecast_at(stations: &[IonosondeStation], at: Geo, radius_km: f64) -> Option<Forecast> {
    let (mut weighted, mut weights, mut used) = (0.0, 0.0, 0);
    let mut nearest_km = f64::INFINITY;
    let mut nearest = String::new();
    for station in stations {
        let distance_km = great_circle_km(at, Geo::new(station.latitude, station.longitude));
        if distance_km < nearest_km {
            nearest_km = distance_km;
            nearest = if station.name.is_empty() {
                station.code.clone()
            } else {
                station.name.clone()
            };
        }
        if distance_km > radius_km {
            continue;
        }
        let confidence = (station.confidence.unwrap_or(100.0) / 100.0).max(0.1);
        let weight = confidence / distance_km.max(NEAR_KM).powi(2);
        weighted += weight * station.muf3000_mhz;
        weights += weight;
        used += 1;
    }
    (used >= FORECAST_MIN_STATIONS && weights > 0.0).then(|| Forecast {
        muf3000_mhz: weighted / weights,
        stations: used,
        nearest_km,
        nearest,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct Comparison {
    pub cell: Cell,
    pub measured_muf3000_mhz: f64,
    pub forecast: Forecast,
    pub delta_mhz: f64,
}

#[must_use]
pub fn compare(cells: &[Cell], stations: &[IonosondeStation]) -> Vec<Comparison> {
    cells
        .iter()
        .filter_map(|cell| {
            let measured = cell.measured_muf3000_mhz?;
            let forecast = forecast_at(stations, cell.centre, FORECAST_RADIUS_KM)?;
            Some(Comparison {
                cell: cell.clone(),
                measured_muf3000_mhz: measured,
                delta_mhz: measured - forecast.muf3000_mhz,
                forecast,
            })
        })
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Agreement {
    pub cells: usize,
    pub above: usize,
    pub median_delta_mhz: f64,
    pub widest_above: Option<String>,
}

#[must_use]
pub fn agreement(comparisons: &[Comparison]) -> Agreement {
    if comparisons.is_empty() {
        return Agreement::default();
    }
    let mut deltas: Vec<f64> = comparisons.iter().map(|entry| entry.delta_mhz).collect();
    deltas.sort_by(f64::total_cmp);
    let middle = deltas.len() / 2;
    let median_delta_mhz = if deltas.len() % 2 == 1 {
        deltas[middle]
    } else {
        (deltas[middle - 1] + deltas[middle]) / 2.0
    };
    let above: Vec<&Comparison> = comparisons
        .iter()
        .filter(|entry| entry.delta_mhz > 0.0)
        .collect();
    let widest_above = above
        .iter()
        .max_by(|a, b| a.delta_mhz.total_cmp(&b.delta_mhz))
        .map(|entry| entry.cell.key.clone());
    Agreement {
        cells: comparisons.len(),
        above: above.len(),
        median_delta_mhz,
        widest_above,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn station(code: &str, latitude: f64, longitude: f64, muf: f64) -> IonosondeStation {
        IonosondeStation {
            code: code.to_owned(),
            name: "Somewhere".to_owned(),
            latitude,
            longitude,
            muf3000_mhz: muf,
            fof2_mhz: None,
            m3000: None,
            confidence: None,
            measured_at: "2026-08-16T12:00:00Z".to_owned(),
        }
    }

    fn cell(key: &str, latitude: f64, muf: Option<f64>) -> Cell {
        Cell {
            key: key.to_owned(),
            centre: Geo::new(latitude, -1.0),
            weight: 1.0,
            decodes: 1,
            callsigns: 1,
            best_freq_hz: 14_074_000.0,
            best_snr_db: -12.0,
            measured_muf3000_mhz: muf,
            median_distance_km: 3_000.0,
            last_seen: 0,
        }
    }

    fn at(latitude: f64, longitude: f64) -> Geo {
        Geo::new(latitude, longitude)
    }

    #[test]
    fn two_equally_close_sites_are_averaged() {
        let forecast = forecast_at(
            &[
                station("W", 50.0, -5.0, 18.0),
                station("E", 50.0, 5.0, 22.0),
            ],
            at(50.0, 0.0),
            FORECAST_RADIUS_KM,
        )
        .expect("a forecast");
        assert!((forecast.muf3000_mhz - 20.0).abs() < 1e-6);
        assert_eq!(forecast.stations, 2);
    }

    #[test]
    fn the_nearer_site_counts_more() {
        let forecast = forecast_at(
            &[
                station("NEAR", 50.0, -1.0, 30.0),
                station("FAR", 50.0, 10.0, 10.0),
            ],
            at(50.0, 0.0),
            FORECAST_RADIUS_KM,
        )
        .expect("a forecast");
        assert!(forecast.muf3000_mhz > 25.0);
        assert_eq!(forecast.nearest, "Somewhere");
        assert!(forecast.nearest_km < 100.0);
    }

    #[test]
    fn one_distant_site_is_no_forecast() {
        assert!(
            forecast_at(
                &[station("A", 50.0, 0.0, 20.0)],
                at(50.0, 0.0),
                FORECAST_RADIUS_KM
            )
            .is_none()
        );
        assert!(forecast_at(&[], at(50.0, 0.0), FORECAST_RADIUS_KM).is_none());
        let far = [
            station("A", -50.0, 170.0, 20.0),
            station("B", -40.0, 160.0, 20.0),
        ];
        assert!(forecast_at(&far, at(50.0, 0.0), FORECAST_RADIUS_KM).is_none());
    }

    #[test]
    fn a_confident_site_outweighs_a_doubtful_one() {
        let sure = IonosondeStation {
            confidence: Some(100.0),
            ..station("SURE", 50.0, -5.0, 30.0)
        };
        let shaky = IonosondeStation {
            confidence: Some(10.0),
            ..station("SHAKY", 50.0, 5.0, 10.0)
        };
        let forecast =
            forecast_at(&[sure, shaky], at(50.0, 0.0), FORECAST_RADIUS_KM).expect("a forecast");
        assert!(forecast.muf3000_mhz > 20.0);
    }

    fn sondes() -> Vec<IonosondeStation> {
        vec![
            station("W", 51.0, -3.0, 16.0),
            station("E", 52.0, 1.0, 16.0),
        ]
    }

    #[test]
    fn the_gap_between_measured_and_forecast_is_reported() {
        let compared = compare(&[cell("IO91", 51.5, Some(18.0))], &sondes());
        assert_eq!(compared.len(), 1);
        assert!((compared[0].forecast.muf3000_mhz - 16.0).abs() < 1e-6);
        assert!((compared[0].delta_mhz - 2.0).abs() < 1e-6);
    }

    #[test]
    fn unmeasured_or_remote_cells_are_skipped() {
        assert!(compare(&[cell("IO91", 51.5, None)], &sondes()).is_empty());
        let remote = Cell {
            centre: Geo::new(-40.0, 150.0),
            ..cell("QF", -40.0, Some(18.0))
        };
        assert!(compare(&[remote], &sondes()).is_empty());
    }

    #[test]
    fn agreement_says_how_often_the_receiver_beat_the_network() {
        let compared = compare(
            &[
                cell("IO91", 51.5, Some(18.0)),
                cell("IO92", 52.5, Some(20.0)),
                cell("IO90", 50.5, Some(12.0)),
            ],
            &sondes(),
        );
        let summary = agreement(&compared);
        assert_eq!(summary.cells, 3);
        assert_eq!(summary.above, 2);
        assert!((summary.median_delta_mhz - 2.0).abs() < 1e-6);
        assert_eq!(summary.widest_above.as_deref(), Some("IO92"));
        assert_eq!(agreement(&[]), Agreement::default());
    }
}
