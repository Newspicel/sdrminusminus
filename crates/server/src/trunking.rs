use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use sdrmm_engine::Engine;
use sdrmm_wire::{
    ChannelParams, ChannelSettings, DecodedRecord, DecoderEvent, DmrParams, DmrSlots,
    DmrTrunkProtocol, DvFrame, DvFrameKind, NodeBody, ServerEvent, StateScope,
};
use tokio::{
    sync::broadcast::error::RecvError,
    task::JoinHandle,
    time::{MissedTickBehavior, interval},
};

use crate::Store;

const RECONCILE_INTERVAL: Duration = Duration::from_secs(1);
const MAX_FOLLOWERS_PER_SYSTEM: usize = 16;

#[derive(Default)]
pub(crate) struct Trunking {
    followers: Mutex<HashMap<(u32, u32), String>>,
}

impl Trunking {
    pub(crate) fn followers(&self) -> Vec<((u32, u32), String)> {
        self.followers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .map(|(source, node)| (*source, node.clone()))
            .collect()
    }

    fn insert(&self, device_set: u32, channel: u32, node: String) {
        self.followers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert((device_set, channel), node);
    }

    fn remove(&self, device_set: u32, channel: u32) {
        self.followers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&(device_set, channel));
    }
}

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
    CapacityPlus {
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
            Self::CapacityPlus { system_node, .. } | Self::TierThree { system_node, .. } => {
                system_node
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Follower {
    device_set: u32,
    channel: u32,
    system_node: String,
    freq_hz: u64,
}

#[derive(Clone, Copy)]
struct Grant {
    logical_channel: u16,
    slot: u8,
    freq_hz: u64,
}

pub(crate) fn spawn(
    engine: Arc<Engine>,
    store: Arc<Store>,
    trunking: Arc<Trunking>,
) -> Option<JoinHandle<()>> {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::warn!("no runtime in context: trunked traffic channels will not be followed");
        return None;
    };
    let _guard = handle.enter();
    Some(tokio::spawn(run(engine, store, trunking)))
}

async fn run(engine: Arc<Engine>, store: Arc<Store>, trunking: Arc<Trunking>) {
    let mut decoded = engine.subscribe_decoded();
    let mut events = engine.subscribe_events();
    let mut carriers = resolve_carriers(&engine, &store);
    let mut definitions: HashMap<(String, u16), u64> = HashMap::new();
    let mut followers: HashMap<FollowerKey, Follower> = HashMap::new();
    let mut failed: HashSet<(FollowerKey, u64)> = HashSet::new();
    provision_capacity_plus(
        &engine,
        carriers.values().flatten(),
        false,
        &mut followers,
        &mut failed,
        &trunking,
    );
    let mut tick = interval(RECONCILE_INTERVAL);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            received = decoded.recv() => match received {
                Ok(record) => observe(
                    &engine,
                    &record,
                    &carriers,
                    &mut definitions,
                    &mut followers,
                    &mut failed,
                    &trunking,
                ),
                Err(RecvError::Lagged(count)) => {
                    tracing::warn!(count, "trunk follower lost decoder records");
                }
                Err(RecvError::Closed) => break,
            },
            received = events.recv() => match received {
                Ok(ServerEvent::StateChanged { scope: StateScope::All | StateScope::DeviceSet(_) | StateScope::Workspaces }) => reconcile(
                    &engine,
                    &store,
                    &mut carriers,
                    &mut followers,
                    &mut definitions,
                    &mut failed,
                    &trunking,
                ),
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
            _ = tick.tick() => reconcile(
                &engine,
                &store,
                &mut carriers,
                &mut followers,
                &mut definitions,
                &mut failed,
                &trunking,
            ),
        }
    }
}

