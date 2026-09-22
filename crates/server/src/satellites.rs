use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use sdrmm_engine::{Doppler, Engine};
use sdrmm_orbit::{Satellite, Tle};
use sdrmm_wire::{
    NodeBody, PatchGraph, SatelliteNode, SatellitePass, SatelliteStatus, ServerEvent,
};

use crate::{AppState, gps::GpsHub, workspace};

mod catalog;
mod track;

pub(crate) use catalog::Catalog;

const TICK: Duration = Duration::from_millis(250);
const PUBLISH_EVERY: Duration = Duration::from_secs(1);
const PASS_EVERY: Duration = Duration::from_secs(60);
const BIND_EVERY: Duration = Duration::from_secs(2);
const PASS_HORIZON_S: f64 = 2.0 * 86_400.0;
const HORIZON_DEG: f64 = 0.0;

#[derive(Clone, Debug, PartialEq)]
struct Wiring {
    node: String,
    settings: SatelliteNode,
    position: Option<String>,
    targets: Vec<String>,
}

#[derive(Clone, Default)]
struct Plan {
    graph: Arc<PatchGraph>,
    wirings: Vec<Wiring>,
}

pub(crate) struct SatelliteHub {
    plan: Arc<Mutex<Plan>>,
    statuses: Arc<Mutex<HashMap<String, SatelliteStatus>>>,
    wake: Mutex<Option<mpsc::Sender<()>>>,
    stop: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
    pub(crate) catalog: Catalog,
}

impl Default for SatelliteHub {
    fn default() -> Self {
        Self {
            plan: Arc::new(Mutex::new(Plan::default())),
            statuses: Arc::new(Mutex::new(HashMap::new())),
            wake: Mutex::new(None),
            stop: Arc::new(AtomicBool::new(false)),
            worker: Mutex::new(None),
            catalog: Catalog::default(),
        }
    }
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl SatelliteHub {
    pub(crate) fn snapshot(&self) -> Vec<ServerEvent> {
        locked(&self.statuses)
            .values()
            .map(|status| ServerEvent::SatelliteUpdate {
                status: Box::new(status.clone()),
            })
            .collect()
    }

    pub(crate) fn reconcile(&self, state: &AppState) {
        let graph = match state.store.active_workspace() {
            Ok(active) => active
                .map(|active| active.snapshot.graph)
                .unwrap_or_default(),
            Err(error) => {
                tracing::warn!(%error, "could not reconcile satellites");
                return;
            }
        };
        let wirings = wirings(&graph);
        let idle = wirings.is_empty();
        let tracked: HashSet<String> = wirings.iter().map(|wiring| wiring.node.clone()).collect();
        *locked(&self.plan) = Plan {
            graph: Arc::new(graph),
            wirings,
        };
        locked(&self.statuses).retain(|node, _| tracked.contains(node));
        if idle && locked(&self.worker).is_none() {
            return;
        }
        self.ensure_worker(state);
        if let Some(wake) = locked(&self.wake).as_ref() {
            let _ = wake.send(());
        }
    }

    fn ensure_worker(&self, state: &AppState) {
        let mut worker = locked(&self.worker);
        if worker.as_ref().is_some_and(|handle| !handle.is_finished()) {
            return;
        }
        let (wake_tx, wake_rx) = mpsc::channel();
        let tracker = Tracker {
            engine: Arc::downgrade(&state.engine),
            gps: state.gps.clone(),
            plan: self.plan.clone(),
            statuses: self.statuses.clone(),
            stop: self.stop.clone(),
            satellites: HashMap::new(),
            steered: HashSet::new(),
            bindings: HashMap::new(),
            bound_at: None,
            published_at: None,
        };
        match std::thread::Builder::new()
            .name("sdrmm-satellites".to_owned())
            .spawn(move || tracker.run(&wake_rx))
        {
            Ok(handle) => {
                *worker = Some(handle);
                *locked(&self.wake) = Some(wake_tx);
            }
            Err(error) => tracing::error!(%error, "could not start the satellite tracker"),
        }
    }
}

impl Drop for SatelliteHub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        locked(&self.wake).take();
        if let Some(worker) = locked(&self.worker).take()
            && worker.join().is_err()
        {
            tracing::error!("satellite tracker panicked");
        }
    }
}

