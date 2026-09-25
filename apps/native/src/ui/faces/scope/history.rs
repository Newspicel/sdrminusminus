use super::traces::{DbWindow, requantize};
use super::view::above;

pub const HISTORY_ROWS: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowMeta {
    pub centre_hz: f64,
    pub span_hz: f64,
    pub db_min: f64,
    pub db_max: f64,
    pub at_ms: f64,
}

impl RowMeta {
    #[must_use]
    pub fn window(&self) -> DbWindow {
        DbWindow {
            min: self.db_min,
            max: self.db_max,
        }
    }

    #[must_use]
    pub fn key(&self) -> FrequencyKey {
        FrequencyKey {
            centre_hz: self.centre_hz,
            span_hz: self.span_hz,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpectrumHistory {
    pub rows: Vec<u8>,
    pub count: usize,
    pub bins: usize,
    pub meta: Vec<RowMeta>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrequencyKey {
    pub centre_hz: f64,
    pub span_hz: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameKey {
    pub centre_hz: f64,
    pub span_hz: f64,
    pub bins: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Retune {
    None,
    Shift(f64),
    Reseed,
}

#[must_use]
pub fn resample_rows(rows: &[u8], count: usize, from_bins: usize, to_bins: usize) -> Vec<u8> {
    let mut out = vec![0u8; count * to_bins];
    if from_bins == 0 || to_bins == 0 {
        return out;
    }
    for row in 0..count {
        let from = row * from_bins;
        let to = row * to_bins;
        for x in 0..to_bins {
            let low = (x * from_bins / to_bins).min(from_bins - 1);
            let high = ((x + 1) * from_bins)
                .div_ceil(to_bins)
                .saturating_sub(1)
                .max(low)
                .min(from_bins - 1);
            out[to + x] = rows[from + low..=from + high]
                .iter()
                .copied()
                .max()
                .unwrap_or(0);
        }
    }
    out
}

#[derive(Default)]
pub struct History {
    ring: Vec<u8>,
    meta: Vec<Option<RowMeta>>,
    bins: usize,
    write: usize,
    filled: usize,
}

impl History {
    pub fn record(&mut self, bins: &[u8], meta: RowMeta) {
        if bins.is_empty() {
            return;
        }
        if bins.len() != self.bins {
            if self.filled > 0 {
                self.ring = resample_rows(&self.ring, HISTORY_ROWS, self.bins, bins.len());
            } else {
                self.ring = vec![0; bins.len() * HISTORY_ROWS];
                self.meta = vec![None; HISTORY_ROWS];
                self.write = 0;
            }
            self.bins = bins.len();
        }
        if self.meta.len() != HISTORY_ROWS {
            self.meta = vec![None; HISTORY_ROWS];
        }
        let at = self.write * self.bins;
        self.ring[at..at + self.bins].copy_from_slice(bins);
        self.meta[self.write] = Some(meta);
        self.write = (self.write + 1) % HISTORY_ROWS;
        self.filled = (self.filled + 1).min(HISTORY_ROWS);
    }

    #[must_use]
    pub fn newest(&self) -> Option<RowMeta> {
        (self.filled > 0)
            .then(|| self.meta[(self.write + HISTORY_ROWS - 1) % HISTORY_ROWS])
            .flatten()
    }

    #[must_use]
    pub fn read(&self) -> SpectrumHistory {
        let first = (self.write + HISTORY_ROWS - self.filled) % HISTORY_ROWS;
        let head = self.filled.min(HISTORY_ROWS - first);
        let mut rows = Vec::with_capacity(self.filled * self.bins);
        rows.extend_from_slice(&self.ring[first * self.bins..(first + head) * self.bins]);
        rows.extend_from_slice(&self.ring[..(self.filled - head) * self.bins]);
        let meta = (0..self.filled)
            .filter_map(|at| self.meta[(first + at) % HISTORY_ROWS])
            .collect();
        SpectrumHistory {
            rows,
            count: self.filled,
            bins: self.bins,
            meta,
        }
    }
}

#[must_use]
pub fn retune_action(previous: Option<FrameKey>, next: FrameKey) -> Retune {
    let Some(previous) = previous else {
        return Retune::None;
    };
    if previous == next {
        return Retune::None;
    }
    if previous.span_hz == next.span_hz && previous.bins == next.bins && next.span_hz > 0.0 {
        return Retune::Shift((next.centre_hz - previous.centre_hz) / next.span_hz);
    }
    Retune::Reseed
}

#[must_use]
pub fn seed_target(history: &SpectrumHistory, frame: Option<FrequencyKey>) -> Option<FrequencyKey> {
    if let Some(frame) = frame.filter(|frame| frame.span_hz > 0.0) {
        return Some(frame);
    }
    history.meta.last().map(RowMeta::key)
}

#[must_use]
pub fn seed_rows(
    history: &SpectrumHistory,
    frame: Option<FrequencyKey>,
    held: Option<DbWindow>,
) -> Vec<u8> {
    if let Some(target) = seed_target(history, frame) {
        return align_history(history, target, held);
    }
    match held {
        Some(held) => requantize_history(history, held),
        None => history.rows.clone(),
    }
}

#[must_use]
pub fn requantize_history(history: &SpectrumHistory, to: DbWindow) -> Vec<u8> {
    let mut out = history.rows.clone();
    let mut scratch = Vec::new();
    for row in 0..history.count {
        let Some(meta) = history.meta.get(row) else {
            continue;
        };
        let at = row * history.bins;
        requantize(
            &history.rows[at..at + history.bins],
            meta.window(),
            to,
            &mut scratch,
        );
        out[at..at + history.bins].copy_from_slice(&scratch);
    }
    out
}

#[must_use]
pub fn bin_shift(row: FrequencyKey, to: FrequencyKey, bins: usize) -> Option<i64> {
    if !above(to.span_hz, 0.0) || row.span_hz != to.span_hz {
        return None;
    }
    let shift = ((to.centre_hz - row.centre_hz) / to.span_hz * bins as f64).round() as i64;
    (shift.unsigned_abs() < bins as u64).then_some(shift)
}

#[must_use]
pub fn align_history(
    history: &SpectrumHistory,
    to: FrequencyKey,
    held: Option<DbWindow>,
) -> Vec<u8> {
    let bins = history.bins;
    let mut out = vec![0u8; history.count * bins];
    let mut scratch = Vec::new();
    for row in 0..history.count {
        let at = row * bins;
        let source = &history.rows[at..at + bins];
        let Some(meta) = history.meta.get(row) else {
            out[at..at + bins].copy_from_slice(source);
            continue;
        };
        let Some(shift) = bin_shift(meta.key(), to, bins) else {
            continue;
        };
        let placed: &[u8] = match held {
            Some(held) => {
                requantize(source, meta.window(), held, &mut scratch);
                &scratch
            }
            None => source,
        };
        let shift_by = shift.unsigned_abs() as usize;
        if shift >= 0 {
            out[at..at + bins - shift_by].copy_from_slice(&placed[shift_by..]);
        } else {
            out[at + shift_by..at + bins].copy_from_slice(&placed[..bins - shift_by]);
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeedPlacement {
    pub skip: usize,
    pub rows: usize,
    pub write: usize,
}

#[must_use]
pub fn seed_placement(count: usize, rings: usize) -> SeedPlacement {
    let rows = count.min(rings);
    SeedPlacement {
        skip: count - rows,
        rows,
        write: if rings > 0 { rows % rings } else { 0 },
    }
}

#[must_use]
pub fn next_ring_row(row: usize, rings: usize) -> usize {
    if rings > 0 { (row + 1) % rings } else { 0 }
}

#[must_use]
pub fn rows_for_height(height_px: f64, ratio: f64, rings: usize) -> f64 {
    let ratio = if ratio > 0.0 { ratio } else { 1.0 };
    (height_px / ratio).round().clamp(2.0, rings as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(centre_hz: f64, span_hz: f64, db_min: f64, db_max: f64) -> RowMeta {
        RowMeta {
            centre_hz,
            span_hz,
            db_min,
            db_max,
            at_ms: 0.0,
        }
    }

    fn plain(centre_hz: f64) -> RowMeta {
        meta(centre_hz, 2e6, -100.0, -20.0)
    }

    fn history(rows: &[&[u8]], meta: Vec<RowMeta>) -> SpectrumHistory {
        SpectrumHistory {
            rows: rows.concat(),
            count: rows.len(),
            bins: rows.first().map_or(0, |row| row.len()),
            meta,
        }
    }

    fn rows_of(history: &SpectrumHistory) -> Vec<Vec<u8>> {
        history
            .rows
            .chunks(history.bins)
            .map(<[u8]>::to_vec)
            .collect()
    }

    fn key(centre_hz: f64, span_hz: f64, bins: usize) -> FrameKey {
        FrameKey {
            centre_hz,
            span_hz,
            bins,
        }
    }

    #[test]
    fn nothing_moves_before_the_first_frame_or_while_the_tuning_holds() {
        let held = key(100e6, 2e6, 1024);
        assert_eq!(retune_action(None, held), Retune::None);
        assert_eq!(retune_action(Some(held), held), Retune::None);
    }

    #[test]
    fn a_centre_move_shifts_by_its_share_of_the_span() {
        let held = key(100e6, 2e6, 1024);
        assert_eq!(
            retune_action(Some(held), key(100.5e6, 2e6, 1024)),
            Retune::Shift(0.25)
        );
        assert_eq!(
            retune_action(Some(held), key(99.5e6, 2e6, 1024)),
            Retune::Shift(-0.25)
        );
    }

    #[test]
    fn a_span_or_resolution_change_reseeds() {
        let held = key(100e6, 2e6, 1024);
        assert_eq!(
            retune_action(Some(held), key(100e6, 1e6, 1024)),
            Retune::Reseed
        );
        assert_eq!(
            retune_action(Some(held), key(100e6, 2e6, 2048)),
            Retune::Reseed
        );
        assert_eq!(
            retune_action(Some(held), key(101e6, 0.0, 1024)),
            Retune::Reseed
        );
    }

    #[test]
    fn a_row_shifts_by_whole_bins_or_gives_up() {
        let row = FrequencyKey {
            centre_hz: 100e6,
            span_hz: 4e6,
        };
        let to = |centre_hz: f64, span_hz: f64| FrequencyKey { centre_hz, span_hz };
        assert_eq!(bin_shift(row, to(101e6, 4e6), 4), Some(1));
        assert_eq!(bin_shift(row, to(99e6, 4e6), 4), Some(-1));
        assert_eq!(bin_shift(row, row, 4), Some(0));
        assert_eq!(bin_shift(row, to(100e6, 2e6), 4), None);
        assert_eq!(bin_shift(row, to(104e6, 4e6), 4), None);
        assert_eq!(bin_shift(row, to(100e6, 0.0), 4), None);
    }

    #[test]
    fn an_old_row_slides_to_keep_its_absolute_frequency() {
        let past = history(
            &[&[1, 2, 3, 4], &[5, 6, 7, 8]],
            vec![plain(100e6), plain(100.5e6)],
        );
        let target = FrequencyKey {
            centre_hz: 100.5e6,
            span_hz: 2e6,
        };
        let aligned = align_history(&past, target, None);
        assert_eq!(aligned, [2, 3, 4, 0, 5, 6, 7, 8]);
        let down = history(&[&[1, 2, 3, 4]], vec![plain(100e6)]);
        let lower = FrequencyKey {
            centre_hz: 99.5e6,
            span_hz: 2e6,
        };
        assert_eq!(align_history(&down, lower, None), [0, 1, 2, 3]);
    }

    #[test]
    fn a_row_at_another_span_is_blanked_and_one_without_meta_kept() {
        let target = FrequencyKey {
            centre_hz: 100e6,
            span_hz: 2e6,
        };
        let past = history(
            &[&[1, 2, 3, 4], &[5, 6, 7, 8]],
            vec![meta(100e6, 1e6, -100.0, -20.0), plain(100e6)],
        );
        assert_eq!(align_history(&past, target, None), [0, 0, 0, 0, 5, 6, 7, 8]);
        let bare = history(&[&[9, 9, 9, 9]], Vec::new());
        assert_eq!(align_history(&bare, target, None), [9, 9, 9, 9]);
    }

    #[test]
    fn a_sliding_row_is_requantized_into_a_held_window() {
        let past = history(&[&[255, 255, 255, 255]], vec![plain(100.5e6)]);
        let target = FrequencyKey {
            centre_hz: 100e6,
            span_hz: 2e6,
        };
        let held = DbWindow {
            min: -100.0,
            max: 0.0,
        };
        assert_eq!(align_history(&past, target, Some(held)), [0, 204, 204, 204]);
    }

    #[test]
    fn history_rows_measured_under_different_windows_share_one_scale() {
        let past = history(
            &[&[159, 0], &[127, 0]],
            vec![
                meta(100e6, 2e6, -100.0, -20.0),
                meta(100e6, 2e6, -100.0, 0.0),
            ],
        );
        let to = DbWindow {
            min: -100.0,
            max: 0.0,
        };
        let rows = requantize_history(&past, to);
        assert_eq!(rows[0], 127);
        assert_eq!(rows[2], 127);
        let bare = history(&[&[7, 9]], Vec::new());
        assert_eq!(requantize_history(&bare, to), [7, 9]);
    }

    #[test]
    fn resampling_keeps_a_narrow_peak_and_stretches_a_short_row() {
        assert_eq!(resample_rows(&[0, 9, 0, 0], 1, 4, 2), [9, 0]);
        assert_eq!(resample_rows(&[1, 2], 1, 2, 4), [1, 1, 2, 2]);
    }

    #[test]
    fn a_history_is_empty_until_a_row_arrives() {
        let held = History::default();
        assert_eq!(held.read(), SpectrumHistory::default());
        assert_eq!(held.newest(), None);
    }

    #[test]
    fn a_history_reads_oldest_first_with_its_windows() {
        let mut held = History::default();
        held.record(&[1, 2], meta(100e6, 2e6, -110.0, -20.0));
        held.record(&[3, 4], meta(100e6, 2e6, -100.0, 0.0));
        let read = held.read();
        assert_eq!(rows_of(&read), [vec![1, 2], vec![3, 4]]);
        assert_eq!(read.meta[0].db_min, -110.0);
        assert_eq!(read.meta[1].db_max, 0.0);
        assert_eq!(held.newest().map(|meta| meta.db_min), Some(-100.0));
    }

    #[test]
    fn a_history_keeps_the_newest_rows_once_the_ring_wraps() {
        let mut held = History::default();
        for row in 0..HISTORY_ROWS + 2 {
            held.record(&[(row & 0xff) as u8, 0], plain(100e6));
        }
        let read = held.read();
        assert_eq!(read.count, HISTORY_ROWS);
        let rows = rows_of(&read);
        assert_eq!(rows[0], [2, 0]);
        assert_eq!(rows[rows.len() - 1], [((HISTORY_ROWS + 1) & 0xff) as u8, 0]);
    }

    #[test]
    fn a_history_rescales_what_it_held_when_the_bin_count_changes() {
        let mut held = History::default();
        held.record(&[1, 2], plain(100e6));
        held.record(&[7, 8, 9], plain(100e6));
        let read = held.read();
        assert_eq!(rows_of(&read), [vec![1, 2, 2], vec![7, 8, 9]]);
        assert_eq!(read.meta.len(), 2);
    }

    #[test]
    fn seeding_lays_rows_at_the_bottom_and_keeps_the_newest() {
        let place = |skip, rows, write| SeedPlacement { skip, rows, write };
        assert_eq!(seed_placement(3, 8), place(0, 3, 3));
        assert_eq!(seed_placement(10, 8), place(2, 8, 0));
        assert_eq!(seed_placement(8, 8), place(0, 8, 0));
        assert_eq!(seed_placement(0, 8), place(0, 0, 0));
    }

    #[test]
    fn the_ring_cursor_wraps_and_visits_every_row() {
        assert_eq!(next_ring_row(0, 4), 1);
        assert_eq!(next_ring_row(3, 4), 0);
        let mut row = 0;
        let mut seen = std::collections::HashSet::new();
        for _ in 0..8 {
            seen.insert(row);
            row = next_ring_row(row, 8);
        }
        assert_eq!(seen.len(), 8);
        assert_eq!(row, 0);
    }

    #[test]
    fn one_history_row_is_shown_per_layout_pixel() {
        assert_eq!(rows_for_height(600.0, 2.0, 1024), 300.0);
        assert_eq!(rows_for_height(300.0, 1.0, 1024), 300.0);
        assert_eq!(rows_for_height(4096.0, 1.0, 1024), 1024.0);
        assert_eq!(rows_for_height(1.0, 2.0, 1024), 2.0);
    }
}