fn resolve_carriers(engine: &Engine, store: &Store) -> HashMap<(u32, u32), Vec<Carrier>> {
    let Ok(Some(workspace)) = store.active_workspace() else {
        return HashMap::new();
    };
    let graph = &workspace.snapshot.graph;
    let state = engine.snapshot();
    let live: HashMap<String, (u32, u32)> = crate::workspace::bind(graph, &state)
        .into_iter()
        .flat_map(|binding| {
            binding
                .channels
                .into_iter()
                .map(move |(node, channel)| (node, (binding.device_set, channel)))
        })
        .collect();
    let mut carriers: HashMap<(u32, u32), Vec<Carrier>> = HashMap::new();
    for system in &graph.nodes {
        let NodeBody::DmrTrunk(settings) = &system.body else {
            continue;
        };
        for source_node in graph.sources_of(&system.id, "events") {
            let Some(&(device_set, channel)) = live.get(source_node) else {
                continue;
            };
            let Some(set) = state.device_sets.iter().find(|set| set.id == device_set) else {
                continue;
            };
            let Some(info) = set.channels.iter().find(|info| info.id == channel) else {
                continue;
            };
            let ChannelParams::Dmr(params) = &info.settings.params else {
                continue;
            };
            let center_hz = set.settings.center_hz.unwrap_or(100_000_000.0);
            let freq_hz = (center_hz + info.settings.offset_hz).round().max(0.0) as u64;
            carriers
                .entry((device_set, channel))
                .or_default()
                .push(Carrier {
                    system_node: system.id.clone(),
                    protocol: settings.protocol,
                    device_set,
                    channel,
                    stream: info.stream,
                    center_hz,
                    freq_hz,
                    ignore_crc: params.ignore_crc,
                });
        }
    }
    carriers
}

fn observe(
    engine: &Engine,
    record: &DecodedRecord,
    carriers: &HashMap<(u32, u32), Vec<Carrier>>,
    definitions: &mut HashMap<(String, u16), u64>,
    followers: &mut HashMap<FollowerKey, Follower>,
    failed: &mut HashSet<(FollowerKey, u64)>,
    trunking: &Trunking,
) {
    let Some(bound) = carriers.get(&(record.device_set, record.channel)) else {
        return;
    };
    let DecoderEvent::Dv(frame) = &record.event else {
        return;
    };
    for carrier in bound {
        if carrier.protocol == DmrTrunkProtocol::Auto && is_capacity_plus(frame) {
            provision_capacity_plus(
                engine,
                carriers
                    .values()
                    .flatten()
                    .filter(|candidate| candidate.system_node == carrier.system_node),
                true,
                followers,
                failed,
                trunking,
            );
        }
        if !matches!(
            carrier.protocol,
            DmrTrunkProtocol::Auto | DmrTrunkProtocol::TierThree
        ) {
            continue;
        }
        if let Some(definition) = &frame.channel_definition {
            definitions.insert(
                (carrier.system_node.clone(), definition.channel),
                definition.rx_hz,
            );
        }
        if frame.kind != DvFrameKind::Control
            || (frame.source.is_none() && frame.destination.is_none())
        {
            continue;
        }
        let (Some(logical_channel), Some(slot)) = (frame.channel, frame.slot) else {
            continue;
        };
        let Some(&freq_hz) = definitions.get(&(carrier.system_node.clone(), logical_channel))
        else {
            continue;
        };
        ensure_follower(
            engine,
            carrier,
            FollowerKey::TierThree {
                system_node: carrier.system_node.clone(),
                logical_channel,
                slot,
            },
            Grant {
                logical_channel,
                slot,
                freq_hz,
            },
            followers,
            failed,
            trunking,
        );
    }
}

fn is_capacity_plus(frame: &DvFrame) -> bool {
    frame.manufacturer_id == Some(0x10)
        && frame
            .opcode
            .as_deref()
            .is_some_and(|opcode| opcode.contains("Capacity Plus"))
}

fn provision_capacity_plus<'a>(
    engine: &Engine,
    carriers: impl Iterator<Item = &'a Carrier>,
    include_auto: bool,
    followers: &mut HashMap<FollowerKey, Follower>,
    failed: &mut HashSet<(FollowerKey, u64)>,
    trunking: &Trunking,
) {
    for carrier in carriers.filter(|carrier| {
        carrier.protocol == DmrTrunkProtocol::CapacityPlus
            || (include_auto && carrier.protocol == DmrTrunkProtocol::Auto)
    }) {
        for slot in [1, 2] {
            ensure_follower(
                engine,
                carrier,
                FollowerKey::CapacityPlus {
                    system_node: carrier.system_node.clone(),
                    device_set: carrier.device_set,
                    carrier: carrier.channel,
                    slot,
                },
                Grant {
                    logical_channel: 0,
                    slot,
                    freq_hz: carrier.freq_hz,
                },
                followers,
                failed,
                trunking,
            );
        }
    }
}

