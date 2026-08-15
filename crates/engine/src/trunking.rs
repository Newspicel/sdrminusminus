//! Following a trunked system's traffic channels: control-channel grants become extra
//! receivers on the same radio. Which channels carry a system is a workspace question, so the
//! answer is pushed in through [`Engine::configure_trunking`] rather than read from here.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak, mpsc},
    time::{Duration, Instant},
};

use sdrmm_wire::{
    AudioProcessing, ChannelParams, ChannelSettings, DecodedRecord, DecoderEvent, DmrParams,
    DmrSlots, DmrTrunkProtocol, DvFrame, DvFrameKind, DvTrunkProtocol, StateScope, TrunkFollower,
    TrunkProblem, TrunkSystemStatus,
};

use crate::Engine;

const RECONCILE_INTERVAL: Duration = Duration::from_secs(1);

/// Every follower is a full C4FM demodulator, so this is a CPU bound as much as a protocol one.
const MAX_FOLLOWERS_PER_SYSTEM: usize = 16;

/// A grant outside the sampled bandwidth is refused every time it is tried, so failures back
/// off instead of being retried at the reconcile rate for as long as the system runs.
const RETRY_BASE: Duration = Duration::from_secs(5);
const RETRY_MAX: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrunkSystem {
    pub node: String,
    pub protocol: DmrTrunkProtocol,
    /// `(device set, channel)` of every control channel feeding this node.
    pub carriers: Vec<(u32, u32)>,
}

pub(crate) enum TrunkInput {
    Record(Box<DecodedRecord>),
    Configure(Vec<TrunkSystem>),
}

