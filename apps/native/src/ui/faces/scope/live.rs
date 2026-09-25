use std::{cell::RefCell, collections::HashMap, sync::OnceLock, time::Instant};

use sdrmm_wire::frame::SpectrumFrame;

use super::{
    colormap::Colormap,
    gpu::WaterfallFeed,
    history::{
        FrameKey, FrequencyKey, History, Retune, RowMeta, SpectrumHistory, align_history,
        retune_action, seed_rows,
    },
    plot::DensityLayer,
    traces::{
        DbWindow, FrameTween, ReadoutHold, TraceState, VideoAverage, dequantize, quantize_db,
        requantize,
    },
    view::SpectrumView,
};

const LANE_GRACE_MS: f64 = 5_000.0;

#[must_use]
pub fn now_ms() -> f64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameMeta {
    pub centre_hz: f64,
    pub span_hz: f64,
    pub db_min: f64,
    pub db_max: f64,
}

impl FrameMeta {
    #[must_use]
    pub fn of(frame: &SpectrumFrame<'_>) -> Self {
        Self {
            centre_hz: frame.center_hz,
            span_hz: f64::from(frame.span_hz),
            db_min: f64::from(frame.db_min),
            db_max: f64::from(frame.db_max),
        }
    }

    #[must_use]
    pub fn window(self) -> DbWindow {
        DbWindow {
            min: self.db_min,
            max: self.db_max,
        }
    }

    #[must_use]
    pub fn frequency(self) -> FrequencyKey {
        FrequencyKey {
            centre_hz: self.centre_hz,
            span_hz: self.span_hz,
        }
    }
}

#[derive(Default)]
struct Lane {
    history: History,
    last_seq: Option<u32>,
    latest: Option<(FrameMeta, usize)>,
}

thread_local! {
    static LANES: RefCell<HashMap<(u32, u32), Lane>> = RefCell::new(HashMap::new());
}

pub fn record(lane: (u32, u32), frame: &SpectrumFrame<'_>) {
    LANES.with(|lanes| {
        let mut lanes = lanes.borrow_mut();
        let held = lanes.entry(lane).or_default();
        if held.last_seq == Some(frame.seq) {
            return;
        }
        held.last_seq = Some(frame.seq);
        let meta = FrameMeta::of(frame);
        held.latest = Some((meta, frame.bins.len()));
        held.history.record(
            frame.bins,
            RowMeta {
                centre_hz: meta.centre_hz,
                span_hz: meta.span_hz,
                db_min: meta.db_min,
                db_max: meta.db_max,
                at_ms: now_ms(),
            },
        );
    });
}

fn fresh(lane: &Lane) -> bool {
    lane.history
        .newest()
        .is_some_and(|newest| now_ms() - newest.at_ms < LANE_GRACE_MS)
}

#[must_use]
pub fn lane_history(lane: (u32, u32)) -> SpectrumHistory {
    LANES.with(|lanes| {
        lanes
            .borrow()
            .get(&lane)
            .filter(|held| fresh(held))
            .map(|held| held.history.read())
            .unwrap_or_default()
    })
}

