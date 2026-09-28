use std::{
    collections::VecDeque,
    pin::pin,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use tokio::sync::Notify;

use crate::{
    missions::views::{
        DfView, HuntView, MissionsView, RadarView, RetargetNotice, RgbaImage, SurveyPoint,
        SurveyView,
    },
    records::{LinkState, Notice, PoseView},
};

#[derive(Clone, Debug, PartialEq, uniffi::Enum)]
pub enum CoreEvent {
    Link { state: LinkState },
    Missions { view: MissionsView },
    Pose { view: PoseView },
    Hunt { view: HuntView },
    Df { view: DfView },
    Radar { view: RadarView },
    RadarImage { image: RgbaImage },
    Survey { view: SurveyView },
    SurveyPoints { points: Vec<SurveyPoint> },
    Retarget { notice: RetargetNotice },
    Notice { notice: Notice },
}

pub(crate) const FIFO_CAPACITY: usize = 128;
pub(crate) const MAX_PENDING_POINTS: usize = 500;
const STATE_GAP: Duration = Duration::from_millis(100);
const IMAGE_GAP: Duration = Duration::from_millis(200);
const SLOT_COUNT: usize = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Link,
    Missions,
    Pose,
    Hunt,
    Df,
    Radar,
    RadarImage,
    Survey,
    SurveyPoints,
}

const SLOTS: [Slot; SLOT_COUNT] = [
    Slot::Link,
    Slot::Missions,
    Slot::Pose,
    Slot::Hunt,
    Slot::Df,
    Slot::Radar,
    Slot::RadarImage,
    Slot::Survey,
    Slot::SurveyPoints,
];

impl Slot {
    const fn of(event: &CoreEvent) -> Option<Self> {
        match event {
            CoreEvent::Link { .. } => Some(Self::Link),
            CoreEvent::Missions { .. } => Some(Self::Missions),
            CoreEvent::Pose { .. } => Some(Self::Pose),
            CoreEvent::Hunt { .. } => Some(Self::Hunt),
            CoreEvent::Df { .. } => Some(Self::Df),
            CoreEvent::Radar { .. } => Some(Self::Radar),
            CoreEvent::RadarImage { .. } => Some(Self::RadarImage),
            CoreEvent::Survey { .. } => Some(Self::Survey),
            CoreEvent::SurveyPoints { .. } => Some(Self::SurveyPoints),
            CoreEvent::Retarget { .. } | CoreEvent::Notice { .. } => None,
        }
    }