fn ensure_follower(
    engine: &Engine,
    carrier: &Carrier,
    key: FollowerKey,
    grant: Grant,
    followers: &mut HashMap<FollowerKey, Follower>,
    failed: &mut HashSet<(FollowerKey, u64)>,
    trunking: &Trunking,
) {
    if followers
        .get(&key)
        .is_some_and(|follower| follower.freq_hz == grant.freq_hz)
    {
        return;
    }
    let failure = (key.clone(), grant.freq_hz);
    if failed.contains(&failure) {
        return;
    }
    if !followers.contains_key(&key)
        && followers
            .keys()
            .filter(|candidate| candidate.system_node() == key.system_node())
            .count()
            >= MAX_FOLLOWERS_PER_SYSTEM
    {
        failed.insert(failure);
        tracing::warn!(
            node = key.system_node(),
            "DMR trunk system reached its traffic channel limit"
        );
        return;
    }
    let slots = if grant.slot == 1 {
        DmrSlots::One
    } else {
        DmrSlots::Two
    };
    let settings = ChannelSettings {
        offset_hz: grant.freq_hz as f64 - carrier.center_hz,
        squelch_db: None,
        params: ChannelParams::Dmr(DmrParams {
            slots,
            ignore_crc: carrier.ignore_crc,
        }),
    };
    let result = if let Some(follower) = followers.get_mut(&key) {
        engine
            .patch_channel(carrier.device_set, follower.channel, settings)
            .map(|()| follower.channel)
    } else {
        engine.add_channel(carrier.device_set, carrier.stream, settings)
    };
    match result {
        Ok(channel) => {
            followers.insert(
                key,
                Follower {
                    device_set: carrier.device_set,
                    channel,
                    system_node: carrier.system_node.clone(),
                    freq_hz: grant.freq_hz,
                },
            );
            trunking.insert(carrier.device_set, channel, carrier.system_node.clone());
            engine.emit_scope(StateScope::DeviceSet(carrier.device_set));
        }
        Err(error) => {
            failed.insert(failure);
            tracing::warn!(
                node = carrier.system_node,
                logical_channel = grant.logical_channel,
                slot = grant.slot,
                freq_hz = grant.freq_hz,
                %error,
                "could not follow DMR traffic channel"
            );
        }
    }
}

fn reconcile(
    engine: &Engine,
    store: &Store,
    carriers: &mut HashMap<(u32, u32), Vec<Carrier>>,
    followers: &mut HashMap<FollowerKey, Follower>,
    definitions: &mut HashMap<(String, u16), u64>,
    failed: &mut HashSet<(FollowerKey, u64)>,
    trunking: &Trunking,
) {
    let fresh = resolve_carriers(engine, store);
    followers.retain(|key, follower| {
        let valid = match key {
            FollowerKey::CapacityPlus {
                system_node,
                device_set,
                carrier,
                ..
            } => fresh.get(&(*device_set, *carrier)).is_some_and(|bound| {
                bound.iter().any(|source| {
                    source.system_node == *system_node
                        && matches!(
                            source.protocol,
                            DmrTrunkProtocol::Auto | DmrTrunkProtocol::CapacityPlus
                        )
                })
            }),
            FollowerKey::TierThree { system_node, .. } => fresh.values().flatten().any(|source| {
                source.device_set == follower.device_set
                    && source.system_node == *system_node
                    && matches!(
                        source.protocol,
                        DmrTrunkProtocol::Auto | DmrTrunkProtocol::TierThree
                    )
            }),
        };
        if valid {
            return true;
        }
        if let Err(error) = engine.remove_channel(follower.device_set, follower.channel) {
            tracing::warn!(
                device_set = follower.device_set,
                channel = follower.channel,
                %error,
                "could not remove an orphaned DMR trunk follower"
            );
        }
        trunking.remove(follower.device_set, follower.channel);
        engine.emit_scope(StateScope::DeviceSet(follower.device_set));
        false
    });
    definitions.retain(|(node, _), _| {
        fresh
            .values()
            .flatten()
            .any(|carrier| carrier.system_node == *node)
    });
    failed.clear();
    provision_capacity_plus(
        engine,
        fresh.values().flatten(),
        false,
        followers,
        failed,
        trunking,
    );
    *carriers = fresh;
}