#[must_use]
pub fn lane_latest(lane: (u32, u32)) -> Option<(FrameMeta, usize)> {
    LANES.with(|lanes| {
        lanes
            .borrow()
            .get(&lane)
            .filter(|held| fresh(held))
            .and_then(|held| held.latest)
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub range: Option<DbWindow>,
    pub average: u32,
    pub view: SpectrumView,
    pub phosphor: bool,
    pub colormap: Colormap,
}

pub type Shown = (u64, u64);

#[derive(Default)]
pub struct Live {
    pub frame: Option<FrameMeta>,
    pub key: Option<FrameKey>,
    pub raw: Vec<f32>,
    pub tween: FrameTween,
    pub video: VideoAverage,
    pub traces: Option<TraceState>,
    pub density: Option<DensityLayer>,
    pub readout: ReadoutHold,
    row: Vec<u8>,
    revision: u64,
    received: u64,
}

impl Live {
    #[must_use]
    pub fn seeded(latest: Option<(FrameMeta, usize)>) -> Self {
        Self {
            frame: latest.map(|(meta, _)| meta),
            key: latest.map(|(meta, bins)| FrameKey {
                centre_hz: meta.centre_hz,
                span_hz: meta.span_hz,
                bins,
            }),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn now_ms(&self) -> f64 {
        now_ms()
    }

    #[must_use]
    pub fn shown(&self, _now: f64) -> Shown {
        (
            self.revision,
            self.density.as_ref().map_or(0, |layer| layer.revision),
        )
    }

    pub fn sync_density(&mut self, on: bool, colormap: Colormap) {
        match (&mut self.density, on) {
            (Some(layer), true) => layer.set_colormap(colormap),
            (None, true) => self.density = Some(DensityLayer::new(colormap)),
            (_, false) => self.density = None,
        }
    }

    pub fn clear_density(&mut self) {
        if let Some(layer) = self.density.as_mut() {
            layer.clear();
        }
    }

    pub fn receive(
        &mut self,
        frame: &SpectrumFrame<'_>,
        settings: Settings,
        feed: &mut WaterfallFeed,
        history: impl FnOnce() -> SpectrumHistory,
    ) -> bool {
        let meta = FrameMeta::of(frame);
        let key = FrameKey {
            centre_hz: meta.centre_hz,
            span_hz: meta.span_hz,
            bins: frame.bins.len(),
        };
        let action = retune_action(self.key, key);
        self.key = Some(key);
        self.frame = Some(meta);
        let window = settings.range.unwrap_or(meta.window());
        dequantize(frame.bins, meta.window(), &mut self.raw);
        if action != Retune::None {
            self.video.reset();
        }
        let averaged = settings.average > 1;
        let db = self.video.apply(&self.raw, settings.average).to_vec();
        let now = now_ms();
        if action == Retune::None {
            self.tween.push(&db, now);
        } else {
            self.tween.jump(&db, now);
        }
        let seeded = self.retune(action, meta, settings.range, feed, history);
        if !seeded {
            self.waterfall_row(frame.bins, &db, averaged, meta, settings.range);
            feed.push_row(&self.row);
        }
        TraceState::accumulate(&mut self.traces, &db);
        if let Some(layer) = self.density.as_mut() {
            layer.add(&db, settings.view, window);
        }
        self.revision += 1;
        self.received += 1;
        self.received == 1 || self.received.is_multiple_of(8)
    }

    fn retune(
        &mut self,
        action: Retune,
        meta: FrameMeta,
        held: Option<DbWindow>,
        feed: &mut WaterfallFeed,
        history: impl FnOnce() -> SpectrumHistory,
    ) -> bool {
        if action == Retune::None {
            return false;
        }
        self.traces = None;
        self.clear_density();
        match action {
            Retune::Shift(delta) => {
                feed.shift_rows(delta);
                false
            }
            _ => {
                let rows = history();
                if rows.count == 0 {
                    return false;
                }
                feed.seed(
                    align_history(&rows, meta.frequency(), held),
                    rows.count,
                    rows.bins,
                );
                true
            }
        }
    }

    fn waterfall_row(
        &mut self,
        bins: &[u8],
        shown: &[f32],
        averaged: bool,
        meta: FrameMeta,
        held: Option<DbWindow>,
    ) {
        if averaged {
            quantize_db(shown, held.unwrap_or(meta.window()), &mut self.row);
            return;
        }
        match held {
            None => {
                self.row.clear();
                self.row.extend_from_slice(bins);
            }
            Some(held) => requantize(bins, meta.window(), held, &mut self.row),
        }
    }
}

pub fn reseed(
    feed: &mut WaterfallFeed,
    lane: (u32, u32),
    frame: Option<FrameMeta>,
    held: Option<DbWindow>,
) {
    let past = lane_history(lane);
    if past.count == 0 {
        return;
    }
    let rows = seed_rows(&past, frame.map(FrameMeta::frequency), held);
    feed.seed(rows, past.count, past.bins);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(seq: u32, centre_hz: f64, bins: &[u8]) -> SpectrumFrame<'_> {
        SpectrumFrame {
            stream_id: 1,
            seq,
            timestamp: 0,
            center_hz: centre_hz,
            span_hz: 2e6,
            db_min: -100.0,
            db_max: -20.0,
            bins,
        }
    }

    fn settings(average: u32) -> Settings {
        Settings {
            range: None,
            average,
            view: SpectrumView::default(),
            phosphor: false,
            colormap: Colormap::Classic,
        }
    }

    #[test]
    fn a_lane_records_each_frame_once_however_many_scopes_hear_it() {
        let lane = (90, 0);
        record(lane, &frame(1, 100e6, &[1, 2]));
        record(lane, &frame(1, 100e6, &[1, 2]));
        record(lane, &frame(2, 100e6, &[3, 4]));
        assert_eq!(lane_history(lane).count, 2);
        assert_eq!(lane_latest(lane).map(|(_, bins)| bins), Some(2));
        assert_eq!(lane_history((91, 0)).count, 0);
    }

    #[test]
    fn metadata_is_due_on_the_first_frame_and_every_eighth() {
        let mut live = Live::default();
        let mut feed = WaterfallFeed::default();
        let due: Vec<bool> = (0..9)
            .map(|seq| {
                live.receive(
                    &frame(seq, 100e6, &[10, 20]),
                    settings(1),
                    &mut feed,
                    SpectrumHistory::default,
                )
            })
            .collect();
        assert_eq!(
            due,
            [true, false, false, false, false, false, false, true, false]
        );
        assert_eq!(live.traces.as_ref().map(|traces| traces.frames), Some(9));
    }

    #[test]
    fn a_retune_restarts_the_traces() {
        let mut live = Live::default();
        let mut feed = WaterfallFeed::default();
        live.receive(
            &frame(0, 100e6, &[10, 20]),
            settings(1),
            &mut feed,
            SpectrumHistory::default,
        );
        live.receive(
            &frame(1, 100e6, &[10, 20]),
            settings(1),
            &mut feed,
            SpectrumHistory::default,
        );
        live.receive(
            &frame(2, 100.5e6, &[10, 20]),
            settings(1),
            &mut feed,
            SpectrumHistory::default,
        );
        assert_eq!(live.traces.as_ref().map(|traces| traces.frames), Some(1));
    }

    #[test]
    fn an_averaged_frame_is_requantized_for_the_waterfall() {
        let mut live = Live::default();
        let mut feed = WaterfallFeed::default();
        live.receive(
            &frame(0, 100e6, &[0, 255]),
            settings(4),
            &mut feed,
            SpectrumHistory::default,
        );
        assert_eq!(live.row, [0, 255]);
        let held = Settings {
            range: Some(DbWindow {
                min: -100.0,
                max: 0.0,
            }),
            ..settings(1)
        };
        live.receive(
            &frame(1, 100e6, &[0, 255]),
            held,
            &mut feed,
            SpectrumHistory::default,
        );
        assert_eq!(live.row, [0, 204]);
    }

    #[test]
    fn phosphor_follows_its_switch_and_colours() {
        let mut live = Live::default();
        live.sync_density(true, Colormap::Classic);
        assert!(live.density.is_some());
        live.sync_density(false, Colormap::Classic);
        assert!(live.density.is_none());
    }
}
