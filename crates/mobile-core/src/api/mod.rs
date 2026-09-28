use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Instant,
};

use sdrmm_wire::about::API_PROTOCOL;
use tokio::sync::{mpsc, watch};

use crate::{
    error::CoreError,
    events::{CoreEvent, EventQueue, Pop},
    link::{Activity, INBOUND_CAPACITY, LinkCmd, LinkHandle, Net, Session, Subscriptions, Wires},
    logging::{self, LogListener},
    missions::{MissionHub, MissionWires},
    notices,
    pose::{PoseHub, PoseWires},
    records::{CoreAbout, CoreConfig, LicenseEntry, LinkState, Notice},
    runtime::CoreRuntime,
    vault::{SecretVault, Vault},
};

mod guidance;
mod link;
mod missions;
mod pairing;
mod pose;
mod servers;

pub use guidance::nav_handoff_uri;

#[derive(uniffi::Object)]
pub struct MobileCore {
    inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub(crate) config: CoreConfig,
    pub(crate) runtime: CoreRuntime,
    pub(crate) events: EventQueue,
    pub(crate) vault: Vault,
    pub(crate) net: Net,
    pub(crate) wires: Wires,
    pub(crate) sessions: watch::Receiver<Option<Arc<Session>>>,
    pub(crate) activity: watch::Sender<Activity>,
    pub(crate) pose: PoseHub,
    pub(crate) missions: MissionHub,
    pub(crate) link: Mutex<Option<LinkHandle>>,
}

#[uniffi::export]
impl MobileCore {
    #[uniffi::constructor]
    pub fn new(config: CoreConfig, vault: Arc<dyn SecretVault>) -> Result<Arc<Self>, CoreError> {
        if !Path::new(&config.data_dir).is_dir() {
            return Err(CoreError::internal(format!(
                "No data dir {}",
                config.data_dir
            )));
        }
        let events = EventQueue::default();
        if !logging::install() {
            events.emit(CoreEvent::Notice {
                notice: Notice::warn("Core logs off"),
            });
        }
        let runtime = CoreRuntime::start()?;
        tracing::info!(
            core = env!("CARGO_PKG_VERSION"),
            app = %config.app_version,
            platform = ?config.platform,
            device = %config.device_model,
            "core started"
        );
        events.emit(CoreEvent::Link {
            state: LinkState::Offline,
        });
        let inner = Inner::wire(config, runtime, events, Vault::new(vault));
        Ok(Arc::new(Self {
            inner: Arc::new(inner),
        }))
    }

    pub fn about(&self) -> CoreAbout {
        CoreAbout {
            core_version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol: API_PROTOCOL,
        }
    }

    pub fn notices(&self) -> Vec<LicenseEntry> {
        notices::entries().unwrap_or_else(|error| {
            tracing::error!(%error, "license notices unreadable");
            self.inner.notice(Notice::error("Licenses unreadable"));
            Vec::new()
        })
    }

    pub fn set_log_listener(&self, listener: Option<Arc<dyn LogListener>>) {
        logging::set_listener(listener);
    }

    pub async fn next_event(&self) -> Option<CoreEvent> {
        loop {
            let until = match self.inner.events.pop(Instant::now()) {
                Pop::Event(event) => return Some(*event),
                Pop::Closed => return None,
                Pop::Wait(until) => until,
            };
            let events = self.inner.events.clone();
            let waited = self
                .inner
                .runtime
                .run(async move {
                    events.wait(until).await;
                    Ok(())
                })
                .await;
            if waited.is_err() {
                return None;
            }
        }
    }

    pub fn shutdown(&self) {
        if let Some(link) = self.inner.link().take() {
            link.send(LinkCmd::Stop);
        }
        self.inner.events.close();
        self.inner.runtime.shutdown();
    }
}

