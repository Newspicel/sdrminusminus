use crate::runtime::SpectrumSnapshot;

/// The strongest thing on the air in one block, and how far it stands above everything else.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Peak {
    pub(crate) hz: f64,
    pub(crate) db: f32,
    pub(crate) floor_db: f32,
}

/// Finds the loudest carrier in a block without being told where to look.
///
/// The noise floor is the median bin rather than the mean: a strong signal drags a mean up towards
/// itself and hides behind the raised floor, while a median is unmoved by anything occupying a
/// minority of the span.
#[derive(Default)]
pub(crate) struct CloseCall {
    scratch: Vec<f32>,
}

impl CloseCall {
    pub(crate) fn strongest(
        &mut self,
        snapshot: &SpectrumSnapshot,
        margin_db: f32,
    ) -> Option<Peak> {
        let n = snapshot.db.len();
        if n == 0 || !snapshot.span_hz.is_finite() || snapshot.span_hz <= 0.0 {
            return None;
        }
        let guard = snapshot.lo_guard();
        let live = |i: usize| !guard.as_ref().is_some_and(|g| g.contains(&i));

        self.scratch.clear();
        self.scratch
            .extend((0..n).filter(|&i| live(i)).map(|i| snapshot.db[i]));
        let floor_db = median(&mut self.scratch)?;

        let (bin, db) = (0..n)
            .filter(|&i| live(i))
            .map(|i| (i, snapshot.db[i]))
            .fold((0usize, f32::NEG_INFINITY), |best, next| {
                if next.1 > best.1 { next } else { best }
            });
        if !db.is_finite() || db < floor_db + margin_db {
            return None;
        }
        let span = f64::from(snapshot.span_hz);
        Some(Peak {
            hz: snapshot.center_hz + (bin as f64 - n as f64 / 2.0) / n as f64 * span,
            db,
            floor_db,
        })
    }
}

fn median(values: &mut [f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    let mid = values.len() / 2;
    let (_, at, _) = values.select_nth_unstable_by(mid, f32::total_cmp);
    at.is_finite().then_some(*at)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn snapshot(center_hz: f64, lo_hz: f64, db: Vec<f32>) -> SpectrumSnapshot {
        SpectrumSnapshot {
            seq: 1,
            timestamp: 0,
            center_hz,
            span_hz: 1_024_000.0,
            lo_hz,
            db: Arc::from(db.as_slice()),
        }
    }

    #[test]
    fn the_loudest_carrier_is_reported_at_its_own_frequency() {
        let mut db = vec![-95.0f32; 1024];
        db[640] = -40.0;
        let snap = snapshot(100e6, 100e6 - 250e3, db);
        let found = CloseCall::default()
            .strongest(&snap, 12.0)
            .expect("a carrier 55 dB over the floor");
        assert_eq!(found.db, -40.0);
        assert_eq!(found.floor_db, -95.0);
        assert!(
            (found.hz - 100_128_000.0).abs() < 1_000.0,
            "reported {} Hz",
            found.hz
        );
    }

    #[test]
    fn an_empty_band_calls_nothing() {
        let snap = snapshot(100e6, 100e6 - 250e3, vec![-95.0f32; 1024]);
        assert_eq!(CloseCall::default().strongest(&snap, 12.0), None);
    }

    #[test]
    fn a_carrier_that_barely_clears_the_noise_is_not_a_close_call() {
        let mut db = vec![-95.0f32; 1024];
        db[300] = -89.0;
        let snap = snapshot(100e6, 100e6 - 250e3, db);
        assert_eq!(
            CloseCall::default().strongest(&snap, 12.0),
            None,
            "6 dB is not the 12 dB asked for"
        );
        assert!(CloseCall::default().strongest(&snap, 4.0).is_some());
    }

    #[test]
    fn the_front_ends_own_spike_is_never_the_close_call() {
        let mut db = vec![-95.0f32; 1024];
        db[512] = 0.0;
        let snap = snapshot(100e6, 100e6, db);
        assert_eq!(
            CloseCall::default().strongest(&snap, 12.0),
            None,
            "the receiver called itself"
        );
    }

    #[test]
    fn a_busy_band_does_not_raise_the_floor_over_its_own_signals() {
        let mut db = vec![-95.0f32; 1024];
        for bin in (0..400).step_by(4) {
            db[bin] = -60.0;
        }
        db[900] = -30.0;
        let snap = snapshot(100e6, 100e6 - 250e3, db);
        let found = CloseCall::default()
            .strongest(&snap, 12.0)
            .expect("the strongest of many");
        assert_eq!(found.db, -30.0);
        assert_eq!(found.floor_db, -95.0, "a mean would have hidden the floor");
    }

    #[test]
    fn a_span_with_nothing_in_it_is_refused_rather_than_guessed() {
        let mut empty = snapshot(100e6, 100e6, Vec::new());
        assert_eq!(CloseCall::default().strongest(&empty, 12.0), None);
        empty = snapshot(100e6, 100e6, vec![-95.0; 8]);
        empty.span_hz = 0.0;
        assert_eq!(CloseCall::default().strongest(&empty, 12.0), None);
    }
}
