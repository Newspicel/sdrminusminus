use std::collections::HashMap;

use sdrmm_wire::{
    array::ArrayStatus,
    frame::{self, FrameKind, FusionGridFrame},
    fusion::DfFusionState,
    hunt::HuntStatus,
    mission as wire,
    phone::PhoneSelf,
    processor::ProcessorReading,
    radar::RadarUpdate,
    survey::SurveyGrid,
    ws::{ServerEvent, StateScope, SurfaceFit},
};

use super::{
    df::DfDrive,
    heat, hunt,
    listing::{Entry, Listing, Target},
    radar::{self, RadarPainter},
    survey::Survey,
    views::{DfState, MissionsView, TargetMode},
};
use crate::{
    events::CoreEvent, guidance::Retargeter, link::Subscriptions, pose::PoseSnapshot,
    records::Notice,
};

pub(crate) const RADAR_FIT: SurfaceFit = SurfaceFit {
    cols: 256,
    rows: 128,
};
pub(crate) const REFETCH_DEBOUNCE_MS: i64 = 300;
pub(crate) const LISTING_RETRY_MS: i64 = 10_000;
pub(crate) const RADAR_STALE_MS: i64 = 5_000;
pub(crate) const HEAT_EVERY_MS: i64 = 1_000;
pub(crate) const GUIDE_EVERY_MS: i64 = 500;

pub(crate) enum Input {
    Live { phone_id: String },
    Down,
    Event(Box<ServerEvent>),
    Frame(Vec<u8>),
    Listing(Box<wire::MissionsResponse>),
    ListingFailed(String),
    PhoneSelf(Box<PhoneSelf>),
    Seeded { mission: String, seed: Seed },
    Open(String),
    Close,
    Acted(Box<wire::Mission>),
    TargetMode(TargetMode),
    Pose(Option<PoseSnapshot>),
    Background(bool),
    Tick,
}