fn wirings(graph: &PatchGraph) -> Vec<Wiring> {
    graph
        .nodes
        .iter()
        .filter_map(|node| match &node.body {
            NodeBody::Satellite(settings) => Some(Wiring {
                node: node.id.clone(),
                settings: settings.clone(),
                position: graph
                    .sources_of(&node.id, "position")
                    .next()
                    .map(str::to_owned),
                targets: graph
                    .targets_of(&node.id, "control")
                    .filter(|target| {
                        graph
                            .node(target)
                            .is_some_and(|node| matches!(node.body, NodeBody::Channel(_)))
                    })
                    .map(str::to_owned)
                    .collect(),
            }),
            _ => None,
        })
        .collect()
}

struct Orbit {
    tle: String,
    satellite: Result<Satellite, String>,
    pass: Option<SatellitePass>,
    passed_at: Option<Instant>,
}

struct Tracker {
    engine: Weak<Engine>,
    gps: Arc<GpsHub>,
    plan: Arc<Mutex<Plan>>,
    statuses: Arc<Mutex<HashMap<String, SatelliteStatus>>>,
    stop: Arc<AtomicBool>,
    satellites: HashMap<String, Orbit>,
    steered: HashSet<(u32, u32)>,
    bindings: HashMap<String, (u32, u32)>,
    bound_at: Option<Instant>,
    published_at: Option<Instant>,
}

impl Tracker {
    fn run(mut self, wake: &mpsc::Receiver<()>) {
        let mut rebind = true;
        loop {
            if self.stop.load(Ordering::Acquire) {
                return;
            }
            let Some(engine) = self.engine.upgrade() else {
                return;
            };
            let plan = locked(&self.plan).clone();
            if rebind || self.bound_at.is_none_or(|at| at.elapsed() >= BIND_EVERY) {
                self.bind(&engine, &plan.graph);
            }
            self.tick(&engine, &plan.wirings, rebind);
            drop(engine);
            rebind = match wake.recv_timeout(TICK) {
                Ok(()) => true,
                Err(RecvTimeoutError::Timeout) => false,
                Err(RecvTimeoutError::Disconnected) => return,
            };
        }
    }

    fn bind(&mut self, engine: &Engine, graph: &PatchGraph) {
        self.bindings = workspace::bind(graph, &engine.snapshot())
            .into_iter()
            .flat_map(|binding| {
                binding
                    .channels
                    .into_iter()
                    .map(move |(node, channel)| (node, (binding.device_set, channel)))
            })
            .collect();
        self.bound_at = Some(Instant::now());
    }

    fn tick(&mut self, engine: &Engine, wirings: &[Wiring], changed: bool) {
        let now = unix_now();
        let publish = changed
            || self
                .published_at
                .is_none_or(|at| at.elapsed() >= PUBLISH_EVERY);
        self.satellites
            .retain(|node, _| wirings.iter().any(|wiring| wiring.node == *node));
        let mut steered = HashSet::new();
        let mut statuses = HashMap::new();
        for wiring in wirings {
            let status = self.follow(engine, wiring, now, &mut steered);
            statuses.insert(wiring.node.clone(), status);
        }
        for &(ds, ch) in self.steered.difference(&steered) {
            if let Err(error) = engine.steer_channel(ds, ch, Doppler::default()) {
                tracing::debug!(%error, ds, ch, "could not release a Doppler correction");
            }
        }
        self.steered = steered;
        *locked(&self.statuses) = statuses.clone();
        if publish {
            self.published_at = Some(Instant::now());
            for status in statuses.into_values() {
                engine.emit_event(ServerEvent::SatelliteUpdate {
                    status: Box::new(status),
                });
            }
        }
    }