#[cfg(test)]
mod tests {
    use sdrmm_device::DeviceRegistry;
    use sdrmm_device_virtual::VirtualDriver;

    use super::*;

    #[test]
    fn follower_aliases_are_inserted_and_removed() {
        let trunking = Trunking::default();
        trunking.insert(3, 7, "system".to_owned());
        assert_eq!(trunking.followers(), vec![((3, 7), "system".to_owned())]);
        trunking.remove(3, 7);
        assert!(trunking.followers().is_empty());
    }

    #[test]
    fn grant_creates_one_slot_filtered_dmr_channel() {
        let mut registry = DeviceRegistry::new();
        registry.register(1, Box::new(VirtualDriver::new()));
        let engine = Engine::with_registry(registry, None);
        let device_set = engine
            .create_device_set("virtual:siggen")
            .expect("virtual device");
        let carrier = Carrier {
            system_node: "system".to_owned(),
            protocol: DmrTrunkProtocol::TierThree,
            device_set,
            channel: 1,
            stream: 0,
            center_hz: 100_000_000.0,
            freq_hz: 100_000_000,
            ignore_crc: false,
        };
        let trunking = Trunking::default();
        let mut followers = HashMap::new();
        let mut failed = HashSet::new();
        ensure_follower(
            &engine,
            &carrier,
            FollowerKey::TierThree {
                system_node: "system".to_owned(),
                logical_channel: 17,
                slot: 2,
            },
            Grant {
                logical_channel: 17,
                slot: 2,
                freq_hz: 100_125_000,
            },
            &mut followers,
            &mut failed,
            &trunking,
        );
        let snapshot = engine.snapshot();
        let channel = &snapshot.device_sets[0].channels[0];
        assert_eq!(channel.settings.offset_hz, 125_000.0);
        assert_eq!(
            channel.settings.params,
            ChannelParams::Dmr(DmrParams {
                slots: DmrSlots::Two,
                ignore_crc: false,
            })
        );
    }

    #[test]
    fn capacity_plus_provisions_both_slots_for_each_known_carrier() {
        let mut registry = DeviceRegistry::new();
        registry.register(1, Box::new(VirtualDriver::new()));
        let engine = Engine::with_registry(registry, None);
        let device_set = engine
            .create_device_set("virtual:siggen")
            .expect("virtual device");
        let carrier = Carrier {
            system_node: "system".to_owned(),
            protocol: DmrTrunkProtocol::CapacityPlus,
            device_set,
            channel: 9,
            stream: 0,
            center_hz: 100_000_000.0,
            freq_hz: 100_250_000,
            ignore_crc: true,
        };
        let trunking = Trunking::default();
        let mut followers = HashMap::new();
        let mut failed = HashSet::new();
        provision_capacity_plus(
            &engine,
            std::iter::once(&carrier),
            false,
            &mut followers,
            &mut failed,
            &trunking,
        );
        let snapshot = engine.snapshot();
        let channels = &snapshot.device_sets[0].channels;
        assert_eq!(channels.len(), 2);
        assert!(channels.iter().any(|channel| {
            matches!(
                channel.settings.params,
                ChannelParams::Dmr(DmrParams {
                    slots: DmrSlots::One,
                    ignore_crc: true
                })
            )
        }));
        assert!(channels.iter().any(|channel| {
            matches!(
                channel.settings.params,
                ChannelParams::Dmr(DmrParams {
                    slots: DmrSlots::Two,
                    ignore_crc: true
                })
            )
        }));
    }
}
