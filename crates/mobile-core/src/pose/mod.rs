use std::{sync::Arc, time::Duration};

use sdrmm_wire::phone::POSE_MIN_INTERVAL_MS;
use tokio::sync::{mpsc, watch};

use crate::{
    events::{CoreEvent, EventQueue},
    link::{Activity, Session},
    records::{
        HeadingMode, HeadingSample, LocationSample, MotionSample, Mount, PoseSettings, PoseView,
    },
    runtime::CoreRuntime,
};

mod align;
mod axis;
mod engine;
mod filter;
mod publish;
mod vector;

pub(crate) use engine::PoseSnapshot;
pub use engine::{PoseEngine, PoseStep};
pub use publish::PoseOut;

pub(crate) const INPUT_CAPACITY: usize = 512;
pub(crate) const PUBLISH_GAP_MS: u64 = 2 * POSE_MIN_INTERVAL_MS;
const TICK: Duration = Duration::from_millis(250);
const VIEW_REFRESH_MS: i64 = 1_000;

pub(crate) const DEFAULT_SETTINGS: PoseSettings = PoseSettings {
    heading_mode: HeadingMode::Auto,
    mount: Mount::Flat,
    mount_offset_deg: 0.0,
    share_pose: false,
};

pub(crate) enum PoseInput {
    Location(LocationSample),
    Heading(HeadingSample),
    Motion(MotionSample),
    Settings(PoseSettings),
    StartAlign,
    CancelAlign,
}

pub(crate) struct PoseHub {
    input: mpsc::Sender<PoseInput>,
    events: EventQueue,
}

impl PoseHub {
    pub(crate) fn push(&self, input: PoseInput) {
        if let Err(mpsc::error::TrySendError::Full(_)) = self.input.try_send(input) {
            self.events.missed(1);
        }
    }
}

pub(crate) struct PoseWires {
    pub(crate) events: EventQueue,
    pub(crate) out: watch::Sender<Option<PoseOut>>,
    pub(crate) snapshot: watch::Sender<Option<PoseSnapshot>>,
    pub(crate) needed: watch::Receiver<bool>,
    pub(crate) sessions: watch::Receiver<Option<Arc<Session>>>,
    pub(crate) activity: watch::Sender<Activity>,
}

pub(crate) fn start(runtime: &CoreRuntime, wires: PoseWires) -> PoseHub {
    let (input, inputs) = mpsc::channel(INPUT_CAPACITY);
    let hub = PoseHub {
        input,
        events: wires.events.clone(),
    };
    runtime.spawn(run(PoseEngine::new(DEFAULT_SETTINGS), inputs, wires));
    hub
}

pub(crate) fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

fn apply(engine: &mut PoseEngine, input: PoseInput, now: i64) -> PoseStep {
    match input {
        PoseInput::Location(sample) => engine.location(sample),
        PoseInput::Heading(sample) => engine.heading(sample),
        PoseInput::Motion(sample) => engine.motion(sample),
        PoseInput::Settings(settings) => engine.settings(settings),
        PoseInput::StartAlign => engine.start_align(now),
        PoseInput::CancelAlign => engine.cancel_align(),
    }
}

struct Shown {
    view: Option<PoseView>,
    at_ms: i64,
}

impl Shown {
    fn changed(&mut self, view: &PoseView, now: i64) -> bool {
        let same = self.view.as_ref().is_some_and(|shown| {
            shown.fix_age_ms.is_some() == view.fix_age_ms.is_some()
                && PoseView {
                    fix_age_ms: view.fix_age_ms,
                    ..shown.clone()
                } == *view
        });
        let aging = view.fix_age_ms.is_some() && now - self.at_ms >= VIEW_REFRESH_MS;
        if same && !aging {
            return false;
        }
        self.view = Some(view.clone());
        self.at_ms = now;
        true
    }
}