impl Inner {
    fn wire(config: CoreConfig, runtime: CoreRuntime, events: EventQueue, vault: Vault) -> Self {
        let (inbound, inbound_rx) = mpsc::channel(INBOUND_CAPACITY);
        let (sessions, sessions_rx) = watch::channel(None);
        let (subs, subs_rx) = watch::channel(Subscriptions::new());
        let (pose_out, pose_rx) = watch::channel(None);
        let (snapshot, snapshot_rx) = watch::channel(None);
        let (needed, needed_rx) = watch::channel(false);
        let (activity, activity_rx) = watch::channel(Activity::default());
        let net = Net::new(config.platform);
        let pose = crate::pose::start(
            &runtime,
            PoseWires {
                events: events.clone(),
                out: pose_out,
                snapshot,
                needed: needed_rx,
                sessions: sessions_rx.clone(),
                activity: activity.clone(),
            },
        );
        let missions = crate::missions::start(
            &runtime,
            MissionWires {
                events: events.clone(),
                inbound: inbound_rx,
                pose: snapshot_rx,
                subs,
                needed,
                activity: activity.clone(),
            },
        );
        let wires = Wires {
            events: events.clone(),
            inbound,
            sessions,
            subs: subs_rx,
            pose: pose_rx,
            activity: activity_rx,
            net: net.clone(),
        };
        Self {
            config,
            runtime,
            events,
            vault,
            net,
            wires,
            sessions: sessions_rx,
            activity,
            pose,
            missions,
            link: Mutex::new(None),
        }
    }

    pub(crate) fn notice(&self, notice: Notice) {
        self.events.emit(CoreEvent::Notice { notice });
    }

    pub(crate) fn session(&self) -> Option<Arc<Session>> {
        self.sessions.borrow().clone()
    }

