use std::{
    collections::HashSet,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

use tokio::sync::broadcast;

use super::TrunkInput;
use crate::{Engine, occupancy::OCCUPIED_MARGIN_DB, runtime::SpectrumSnapshot};

pub(crate) struct Watch {
    pub device_set: u32,
    pub stream: u32,
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

impl Watch {
    pub(crate) fn live(&self) -> bool {
        !self.done.load(Ordering::Relaxed)
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Watch {
    pub(crate) fn start(
        engine: &Arc<Engine>,
        tx: mpsc::Sender<TrunkInput>,
        device_set: u32,
        stream: u32,
    ) -> Option<Self> {
        let rx = engine.subscribe_spectrum(device_set, stream).ok()?;
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let alive = stop.clone();
        let finished = done.clone();
        let weak = Arc::downgrade(engine);
        let spawned = std::thread::Builder::new()
            .name(format!("sdrmm-trunk-carriers-{device_set}"))
            .spawn(move || {
                run(&weak, &alive, rx, &tx, device_set);
                finished.store(true, Ordering::Relaxed);
            });
        if let Err(error) = spawned {
            tracing::warn!(device_set, %error, "could not watch the band for carriers");
            return None;
        }
        Some(Self {
            device_set,
            stream,
            stop,
            done,
        })
    }
}

fn run(
    engine: &Weak<Engine>,
    stop: &Arc<AtomicBool>,
    mut rx: broadcast::Receiver<SpectrumSnapshot>,
    tx: &mpsc::Sender<TrunkInput>,
    device_set: u32,
) {
    while !stop.load(Ordering::Relaxed) && engine.strong_count() > 0 {
        let snapshot = match rx.blocking_recv() {
            Ok(snapshot) => snapshot,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        };
        let busy = occupied(&snapshot);
        if busy.is_empty() {
            continue;
        }
        if tx
            .send(TrunkInput::Carriers {
                device_set,
                freq_hz: busy,
            })
            .is_err()
        {
            return;
        }
    }
}

fn occupied(snapshot: &SpectrumSnapshot) -> Vec<u64> {
    let bins = snapshot.db.len();
    if bins == 0 || !snapshot.span_hz.is_finite() || snapshot.span_hz <= 0.0 {
        return Vec::new();
    }
    let Some(floor) = noise_floor(&snapshot.db) else {
        return Vec::new();
    };
    let threshold = floor + OCCUPIED_MARGIN_DB;
    let guard = snapshot.lo_guard();
    let bin_hz = f64::from(snapshot.span_hz) / bins as f64;
    let first_hz = snapshot.center_hz - f64::from(snapshot.span_hz) / 2.0;
    let mut seen: HashSet<u64> = HashSet::new();
    for (i, level) in snapshot.db.iter().enumerate() {
        if !level.is_finite()
            || *level < threshold
            || guard.as_ref().is_some_and(|guard| guard.contains(&i))
        {
            continue;
        }
        let hz = first_hz + (i as f64 + 0.5) * bin_hz;
        if hz > 0.0 {
            seen.insert(hz as u64);
        }
    }
    let mut busy: Vec<u64> = seen.into_iter().collect();
    busy.sort_unstable();
    busy
}

fn noise_floor(db: &[f32]) -> Option<f32> {
    let mut finite: Vec<f32> = db
        .iter()
        .copied()
        .filter(|level| level.is_finite())
        .collect();
    let last = finite.len().checked_sub(1)?;
    let at = last / 4;
    let (_, nth, _) = finite.select_nth_unstable_by(at, f32::total_cmp);
    Some(*nth)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(db: Vec<f32>) -> SpectrumSnapshot {
        SpectrumSnapshot {
            seq: 0,
            timestamp: 0,
            center_hz: 451_000_000.0,
            span_hz: 1_024_000.0,
            lo_hz: 451_000_000.0,
            db: db.into(),
        }
    }

    #[test]
    fn a_quiet_band_reports_no_carriers() {
        assert!(occupied(&snapshot(vec![-100.0; 1024])).is_empty());
    }

    #[test]
    fn a_carrier_is_reported_at_the_frequency_it_sits_on() {
        let mut db = vec![-100.0f32; 1024];
        db[768] = -40.0;

        let busy = occupied(&snapshot(db));

        assert_eq!(busy.len(), 1);
        let expected = 451_000_000 - 512_000 + 768 * 1000 + 500;
        assert_eq!(busy[0], expected);
    }

    #[test]
    fn the_local_oscillator_artifact_is_never_called_a_carrier() {
        let mut db = vec![-100.0f32; 1024];
        db[512] = -20.0;

        assert!(
            occupied(&snapshot(db)).is_empty(),
            "the receiver's own DC term was reported as traffic"
        );
    }

    #[test]
    fn a_band_with_no_quiet_bins_reports_nothing() {
        assert!(occupied(&snapshot(vec![-40.0; 1024])).is_empty());
    }
}