/// Resolved against live engine state rather than carried in the config: the centre frequency
/// moves under the follower's feet as the radio is retuned.
#[derive(Clone)]
struct Carrier {
    system_node: String,
    protocol: DmrTrunkProtocol,
    device_set: u32,
    channel: u32,
    stream: u32,
    center_hz: f64,
    freq_hz: u64,
    ignore_crc: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum FollowerKey {
    KnownCarrier {
        system_node: String,
        device_set: u32,
        carrier: u32,
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
    };
    let spawned = std::thread::Builder::new()
        .name("sdrmm-trunk".to_string())
        .spawn(move || follower.run(&rx));
    if let Err(e) = spawned {
        tracing::error!("failed to spawn the trunk follower: {e}");
    }
}

struct Follower {
    engine: Weak<Engine>,
    status: Arc<Mutex<Vec<TrunkSystemStatus>>>,
    systems: Vec<TrunkSystem>,
    carriers: HashMap<(u32, u32), Vec<Carrier>>,
    /// `(system node, logical channel)` → receive frequency: a Tier III grant names a channel
    /// number and nothing else.
    definitions: HashMap<(String, u16), u64>,
    followers: HashMap<FollowerKey, FollowerChannel>,
    problems: HashMap<FollowerKey, Problem>,
    detected: HashMap<String, DvTrunkProtocol>,
}

impl Follower {
    /// The inbox is the only blocking point and its timeout is the reconcile tick.
    fn run(mut self, rx: &mpsc::Receiver<TrunkInput>) {
        let mut next_reconcile = Instant::now();
        loop {
            let wait = next_reconcile.saturating_duration_since(Instant::now());
            match rx.recv_timeout(wait) {
                Ok(TrunkInput::Record(record)) => self.observe(&record),
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

    /// A radio with no centre frequency yet is left out rather than guessed at: every grant is
    /// an offset from that centre, so a guess puts the follower where nothing is transmitting.
    fn resolve_carriers(&self, engine: &Engine) -> HashMap<(u32, u32), Vec<Carrier>> {
        let state = engine.snapshot();
        let mut carriers: HashMap<(u32, u32), Vec<Carrier>> = HashMap::new();
        for system in &self.systems {
            for &(device_set, channel) in &system.carriers {
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
                carriers
                    .entry((device_set, channel))
                    .or_default()
                    .push(Carrier {
                        system_node: system.node.clone(),
                        protocol: system.protocol,
                        device_set,
                        channel,
                        stream: info.stream,
                        center_hz,
                        freq_hz: absolute.round() as u64,
                        ignore_crc: params.ignore_crc,
                    });
            }
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
        let Some(bound) = self
            .carriers
            .get(&(record.device_set, record.channel))
            .cloned()
        else {
            return;
        };
        let DecoderEvent::Dv(frame) = &record.event else {
            return;
        };
        // A frame that survived only because the channel ignores CRCs may be reported, but it
        // may not steer a receiver: noise would fabricate grants.
        if frame.crc_verified == Some(false) {
            return;
        }
        for carrier in &bound {
            if let Some(protocol) = frame.trunk_protocol {
                self.note_detection(&carrier.system_node, protocol);
            }
            match self.protocol_of(&carrier.system_node, carrier.protocol) {
                Some(DvTrunkProtocol::CapacityPlus) => {
                    self.provision_known_carrier(&engine, carrier)
                }
                Some(DvTrunkProtocol::HyteraXpt) => self.provision_known_carrier(&engine, carrier),
                Some(DvTrunkProtocol::TierThree) => {
                    self.observe_tier_three(&engine, carrier, frame)
                }
                None => {}
            }
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

    fn observe_tier_three(&mut self, engine: &Engine, carrier: &Carrier, frame: &DvFrame) {
        if let Some(definition) = &frame.channel_definition {
            self.definitions.insert(
                (carrier.system_node.clone(), definition.channel),
                definition.rx_hz,
            );
        }
        if frame.kind != DvFrameKind::Control
            || (frame.source.is_none() && frame.destination.is_none())
        {
            return;
        }
        let (Some(logical_channel), Some(slot)) = (frame.channel, frame.slot) else {
            return;
        };
        let Some(&freq_hz) = self
            .definitions
            .get(&(carrier.system_node.clone(), logical_channel))
        else {
            return;
        };
        self.ensure_follower(
            engine,
            carrier,
            FollowerKey::TierThree {
                system_node: carrier.system_node.clone(),
                logical_channel,
                slot,
            },
            Grant { slot, freq_hz },
        );
    }

    fn provision_known_carrier(&mut self, engine: &Engine, carrier: &Carrier) {
        for slot in [1, 2] {
            self.ensure_follower(
                engine,
                carrier,
                FollowerKey::KnownCarrier {
                    system_node: carrier.system_node.clone(),
                    device_set: carrier.device_set,
                    carrier: carrier.channel,
                    slot,
                },
                Grant {
                    slot,
                    freq_hz: carrier.freq_hz,
                },
            );
        }
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
            audio: AudioProcessing::default_for(params.type_id()),
            params,
        };
        // Both emit their own `DeviceSet` scope; nothing here announces it a second time.
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

    /// Returns whether this is news: the first failure is worth a warning and a state change,
    /// the twentieth is not.
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
        self.carriers = self.resolve_carriers(&engine);
        let live: Vec<Carrier> = self.carriers.values().flatten().cloned().collect();
        let known: Vec<(FollowerKey, u32, u32)> = self
            .followers
            .iter()
            .filter(|(key, _)| !self.system_holds(&live, key))
            .map(|(key, follower)| (key.clone(), follower.device_set, follower.channel))
            .collect();
        for (key, device_set, channel) in known {
            // A channel that is already gone is the ordinary case on shutdown and when a radio
            // is closed under the system; only a real refusal is worth saying out loud.
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
        self.problems
            .retain(|key, _| nodes.contains(&key.system_node()));
        for carrier in &live {
            if matches!(
                self.protocol_of(&carrier.system_node, carrier.protocol),
                Some(DvTrunkProtocol::CapacityPlus | DvTrunkProtocol::HyteraXpt)
            ) {
                self.provision_known_carrier(&engine, carrier);
            }
        }
        self.publish();
    }

    fn system_holds(&self, live: &[Carrier], key: &FollowerKey) -> bool {
        match key {
            FollowerKey::KnownCarrier {
                system_node,
                device_set,
                carrier,
                ..
            } => live.iter().any(|source| {
                source.system_node == *system_node
                    && source.device_set == *device_set
                    && source.channel == *carrier
                    && matches!(
                        self.protocol_of(system_node, source.protocol),
                        Some(DvTrunkProtocol::CapacityPlus | DvTrunkProtocol::HyteraXpt)
                    )
            }),
            FollowerKey::TierThree { system_node, .. } => live.iter().any(|source| {
                source.system_node == *system_node
                    && self.protocol_of(system_node, source.protocol)
                        == Some(DvTrunkProtocol::TierThree)
            }),
        }
    }

    /// The engine lock must not be held here: `snapshot` takes it and then this one.
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

    /// Sorted, not left in hash order: this is snapshot state a client diffs.
    fn status_of(&self, system: &TrunkSystem) -> TrunkSystemStatus {
        let mut status = TrunkSystemStatus {
            node: system.node.clone(),
            detected: self.detected.get(&system.node).copied(),
            carriers: self
                .carriers
                .values()
                .flatten()
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
        };
        status
            .followers
            .sort_by_key(|follower| (follower.freq_hz, follower.slot));
        status
            .problems
            .sort_by_key(|problem| (problem.freq_hz, problem.slot));
        status
    }
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

    /// A device set with one DMR channel on it, standing in for a control channel, and the
    /// centre frequency every grant in these tests is an offset from.
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
        let mut follower = Follower {
            engine: Arc::downgrade(engine),
            status: Arc::new(Mutex::new(Vec::new())),
            systems: vec![TrunkSystem {
                node: "trunk".to_owned(),
                protocol,
                carriers: vec![carrier],
            }],
            carriers: HashMap::new(),
            definitions: HashMap::new(),
            followers: HashMap::new(),
            problems: HashMap::new(),
            detected: HashMap::new(),
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

    /// The grant arrives before the channel definition that says where the channel is. Nothing
    /// may be opened on a guess.
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

    /// A frame kept only because the channel ignores CRCs must not steer a receiver.
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

    /// The commonest real refusal: a Tier III system whose traffic channels are outside the
    /// bandwidth the radio is sampling. It has to be visible in state, and it must not be
    /// retried at the reconcile rate for as long as the system runs.
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
            1,
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
}