    pub(crate) fn link(&self) -> MutexGuard<'_, Option<LinkHandle>> {
        self.link.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn start_link(&self, record: crate::vault::ServerRecord) -> LinkHandle {
        crate::link::start(
            self.runtime.handle(),
            record,
            crate::link::socket::TlsDialer::new(self.wires.clone()),
            self.wires.clone(),
            Some(self.vault.clone()),
        )
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::Arc;

    use super::MobileCore;
    use crate::{
        records::{CoreConfig, Platform},
        vault::testing::MemoryVault,
    };

    pub(crate) fn config() -> CoreConfig {
        CoreConfig {
            app_version: "1.0".to_owned(),
            platform: Platform::Ios,
            device_model: "iPhone17,3".to_owned(),
            data_dir: std::env::temp_dir().display().to_string(),
        }
    }

    pub(crate) fn core() -> (Arc<MemoryVault>, Arc<MobileCore>) {
        let vault = Arc::new(MemoryVault::default());
        let core = MobileCore::new(config(), vault.clone()).expect("core starts");
        (vault, core)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{testing::*, *};
    use crate::{
        missions::views::{HuntView, Trend},
        records::NoticeLevel,
        vault::testing::MemoryVault,
    };

    const PATIENCE: Duration = Duration::from_secs(5);

    async fn next(core: &MobileCore) -> Option<CoreEvent> {
        tokio::time::timeout(PATIENCE, core.next_event())
            .await
            .expect("an event or the end")
    }

    fn hunt(readings: u64) -> CoreEvent {
        CoreEvent::Hunt {
            view: HuntView {
                mission: "h".to_owned(),
                freq_hz: 433.92e6,
                level_db: None,
                smooth_db: None,
                floor_db: None,
                best_db: None,
                strength: 0.0,
                trend: Trend::Waiting,
                running: false,
                refusal: None,
                readings,
                sweep: None,
            },
        }
    }

    #[tokio::test]
    async fn a_new_core_starts_offline_and_names_its_protocol() {
        let (_, core) = core();
        assert_eq!(
            next(&core).await,
            Some(CoreEvent::Link {
                state: LinkState::Offline
            })
        );
        assert_eq!(
            core.about(),
            CoreAbout {
                core_version: env!("CARGO_PKG_VERSION").to_owned(),
                protocol: API_PROTOCOL
            }
        );
        assert!(core.notices().iter().any(|entry| entry.name == "uniffi"));
    }

    #[tokio::test]
    async fn next_event_ends_after_shutdown() {
        let (_, core) = core();
        assert!(next(&core).await.is_some());
        let waiting = tokio::spawn({
            let core = core.clone();
            async move { core.next_event().await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        core.shutdown();
        let ended = tokio::time::timeout(PATIENCE, waiting)
            .await
            .expect("woken")
            .expect("joined");
        assert_eq!(ended, None);
        assert_eq!(next(&core).await, None);
        core.inner.events.emit(hunt(1));
        assert_eq!(next(&core).await, None);
    }

    #[tokio::test]
    async fn events_are_coalesced_before_delivery() {
        let (_, core) = core();
        assert!(next(&core).await.is_some());
        for readings in 1..=5 {
            core.inner.events.emit(hunt(readings));
        }
        assert_eq!(next(&core).await, Some(hunt(5)));
        core.inner.events.emit(hunt(6));
        core.inner.events.emit(hunt(7));
        let started = Instant::now();
        assert_eq!(next(&core).await, Some(hunt(7)));
        assert!(started.elapsed() >= Duration::from_millis(50));
    }

    #[tokio::test]
    async fn a_cancelled_wait_loses_no_event() {
        let (_, core) = core();
        assert!(next(&core).await.is_some());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), core.next_event())
                .await
                .is_err()
        );
        core.inner.events.emit(hunt(9));
        assert_eq!(next(&core).await, Some(hunt(9)));
    }

    #[tokio::test]
    async fn a_corrupt_vault_item_is_visible() {
        let (vault, core) = core();
        assert!(next(&core).await.is_some());
        let record = crate::vault::testing::record();
        vault.put(
            &record.key(),
            Ok(serde_json::to_vec(&record).expect("json")),
        );
        vault.put("server/0a", Err(crate::error::VaultError::Corrupt));
        let saved = core.saved_servers().expect("listed");
        assert_eq!(saved, vec![record.saved()]);
        assert_eq!(
            next(&core).await,
            Some(CoreEvent::Notice {
                notice: Notice {
                    level: NoticeLevel::Error,
                    text: "Saved server unreadable. Pair again".to_owned()
                }
            })
        );
        core.forget_server(record.server_id.clone())
            .expect("forgotten");
        assert!(core.saved_servers().expect("listed").is_empty());
    }

    #[tokio::test]
    async fn commands_while_offline_fail_with_not_connected() {
        let (_, core) = core();
        assert!(next(&core).await.is_some());
        assert!(matches!(
            core.connect("00".to_owned()).await,
            Err(CoreError::Internal { .. })
        ));
        assert!(matches!(
            core.parse_pair_link("sdrmm://pair".to_owned()),
            Err(CoreError::InvalidLink { .. })
        ));
        assert_eq!(core.refresh_missions().await, Err(CoreError::NotConnected));
        assert_eq!(
            core.switch_workspace("3".to_owned()).await,
            Err(CoreError::NotConnected)
        );
        assert_eq!(
            core.open_mission("hunt1".to_owned()),
            Err(CoreError::NotConnected)
        );
        assert_eq!(
            core.send(crate::missions::views::MissionCommand::Calibrate)
                .await,
            Err(CoreError::NoMission)
        );
        core.set_pose_settings(crate::records::PoseSettings {
            heading_mode: crate::records::HeadingMode::Auto,
            mount: crate::records::Mount::Flat,
            mount_offset_deg: 0.0,
            share_pose: true,
        });
        core.cancel_align();
        core.close_mission();
        core.network_changed();
        core.set_foreground(false);
        core.set_local_network_allowed(true);
        core.disconnect();
        let mut offline = false;
        while let Some(event) = next(&core).await {
            if event
                == (CoreEvent::Link {
                    state: LinkState::Offline,
                })
            {
                offline = true;
                break;
            }
            assert!(matches!(event, CoreEvent::Pose { .. }), "{event:?}");
        }
        assert!(offline);
        core.shutdown();
    }

    #[test]
    fn a_missing_data_dir_is_refused() {
        let mut config = config();
        config.data_dir = "/nonexistent/sdrmm".to_owned();
        let refused = MobileCore::new(config, Arc::new(MemoryVault::default()));
        assert!(matches!(
            refused.err(),
            Some(CoreError::Internal { message }) if message == "No data dir /nonexistent/sdrmm"
        ));
    }
}
