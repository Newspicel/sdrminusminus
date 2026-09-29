use std::{collections::HashMap, time::Duration};

use axum::extract::ws::Message;
use sdrmm_wire::{
    FusionGridOwned, RangeDopplerOwned, ServerEvent, SpatialSpectrumOwned, StreamKind, SurfaceFit,
    SurfaceFrame, VisibilityOwned,
};
use tokio::{
    sync::broadcast::{self, error::RecvError},
    task::JoinHandle,
    time::Instant,
};

use super::{MEDIA_ID_BASE, Outbox, Session, alloc_stream_id, media_id_live, text_event};
use crate::surfaces::SurfaceItem;

const FIT_NOT_POSITIVE: &str = "fit must be positive";
const NO_STREAM_IDS: &str = "no free media stream ids on this connection";
const SKIPS_TOLD_EVERY: Duration = Duration::from_secs(5);

struct Watching {
    stream_id: u16,
    kind: StreamKind,
    task: JoinHandle<()>,
}

#[derive(Default)]
pub(super) struct Surfaces {
    streams: HashMap<String, Watching>,
}

impl Surfaces {
    pub(super) fn holds(&self, stream_id: u16) -> bool {
        self.streams
            .values()
            .any(|watching| watching.stream_id == stream_id && !watching.task.is_finished())
    }

    fn remove(&mut self, node: &str) -> Option<(u16, StreamKind)> {
        let watching = self.streams.remove(node)?;
        let ended = watching.task.is_finished();
        watching.task.abort();
        (!ended).then_some((watching.stream_id, watching.kind))
    }

