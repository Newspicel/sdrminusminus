mod carriers;
mod prospect;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak, mpsc},
    time::{Duration, Instant},
};

use prospect::Prospector;
use sdrmm_wire::{
    AudioProcessing, ChannelParams, ChannelSettings, DecodedRecord, DecoderEvent, DmrChannelEntry,
    DmrDiscovery, DmrParams, DmrSlots, DmrTrunkProtocol, DvFrame, DvFrameKind, DvTrunkProtocol,
    StateScope, TrunkChannel, TrunkChannelSource, TrunkFollower, TrunkProbe, TrunkProblem,
    TrunkSystemStatus,
};

use crate::Engine;

const RECONCILE_INTERVAL: Duration = Duration::from_secs(1);

const MAX_FOLLOWERS_PER_SYSTEM: usize = 16;

const RETRY_BASE: Duration = Duration::from_secs(5);
const RETRY_MAX: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrunkSystem {
    pub node: String,
    pub protocol: DmrTrunkProtocol,
    pub discovery: DmrDiscovery,
    pub channel_map: Vec<DmrChannelEntry>,
    pub learned: Vec<DmrChannelEntry>,
    pub radio: Option<TrunkRadio>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrunkRadio {
    pub device_set: u32,
    pub stream: u32,
    pub control_hz: u64,
    pub ignore_crc: bool,
}

pub(crate) enum TrunkInput {
    Record(Box<DecodedRecord>),
    Configure(Vec<TrunkSystem>),
    Carriers { device_set: u32, freq_hz: Vec<u64> },
}

#[derive(Clone)]
struct Carrier {
    system_node: String,
    protocol: DmrTrunkProtocol,
    device_set: u32,
    stream: u32,
    center_hz: f64,
    sample_rate: f64,
    freq_hz: u64,
    ignore_crc: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum FollowerKey {
    KnownCarrier {
        system_node: String,
        freq_hz: u64,
        slot: u8,
    },
    TierThree {
        system_node: String,
        logical_channel: u16,
        slot: u8,
    },
}

impl FollowerKey {
    fn system_node(&self) -> &str {
        match self {
            Self::KnownCarrier { system_node, .. } | Self::TierThree { system_node, .. } => {
                system_node
            }
        }
    }

    fn slot(&self) -> u8 {
        match self {
            Self::KnownCarrier { slot, .. } | Self::TierThree { slot, .. } => *slot,
        }
    }

    fn logical_channel(&self) -> Option<u16> {
        match self {
            Self::KnownCarrier { .. } => None,
            Self::TierThree {
                logical_channel, ..
            } => Some(*logical_channel),
        }
    }
}

struct FollowerChannel {
    device_set: u32,
    channel: u32,
    freq_hz: u64,
}

struct Problem {
    freq_hz: u64,
    reason: String,
    since: String,
    attempts: u32,
    next_attempt: Instant,
}

#[derive(Clone, Copy)]
struct Grant {
    slot: u8,
    freq_hz: u64,
}

pub(crate) fn spawn(
    engine: &Arc<Engine>,
    tx: mpsc::Sender<TrunkInput>,
    rx: mpsc::Receiver<TrunkInput>,
    status: Arc<Mutex<Vec<TrunkSystemStatus>>>,
) {
    let follower = Follower {
        engine: Arc::downgrade(engine),
        status,
        systems: Vec::new(),
        carriers: HashMap::new(),
        definitions: HashMap::new(),
        followers: HashMap::new(),
        problems: HashMap::new(),
        detected: HashMap::new(),
        prospectors: HashMap::new(),
        probes: HashMap::new(),
        controls: HashMap::new(),
        watching: Vec::new(),
        tx,
    };
    let spawned = std::thread::Builder::new()
        .name("sdrmm-trunk".to_string())
        .spawn(move || follower.run(&rx));
    if let Err(e) = spawned {
        tracing::error!("failed to spawn the trunk follower: {e}");
    }
}

struct Probe {
    system_node: String,
    freq_hz: u64,
}

struct Follower {
    engine: Weak<Engine>,
    status: Arc<Mutex<Vec<TrunkSystemStatus>>>,
    systems: Vec<TrunkSystem>,
    carriers: HashMap<(u32, u32), Carrier>,
    definitions: HashMap<(String, u16), u64>,
    followers: HashMap<FollowerKey, FollowerChannel>,
    problems: HashMap<FollowerKey, Problem>,
    detected: HashMap<String, DvTrunkProtocol>,
    prospectors: HashMap<String, Prospector>,
    probes: HashMap<(u32, u32), Probe>,
    controls: HashMap<String, FollowerChannel>,
    watching: Vec<carriers::Watch>,
    tx: mpsc::Sender<TrunkInput>,
}

impl Follower {
    fn run(mut self, rx: &mpsc::Receiver<TrunkInput>) {
        let mut next_reconcile = Instant::now();
        loop {
            let wait = next_reconcile.saturating_duration_since(Instant::now());
            match rx.recv_timeout(wait) {
                Ok(TrunkInput::Record(record)) => self.observe(&record),
                Ok(TrunkInput::Carriers {
                    device_set,
                    freq_hz,
                }) => self.note_carriers(device_set, &freq_hz),
                Ok(TrunkInput::Configure(systems)) => {
                    self.systems = systems;
                    self.reconcile();
                    next_reconcile = Instant::now() + RECONCILE_INTERVAL;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.reconcile();
                    next_reconcile = Instant::now() + RECONCILE_INTERVAL;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn resolve_carriers(&self, engine: &Engine) -> HashMap<(u32, u32), Carrier> {
        let state = engine.snapshot();
        let mut carriers = HashMap::new();
        for system in &self.systems {
            let Some(control) = self.controls.get(&system.node) else {
                continue;
            };
            let (device_set, channel) = (control.device_set, control.channel);
            let Some(set) = state.device_sets.iter().find(|set| set.id == device_set) else {
                continue;
            };
            let Some(info) = set.channels.iter().find(|info| info.id == channel) else {
                continue;
            };
            let ChannelParams::Dmr(params) = &info.settings.params else {
                continue;
            };
            let Some(center_hz) = set.settings.center_hz else {
                continue;
            };
            let absolute = center_hz + info.settings.offset_hz;
            if !absolute.is_finite() || absolute <= 0.0 {
                continue;
            }
            carriers.insert(
                (device_set, channel),
                Carrier {
                    system_node: system.node.clone(),
                    protocol: system.protocol,
                    device_set,
                    stream: info.stream,
                    center_hz,
                    sample_rate: crate::sample_rate_of(&set.settings),
                    freq_hz: absolute.round() as u64,
                    ignore_crc: params.ignore_crc,
                },
            );
        }
        carriers
    }

    fn protocol_of(
        &self,
        system_node: &str,
        configured: DmrTrunkProtocol,
    ) -> Option<DvTrunkProtocol> {
        match configured {
            DmrTrunkProtocol::CapacityPlus => Some(DvTrunkProtocol::CapacityPlus),
            DmrTrunkProtocol::HyteraXpt => Some(DvTrunkProtocol::HyteraXpt),
            DmrTrunkProtocol::TierThree => Some(DvTrunkProtocol::TierThree),
            DmrTrunkProtocol::Auto => self.detected.get(system_node).copied(),
        }
    }

    fn observe(&mut self, record: &DecodedRecord) {
        let Some(engine) = self.engine.upgrade() else {
            return;
        };
        let DecoderEvent::Dv(frame) = &record.event else {
            return;
        };
        if let Some(probe) = self.probes.get(&(record.device_set, record.channel)) {
            let (system_node, freq_hz) = (probe.system_node.clone(), probe.freq_hz);
            self.prospect(&engine, &system_node, freq_hz, frame);
            return;
        }
        let Some(carrier) = self
            .carriers
            .get(&(record.device_set, record.channel))
            .cloned()
        else {
            return;
        };
        if frame.crc_verified == Some(false) {
            return;
        }
        if let Some(protocol) = frame.trunk_protocol {
            self.note_detection(&carrier.system_node, protocol);
        }
        match self.protocol_of(&carrier.system_node, carrier.protocol) {
            Some(DvTrunkProtocol::CapacityPlus | DvTrunkProtocol::HyteraXpt) => {
                self.provision_known_carrier(&engine, &carrier);
            }
            Some(DvTrunkProtocol::TierThree) => self.observe_tier_three(&engine, &carrier, frame),
            None => {}
        }
    }

    fn note_detection(&mut self, system_node: &str, protocol: DvTrunkProtocol) {
        if self.detected.get(system_node) == Some(&protocol) {
            return;
        }
        self.detected.insert(system_node.to_owned(), protocol);
        tracing::info!(node = system_node, ?protocol, "trunk system identified");
        self.publish();
    }

    fn prospect(&mut self, engine: &Engine, system_node: &str, freq_hz: u64, frame: &DvFrame) {
        let Some(prospector) = self.prospectors.get_mut(system_node) else {
            return;
        };
        let Some(found) = prospector.note_probe(freq_hz, frame, Instant::now()) else {
            return;
        };
        if !found.confirmed {
            self.publish();
            return;
        }
        tracing::info!(
            node = system_node,
            logical_channel = found.logical_channel,
            freq_hz = found.freq_hz,
            "learned where a trunked logical channel transmits"
        );
        self.retire_probe(engine, system_node, found.freq_hz);
        let carrier = self
            .carriers
            .values()
            .find(|carrier| carrier.system_node == system_node)
            .cloned();
        if let Some(carrier) = carrier {
            self.ensure_follower(
                engine,
                &carrier,
                FollowerKey::TierThree {
                    system_node: system_node.to_owned(),
                    logical_channel: found.logical_channel,
                    slot: found.slot,
                },
                Grant {
                    slot: found.slot,
                    freq_hz: found.freq_hz,
                },
            );
        }
        self.publish();
    }

    fn retire_probe(&mut self, engine: &Engine, system_node: &str, freq_hz: u64) {
        let retired: Vec<(u32, u32)> = self
            .probes
            .iter()
            .filter(|(_, probe)| probe.system_node == system_node && probe.freq_hz == freq_hz)
            .map(|(key, _)| *key)
            .collect();
        for key in retired {
            self.close_probe(engine, key);
        }
    }

    fn close_probe(&mut self, engine: &Engine, key: (u32, u32)) {
        self.probes.remove(&key);
        match engine.remove_channel(key.0, key.1) {
            Ok(()) => {}
            Err(error) if error.is_not_found() => {}
            Err(error) => tracing::warn!(
                device_set = key.0,
                channel = key.1,
                %error,
                "could not close a trunk search receiver"
            ),
        }
    }

    fn note_carriers(&mut self, device_set: u32, freq_hz: &[u64]) {
        let now = Instant::now();
        let listening: Vec<String> = self
            .carriers
            .values()
            .filter(|carrier| carrier.device_set == device_set)
            .map(|carrier| carrier.system_node.clone())
            .collect();
        for node in listening {
            if let Some(prospector) = self.prospectors.get_mut(&node) {
                prospector.note_carriers(freq_hz, now);
            }
        }
    }

    fn watch_bands(&mut self, engine: &Arc<Engine>, live: &[Carrier]) {
        let wanted: Vec<(u32, u32)> = self
            .systems
            .iter()
            .filter(|system| system.discovery.enabled && system.discovery.valid())
            .filter_map(|system| {
                let carrier = live
                    .iter()
                    .find(|carrier| carrier.system_node == system.node)?;
                Some((carrier.device_set, carrier.stream))
            })
            .collect();
        self.watching
            .retain(|watch| watch.live() && wanted.contains(&(watch.device_set, watch.stream)));
        for (device_set, stream) in wanted {
            if self
                .watching
                .iter()
                .any(|watch| watch.device_set == device_set && watch.stream == stream)
            {
                continue;
            }
            if let Some(watch) = carriers::Watch::start(engine, self.tx.clone(), device_set, stream)
            {
                self.watching.push(watch);
            }
        }
    }

    fn tune_control_channels(&mut self, engine: &Engine) {
        let nodes: Vec<String> = self
            .systems
            .iter()
            .filter(|system| system.radio.is_some())
            .map(|system| system.node.clone())
            .collect();
        let dropped: Vec<(String, u32, u32)> = self
            .controls
            .iter()
            .filter(|(node, _)| !nodes.contains(node))
            .map(|(node, control)| (node.clone(), control.device_set, control.channel))
            .collect();
        for (node, device_set, channel) in dropped {
            self.controls.remove(&node);
            match engine.remove_channel(device_set, channel) {
                Ok(()) | Err(_) => {}
            }
        }
        let wanted: Vec<(String, TrunkRadio)> = self
            .systems
            .iter()
            .filter_map(|system| Some((system.node.clone(), system.radio?)))
            .collect();
        for (node, radio) in wanted {
            if self
                .controls
                .get(&node)
                .is_some_and(|control| control.freq_hz == radio.control_hz)
            {
                continue;
            }
            let Some(center_hz) = engine
                .snapshot()
                .device_sets
                .iter()
                .find(|set| set.id == radio.device_set)
                .and_then(|set| set.settings.center_hz)
            else {
                continue;
            };
            let params = ChannelParams::Dmr(DmrParams {
                slots: DmrSlots::Both,
                ignore_crc: radio.ignore_crc,
            });
            let settings = ChannelSettings {
                offset_hz: radio.control_hz as f64 - center_hz,
                squelch_db: None,
                squelch_auto_db: None,
                audio: AudioProcessing::default_for(params.type_id()),
                params,
            };
            let result = match self.controls.get(&node) {
                Some(control) => engine
                    .patch_channel(radio.device_set, control.channel, settings)
                    .map(|()| control.channel),
                None => engine.add_channel(radio.device_set, radio.stream, settings),
            };
            match result {
                Ok(channel) => {
                    self.controls.insert(
                        node,
                        FollowerChannel {
                            device_set: radio.device_set,
                            channel,
                            freq_hz: radio.control_hz,
                        },
                    );
                }
                Err(error) => tracing::warn!(
                    node,
                    freq_hz = radio.control_hz,
                    %error,
                    "could not open the trunk control channel"
                ),
            }
        }
    }

    fn search(&mut self, engine: &Engine, live: &[Carrier]) {
        let now = Instant::now();
        let mut wanted: Vec<(String, u64, Carrier)> = Vec::new();
        let mut keep: Vec<(String, u64)> = Vec::new();
        for system in &self.systems {
            let Some(carrier) = live
                .iter()
                .find(|carrier| carrier.system_node == system.node)
            else {
                continue;
            };
            if self.protocol_of(&system.node, carrier.protocol) != Some(DvTrunkProtocol::TierThree)
            {
                continue;
            }
            let Some(prospector) = self.prospectors.get_mut(&system.node) else {
                continue;
            };
            let Some(reach) = probe_reach(carrier) else {
                continue;
            };
            let taken: Vec<u64> = self
                .followers
                .iter()
                .filter(|(key, _)| key.system_node() == system.node)
                .map(|(_, follower)| follower.freq_hz)
                .chain(std::iter::once(carrier.freq_hz))
                .collect();
            let open: Vec<u64> = self
                .probes
                .values()
                .filter(|probe| probe.system_node == system.node)
                .map(|probe| probe.freq_hz)
                .collect();
            for freq_hz in prospector.schedule(now, &open, |freq_hz| {
                !taken.contains(&freq_hz) && reach.contains(&freq_hz)
            }) {
                keep.push((system.node.clone(), freq_hz));
                if !open.contains(&freq_hz) {
                    wanted.push((system.node.clone(), freq_hz, carrier.clone()));
                }
            }
        }
        let stale: Vec<(u32, u32)> = self
            .probes
            .iter()
            .filter(|(_, probe)| {
                !keep
                    .iter()
                    .any(|(node, freq_hz)| *node == probe.system_node && *freq_hz == probe.freq_hz)
            })
            .map(|(key, _)| *key)
            .collect();
        for key in stale {
            self.close_probe(engine, key);
        }
        for (system_node, freq_hz, carrier) in wanted {
            self.open_probe(engine, &system_node, freq_hz, &carrier);
        }
    }

    fn open_probe(&mut self, engine: &Engine, system_node: &str, freq_hz: u64, carrier: &Carrier) {
        let params = ChannelParams::Dmr(DmrParams {
            slots: DmrSlots::Both,
            ignore_crc: carrier.ignore_crc,
        });
        let settings = ChannelSettings {
            offset_hz: freq_hz as f64 - carrier.center_hz,
            squelch_db: None,
            squelch_auto_db: None,
            audio: AudioProcessing::default_for(params.type_id()),
            params,
        };
        match engine.add_channel(carrier.device_set, carrier.stream, settings) {
            Ok(channel) => {
                self.probes.insert(
                    (carrier.device_set, channel),
                    Probe {
                        system_node: system_node.to_owned(),
                        freq_hz,
                    },
                );
            }
            Err(error) => tracing::debug!(
                node = system_node,
                freq_hz,
                %error,
                "could not open a trunk search receiver"
            ),
        }
    }

    fn observe_tier_three(&mut self, engine: &Engine, carrier: &Carrier, frame: &DvFrame) {
        if let Some(color_code) = frame.color_code
            && let Ok(color_code) = u8::try_from(color_code)
            && let Some(prospector) = self.prospectors.get_mut(&carrier.system_node)
        {
            prospector.note_site(color_code);
        }
        if let Some(definition) = &frame.channel_definition {
            self.definitions.insert(
                (carrier.system_node.clone(), definition.channel),
                definition.rx_hz,
            );
        }
        if frame.kind != DvFrameKind::Control {
            return;
        }
        for call in granted_calls(frame) {
            self.follow_granted(engine, carrier, call);
        }
    }

    fn follow_granted(&mut self, engine: &Engine, carrier: &Carrier, call: GrantedCall) {
        let announced = self
            .definitions
            .get(&(carrier.system_node.clone(), call.logical_channel))
            .copied();
        let Some(freq_hz) = announced.or_else(|| {
            self.prospectors
                .get(&carrier.system_node)
                .and_then(|prospector| prospector.frequency_of(call.logical_channel))
        }) else {
            if let Some(prospector) = self.prospectors.get_mut(&carrier.system_node) {
                prospector.note_grant(
                    call.logical_channel,
                    call.slot,
                    call.destination,
                    call.source,
                    Instant::now(),
                );
            }
            return;
        };
        self.ensure_follower(
            engine,
            carrier,
            FollowerKey::TierThree {
                system_node: carrier.system_node.clone(),
                logical_channel: call.logical_channel,
                slot: call.slot,
            },
            Grant {
                slot: call.slot,
                freq_hz,
            },
        );
    }

    fn provision_known_carrier(&mut self, engine: &Engine, carrier: &Carrier) {
        for freq_hz in self.repeaters_of(carrier) {
            for slot in [1, 2] {
                self.ensure_follower(
                    engine,
                    carrier,
                    FollowerKey::KnownCarrier {
                        system_node: carrier.system_node.clone(),
                        freq_hz,
                        slot,
                    },
                    Grant { slot, freq_hz },
                );
            }
        }
    }

    /// Capacity Plus and XPT grant no frequency, so the plan is the traffic list.
    fn repeaters_of(&self, carrier: &Carrier) -> Vec<u64> {
        let mut freqs = vec![carrier.freq_hz];
        let Some(system) = self
            .systems
            .iter()
            .find(|system| system.node == carrier.system_node)
        else {
            return freqs;
        };
        for entry in &system.channel_map {
            if !freqs.contains(&entry.freq_hz) {
                freqs.push(entry.freq_hz);
            }
        }
        freqs
    }

    fn ensure_follower(
        &mut self,
        engine: &Engine,
        carrier: &Carrier,
        key: FollowerKey,
        grant: Grant,
    ) {
        if self
            .followers
            .get(&key)
            .is_some_and(|follower| follower.freq_hz == grant.freq_hz)
        {
            return;
        }
        if self.backing_off(&key, grant.freq_hz) {
            return;
        }
        if !self.followers.contains_key(&key) && self.at_limit(key.system_node()) {
            self.fail(
                key,
                grant,
                format!("the system already holds {MAX_FOLLOWERS_PER_SYSTEM} traffic channels"),
            );
            return;
        }
        let params = ChannelParams::Dmr(DmrParams {
            slots: if grant.slot == 1 {
                DmrSlots::One
            } else {
                DmrSlots::Two
            },
            ignore_crc: carrier.ignore_crc,
        });
        let settings = ChannelSettings {
            offset_hz: grant.freq_hz as f64 - carrier.center_hz,
            squelch_db: None,
            squelch_auto_db: None,
            audio: AudioProcessing::default_for(params.type_id()),
            params,
        };
        let result = match self.followers.get(&key) {
            Some(follower) => engine
                .patch_channel(carrier.device_set, follower.channel, settings)
                .map(|()| follower.channel),
            None => engine.add_channel(carrier.device_set, carrier.stream, settings),
        };
        match result {
            Ok(channel) => {
                let recovered = self.problems.remove(&key).is_some();
                self.followers.insert(
                    key,
                    FollowerChannel {
                        device_set: carrier.device_set,
                        channel,
                        freq_hz: grant.freq_hz,
                    },
                );
                self.publish();
                if recovered {
                    engine.emit_scope(StateScope::DeviceSet(carrier.device_set));
                }
            }
            Err(error) => {
                let device_set = carrier.device_set;
                let announce = self.fail(key, grant, error.to_string());
                if announce {
                    engine.emit_scope(StateScope::DeviceSet(device_set));
                }
            }
        }
    }

    fn at_limit(&self, system_node: &str) -> bool {
        self.followers
            .keys()
            .filter(|candidate| candidate.system_node() == system_node)
            .count()
            >= MAX_FOLLOWERS_PER_SYSTEM
    }

    fn backing_off(&self, key: &FollowerKey, freq_hz: u64) -> bool {
        self.problems.get(key).is_some_and(|problem| {
            problem.freq_hz == freq_hz && Instant::now() < problem.next_attempt
        })
    }

    fn fail(&mut self, key: FollowerKey, grant: Grant, reason: String) -> bool {
        let now = Instant::now();
        let existing = self
            .problems
            .get(&key)
            .filter(|problem| problem.freq_hz == grant.freq_hz);
        let attempts = existing.map_or(1, |problem| problem.attempts.saturating_add(1));
        let fresh = existing.is_none() || existing.is_some_and(|problem| problem.reason != reason);
        let since = existing.map_or_else(
            || format!("{:.9}", jiff::Timestamp::now()),
            |problem| problem.since.clone(),
        );
        let backoff = RETRY_BASE
            .saturating_mul(1u32 << attempts.saturating_sub(1).min(6))
            .min(RETRY_MAX);
        if fresh {
            tracing::warn!(
                node = key.system_node(),
                logical_channel = key.logical_channel(),
                slot = grant.slot,
                freq_hz = grant.freq_hz,
                reason,
                "cannot follow a trunked traffic channel"
            );
        } else {
            tracing::debug!(
                node = key.system_node(),
                freq_hz = grant.freq_hz,
                attempts,
                "trunked traffic channel still refused"
            );
        }
        self.problems.insert(
            key,
            Problem {
                freq_hz: grant.freq_hz,
                reason,
                since,
                attempts,
                next_attempt: now + backoff,
            },
        );
        if fresh {
            self.publish();
        }
        fresh
    }

    fn reconcile(&mut self) {
        let Some(engine) = self.engine.upgrade() else {
            return;
        };
        self.tune_control_channels(&engine);
        self.carriers = self.resolve_carriers(&engine);
        let live: Vec<Carrier> = self.carriers.values().cloned().collect();
        let known: Vec<(FollowerKey, u32, u32)> = self
            .followers
            .iter()
            .filter(|(key, _)| !self.system_holds(&live, key))
            .map(|(key, follower)| (key.clone(), follower.device_set, follower.channel))
            .collect();
        for (key, device_set, channel) in known {
            match engine.remove_channel(device_set, channel) {
                Ok(()) => {}
                Err(error) if error.is_not_found() => {}
                Err(error) => tracing::warn!(
                    device_set,
                    channel,
                    %error,
                    "could not remove an orphaned trunk follower"
                ),
            }
            self.followers.remove(&key);
        }
        let nodes: Vec<&str> = self
            .systems
            .iter()
            .map(|system| system.node.as_str())
            .collect();
        self.definitions
            .retain(|(node, _), _| nodes.contains(&node.as_str()));
        self.detected
            .retain(|node, _| nodes.contains(&node.as_str()));
        let settled: Vec<FollowerKey> = self
            .problems
            .keys()
            .filter(|key| !nodes.contains(&key.system_node()) || !self.system_holds(&live, key))
            .cloned()
            .collect();
        for key in settled {
            self.problems.remove(&key);
        }
        self.prospectors
            .retain(|node, _| nodes.contains(&node.as_str()));
        for system in &self.systems {
            match self.prospectors.get_mut(&system.node) {
                Some(prospector) => prospector.configure(&system.discovery, &system.channel_map),
                None => {
                    let mut prospector = Prospector::new(&system.discovery, &system.channel_map);
                    prospector.adopt(&system.learned);
                    self.prospectors.insert(system.node.clone(), prospector);
                }
            }
        }
        for carrier in &live {
            if matches!(
                self.protocol_of(&carrier.system_node, carrier.protocol),
                Some(DvTrunkProtocol::CapacityPlus | DvTrunkProtocol::HyteraXpt)
            ) {
                self.provision_known_carrier(&engine, carrier);
            }
        }
        self.watch_bands(&engine, &live);
        let before = self.probes.len();
        self.search(&engine, &live);
        if self.probes.len() != before {
            for device_set in device_sets(&live) {
                engine.emit_scope(StateScope::DeviceSet(device_set));
            }
        }
        self.publish();
    }

    fn system_holds(&self, live: &[Carrier], key: &FollowerKey) -> bool {
        match key {
            FollowerKey::KnownCarrier {
                system_node,
                freq_hz,
                ..
            } => live.iter().any(|source| {
                source.system_node == *system_node
                    && matches!(
                        self.protocol_of(system_node, source.protocol),
                        Some(DvTrunkProtocol::CapacityPlus | DvTrunkProtocol::HyteraXpt)
                    )
                    && self.repeaters_of(source).contains(freq_hz)
            }),
            FollowerKey::TierThree { system_node, .. } => live.iter().any(|source| {
                source.system_node == *system_node
                    && self.protocol_of(system_node, source.protocol)
                        == Some(DvTrunkProtocol::TierThree)
            }),
        }
    }

    fn publish(&self) {
        let statuses = self
            .systems
            .iter()
            .map(|system| self.status_of(system))
            .collect();
        *self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = statuses;
    }

    fn status_of(&self, system: &TrunkSystem) -> TrunkSystemStatus {
        let mut status = TrunkSystemStatus {
            node: system.node.clone(),
            detected: self.detected.get(&system.node).copied(),
            carriers: self
                .carriers
                .values()
                .filter(|carrier| carrier.system_node == system.node)
                .count() as u32,
            followers: self
                .followers
                .iter()
                .filter(|(key, _)| key.system_node() == system.node)
                .map(|(key, follower)| TrunkFollower {
                    device_set: follower.device_set,
                    channel: follower.channel,
                    logical_channel: key.logical_channel(),
                    slot: key.slot(),
                    freq_hz: follower.freq_hz,
                })
                .collect(),
            problems: self
                .problems
                .iter()
                .filter(|(key, _)| key.system_node() == system.node)
                .map(|(key, problem)| TrunkProblem {
                    freq_hz: problem.freq_hz,
                    slot: key.slot(),
                    logical_channel: key.logical_channel(),
                    reason: problem.reason.clone(),
                    since: problem.since.clone(),
                    attempts: problem.attempts,
                })
                .collect(),
            channel_map: self.channel_map_of(&system.node),
            probes: self
                .probes
                .iter()
                .filter(|(_, probe)| probe.system_node == system.node)
                .map(|((device_set, channel), probe)| TrunkProbe {
                    device_set: *device_set,
                    channel: *channel,
                    freq_hz: probe.freq_hz,
                })
                .collect(),
            searching: self
                .prospectors
                .get(&system.node)
                .map_or(0, Prospector::searching),
            color_code: self
                .prospectors
                .get(&system.node)
                .and_then(Prospector::color_code),
        };
        status
            .followers
            .sort_by_key(|follower| (follower.freq_hz, follower.slot));
        status
            .problems
            .sort_by_key(|problem| (problem.freq_hz, problem.slot));
        status.probes.sort_by_key(|probe| probe.freq_hz);
        status
    }

    fn channel_map_of(&self, system_node: &str) -> Vec<TrunkChannel> {
        let mut map: Vec<TrunkChannel> = self
            .definitions
            .iter()
            .filter(|((node, _), _)| node == system_node)
            .map(|((_, logical_channel), freq_hz)| TrunkChannel {
                logical_channel: *logical_channel,
                freq_hz: *freq_hz,
                source: TrunkChannelSource::Announced,
                confidence: 100,
            })
            .collect();
        if let Some(prospector) = self.prospectors.get(system_node) {
            map.extend(prospector.channel_map().into_iter().filter(|channel| {
                !self
                    .definitions
                    .contains_key(&(system_node.to_owned(), channel.logical_channel))
            }));
        }
        map.sort_unstable_by_key(|channel| channel.logical_channel);
        map
    }
}

#[derive(Clone, Copy)]
struct GrantedCall {
    logical_channel: u16,
    slot: u8,
    destination: u32,
    source: Option<u32>,
}

fn granted_calls(frame: &DvFrame) -> Vec<GrantedCall> {
    let updates: Vec<GrantedCall> = frame
        .slot_activity
        .iter()
        .filter_map(|activity| {
            Some(GrantedCall {
                logical_channel: activity.logical_channel?,
                slot: activity.slot,
                destination: activity.destination?,
                source: None,
            })
        })
        .collect();
    if !updates.is_empty() {
        return updates;
    }
    match (frame.channel, frame.slot, frame.destination) {
        (Some(logical_channel), Some(slot), Some(destination)) => vec![GrantedCall {
            logical_channel,
            slot,
            destination,
            source: frame.source,
        }],
        _ => Vec::new(),
    }
}

fn device_sets(live: &[Carrier]) -> Vec<u32> {
    let mut sets: Vec<u32> = live.iter().map(|carrier| carrier.device_set).collect();
    sets.sort_unstable();
    sets.dedup();
    sets
}

fn probe_reach(carrier: &Carrier) -> Option<std::ops::RangeInclusive<u64>> {
    let (low, high) = sdrmm_channels::occupied_band(&ChannelParams::Dmr(DmrParams::default()));
    let nyquist = carrier.sample_rate / 2.0;
    let lowest = (carrier.center_hz - nyquist - low).max(0.0);
    let highest = carrier.center_hz + nyquist - high;
    if !lowest.is_finite() || !highest.is_finite() || highest < lowest {
        return None;
    }
    Some((lowest.ceil() as u64)..=(highest.floor() as u64))
}

#[cfg(test)]
mod tests {
    use sdrmm_device::DeviceRegistry;
    use sdrmm_device_virtual::VirtualDriver;
    use sdrmm_wire::{DecoderEvent, DvChannelDefinition, DvMode};

    use super::*;

    const LOGICAL_CHANNEL: u16 = 17;

    fn engine() -> Arc<Engine> {
        let mut registry = DeviceRegistry::new();
        registry.register(1, Box::new(VirtualDriver::new()));
        Engine::with_registry(registry, None)
    }

    fn control_channel(engine: &Engine) -> (u32, u32, f64) {
        let device_set = engine
            .create_device_set("virtual:siggen")
            .expect("virtual device");
        let channel = engine
            .add_channel(
                device_set,
                0,
                ChannelSettings {
                    offset_hz: 0.0,
                    squelch_db: None,
                    squelch_auto_db: None,
                    params: ChannelParams::Dmr(DmrParams::default()),
                    audio: Default::default(),
                },
            )
            .expect("control channel");
        let center_hz = engine.snapshot().device_sets[0]
            .settings
            .center_hz
            .expect("a tuned radio");
        (device_set, channel, center_hz)
    }

    fn follower(engine: &Arc<Engine>, protocol: DmrTrunkProtocol, carrier: (u32, u32)) -> Follower {
        searching_follower(
            engine,
            protocol,
            carrier,
            DmrDiscovery::default(),
            Vec::new(),
        )
    }

    fn searching_follower(
        engine: &Arc<Engine>,
        protocol: DmrTrunkProtocol,
        carrier: (u32, u32),
        discovery: DmrDiscovery,
        channel_map: Vec<DmrChannelEntry>,
    ) -> Follower {
        let control_hz = engine
            .snapshot()
            .device_sets
            .iter()
            .find(|set| set.id == carrier.0)
            .and_then(|set| set.settings.center_hz)
            .expect("a tuned radio") as u64;
        let mut follower = Follower {
            engine: Arc::downgrade(engine),
            status: Arc::new(Mutex::new(Vec::new())),
            systems: vec![TrunkSystem {
                node: "trunk".to_owned(),
                protocol,
                discovery,
                channel_map,
                learned: Vec::new(),
                radio: Some(TrunkRadio {
                    device_set: carrier.0,
                    stream: 0,
                    control_hz,
                    ignore_crc: false,
                }),
            }],
            controls: HashMap::from([(
                "trunk".to_owned(),
                FollowerChannel {
                    device_set: carrier.0,
                    channel: carrier.1,
                    freq_hz: control_hz,
                },
            )]),
            carriers: HashMap::new(),
            definitions: HashMap::new(),
            followers: HashMap::new(),
            problems: HashMap::new(),
            detected: HashMap::new(),
            prospectors: HashMap::new(),
            probes: HashMap::new(),
            watching: Vec::new(),
            tx: mpsc::channel().0,
        };
        follower.reconcile();
        follower
    }

    fn record(carrier: (u32, u32), frame: DvFrame) -> DecodedRecord {
        DecodedRecord {
            device_set: carrier.0,
            channel: carrier.1,
            at: "2026-08-14T10:00:00Z".to_owned(),
            freq_hz: 451_000_000.0,
            event: DecoderEvent::Dv(frame),
        }
    }

    fn definition(rx_hz: u64) -> DvFrame {
        DvFrame {
            trunk_protocol: Some(DvTrunkProtocol::TierThree),
            crc_verified: Some(true),
            channel_definition: Some(DvChannelDefinition {
                channel: LOGICAL_CHANNEL,
                tx_hz: rx_hz,
                rx_hz,
                color_code: None,
            }),
            ..DvFrame::new(DvMode::Dmr, DvFrameKind::Control)
        }
    }

    fn grant(slot: u8) -> DvFrame {
        DvFrame {
            trunk_protocol: Some(DvTrunkProtocol::TierThree),
            crc_verified: Some(true),
            channel: Some(LOGICAL_CHANNEL),
            slot: Some(slot),
            destination: Some(91),
            ..DvFrame::new(DvMode::Dmr, DvFrameKind::Control)
        }
    }

    fn search_over(center_hz: f64) -> DmrDiscovery {
        DmrDiscovery {
            enabled: true,
            ranges: vec![sdrmm_wire::DmrSearchRange {
                start_hz: center_hz as u64 + 12_500,
                end_hz: center_hz as u64 + 50_000,
                step_hz: 12_500,
            }],
            max_probes: 4,
        }
    }

    fn traffic(destination: u32, source: u32, slot: u8) -> DvFrame {
        DvFrame {
            slot: Some(slot),
            destination: Some(destination),
            source: Some(source),
            ..DvFrame::new(DvMode::Dmr, DvFrameKind::Header)
        }
    }

    fn identified_grant(slot: u8) -> DvFrame {
        DvFrame {
            source: Some(4001),
            ..grant(slot)
        }
    }

    fn status(follower: &Follower) -> TrunkSystemStatus {
        follower
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .first()
            .cloned()
            .expect("one system")
    }

    #[test]
    fn a_grant_on_a_defined_channel_opens_a_slot_filtered_receiver() {
        let engine = engine();
        let (device_set, channel, center_hz) = control_channel(&engine);
        let carrier = (device_set, channel);
        let mut follower = follower(&engine, DmrTrunkProtocol::TierThree, carrier);
        let traffic = center_hz as u64 + 125_000;

        follower.observe(&record(carrier, definition(traffic)));
        follower.observe(&record(carrier, grant(2)));

        let set = engine.snapshot().device_sets.remove(0);
        assert_eq!(set.channels.len(), 2, "the follower was not created");
        let followed = &set.channels[1];
        assert_eq!(followed.settings.offset_hz, 125_000.0);
        assert_eq!(
            followed.settings.params,
            ChannelParams::Dmr(DmrParams {
                slots: DmrSlots::Two,
                ignore_crc: false,
            })
        );
        let status = status(&follower);
        assert_eq!(status.followers.len(), 1);
        assert_eq!(status.followers[0].freq_hz, traffic);
        assert_eq!(status.followers[0].logical_channel, Some(LOGICAL_CHANNEL));
        assert!(status.problems.is_empty());
    }

    #[test]
    fn a_grant_for_an_undefined_channel_opens_nothing() {
        let engine = engine();
        let (device_set, channel, _) = control_channel(&engine);
        let carrier = (device_set, channel);
        let mut follower = follower(&engine, DmrTrunkProtocol::TierThree, carrier);

        follower.observe(&record(carrier, grant(1)));

        assert_eq!(engine.snapshot().device_sets[0].channels.len(), 1);
        assert!(follower.followers.is_empty());
    }

    #[test]
    fn an_unverified_grant_is_ignored() {
        let engine = engine();
        let (device_set, channel, center_hz) = control_channel(&engine);
        let carrier = (device_set, channel);
        let mut follower = follower(&engine, DmrTrunkProtocol::TierThree, carrier);

        follower.observe(&record(carrier, definition(center_hz as u64 + 125_000)));
        let mut unverified = grant(1);
        unverified.crc_verified = Some(false);
        follower.observe(&record(carrier, unverified));

        assert_eq!(engine.snapshot().device_sets[0].channels.len(), 1);
    }

    #[test]
    fn auto_follows_capacity_plus_once_its_signalling_identifies_it() {
        let engine = engine();
        let (device_set, channel, _) = control_channel(&engine);
        let carrier = (device_set, channel);
        let mut follower = follower(&engine, DmrTrunkProtocol::Auto, carrier);
        assert!(
            follower.followers.is_empty(),
            "auto followed before it knew"
        );

        follower.observe(&record(
            carrier,
            DvFrame {
                trunk_protocol: Some(DvTrunkProtocol::CapacityPlus),
                crc_verified: Some(true),
                ..DvFrame::new(DvMode::Dmr, DvFrameKind::Control)
            },
        ));

        let set = engine.snapshot().device_sets.remove(0);
        assert_eq!(set.channels.len(), 3, "both slots were not provisioned");
        let status = status(&follower);
        assert_eq!(status.detected, Some(DvTrunkProtocol::CapacityPlus));
        assert_eq!(status.followers.len(), 2);
        assert_eq!(status.followers[0].slot, 1);
        assert_eq!(status.followers[1].slot, 2);
    }

    #[test]
    fn capacity_plus_keeps_both_slots_of_every_repeater_the_plan_names() {
        let engine = engine();
        let (device_set, channel, center_hz) = control_channel(&engine);
        let carrier = (device_set, channel);
        let second = center_hz as u64 + 25_000;
        let mut follower = searching_follower(
            &engine,
            DmrTrunkProtocol::CapacityPlus,
            carrier,
            DmrDiscovery::default(),
            vec![DmrChannelEntry {
                lcn: 2,
                freq_hz: second,
            }],
        );

        follower.observe(&record(
            carrier,
            DvFrame {
                trunk_protocol: Some(DvTrunkProtocol::CapacityPlus),
                crc_verified: Some(true),
                ..DvFrame::new(DvMode::Dmr, DvFrameKind::Control)
            },
        ));

        let planned = status(&follower);
        assert_eq!(planned.followers.len(), 4);
        assert!(
            planned
                .followers
                .iter()
                .filter(|follower| follower.freq_hz == second)
                .count()
                == 2,
            "the second repeater of the plan was never followed"
        );

        follower.systems[0].channel_map.clear();
        follower.reconcile();

        let after = status(&follower);
        assert_eq!(
            after.followers.len(),
            2,
            "a forgotten repeater kept running"
        );
        assert!(after.followers.iter().all(|kept| kept.freq_hz != second));
    }

    #[test]
    fn auto_follows_hytera_xpt_once_its_signalling_identifies_it() {
        let engine = engine();
        let (device_set, channel, _) = control_channel(&engine);
        let carrier = (device_set, channel);
        let mut follower = follower(&engine, DmrTrunkProtocol::Auto, carrier);
        assert!(
            follower.followers.is_empty(),
            "auto followed before it knew"
        );

        follower.observe(&record(
            carrier,
            DvFrame {
                trunk_protocol: Some(DvTrunkProtocol::HyteraXpt),
                crc_verified: Some(true),
                ..DvFrame::new(DvMode::Dmr, DvFrameKind::Control)
            },
        ));

        let set = engine.snapshot().device_sets.remove(0);
        assert_eq!(set.channels.len(), 3, "both slots were not provisioned");
        assert!(set.channels.iter().any(|channel| {
            matches!(
                channel.settings.params,
                ChannelParams::Dmr(DmrParams {
                    slots: DmrSlots::One,
                    ..
                })
            )
        }));
        assert!(set.channels.iter().any(|channel| {
            matches!(
                channel.settings.params,
                ChannelParams::Dmr(DmrParams {
                    slots: DmrSlots::Two,
                    ..
                })
            )
        }));
        let status = status(&follower);
        assert_eq!(status.detected, Some(DvTrunkProtocol::HyteraXpt));
        assert_eq!(status.followers.len(), 2);
        assert_eq!(status.followers[0].slot, 1);
        assert_eq!(status.followers[1].slot, 2);
    }

    #[test]
    fn a_grant_outside_the_sampled_band_is_reported_once_and_backed_off() {
        let engine = engine();
        let (device_set, channel, center_hz) = control_channel(&engine);
        let carrier = (device_set, channel);
        let mut follower = follower(&engine, DmrTrunkProtocol::TierThree, carrier);
        let far = center_hz as u64 + 10_000_000;

        follower.observe(&record(carrier, definition(far)));
        follower.observe(&record(carrier, grant(1)));
        follower.observe(&record(carrier, grant(1)));

        assert_eq!(engine.snapshot().device_sets[0].channels.len(), 1);
        let status = status(&follower);
        assert_eq!(status.problems.len(), 1);
        assert_eq!(status.problems[0].freq_hz, far);
        assert_eq!(status.problems[0].attempts, 1, "the refusal was retried");
        assert!(!status.problems[0].reason.is_empty());
    }

    #[test]
    fn dropping_the_system_closes_the_receivers_it_opened() {
        let engine = engine();
        let (device_set, channel, center_hz) = control_channel(&engine);
        let carrier = (device_set, channel);
        let mut follower = follower(&engine, DmrTrunkProtocol::TierThree, carrier);
        follower.observe(&record(carrier, definition(center_hz as u64 + 125_000)));
        follower.observe(&record(carrier, grant(1)));
        assert_eq!(engine.snapshot().device_sets[0].channels.len(), 2);

        follower.systems.clear();
        follower.reconcile();

        assert_eq!(
            engine.snapshot().device_sets[0].channels.len(),
            0,
            "the orphaned follower was left running"
        );
        assert!(follower.followers.is_empty());
        assert!(
            follower
                .status
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
    }

    #[test]
    fn a_system_given_a_radio_opens_its_own_control_channel() {
        let engine = engine();
        let device_set = engine
            .create_device_set("virtual:siggen")
            .expect("virtual device");
        let center_hz = engine.snapshot().device_sets[0]
            .settings
            .center_hz
            .expect("a tuned radio");
        let control_hz = center_hz as u64 + 12_500;
        let mut follower = Follower {
            engine: Arc::downgrade(&engine),
            status: Arc::new(Mutex::new(Vec::new())),
            systems: vec![TrunkSystem {
                node: "trunk".to_owned(),
                protocol: DmrTrunkProtocol::TierThree,
                discovery: DmrDiscovery::default(),
                channel_map: Vec::new(),
                learned: Vec::new(),
                radio: Some(TrunkRadio {
                    device_set,
                    stream: 0,
                    control_hz,
                    ignore_crc: false,
                }),
            }],
            carriers: HashMap::new(),
            definitions: HashMap::new(),
            followers: HashMap::new(),
            problems: HashMap::new(),
            detected: HashMap::new(),
            prospectors: HashMap::new(),
            probes: HashMap::new(),
            controls: HashMap::new(),
            watching: Vec::new(),
            tx: mpsc::channel().0,
        };

        follower.reconcile();

        let set = engine.snapshot().device_sets.remove(0);
        assert_eq!(set.channels.len(), 1, "no control receiver was opened");
        assert_eq!(set.channels[0].settings.offset_hz, 12_500.0);
        assert_eq!(status(&follower).carriers, 1);

        follower.systems.clear();
        follower.reconcile();

        assert!(
            engine.snapshot().device_sets[0].channels.is_empty(),
            "the control receiver outlived the system"
        );
    }

    #[test]
    fn moving_the_control_frequency_retunes_the_receiver_in_place() {
        let engine = engine();
        let device_set = engine
            .create_device_set("virtual:siggen")
            .expect("virtual device");
        let center_hz = engine.snapshot().device_sets[0]
            .settings
            .center_hz
            .expect("a tuned radio");
        let radio = |control_hz| {
            Some(TrunkRadio {
                device_set,
                stream: 0,
                control_hz,
                ignore_crc: false,
            })
        };
        let mut follower = Follower {
            engine: Arc::downgrade(&engine),
            status: Arc::new(Mutex::new(Vec::new())),
            systems: vec![TrunkSystem {
                node: "trunk".to_owned(),
                protocol: DmrTrunkProtocol::TierThree,
                discovery: DmrDiscovery::default(),
                channel_map: Vec::new(),
                learned: Vec::new(),
                radio: radio(center_hz as u64 + 12_500),
            }],
            carriers: HashMap::new(),
            definitions: HashMap::new(),
            followers: HashMap::new(),
            problems: HashMap::new(),
            detected: HashMap::new(),
            prospectors: HashMap::new(),
            probes: HashMap::new(),
            controls: HashMap::new(),
            watching: Vec::new(),
            tx: mpsc::channel().0,
        };
        follower.reconcile();
        let opened = engine.snapshot().device_sets[0].channels[0].id;

        follower.systems[0].radio = radio(center_hz as u64 + 25_000);
        follower.reconcile();

        let set = engine.snapshot().device_sets.remove(0);
        assert_eq!(set.channels.len(), 1, "a second control receiver appeared");
        assert_eq!(set.channels[0].id, opened, "the receiver was rebuilt");
        assert_eq!(set.channels[0].settings.offset_hz, 25_000.0);
    }

    #[test]
    fn a_data_call_is_followed_like_any_other() {
        let engine = engine();
        let (device_set, channel, center_hz) = control_channel(&engine);
        let carrier = (device_set, channel);
        let traffic_hz = center_hz as u64 + 25_000;
        let mut follower = searching_follower(
            &engine,
            DmrTrunkProtocol::TierThree,
            carrier,
            DmrDiscovery::default(),
            vec![DmrChannelEntry {
                lcn: LOGICAL_CHANNEL,
                freq_hz: traffic_hz,
            }],
        );

        follower.observe(&record(
            carrier,
            DvFrame {
                trunk_protocol: Some(DvTrunkProtocol::TierThree),
                crc_verified: Some(true),
                channel: Some(LOGICAL_CHANNEL),
                slot: Some(1),
                destination: Some(9_999),
                source: Some(9_998),
                data: Some("data call".to_owned()),
                ..DvFrame::new(DvMode::Dmr, DvFrameKind::Control)
            },
        ));

        let status = status(&follower);
        assert_eq!(status.followers.len(), 1, "a data call went unfollowed");
        assert_eq!(status.followers[0].freq_hz, traffic_hz);
    }

    #[test]
    fn a_capacity_max_channel_update_follows_every_busy_timeslot() {
        let engine = engine();
        let (device_set, channel, center_hz) = control_channel(&engine);
        let carrier = (device_set, channel);
        let traffic_hz = center_hz as u64 + 25_000;
        let mut follower = searching_follower(
            &engine,
            DmrTrunkProtocol::TierThree,
            carrier,
            DmrDiscovery::default(),
            vec![DmrChannelEntry {
                lcn: LOGICAL_CHANNEL,
                freq_hz: traffic_hz,
            }],
        );

        follower.observe(&record(
            carrier,
            DvFrame {
                trunk_protocol: Some(DvTrunkProtocol::TierThree),
                crc_verified: Some(true),
                channel: Some(LOGICAL_CHANNEL),
                slot_activity: vec![
                    sdrmm_wire::DvSlotActivity {
                        slot: 1,
                        activity: "group voice".to_owned(),
                        destination_hash: None,
                        destination: Some(9_001),
                        logical_channel: Some(LOGICAL_CHANNEL),
                    },
                    sdrmm_wire::DvSlotActivity {
                        slot: 2,
                        activity: "group voice".to_owned(),
                        destination_hash: None,
                        destination: Some(9_002),
                        logical_channel: Some(LOGICAL_CHANNEL),
                    },
                ],
                ..DvFrame::new(DvMode::Dmr, DvFrameKind::Control)
            },
        ));

        let status = status(&follower);
        assert_eq!(status.followers.len(), 2, "one timeslot went unfollowed");
        assert!(
            status
                .followers
                .iter()
                .all(|open| open.freq_hz == traffic_hz)
        );
        assert_eq!(status.followers[0].slot, 1);
        assert_eq!(status.followers[1].slot, 2);
    }

    #[test]
    fn a_channel_update_for_an_unknown_channel_starts_the_search() {
        let engine = engine();
        let (device_set, channel, center_hz) = control_channel(&engine);
        let carrier = (device_set, channel);
        let mut follower = searching_follower(
            &engine,
            DmrTrunkProtocol::TierThree,
            carrier,
            search_over(center_hz),
            Vec::new(),
        );

        follower.observe(&record(
            carrier,
            DvFrame {
                trunk_protocol: Some(DvTrunkProtocol::TierThree),
                crc_verified: Some(true),
                channel: Some(LOGICAL_CHANNEL),
                slot_activity: vec![sdrmm_wire::DvSlotActivity {
                    slot: 1,
                    activity: "group voice".to_owned(),
                    destination_hash: None,
                    destination: Some(9_001),
                    logical_channel: Some(LOGICAL_CHANNEL),
                }],
                ..DvFrame::new(DvMode::Dmr, DvFrameKind::Control)
            },
        ));
        follower.reconcile();

        assert_eq!(status(&follower).searching, 1);
        assert_eq!(status(&follower).probes.len(), 4);
    }

    #[test]
    fn a_grant_for_an_unknown_channel_opens_search_receivers() {
        let engine = engine();
        let (device_set, channel, center_hz) = control_channel(&engine);
        let carrier = (device_set, channel);
        let mut follower = searching_follower(
            &engine,
            DmrTrunkProtocol::TierThree,
            carrier,
            search_over(center_hz),
            Vec::new(),
        );
        assert!(follower.probes.is_empty(), "searched before it was asked");

        follower.observe(&record(carrier, identified_grant(1)));
        follower.reconcile();

        let status = status(&follower);
        assert_eq!(status.searching, 1);
        assert_eq!(status.probes.len(), 4, "the raster was not swept");
        assert!(
            status
                .probes
                .iter()
                .all(|probe| probe.freq_hz > center_hz as u64),
            "a search receiver landed on the control channel"
        );
        assert_eq!(engine.snapshot().device_sets[0].channels.len(), 5);
    }

    #[test]
    fn traffic_that_answers_a_grant_teaches_the_system_where_the_channel_lives() {
        let engine = engine();
        let (device_set, channel, center_hz) = control_channel(&engine);
        let carrier = (device_set, channel);
        let mut follower = searching_follower(
            &engine,
            DmrTrunkProtocol::TierThree,
            carrier,
            search_over(center_hz),
            Vec::new(),
        );
        follower.observe(&record(carrier, identified_grant(1)));
        follower.reconcile();
        let probe = status(&follower).probes[1];

        for _ in 0..2 {
            follower.observe(&record(
                (probe.device_set, probe.channel),
                traffic(91, 4001, 1),
            ));
        }

        let status = status(&follower);
        assert_eq!(status.channel_map.len(), 1);
        assert_eq!(status.channel_map[0].logical_channel, LOGICAL_CHANNEL);
        assert_eq!(status.channel_map[0].freq_hz, probe.freq_hz);
        assert_eq!(status.channel_map[0].source, TrunkChannelSource::Learned);
        assert_eq!(status.searching, 0);
        assert!(
            !status
                .probes
                .iter()
                .any(|open| open.freq_hz == probe.freq_hz),
            "the search receiver stayed open on a solved channel"
        );
        assert_eq!(status.followers.len(), 1, "the live call was not picked up");
        assert_eq!(status.followers[0].freq_hz, probe.freq_hz);
        assert_eq!(status.followers[0].logical_channel, Some(LOGICAL_CHANNEL));
    }

    #[test]
    fn a_learned_channel_is_followed_without_searching_again() {
        let engine = engine();
        let (device_set, channel, center_hz) = control_channel(&engine);
        let carrier = (device_set, channel);
        let mut follower = searching_follower(
            &engine,
            DmrTrunkProtocol::TierThree,
            carrier,
            search_over(center_hz),
            Vec::new(),
        );
        follower.observe(&record(carrier, identified_grant(1)));
        follower.reconcile();
        let probe = status(&follower).probes[1];
        for _ in 0..2 {
            follower.observe(&record(
                (probe.device_set, probe.channel),
                traffic(91, 4001, 1),
            ));
        }

        follower.observe(&record(carrier, identified_grant(2)));
        follower.reconcile();

        let status = status(&follower);
        assert_eq!(status.followers.len(), 2);
        assert!(
            status
                .followers
                .iter()
                .all(|open| open.freq_hz == probe.freq_hz)
        );
        assert!(status.probes.is_empty(), "it kept hunting a solved channel");
    }

    #[test]
    fn a_manual_channel_map_is_followed_without_any_search() {
        let engine = engine();
        let (device_set, channel, center_hz) = control_channel(&engine);
        let carrier = (device_set, channel);
        let traffic_hz = center_hz as u64 + 25_000;
        let mut follower = searching_follower(
            &engine,
            DmrTrunkProtocol::TierThree,
            carrier,
            search_over(center_hz),
            vec![DmrChannelEntry {
                lcn: LOGICAL_CHANNEL,
                freq_hz: traffic_hz,
            }],
        );

        follower.observe(&record(carrier, identified_grant(1)));
        follower.reconcile();

        let status = status(&follower);
        assert!(status.probes.is_empty(), "it searched for a known channel");
        assert_eq!(status.followers.len(), 1);
        assert_eq!(status.followers[0].freq_hz, traffic_hz);
        assert_eq!(status.channel_map[0].source, TrunkChannelSource::Manual);
    }

    #[test]
    fn an_announced_channel_never_starts_a_search() {
        let engine = engine();
        let (device_set, channel, center_hz) = control_channel(&engine);
        let carrier = (device_set, channel);
        let mut follower = searching_follower(
            &engine,
            DmrTrunkProtocol::TierThree,
            carrier,
            search_over(center_hz),
            Vec::new(),
        );

        follower.observe(&record(carrier, definition(center_hz as u64 + 12_500)));
        follower.observe(&record(carrier, identified_grant(1)));
        follower.reconcile();

        let status = status(&follower);
        assert!(status.probes.is_empty());
        assert_eq!(status.searching, 0);
        assert_eq!(status.channel_map.len(), 1);
        assert_eq!(status.channel_map[0].source, TrunkChannelSource::Announced);
    }

    #[test]
    fn dropping_the_system_closes_the_search_receivers_it_opened() {
        let engine = engine();
        let (device_set, channel, center_hz) = control_channel(&engine);
        let carrier = (device_set, channel);
        let mut follower = searching_follower(
            &engine,
            DmrTrunkProtocol::TierThree,
            carrier,
            search_over(center_hz),
            Vec::new(),
        );
        follower.observe(&record(carrier, identified_grant(1)));
        follower.reconcile();
        assert_eq!(engine.snapshot().device_sets[0].channels.len(), 5);

        follower.systems.clear();
        follower.reconcile();

        assert_eq!(
            engine.snapshot().device_sets[0].channels.len(),
            0,
            "orphaned search receivers were left running"
        );
        assert!(follower.probes.is_empty());
    }

    #[test]
    fn a_search_receiver_that_finds_a_control_channel_is_not_mistaken_for_traffic() {
        let engine = engine();
        let (device_set, channel, center_hz) = control_channel(&engine);
        let carrier = (device_set, channel);
        let mut follower = searching_follower(
            &engine,
            DmrTrunkProtocol::TierThree,
            carrier,
            search_over(center_hz),
            Vec::new(),
        );
        follower.observe(&record(carrier, identified_grant(1)));
        follower.reconcile();
        let probe = status(&follower).probes[0];

        for _ in 0..2 {
            follower.observe(&record(
                (probe.device_set, probe.channel),
                identified_grant(1),
            ));
        }
        follower.reconcile();

        let status = status(&follower);
        assert!(status.channel_map.is_empty(), "a control channel was bound");
        assert!(
            !status
                .probes
                .iter()
                .any(|open| open.freq_hz == probe.freq_hz),
            "a known control channel stayed in the search"
        );
    }
}