async fn run(mut engine: PoseEngine, mut inputs: mpsc::Receiver<PoseInput>, mut wires: PoseWires) {
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut shown = Shown {
        view: Some(engine.view(now_ms())),
        at_ms: now_ms(),
    };
    let mut needed_open = true;
    let mut sessions_open = true;
    loop {
        let now = now_ms();
        let step = tokio::select! {
            input = inputs.recv() => match input {
                Some(input) => apply(&mut engine, input, now),
                None => break,
            },
            changed = wires.needed.changed(), if needed_open => {
                needed_open = changed.is_ok();
                demand(&mut engine, &wires, now)
            }
            changed = wires.sessions.changed(), if sessions_open => {
                sessions_open = changed.is_ok();
                demand(&mut engine, &wires, now)
            }
            _ = tick.tick() => engine.tick(now),
        };
        deliver(&engine, step, &wires, &mut shown, now);
    }
}

fn demand(engine: &mut PoseEngine, wires: &PoseWires, now: i64) -> PoseStep {
    let needed = *wires.needed.borrow();
    let online = wires.sessions.borrow().is_some();
    engine.demand(needed, online, now)
}

fn deliver(engine: &PoseEngine, step: PoseStep, wires: &PoseWires, shown: &mut Shown, now: i64) {
    for notice in step.notices {
        wires.events.emit(CoreEvent::Notice { notice });
    }
    if let Some(out) = step.publish {
        wires.out.send_replace(Some(out));
    } else if !engine.sharing() {
        wires
            .out
            .send_if_modified(|current| current.take().is_some());
    }
    let view = engine.view(now);
    if shown.changed(&view, now) {
        wires.events.emit(CoreEvent::Pose { view });
    }
    let snapshot = engine.snapshot(now);
    wires.snapshot.send_if_modified(|current| {
        let changed = *current != snapshot;
        *current = snapshot;
        changed
    });
    let sharing = engine.sharing();
    wires.activity.send_if_modified(|activity| {
        let changed = activity.pose_needed != sharing;
        activity.pose_needed = sharing;
        changed
    });
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::events::Pop;

    fn wait(until: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if until() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn a_stopped_share_withdraws_the_last_pose() {
        let runtime = CoreRuntime::start().expect("runtime");
        let events = EventQueue::default();
        let (out, out_rx) = watch::channel(None);
        let (snapshot, snapshot_rx) = watch::channel(None);
        let (needed, needed_rx) = watch::channel(false);
        let (_sessions, sessions_rx) = watch::channel(None);
        let (activity, activity_rx) = watch::channel(Activity::default());
        let hub = start(
            &runtime,
            PoseWires {
                events: events.clone(),
                out,
                snapshot,
                needed: needed_rx,
                sessions: sessions_rx,
                activity,
            },
        );
        needed.send_replace(true);
        hub.push(PoseInput::Settings(PoseSettings {
            share_pose: true,
            ..DEFAULT_SETTINGS
        }));
        hub.push(PoseInput::Location(LocationSample {
            t_unix_ms: now_ms(),
            lat: 52.52,
            lon: 13.405,
            alt_m: None,
            h_acc_m: 5.0,
            v_acc_m: None,
            speed_mps: None,
            speed_acc_mps: None,
            course_deg: None,
            course_acc_deg: None,
        }));
        assert!(wait(|| out_rx
            .borrow()
            .as_ref()
            .is_some_and(|pose| pose.fix.is_some())));
        assert!(wait(|| snapshot_rx.borrow().is_some()));
        assert!(wait(|| activity_rx.borrow().pose_needed));
        hub.push(PoseInput::Settings(DEFAULT_SETTINGS));
        assert!(wait(|| out_rx.borrow().is_none()));
        assert!(wait(|| !activity_rx.borrow().pose_needed));
        let mut clock = Instant::now();
        let mut poses = 0;
        for _ in 0..20 {
            clock += Duration::from_secs(1);
            if let Pop::Event(event) = events.pop(clock)
                && matches!(*event, CoreEvent::Pose { .. })
            {
                poses += 1;
            }
        }
        assert!(poses >= 1);
        runtime.shutdown();
    }
}
