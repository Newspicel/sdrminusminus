use std::{
    path::Path,
    sync::{Arc, Mutex, PoisonError},
    time::Instant,
};

use sdrmm_wire::about::API_PROTOCOL;

use crate::{
    error::CoreError,
    events::{CoreEvent, EventQueue, Pop},
    logging::{self, LogListener},
    notices,
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
    #[expect(dead_code)]
    pub(crate) config: CoreConfig,
    pub(crate) runtime: CoreRuntime,
    pub(crate) events: EventQueue,
    pub(crate) vault: Vault,
    unbuilt: Mutex<Vec<&'static str>>,
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
        Ok(Arc::new(Self {
            inner: Arc::new(Inner {
                config,
                runtime,
                events,
                vault: Vault::new(vault),
                unbuilt: Mutex::default(),
            }),
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
        self.inner.events.close();
        self.inner.runtime.shutdown();
    }
}

impl Inner {
    pub(crate) fn notice(&self, notice: Notice) {
        self.events.emit(CoreEvent::Notice { notice });
    }

    pub(crate) fn not_built(&self, feature: &'static str, _input: impl Sized) {
        let first = {
            let mut seen = self.unbuilt.lock().unwrap_or_else(PoisonError::into_inner);
            let first = !seen.contains(&feature);
            if first {
                seen.push(feature);
            }
            first
        };
        if first {
            self.notice(Notice::error(format!("{feature} not built yet")));
        }
    }
}

pub(crate) fn not_connected<T>(_input: impl Sized) -> Result<T, CoreError> {
    Err(CoreError::NotConnected)
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
    async fn unbuilt_parts_fail_visibly_once() {
        let (_, core) = core();
        assert!(next(&core).await.is_some());
        assert_eq!(
            core.connect("00".to_owned()).await,
            Err(CoreError::NotConnected)
        );
        assert_eq!(
            core.parse_pair_link("sdrmm://pair".to_owned()),
            Err(CoreError::NotConnected)
        );
        core.set_pose_settings(crate::records::PoseSettings {
            heading_mode: crate::records::HeadingMode::Auto,
            mount: crate::records::Mount::Flat,
            mount_offset_deg: 0.0,
            share_pose: true,
        });
        core.cancel_align();
        core.close_mission();
        assert_eq!(
            next(&core).await,
            Some(CoreEvent::Notice {
                notice: Notice::error("Pose not built yet")
            })
        );
        assert_eq!(
            next(&core).await,
            Some(CoreEvent::Notice {
                notice: Notice::error("Missions not built yet")
            })
        );
        core.disconnect();
        assert_eq!(
            next(&core).await,
            Some(CoreEvent::Link {
                state: LinkState::Offline
            })
        );
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
