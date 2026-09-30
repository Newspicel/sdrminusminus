use std::{
    f64::consts::TAU,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use arc_swap::ArcSwap;
use num_complex::Complex;
use sdrmm_device::{GapScope, LaneMark, RxSink, UNKNOWN_ERROR, Uncertainty};
use sdrmm_wire::NoiseSource;

use super::{
    BLOCK_LEN, BenchWorld, LaneTruth, NoiseSchedule,
    device::DeviceTune,
    render::{LaneRenderer, LaneSetup},
    scene::{LaneImpairments, ReportedGap, Scene, mix, mix_key},
};

const MAX_NAP_S: f64 = 0.02;
const MARK_WINDOW: u64 = BLOCK_LEN as u64;

pub(crate) struct LaneContext {
    pub(crate) world: Arc<BenchWorld>,
    pub(crate) slot: usize,
    pub(crate) lane: usize,
    pub(crate) tune: Arc<ArcSwap<DeviceTune>>,
    pub(crate) origin_s: f64,
}

#[derive(Clone, Copy, Debug)]
struct Timeline {
    base_s: f64,
    base_index: f64,
    rate: f64,
}

impl Timeline {
    fn at(&self, h: i64, offset: i64) -> f64 {
        self.base_s + ((h + offset) as f64 - self.base_index) / self.rate
    }

    fn rebase(&mut self, h: i64, offset: i64, rate: f64) {
        self.base_s = self.at(h, offset);
        self.base_index = (h + offset) as f64;
        self.rate = rate;
    }
}

fn lane_rate(sample_rate: f64, ppm: f64) -> f64 {
    sample_rate * (1.0 + ppm * 1e-6)
}

struct LaneRun {
    ctx: LaneContext,
    feed: Option<usize>,
    marks_noise: bool,
    tune: Arc<DeviceTune>,
    impairments: Arc<LaneImpairments>,
    scene: Arc<Scene>,
    schedule: Arc<NoiseSchedule>,
    renderer: LaneRenderer,
    timeline: Timeline,
    h: i64,
    next_slip: usize,
    next_gap: usize,
    retunes: u64,
    marked_until_s: f64,
    stale_truth: bool,
}

pub(crate) fn run(ctx: LaneContext, mut sink: RxSink, running: &AtomicBool) {
    let mut lane = LaneRun::start(ctx);
    let mut block = vec![Complex::new(0.0f32, 0.0); BLOCK_LEN];
    lane.mark_initial_noise(&mut sink);
    while running.load(Ordering::Acquire) {
        lane.refresh(&mut sink);
        lane.apply_due_events(&mut sink);
        lane.publish_truth(sink.index());
        let len = lane.next_len(sink.index());
        let end_s = lane.time(lane.h + len as i64);
        if !pace(&lane.ctx.world, end_s, running) {
            return;
        }
        lane.refresh_schedule();
        lane.post_noise_marks(&mut sink, end_s);
        lane.render(&mut block[..len]);
        sink.push(&block[..len]);
        lane.h += len as i64;
    }
}

fn pace(world: &BenchWorld, until_s: f64, running: &AtomicBool) -> bool {
    loop {
        if !running.load(Ordering::Acquire) {
            return false;
        }
        let wait = until_s - world.true_time_s();
        if wait <= 0.0 {
            return true;
        }
        std::thread::sleep(Duration::from_secs_f64(wait.min(MAX_NAP_S)));
    }
}

impl LaneRun {
    fn start(ctx: LaneContext) -> Self {
        let world = ctx.world.clone();
        let feed = world.feed_of(ctx.slot);
        let tune = ctx.tune.load_full();
        let impairments = world.impairments(ctx.slot, ctx.lane);
        let scene = world.scene();
        let schedule =
            feed.map_or_else(|| Arc::new(NoiseSchedule::default()), |f| world.schedule(f));
        let rate = lane_rate(tune.sample_rate, impairments.ppm);
        let timeline = Timeline {
            base_s: ctx.origin_s,
            base_index: 0.0,
            rate,
        };
        let thermal_seed = mix_key(scene.seed ^ mix(ctx.lane as u64), &world.spec(ctx.slot).key);
        let mut lane = Self {
            marks_noise: feed == Some(ctx.slot),
            feed,
            renderer: LaneRenderer::new(thermal_seed),
            tune,
            impairments,
            scene,
            schedule,
            timeline,
            h: 0,
            next_slip: 0,
            next_gap: 0,
            retunes: 0,
            marked_until_s: f64::NEG_INFINITY,
            stale_truth: true,
            ctx,
        };
        lane.marked_until_s = lane.time(0);
        lane.reconfigure();
        lane
    }

    fn time(&self, h: i64) -> f64 {
        self.timeline.at(h, self.impairments.start_offset)
    }

    fn mark_initial_noise(&self, sink: &mut RxSink) {
        if self.marks_noise && self.noise_on_at(self.time(self.h)) {
            sink.mark(LaneMark::NoiseSource {
                on: true,
                in_flight: 0,
            });
        }
    }

    fn noise_on_at(&self, time_s: f64) -> bool {
        self.schedule
            .switches
            .iter()
            .rev()
            .find(|switch| switch.at_s <= time_s)
            .is_some_and(|switch| switch.on)
    }

    fn refresh(&mut self, sink: &mut RxSink) {
        let tuned = self.refresh_tune(sink);
        let impaired = self.refresh_impairments();
        let scene = self.ctx.world.scene();
        let moved = !Arc::ptr_eq(&scene, &self.scene);
        self.scene = scene;
        if tuned || impaired || moved {
            self.reconfigure();
        }
    }

    fn refresh_schedule(&mut self) {
        if let Some(feed) = self.feed {
            self.schedule = self.ctx.world.schedule(feed);
        }
    }

    fn refresh_tune(&mut self, sink: &mut RxSink) -> bool {
        let tune = self.ctx.tune.load_full();
        if Arc::ptr_eq(&tune, &self.tune) {
            return false;
        }
        let lane = self.ctx.lane;
        if tune.center(lane) != self.tune.center(lane)
            || tune.radio_center_hz != self.tune.radio_center_hz
        {
            self.retunes += 1;
            sink.mark(LaneMark::Retuned {
                in_flight: MARK_WINDOW,
            });
        }
        if tune.gain(lane) != self.tune.gain(lane) {
            sink.mark(LaneMark::GainChanged {
                in_flight: MARK_WINDOW,
            });
        }
        if tune.sample_rate != self.tune.sample_rate {
            let rate = lane_rate(tune.sample_rate, self.impairments.ppm);
            self.timeline
                .rebase(self.h, self.impairments.start_offset, rate);
            sink.realigned(Uncertainty::RateWrite, UNKNOWN_ERROR, GapScope::Device);
        }
        self.tune = tune;
        true
    }

    fn refresh_impairments(&mut self) -> bool {
        let impairments = self.ctx.world.impairments(self.ctx.slot, self.ctx.lane);
        if Arc::ptr_eq(&impairments, &self.impairments) {
            return false;
        }
        if impairments.ppm != self.impairments.ppm {
            let rate = lane_rate(self.tune.sample_rate, impairments.ppm);
            self.timeline
                .rebase(self.h, self.impairments.start_offset, rate);
        }
        self.impairments = impairments;
        self.next_slip = 0;
        self.next_gap = 0;
        true
    }

    fn scramble_rad(&self) -> f64 {
        if !self.impairments.scramble_on_retune {
            return 0.0;
        }
        let key = &self.ctx.world.spec(self.ctx.slot).key;
        let draw =
            mix(mix_key(self.scene.seed, key) ^ mix(self.ctx.lane as u64) ^ mix(self.retunes));
        TAU * (draw >> 11) as f64 / (1u64 << 53) as f64
    }

    fn reconfigure(&mut self) {
        let world = self.ctx.world.clone();
        let spec = world.spec(self.ctx.slot);
        let lane = self.ctx.lane;
        let setup = LaneSetup {
            position: self
                .scene
                .positions
                .get(spec.first_element + lane)
                .copied()
                .unwrap_or_default(),
            center_hz: self.tune.center(lane),
            radio_center_hz: self.tune.radio_center_hz,
            sample_rate: self.tune.sample_rate,
            impairments: &self.impairments,
            gain_setting_db: self.tune.gain(lane),
            scramble_rad: self.scramble_rad(),
            pilot: spec.pilot,
            noise_seed: self
                .feed
                .map(|feed| mix_key(self.scene.seed, &world.spec(feed).key)),
            isolated: self
                .feed
                .is_some_and(|feed| world.spec(feed).noise_source == NoiseSource::Isolated),
        };
        self.renderer.configure(&self.scene, &setup);
        self.stale_truth = true;
    }

    fn publish_truth(&mut self, index: u64) {
        if !self.stale_truth {
            return;
        }
        let lane = self.ctx.lane;
        let impairments = &self.impairments;
        let turn_deg = impairments.phase_deg
            + impairments.phase_per_db * self.tune.gain(lane)
            + self.scramble_rad().to_degrees();
        let first_h = self.h.saturating_sub_unsigned(index);
        let truth = LaneTruth {
            phase_deg: turn_deg.rem_euclid(360.0),
            gain_db: impairments.gain_db,
            first_sample_s: self.time(first_h) - impairments.frac_delay / self.tune.sample_rate,
            rate_hz: self.timeline.rate,
            ppm: impairments.ppm,
        };
        self.ctx.world.publish_truth(self.ctx.slot, lane, truth);
        self.stale_truth = false;
    }

    fn apply_due_events(&mut self, sink: &mut RxSink) {
        loop {
            let at = sink.index();
            let slip = self
                .impairments
                .slips
                .get(self.next_slip)
                .filter(|slip| slip.at <= at)
                .copied();
            let gap = self
                .impairments
                .gaps
                .get(self.next_gap)
                .filter(|gap| gap.at <= at)
                .copied();
            match (slip, gap) {
                (Some(slip), Some(gap)) if gap.at < slip.at => self.apply_gap(gap, sink),
                (Some(slip), _) => {
                    self.h = self.h.saturating_add(slip.samples);
                    self.next_slip += 1;
                    self.stale_truth = true;
                }
                (None, Some(gap)) => self.apply_gap(gap, sink),
                (None, None) => return,
            }
        }
    }

    fn apply_gap(&mut self, gap: ReportedGap, sink: &mut RxSink) {
        self.h = self.h.saturating_add_unsigned(gap.missing);
        sink.dropped_estimate(gap.reported, gap.error, GapScope::Lane);
        self.next_gap += 1;
        self.stale_truth = true;
    }

    fn next_len(&self, at: u64) -> usize {
        let slip = self.impairments.slips.get(self.next_slip).map(|s| s.at);
        let gap = self.impairments.gaps.get(self.next_gap).map(|g| g.at);
        slip.into_iter()
            .chain(gap)
            .min()
            .map_or(BLOCK_LEN, |next| {
                next.saturating_sub(at).min(BLOCK_LEN as u64) as usize
            })
            .max(1)
    }

    fn post_noise_marks(&mut self, sink: &mut RxSink, end_s: f64) {
        if self.marks_noise {
            for switch in &self.schedule.switches {
                if switch.at_s >= self.marked_until_s && switch.at_s < end_s {
                    sink.mark(LaneMark::NoiseSource {
                        on: switch.on,
                        in_flight: MARK_WINDOW,
                    });
                }
            }
        }
        self.marked_until_s = self.marked_until_s.max(end_s);
    }

    fn render(&mut self, out: &mut [Complex<f32>]) {
        let start_s = self.time(self.h);
        let step_s = 1.0 / self.timeline.rate;
        let end_s = start_s + out.len() as f64 * step_s;
        let mut on = self.noise_on_at(start_s);
        let mut from = 0;
        for switch in &self.schedule.switches {
            if switch.at_s <= start_s || switch.at_s >= end_s {
                continue;
            }
            let split = (((switch.at_s - start_s) / step_s).ceil() as usize).clamp(from, out.len());
            if split > from {
                let at = start_s + from as f64 * step_s;
                self.renderer
                    .render(self.h + from as i64, at, step_s, on, &mut out[from..split]);
                from = split;
            }
            on = switch.on;
        }
        let at = start_s + from as f64 * step_s;
        self.renderer
            .render(self.h + from as i64, at, step_s, on, &mut out[from..]);
    }
}