    pub(super) fn abort(self) {
        for watching in self.streams.into_values() {
            watching.task.abort();
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct SurfaceStream {
    pub(super) stream_id: u16,
    pub(super) kind: StreamKind,
    pub(super) fit: Option<SurfaceFit>,
}

impl Session {
    pub(super) async fn subscribe_surface(&mut self, node: String, fit: Option<SurfaceFit>) {
        if fit.is_some_and(|fit| fit.cols == 0 || fit.rows == 0) {
            self.send_error(FIT_NOT_POSITIVE).await;
            return;
        }
        let Some((kind, frames)) = self.state.surfaces.subscribe(&node) else {
            self.send_error(format!("no surface {node}")).await;
            return;
        };
        self.unsubscribe_surface(&node).await;
        let live = |id: u16| {
            media_id_live(
                &self.audio,
                &self.video,
                &self.iq,
                &self.symbols,
                &self.surfaces,
                id,
            )
        };
        let Some(stream_id) =
            alloc_stream_id(&mut self.next_media_id, MEDIA_ID_BASE..=u16::MAX, live)
        else {
            self.send_error(NO_STREAM_IDS).await;
            return;
        };
        let started = ServerEvent::SurfaceStreamStarted {
            stream_id,
            node: node.clone(),
            kind,
        };
        let _ = self.out.send(text_event(&started)).await;
        let stream = SurfaceStream {
            stream_id,
            kind,
            fit,
        };
        let task = spawn(stream, frames, self.out.clone());
        self.surfaces.streams.insert(
            node,
            Watching {
                stream_id,
                kind,
                task,
            },
        );
    }

    pub(super) async fn unsubscribe_surface(&mut self, node: &str) {
        if let Some((stream_id, kind)) = self.surfaces.remove(node) {
            let stopped = ServerEvent::StreamStopped { stream_id, kind };
            let _ = self.out.send(text_event(&stopped)).await;
        }
    }
}

#[derive(Debug, Default)]
struct Skips {
    total: u64,
    told: Option<Instant>,
}

impl Skips {
    fn add(&mut self, count: u64, now: Instant) -> Option<u64> {
        self.total = self.total.saturating_add(count);
        let due = self
            .told
            .is_none_or(|told| now.saturating_duration_since(told) >= SKIPS_TOLD_EVERY);
        due.then(|| {
            self.told = Some(now);
            self.total
        })
    }
}

pub(super) fn spawn(
    stream: SurfaceStream,
    frames: broadcast::Receiver<SurfaceItem>,
    out: Outbox,
) -> JoinHandle<()> {
    tokio::spawn(forward(stream, frames, out))
}

async fn forward(stream: SurfaceStream, mut frames: broadcast::Receiver<SurfaceItem>, out: Outbox) {
    let mut skips = Skips::default();
    loop {
        match frames.recv().await {
            Ok((_, frame)) => {
                let bytes = encode(&frame, stream.stream_id, stream.fit);
                if out.send(Message::Binary(bytes.into())).await.is_err() {
                    break;
                }
            }
            Err(RecvError::Lagged(count)) => {
                out.dropped(count);
                let Some(total) = skips.add(count, Instant::now()) else {
                    continue;
                };
                let told = ServerEvent::Error {
                    message: format!("surface frames skipped: {total}"),
                };
                if out.send(text_event(&told)).await.is_err() {
                    break;
                }
            }
            Err(RecvError::Closed) => {
                let stopped = ServerEvent::StreamStopped {
                    stream_id: stream.stream_id,
                    kind: stream.kind,
                };
                let _ = out.send(text_event(&stopped)).await;
                break;
            }
        }
    }
}

pub(super) fn encode(frame: &SurfaceFrame, stream_id: u16, fit: Option<SurfaceFit>) -> Vec<u8> {
    match fit.and_then(|fit| fitted(frame, fit)) {
        Some(pooled) => pooled.encode(stream_id),
        None => frame.encode(stream_id),
    }
}

pub(super) fn fitted(frame: &SurfaceFrame, fit: SurfaceFit) -> Option<SurfaceFrame> {
    match frame {
        SurfaceFrame::RangeDoppler(owned) => {
            range_doppler(owned, fit).map(SurfaceFrame::RangeDoppler)
        }
        SurfaceFrame::SpatialSpectrum(owned) => {
            spatial(owned, fit).map(SurfaceFrame::SpatialSpectrum)
        }
        SurfaceFrame::Visibility(owned) => visibility(owned, fit).map(SurfaceFrame::Visibility),
        SurfaceFrame::FusionGrid(owned) => fusion_grid(owned, fit).map(SurfaceFrame::FusionGrid),
    }
}

#[derive(Clone, Copy, Debug)]
struct Axis {
    size: usize,
    factor: usize,
    shift: usize,
}

impl Axis {
    fn fitted(size: u16, fit: u16) -> Self {
        let size = usize::from(size);
        Self {
            size,
            factor: size.div_ceil(usize::from(fit).max(1)).max(1),
            shift: 0,
        }
    }

    fn circular(size: u16, fit: u16) -> Self {
        let least = Self::fitted(size, fit);
        let factor = (least.factor..=least.size)
            .find(|factor| least.size.is_multiple_of(*factor))
            .unwrap_or(least.factor);
        Self {
            factor,
            shift: factor / 2,
            ..least
        }
    }

    fn kept(size: u16) -> Self {
        Self {
            size: usize::from(size),
            factor: 1,
            shift: 0,
        }
    }

    const fn whole(self) -> bool {
        self.factor == 1
    }

    const fn len(self) -> usize {
        self.size.div_ceil(self.factor)
    }

    fn count(self) -> u16 {
        u16::try_from(self.len()).unwrap_or(u16::MAX)
    }

    fn step(self, step: f32) -> f32 {
        step * self.factor as f32
    }

    fn first(self, first: f32, step: f32) -> f32 {
        first + (self.factor - 1) as f32 / 2.0 * step
    }

    fn stretch(self) -> f64 {
        if self.size == 0 {
            return 1.0;
        }
        (self.len() * self.factor) as f64 / self.size as f64
    }

    const fn slot(self, index: usize) -> Option<usize> {
        ((index + self.shift) / self.factor).checked_rem(self.len())
    }
}

fn range_doppler(frame: &RangeDopplerOwned, fit: SurfaceFit) -> Option<RangeDopplerOwned> {
    let ranges = Axis::fitted(frame.ranges, fit.cols);
    let dopplers = Axis::fitted(frame.dopplers, fit.rows);
    if ranges.whole() && dopplers.whole() {
        return None;
    }
    Some(RangeDopplerOwned {
        stream_id: frame.stream_id,
        seq: frame.seq,
        timestamp: frame.timestamp,
        ranges: ranges.count(),
        dopplers: dopplers.count(),
        range_first_m: ranges.first(frame.range_first_m, frame.range_step_m),
        range_step_m: ranges.step(frame.range_step_m),
        doppler_first_hz: dopplers.first(frame.doppler_first_hz, frame.doppler_step_hz),
        doppler_step_hz: dopplers.step(frame.doppler_step_hz),
        carrier_hz: frame.carrier_hz,
        db_min: frame.db_min,
        db_max: frame.db_max,
        cells: pool_max(&frame.cells, dopplers, ranges),
    })
}

fn fusion_grid(frame: &FusionGridOwned, fit: SurfaceFit) -> Option<FusionGridOwned> {
    let cols = Axis::fitted(frame.cols, fit.cols);
    let rows = Axis::fitted(frame.rows, fit.rows);
    if cols.whole() && rows.whole() {
        return None;
    }
    Some(FusionGridOwned {
        stream_id: frame.stream_id,
        seq: frame.seq,
        timestamp: frame.timestamp,
        south: frame.north - (frame.north - frame.south) * rows.stretch(),
        west: frame.west,
        north: frame.north,
        east: frame.west + (frame.east - frame.west) * cols.stretch(),
        cols: cols.count(),
        rows: rows.count(),
        cells: pool_max(&frame.cells, rows, cols),
    })
}

fn stretched_band(center_hz: f64, span_hz: f32, bins: Axis) -> (f64, f32) {
    let low = center_hz - f64::from(span_hz) / 2.0;
    let span = f64::from(span_hz) * bins.stretch();
    (low + span / 2.0, span as f32)
}

fn spatial(frame: &SpatialSpectrumOwned, fit: SurfaceFit) -> Option<SpatialSpectrumOwned> {
    let bearings = Axis::circular(frame.bearings, fit.rows);
    let bins = Axis::fitted(frame.bins, fit.cols);
    if bearings.whole() && bins.whole() {
        return None;
    }
    let (center_hz, span_hz) = stretched_band(frame.center_hz, frame.span_hz, bins);
    Some(SpatialSpectrumOwned {
        stream_id: frame.stream_id,
        seq: frame.seq,
        timestamp: frame.timestamp,
        center_hz,
        span_hz,
        bearings: bearings.count(),
        bins: bins.count(),
        db_min: frame.db_min,
        db_max: frame.db_max,
        cells: pool_max(&frame.cells, bearings, bins),
    })
}

fn visibility(frame: &VisibilityOwned, fit: SurfaceFit) -> Option<VisibilityOwned> {
    let baselines = Axis::kept(frame.baselines);
    let bins = Axis::fitted(frame.bins, fit.cols);
    if bins.whole() {
        return None;
    }
    let (center_hz, span_hz) = stretched_band(frame.center_hz, frame.span_hz, bins);
    let (amplitude, phase) = pool_strongest(&frame.amplitude, &frame.phase, baselines, bins);
    Some(VisibilityOwned {
        stream_id: frame.stream_id,
        seq: frame.seq,
        timestamp: frame.timestamp,
        center_hz,
        span_hz,
        baselines: baselines.count(),
        bins: bins.count(),
        db_min: frame.db_min,
        db_max: frame.db_max,
        amplitude,
        phase,
    })
}

fn pooled_slot(index: usize, rows: Axis, cols: Axis) -> Option<usize> {
    let width = cols.size.max(1);
    let (row, col) = (index / width, index % width);
    if row >= rows.size {
        return None;
    }
    Some(rows.slot(row)? * cols.len() + cols.slot(col)?)
}

fn pool_max(cells: &[u8], rows: Axis, cols: Axis) -> Vec<u8> {
    let mut pooled = vec![0; rows.len() * cols.len()];
    for (index, &cell) in cells.iter().enumerate() {
        let Some(slot) = pooled_slot(index, rows, cols).and_then(|slot| pooled.get_mut(slot))
        else {
            break;
        };
        *slot = (*slot).max(cell);
    }
    pooled
}

fn pool_strongest(amplitude: &[u8], phase: &[u8], rows: Axis, cols: Axis) -> (Vec<u8>, Vec<u8>) {
    let cells = rows.len() * cols.len();
    let mut strongest: Vec<Option<(u8, u8)>> = vec![None; cells];
    for (index, pair) in amplitude
        .iter()
        .copied()
        .zip(phase.iter().copied())
        .enumerate()
    {
        let Some(slot) = pooled_slot(index, rows, cols).and_then(|slot| strongest.get_mut(slot))
        else {
            break;
        };
        if slot.is_none_or(|(held, _)| pair.0 > held) {
            *slot = Some(pair);
        }
    }
    strongest
        .into_iter()
        .map(|pair| pair.unwrap_or_default())
        .unzip()
}

#[cfg(test)]
mod tests;