    const fn gap(self) -> Duration {
        match self {
            Self::Link | Self::Missions => Duration::ZERO,
            Self::RadarImage => IMAGE_GAP,
            Self::Pose
            | Self::Hunt
            | Self::Df
            | Self::Radar
            | Self::Survey
            | Self::SurveyPoints => STATE_GAP,
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum Pop {
    Event(Box<CoreEvent>),
    Wait(Option<Instant>),
    Closed,
}

#[derive(Clone, Default)]
pub(crate) struct EventQueue {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    wake: Notify,
}

#[derive(Default)]
struct Queue {
    pending: [Option<CoreEvent>; SLOT_COUNT],
    sent: [Option<Instant>; SLOT_COUNT],
    fifo: VecDeque<CoreEvent>,
    missed: u64,
    closed: bool,
}

impl EventQueue {
    pub(crate) fn emit(&self, event: CoreEvent) {
        if !self.lock().push(event) {
            return;
        }
        self.shared.wake.notify_waiters();
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn missed(&self, count: u64) {
        {
            let mut queue = self.lock();
            if queue.closed || count == 0 {
                return;
            }
            queue.missed = queue.missed.saturating_add(count);
        }
        self.shared.wake.notify_waiters();
    }

    pub(crate) fn close(&self) {
        {
            let mut queue = self.lock();
            *queue = Queue::default();
            queue.closed = true;
        }
        self.shared.wake.notify_waiters();
    }

    pub(crate) fn pop(&self, now: Instant) -> Pop {
        self.lock().pop(now)
    }

    pub(crate) async fn wait(&self, until: Option<Instant>) {
        let mut notified = pin!(self.shared.wake.notified());
        notified.as_mut().enable();
        if self.lock().ready(Instant::now()) {
            return;
        }
        match until {
            Some(until) => {
                let _ = tokio::time::timeout_at(until.into(), notified).await;
            }
            None => notified.await,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl Queue {
    fn push(&mut self, event: CoreEvent) -> bool {
        if self.closed {
            return false;
        }
        match event {
            CoreEvent::SurveyPoints { points } => self.merge_points(points),
            event => match Slot::of(&event) {
                Some(slot) => self.pending[slot.index()] = Some(event),
                None => self.enqueue(event),
            },
        }
        true
    }

    fn enqueue(&mut self, event: CoreEvent) {
        if self.fifo.len() == FIFO_CAPACITY {
            self.fifo.pop_front();
            self.missed = self.missed.saturating_add(1);
        }
        self.fifo.push_back(event);
    }

    fn merge_points(&mut self, mut fresh: Vec<SurveyPoint>) {
        let slot = &mut self.pending[Slot::SurveyPoints.index()];
        let mut points = match slot.take() {
            Some(CoreEvent::SurveyPoints { mut points }) => {
                points.append(&mut fresh);
                points
            }
            _ => fresh,
        };
        let excess = points.len().saturating_sub(MAX_PENDING_POINTS);
        points.drain(..excess);
        *slot = Some(CoreEvent::SurveyPoints { points });
        self.missed = self
            .missed
            .saturating_add(u64::try_from(excess).unwrap_or(u64::MAX));
    }

    fn pop(&mut self, now: Instant) -> Pop {
        if self.closed {
            return Pop::Closed;
        }
        if self.missed > 0 {
            return Pop::Event(Box::new(missed_notice(std::mem::take(&mut self.missed))));
        }
        for slot in SLOTS {
            if self.due(slot, now)
                && let Some(event) = self.pending[slot.index()].take()
            {
                self.sent[slot.index()] = Some(now);
                return Pop::Event(Box::new(event));
            }
        }
        match self.fifo.pop_front() {
            Some(event) => Pop::Event(Box::new(event)),
            None => Pop::Wait(self.next_due()),
        }
    }

    fn ready(&self, now: Instant) -> bool {
        self.closed
            || self.missed > 0
            || !self.fifo.is_empty()
            || SLOTS
                .into_iter()
                .any(|slot| self.pending[slot.index()].is_some() && self.due(slot, now))
    }

    fn due(&self, slot: Slot, now: Instant) -> bool {
        self.sent[slot.index()]
            .and_then(|sent| sent.checked_add(slot.gap()))
            .is_none_or(|at| now >= at)
    }

    fn next_due(&self) -> Option<Instant> {
        SLOTS
            .into_iter()
            .filter(|slot| self.pending[slot.index()].is_some())
            .filter_map(|slot| self.sent[slot.index()]?.checked_add(slot.gap()))
            .min()
    }
}

fn missed_notice(count: u64) -> CoreEvent {
    CoreEvent::Notice {
        notice: Notice::warn(format!("Missed {count} updates")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        missions::views::Trend,
        records::{LatLon, NoticeLevel},
    };

    fn hunt(level: f32) -> CoreEvent {
        CoreEvent::Hunt {
            view: HuntView {
                mission: "h".to_owned(),
                freq_hz: 145e6,
                level_db: Some(level),
                smooth_db: None,
                floor_db: None,
                best_db: None,
                strength: 0.0,
                trend: Trend::Waiting,
                running: true,
                refusal: None,
                readings: 0,
                sweep: None,
            },
        }
    }

    fn notice(text: &str) -> CoreEvent {
        CoreEvent::Notice {
            notice: Notice::warn(text),
        }
    }

    fn link(state: LinkState) -> CoreEvent {
        CoreEvent::Link { state }
    }

    fn image(width: u32) -> CoreEvent {
        CoreEvent::RadarImage {
            image: RgbaImage {
                width,
                height: 1,
                rgba: vec![0; width as usize * 4],
                range_max_km: 10.0,
                doppler_span_hz: 100.0,
            },
        }
    }

    fn points(count: usize, start: usize) -> CoreEvent {
        CoreEvent::SurveyPoints {
            points: (start..start + count)
                .map(|index| SurveyPoint {
                    at: LatLon {
                        lat: 0.0,
                        lon: index as f64,
                    },
                    level_db: -50.0,
                })
                .collect(),
        }
    }

    fn drain(queue: &EventQueue, now: Instant) -> Vec<CoreEvent> {
        let mut out = Vec::new();
        while let Pop::Event(event) = queue.pop(now) {
            out.push(*event);
        }
        out
    }

    #[test]
    fn events_are_coalesced_per_kind() {
        let queue = EventQueue::default();
        let now = Instant::now();
        queue.emit(hunt(-80.0));
        queue.emit(hunt(-70.0));
        queue.emit(link(LinkState::Offline));
        queue.emit(hunt(-60.0));
        queue.emit(link(LinkState::Online {
            server: "shack".to_owned(),
        }));
        assert_eq!(
            drain(&queue, now),
            vec![
                link(LinkState::Online {
                    server: "shack".to_owned()
                }),
                hunt(-60.0)
            ]
        );
    }

    #[test]
    fn high_rate_kinds_wait_their_gap() {
        let queue = EventQueue::default();
        let start = Instant::now();
        queue.emit(hunt(-80.0));
        queue.emit(image(1));
        assert_eq!(drain(&queue, start), vec![hunt(-80.0), image(1)]);
        queue.emit(hunt(-70.0));
        queue.emit(image(2));
        queue.emit(link(LinkState::Offline));
        assert_eq!(drain(&queue, start), vec![link(LinkState::Offline)]);
        assert_eq!(queue.pop(start), Pop::Wait(Some(start + STATE_GAP)));
        assert_eq!(drain(&queue, start + STATE_GAP), vec![hunt(-70.0)]);
        assert_eq!(
            queue.pop(start + STATE_GAP),
            Pop::Wait(Some(start + IMAGE_GAP))
        );
        assert_eq!(drain(&queue, start + IMAGE_GAP), vec![image(2)]);
        assert_eq!(queue.pop(start + IMAGE_GAP), Pop::Wait(None));
    }

    #[test]
    fn a_full_queue_reports_missed_updates() {
        let queue = EventQueue::default();
        let now = Instant::now();
        for index in 0..200 {
            queue.emit(notice(&index.to_string()));
        }
        let delivered = drain(&queue, now);
        assert_eq!(delivered.len(), FIFO_CAPACITY + 1);
        assert_eq!(
            delivered[0],
            CoreEvent::Notice {
                notice: Notice {
                    level: NoticeLevel::Warn,
                    text: "Missed 72 updates".to_owned()
                }
            }
        );
        assert_eq!(delivered[1], notice("72"));
        assert_eq!(delivered[FIFO_CAPACITY], notice("199"));
    }

    #[test]
    fn drops_reported_by_a_producer_arrive_as_one_notice() {
        let queue = EventQueue::default();
        queue.missed(3);
        queue.missed(4);
        queue.missed(0);
        assert_eq!(
            drain(&queue, Instant::now()),
            vec![CoreEvent::Notice {
                notice: Notice::warn("Missed 7 updates")
            }]
        );
    }

    #[test]
    fn survey_points_are_appended_and_capped() {
        let queue = EventQueue::default();
        let now = Instant::now();
        queue.emit(points(2, 0));
        queue.emit(points(3, 2));
        assert_eq!(drain(&queue, now), vec![points(5, 0)]);
        queue.emit(points(MAX_PENDING_POINTS, 0));
        queue.emit(points(10, MAX_PENDING_POINTS));
        let later = now + STATE_GAP;
        assert_eq!(
            drain(&queue, later),
            vec![
                CoreEvent::Notice {
                    notice: Notice::warn("Missed 10 updates")
                },
                points(MAX_PENDING_POINTS, 10)
            ]
        );
    }

    #[test]
    fn discrete_events_keep_their_order_behind_state() {
        let queue = EventQueue::default();
        queue.emit(notice("a"));
        queue.emit(hunt(-50.0));
        queue.emit(notice("b"));
        assert_eq!(
            drain(&queue, Instant::now()),
            vec![hunt(-50.0), notice("a"), notice("b")]
        );
    }

    #[test]
    fn a_closed_queue_ends_and_ignores_new_events() {
        let queue = EventQueue::default();
        queue.emit(notice("a"));
        queue.close();
        queue.emit(notice("b"));
        queue.missed(2);
        assert_eq!(queue.pop(Instant::now()), Pop::Closed);
    }

    #[tokio::test]
    async fn a_waiter_wakes_when_an_event_arrives() {
        let queue = EventQueue::default();
        let waiter = tokio::spawn({
            let queue = queue.clone();
            async move { queue.wait(None).await }
        });
        tokio::task::yield_now().await;
        queue.emit(notice("x"));
        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("woken")
            .expect("joined");
        assert_eq!(queue.pop(Instant::now()), Pop::Event(Box::new(notice("x"))));
    }

    #[tokio::test]
    async fn a_waiter_wakes_at_its_deadline() {
        let queue = EventQueue::default();
        let started = Instant::now();
        queue.wait(Some(started + Duration::from_millis(20))).await;
        assert!(started.elapsed() >= Duration::from_millis(20));
    }

    #[tokio::test]
    async fn closing_wakes_every_waiter() {
        let queue = EventQueue::default();
        let waiters: Vec<_> = (0..3)
            .map(|_| {
                let queue = queue.clone();
                tokio::spawn(async move { queue.wait(None).await })
            })
            .collect();
        tokio::task::yield_now().await;
        queue.close();
        for waiter in waiters {
            tokio::time::timeout(Duration::from_secs(5), waiter)
                .await
                .expect("woken")
                .expect("joined");
        }
    }

    #[tokio::test]
    async fn a_ready_queue_does_not_wait() {
        let queue = EventQueue::default();
        queue.emit(notice("x"));
        tokio::time::timeout(Duration::from_secs(5), queue.wait(None))
            .await
            .expect("no wait");
    }
}