    fn follow(
        &mut self,
        engine: &Engine,
        wiring: &Wiring,
        now: f64,
        steered: &mut HashSet<(u32, u32)>,
    ) -> SatelliteStatus {
        let mut status = SatelliteStatus {
            node: wiring.node.clone(),
            ..SatelliteStatus::default()
        };
        let Some(orbit) = orbit_of(&mut self.satellites, wiring) else {
            status.error = Some("pick a satellite".to_owned());
            return status;
        };
        let satellite = match &orbit.satellite {
            Ok(satellite) => satellite,
            Err(error) => {
                status.error = Some(error.clone());
                return status;
            }
        };
        status.name = satellite.tle.name.clone();
        status.catalog = Some(satellite.tle.catalog.clone());
        status.tle_age_days = Some(satellite.age_days(now));
        let Some(source) = &wiring.position else {
            status.error = Some("wire a position".to_owned());
            return status;
        };
        let Some(fix) = self.gps.fix(source) else {
            status.error = Some("waiting for a position fix".to_owned());
            return status;
        };
        let observer = track::observer(&fix);
        let steering = match track::steer(satellite, &observer, &wiring.settings, now) {
            Ok(steering) => steering,
            Err(error) => {
                status.error = Some(error.to_string());
                return status;
            }
        };
        if orbit.passed_at.is_none_or(|at| at.elapsed() >= PASS_EVERY)
            || orbit
                .pass
                .is_some_and(|pass| pass.los.is_some_and(|los| (los as f64) < now))
        {
            orbit.passed_at = Some(Instant::now());
            orbit.pass = satellite
                .next_pass(&observer, now, PASS_HORIZON_S, HORIZON_DEG)
                .ok()
                .flatten()
                .map(track::pass);
        }
        status.look = Some(steering.look);
        status.visible = steering.look.elevation_deg >= HORIZON_DEG;
        status.next_pass = orbit.pass;
        status.uplink_hz = steering.uplink_hz;
        status.doppler_hz = steering.doppler.map(|doppler| doppler.shift_hz);
        status.doppler_rate_hz_s = steering.doppler.map(|doppler| doppler.rate_hz_s);
        if wiring.targets.is_empty() {
            return status;
        }
        let (Some(downlink_hz), Some(doppler)) = (wiring.settings.downlink_hz, steering.doppler)
        else {
            status.error = Some("set a downlink frequency".to_owned());
            return status;
        };
        for target in &wiring.targets {
            let Some(&(ds, ch)) = self.bindings.get(target) else {
                continue;
            };
            let driven = engine
                .tune_channel(ds, ch, downlink_hz)
                .and_then(|_| engine.steer_channel(ds, ch, doppler));
            match driven {
                Ok(()) => {
                    steered.insert((ds, ch));
                    status.driving.push(target.clone());
                }
                Err(error) => status.error = Some(format!("{target}: {error}")),
            }
        }
        status
    }
}

fn orbit_of<'a>(
    satellites: &'a mut HashMap<String, Orbit>,
    wiring: &Wiring,
) -> Option<&'a mut Orbit> {
    let tle = wiring.settings.tle.as_ref()?;
    if satellites
        .get(&wiring.node)
        .is_none_or(|orbit| orbit.tle != *tle)
    {
        let satellite = Tle::parse(tle)
            .and_then(Satellite::new)
            .map_err(|error| error.to_string());
        satellites.insert(
            wiring.node.clone(),
            Orbit {
                tle: tle.clone(),
                satellite,
                pass: None,
                passed_at: None,
            },
        );
    }
    satellites.get_mut(&wiring.node)
}

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{ChannelNode, GpsNode, PatchEdge, PatchNode, PortRef, Position};

    use super::*;

    fn node(id: &str, body: NodeBody) -> PatchNode {
        PatchNode {
            id: id.to_owned(),
            body,
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        }
    }

    fn edge(from: (&str, &str), to: (&str, &str)) -> PatchEdge {
        PatchEdge {
            from: PortRef {
                node: from.0.to_owned(),
                port: from.1.to_owned(),
            },
            to: PortRef {
                node: to.0.to_owned(),
                port: to.1.to_owned(),
            },
        }
    }

    #[test]
    fn only_wired_decoders_and_the_wired_position_are_followed() {
        let channel = || {
            NodeBody::Channel(ChannelNode {
                channel_type: "nfm".to_owned(),
                record_calls: false,
                tuning_locked: false,
            })
        };
        let graph = PatchGraph {
            nodes: vec![
                node("sat", NodeBody::Satellite(SatelliteNode::default())),
                node("gps", NodeBody::Gps(GpsNode::default())),
                node("voice", channel()),
                node("other", channel()),
            ],
            edges: vec![
                edge(("gps", "position"), ("sat", "position")),
                edge(("sat", "control"), ("voice", "control")),
            ],
        };
        let found = wirings(&graph);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].position.as_deref(), Some("gps"));
        assert_eq!(found[0].targets, vec!["voice".to_owned()]);
    }
}