pub(crate) enum Seed {
    Radar(Box<RadarUpdate>),
    Survey(Box<SurveyGrid>),
    Fusion(Box<DfFusionState>),
    Arrays(Vec<ArrayStatus>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SeedRequest {
    Radar(String),
    Survey(String),
    Fusion(String),
    Arrays,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Effect {
    Emit(Box<CoreEvent>),
    FetchListing,
    FetchSelf,
    Seed { mission: String, what: SeedRequest },
}

struct HuntOpen {
    device_set: Option<u32>,
    channel: Option<u32>,
    freq_hz: f64,
    status: Option<HuntStatus>,
    running: bool,
    refusal: Option<String>,
}

struct RadarOpen {
    update: Option<RadarUpdate>,
    updated_ms: Option<i64>,
    stale: bool,
}

enum View {
    Hunt(HuntOpen),
    Df {
        drive: Box<DfDrive>,
        array: Option<String>,
        array_heard: bool,
        fusion: Option<String>,
        shown: Option<DfState>,
    },
    Radar(RadarOpen),
    Survey(Survey),
}

struct Open {
    entry: Entry,
    view: View,
    retarget: Retargeter,
}

pub(crate) struct Reducer {
    listing: Listing,
    phone_id: Option<String>,
    phone_gps: bool,
    live: bool,
    open: Option<Open>,
    streams: HashMap<u16, String>,
    pose: Option<PoseSnapshot>,
    background: bool,
    refetch_at: Option<i64>,
    self_stale: bool,
    listing_failed: bool,
    truncated_told: u32,
    guide_ms: Option<i64>,
    heat_ms: Option<i64>,
    painter: RadarPainter,
    bad_frame_told: bool,
    effects: Vec<Effect>,
}

impl Reducer {
    pub(crate) fn new() -> Self {
        Self {
            listing: Listing::default(),
            phone_id: None,
            phone_gps: false,
            live: false,
            open: None,
            streams: HashMap::new(),
            pose: None,
            background: false,
            refetch_at: None,
            self_stale: false,
            listing_failed: false,
            truncated_told: 0,
            guide_ms: None,
            heat_ms: None,
            painter: RadarPainter::new(),
            bad_frame_told: false,
            effects: Vec::new(),
        }
    }

    pub(crate) fn listing(&self) -> &Listing {
        &self.listing
    }

    pub(crate) fn open_id(&self) -> Option<&str> {
        self.open.as_ref().map(|open| open.entry.id())
    }

    pub(crate) fn subscriptions(&self) -> Subscriptions {
        let mut subs = Subscriptions::new();
        if self.background {
            return subs;
        }
        match self.open.as_ref().map(|open| (&open.entry, &open.view)) {
            Some((entry, View::Radar(_)))
                if matches!(entry.target, Target::Radar { surface: true }) =>
            {
                subs.insert(entry.id().to_owned(), Some(RADAR_FIT));
            }
            Some((
                _,
                View::Df {
                    fusion: Some(node), ..
                },
            )) => {
                subs.insert(node.clone(), None);
            }
            _ => {}
        }
        subs
    }

    pub(crate) fn pose_needed(&self) -> bool {
        self.phone_gps
            || self
                .phone_id
                .as_deref()
                .is_some_and(|phone| self.listing.wants_phone(phone))
    }

    pub(crate) fn handle(&mut self, input: Input, now_ms: i64) -> Vec<Effect> {
        match input {
            Input::Live { phone_id } => self.live(phone_id),
            Input::Down => self.down(),
            Input::Event(event) => self.event(*event, now_ms),
            Input::Frame(bytes) => self.frame(&bytes, now_ms),
            Input::Listing(response) => self.listed(*response, now_ms),
            Input::ListingFailed(detail) => self.listing_failed(&detail, now_ms),
            Input::PhoneSelf(phone) => self.phone_gps = !phone.phone.gps_nodes.is_empty(),
            Input::Seeded { mission, seed } => self.seeded(&mission, seed, now_ms),
            Input::Open(id) => self.open(&id, now_ms),
            Input::Close => self.open = None,
            Input::Acted(mission) => self.acted(*mission),
            Input::TargetMode(mode) => self.target_mode(mode, now_ms),
            Input::Pose(pose) => self.pose(pose, now_ms),
            Input::Background(background) => self.background = background,
            Input::Tick => self.tick(now_ms),
        }
        std::mem::take(&mut self.effects)
    }

    fn emit(&mut self, event: CoreEvent) {
        self.effects.push(Effect::Emit(Box::new(event)));
    }

    fn notice(&mut self, notice: Notice) {
        self.emit(CoreEvent::Notice { notice });
    }

    fn live(&mut self, phone_id: String) {
        self.live = true;
        self.phone_id = Some(phone_id);
        self.streams.clear();
        self.refetch_at = None;
        self.self_stale = false;
        self.effects.push(Effect::FetchListing);
        self.effects.push(Effect::FetchSelf);
        if let Some(open) = &self.open {
            let requests = seeds(&open.entry);
            let mission = open.entry.id().to_owned();
            self.effects
                .extend(requests.into_iter().map(|what| Effect::Seed {
                    mission: mission.clone(),
                    what,
                }));
        }
    }

    fn down(&mut self) {
        self.live = false;
        self.streams.clear();
        if let Some(Open {
            view: View::Radar(radar),
            ..
        }) = &mut self.open
        {
            radar.stale = true;
        }
        self.emit_radar();
    }

    fn schedule_refetch(&mut self, now_ms: i64) {
        if self.refetch_at.is_none() {
            self.refetch_at = Some(now_ms + REFETCH_DEBOUNCE_MS);
        }
    }

    fn tick(&mut self, now_ms: i64) {
        if self.refetch_at.is_some_and(|at| now_ms >= at) {
            self.refetch_at = None;
            if self.live {
                self.effects.push(Effect::FetchListing);
                if std::mem::take(&mut self.self_stale) {
                    self.effects.push(Effect::FetchSelf);
                }
            }
        }
        let stale_radar = match &mut self.open {
            Some(Open {
                view: View::Radar(radar),
                ..
            }) => {
                let stale = radar
                    .updated_ms
                    .is_none_or(|at| now_ms - at > RADAR_STALE_MS);
                let changed = stale && !radar.stale;
                radar.stale = stale;
                changed
            }
            _ => false,
        };
        if stale_radar {
            self.emit_radar();
        }
        let df_changed = match &self.open {
            Some(Open {
                view: View::Df { drive, shown, .. },
                ..
            }) => *shown != Some(drive.state(now_ms)),
            _ => false,
        };
        if df_changed {
            self.emit_df(now_ms);
        }
    }

    fn event(&mut self, event: ServerEvent, now_ms: i64) {
        match event {
            ServerEvent::StateChanged { scope } => self.state_changed(&scope, now_ms),
            ServerEvent::HuntUpdate { device_set, status } => self.hunt(device_set, *status),
            ServerEvent::ProcessorUpdate { node, reading } => self.reading(&node, *reading, now_ms),
            ServerEvent::ArrayUpdate { status } => {
                let routed = match &mut self.open {
                    Some(Open {
                        view:
                            View::Df {
                                drive,
                                array,
                                array_heard,
                                ..
                            },
                        ..
                    }) if array.as_deref() == Some(status.node.as_str()) => {
                        drive.array(&status);
                        *array_heard = true;
                        true
                    }
                    _ => false,
                };
                if routed {
                    self.emit_df(now_ms);
                }
            }
            ServerEvent::DfFusionUpdate { node, state } => {
                let routed = match &mut self.open {
                    Some(Open {
                        view: View::Df { drive, fusion, .. },
                        ..
                    }) if fusion.as_deref() == Some(node.as_str()) => {
                        drive.fusion(*state);
                        true
                    }
                    _ => false,
                };
                if routed {
                    self.emit_df(now_ms);
                }
            }
            ServerEvent::SurveyUpdate { node, update } => {
                let routed = match &mut self.open {
                    Some(Open {
                        entry,
                        view: View::Survey(survey),
                        ..
                    }) if entry.id() == node => Some((survey.update(&update), survey.view(&node))),
                    _ => None,
                };
                if let Some((point, view)) = routed {
                    if let Some(point) = point {
                        self.emit(CoreEvent::SurveyPoints {
                            points: vec![point],
                        });
                    }
                    self.emit(CoreEvent::Survey { view });
                }
            }
            ServerEvent::SurfaceStreamStarted {
                stream_id, node, ..
            } => {
                self.streams.insert(stream_id, node);
            }
            ServerEvent::StreamStopped { stream_id, .. } => {
                self.streams.remove(&stream_id);
            }
            ServerEvent::Error { message } => self.notice(Notice::warn(message)),
            _ => {}
        }
    }

    fn state_changed(&mut self, scope: &StateScope, now_ms: i64) {
        match scope {
            StateScope::Missions => self.schedule_refetch(now_ms),
            StateScope::Workspaces | StateScope::All => {
                self.self_stale = true;
                self.schedule_refetch(now_ms);
            }
            StateScope::Phones => {
                self.schedule_refetch(now_ms);
                self.effects.push(Effect::FetchSelf);
            }
            _ => {}
        }
    }

    fn hunt(&mut self, device_set: u32, status: HuntStatus) {
        let Some(Open {
            entry,
            view: View::Hunt(open),
            ..
        }) = &mut self.open
        else {
            return;
        };
        if open.device_set != Some(device_set) || open.channel != Some(status.settings.channel) {
            return;
        }
        open.running = status.error.is_none();
        open.status = Some(status);
        let view = hunt_view(entry.id(), open);
        self.emit(CoreEvent::Hunt { view });
    }

    fn reading(&mut self, node: &str, reading: ProcessorReading, now_ms: i64) {
        let pose = self.pose;
        let routed = match (&mut self.open, reading) {
            (
                Some(Open {
                    entry,
                    view: View::Df { drive, .. },
                    ..
                }),
                ProcessorReading::Df(reading),
            ) if entry.id() == node => {
                drive.reading(reading, pose, now_ms);
                Some(true)
            }
            (
                Some(Open {
                    entry,
                    view: View::Radar(radar),
                    ..
                }),
                ProcessorReading::PassiveRadar(update),
            ) if entry.id() == node => {
                radar.update = Some(update);
                radar.updated_ms = Some(now_ms);
                radar.stale = false;
                Some(false)
            }
            _ => None,
        };
        match routed {
            Some(true) => self.emit_df(now_ms),
            Some(false) => self.emit_radar(),
            None => {}
        }
    }

    fn frame(&mut self, bytes: &[u8], now_ms: i64) {
        if self.background {
            return;
        }
        let Ok(header) = frame::peek_header(bytes) else {
            return self.bad_frame();
        };
        let Some(node) = self.streams.get(&header.stream_id).cloned() else {
            tracing::debug!(stream = header.stream_id, "frame for no mission");
            return;
        };
        let wanted = self.subscriptions();
        if !wanted.contains_key(&node) {
            return;
        }
        match header.kind {
            FrameKind::RangeDoppler => match self.painter.paint(bytes) {
                Ok(image) => self.emit(CoreEvent::RadarImage { image }),
                Err(error) => {
                    tracing::warn!(%error, "radar frame refused");
                    self.bad_frame();
                }
            },
            FrameKind::FusionGrid if since(self.heat_ms, now_ms) >= HEAT_EVERY_MS => {
                match FusionGridFrame::decode(bytes) {
                    Ok(grid) => {
                        let bands = heat::bands(&grid);
                        if let Some(Open {
                            view: View::Df { drive, .. },
                            ..
                        }) = &mut self.open
                        {
                            drive.heat(bands);
                        }
                        self.heat_ms = Some(now_ms);
                        self.emit_df(now_ms);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "fusion grid refused");
                        self.bad_frame();
                    }
                }
            }
            _ => {}
        }
    }

    fn bad_frame(&mut self) {
        if !self.bad_frame_told {
            self.bad_frame_told = true;
            self.notice(Notice::warn("Bad frame from server"));
        }
    }

    fn listed(&mut self, response: wire::MissionsResponse, now_ms: i64) {
        self.listing_failed = false;
        self.tell_truncated(response.truncated);
        self.listing = Listing::new(response);
        let view: MissionsView = self.listing.view();
        self.emit(CoreEvent::Missions { view });
        let Some(open) = &self.open else {
            return;
        };
        let id = open.entry.id().to_owned();
        match self.listing.find(&id).cloned() {
            None => {
                self.open = None;
                self.notice(Notice::warn("Mission gone"));
            }
            Some(entry) if !same_wiring(&entry, &open.entry) => self.open(&id, now_ms),
            Some(entry) => self.refresh_entry(entry),
        }
    }

    fn tell_truncated(&mut self, truncated: u32) {
        if truncated == self.truncated_told {
            return;
        }
        self.truncated_told = truncated;
        if truncated > 0 {
            self.notice(Notice::warn(format!("{truncated} missions not listed")));
        }
    }

    fn refresh_entry(&mut self, entry: Entry) {
        let Some(open) = &mut self.open else {
            return;
        };
        if let (
            View::Hunt(hunt),
            Target::Hunt {
                running, status, ..
            },
        ) = (&mut open.view, &entry.target)
        {
            hunt.running = *running;
            hunt.refusal = entry.mission.blocker.clone();
            if hunt.status.is_none() {
                hunt.status.clone_from(status);
            }
            let view = hunt_view(entry.id(), hunt);
            open.entry = entry;
            self.emit(CoreEvent::Hunt { view });
        } else {
            open.entry = entry;
        }
    }

    fn listing_failed(&mut self, detail: &str, now_ms: i64) {
        tracing::warn!(detail, "missions fetch failed");
        if !self.listing_failed {
            self.listing_failed = true;
            self.notice(Notice::warn("Missions unavailable"));
        }
        self.refetch_at = Some(now_ms + LISTING_RETRY_MS);
    }

    fn acted(&mut self, mission: wire::Mission) {
        if self.listing.response.is_none() {
            return;
        }
        self.listing.replace(mission);
        let view = self.listing.view();
        self.emit(CoreEvent::Missions { view });
        if let Some(entry) = self.open_id().and_then(|id| self.listing.find(id)).cloned() {
            self.refresh_entry(entry);
        }
    }

    fn open(&mut self, id: &str, now_ms: i64) {
        let Some(entry) = self.listing.find(id).cloned() else {
            self.notice(Notice::warn("Mission gone"));
            return;
        };
        let view = match &entry.target {
            Target::Hunt {
                device_set,
                channel,
                freq_hz,
                status,
                running,
            } => View::Hunt(HuntOpen {
                device_set: *device_set,
                channel: *channel,
                freq_hz: *freq_hz,
                status: status.clone(),
                running: *running,
                refusal: entry.mission.blocker.clone(),
            }),
            Target::Df {
                array,
                fusion,
                freq_hz,
                state,
                ..
            } => View::Df {
                drive: Box::new(DfDrive::new(
                    Some(entry.id().to_owned()),
                    *freq_hz,
                    state.clone(),
                )),
                array: array.clone(),
                array_heard: false,
                fusion: fusion.clone(),
                shown: None,
            },
            Target::Fusion { state } => View::Df {
                drive: Box::new(DfDrive::new(None, 0.0, state.clone())),
                array: None,
                array_heard: false,
                fusion: Some(entry.id().to_owned()),
                shown: None,
            },
            Target::Radar { .. } => View::Radar(RadarOpen {
                update: None,
                updated_ms: None,
                stale: true,
            }),
            Target::Survey {
                freq_hz,
                recording,
                cells,
            } => View::Survey(Survey::new(*freq_hz, *recording, u64::from(*cells))),
        };
        let requests = seeds(&entry);
        self.open = Some(Open {
            entry,
            view,
            retarget: Retargeter::default(),
        });
        self.effects
            .extend(requests.into_iter().map(|what| Effect::Seed {
                mission: id.to_owned(),
                what,
            }));
        self.emit_open(now_ms);
    }

    fn emit_open(&mut self, now_ms: i64) {
        let event = match &self.open {
            Some(Open {
                entry,
                view: View::Hunt(hunt),
                ..
            }) => Some(CoreEvent::Hunt {
                view: hunt_view(entry.id(), hunt),
            }),
            Some(Open {
                entry,
                view: View::Survey(survey),
                ..
            }) => Some(CoreEvent::Survey {
                view: survey.view(entry.id()),
            }),
            _ => None,
        };
        if let Some(event) = event {
            self.emit(event);
        }
        self.emit_df(now_ms);
        self.emit_radar();
    }

    fn seeded(&mut self, mission: &str, seed: Seed, now_ms: i64) {
        let Some(open) = &mut self.open else {
            return;
        };
        if open.entry.id() != mission {
            return;
        }
        match (&mut open.view, seed) {
            (View::Radar(radar), Seed::Radar(update)) => {
                radar.update = Some(*update);
                radar.updated_ms = Some(now_ms);
                radar.stale = false;
                self.emit_radar();
            }
            (View::Survey(survey), Seed::Survey(grid)) => {
                let points = survey.seed(&grid);
                let view = survey.view(mission);
                if !points.is_empty() {
                    self.emit(CoreEvent::SurveyPoints { points });
                }
                self.emit(CoreEvent::Survey { view });
            }
            (View::Df { drive, .. }, Seed::Fusion(state)) => {
                drive.fusion(*state);
                self.emit_df(now_ms);
            }
            (
                View::Df {
                    drive,
                    array: Some(array),
                    array_heard: false,
                    ..
                },
                Seed::Arrays(statuses),
            ) => {
                if let Some(status) = statuses.iter().find(|status| status.node == *array) {
                    drive.array(status);
                    self.emit_df(now_ms);
                }
            }
            _ => {}
        }
    }

    fn target_mode(&mut self, mode: TargetMode, now_ms: i64) {
        if let Some(Open {
            view: View::Df { drive, .. },
            ..
        }) = &mut self.open
        {
            drive.target_mode = mode;
            self.emit_df(now_ms);
        }
    }

    fn pose(&mut self, pose: Option<PoseSnapshot>, now_ms: i64) {
        self.pose = pose;
        if since(self.guide_ms, now_ms) >= GUIDE_EVERY_MS {
            self.emit_df(now_ms);
        }
    }

    fn emit_df(&mut self, now_ms: i64) {
        let Some(Open {
            entry,
            view: View::Df { drive, shown, .. },
            retarget,
        }) = &mut self.open
        else {
            return;
        };
        let view = drive.view(entry.id(), self.pose, now_ms);
        *shown = Some(view.state);
        let notice = view
            .target
            .and_then(|target| retarget.offer(entry.id(), target, now_ms));
        self.guide_ms = Some(now_ms);
        if let Some(notice) = notice {
            self.emit(CoreEvent::Retarget { notice });
        }
        self.emit(CoreEvent::Df { view });
    }

    fn emit_radar(&mut self) {
        let Some(Open {
            entry,
            view: View::Radar(open),
            ..
        }) = &self.open
        else {
            return;
        };
        let view = open.update.as_ref().map_or_else(
            || radar::empty(entry.id()),
            |update| radar::view(entry.id(), update, open.stale),
        );
        self.emit(CoreEvent::Radar { view });
    }
}

fn since(then: Option<i64>, now_ms: i64) -> i64 {
    then.map_or(i64::MAX, |then| now_ms.saturating_sub(then))
}

fn hunt_view(mission: &str, hunt: &HuntOpen) -> super::views::HuntView {
    match &hunt.status {
        Some(status) => hunt::project(mission, status, hunt.running),
        None => hunt::idle(mission, hunt.freq_hz, hunt.running, hunt.refusal.clone()),
    }
}

fn seeds(entry: &Entry) -> Vec<SeedRequest> {
    match &entry.target {
        Target::Radar { .. } => vec![SeedRequest::Radar(entry.id().to_owned())],
        Target::Survey { .. } => vec![SeedRequest::Survey(entry.id().to_owned())],
        Target::Df { array, .. } => entry
            .fusion_node()
            .map(|node| SeedRequest::Fusion(node.to_owned()))
            .into_iter()
            .chain(array.as_ref().map(|_| SeedRequest::Arrays))
            .collect(),
        Target::Fusion { .. } => vec![SeedRequest::Fusion(entry.id().to_owned())],
        Target::Hunt { .. } => Vec::new(),
    }
}

fn same_wiring(next: &Entry, current: &Entry) -> bool {
    match (&next.target, &current.target) {
        (
            Target::Hunt {
                device_set: a_set,
                channel: a_channel,
                ..
            },
            Target::Hunt {
                device_set: b_set,
                channel: b_channel,
                ..
            },
        ) => a_set == b_set && a_channel == b_channel,
        (
            Target::Df {
                array: a_array,
                fusion: a_fusion,
                ..
            },
            Target::Df {
                array: b_array,
                fusion: b_fusion,
                ..
            },
        ) => a_array == b_array && a_fusion == b_fusion,
        (Target::Fusion { .. }, Target::Fusion { .. })
        | (Target::Survey { .. }, Target::Survey { .. }) => true,
        (Target::Radar { surface: a }, Target::Radar { surface: b }) => a == b,
        _ => false,
    }
}
