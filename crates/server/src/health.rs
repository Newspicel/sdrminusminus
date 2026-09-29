use std::{
    sync::{
        Arc, Weak,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use axum::body::Bytes;
use sdrmm_engine::Engine;
use sdrmm_wire::{
    DeviceSet, MAX_HEALTH_LABEL_CHARS, MAX_HEALTH_RADIOS, RadioHealth, SiteHealth, StateSnapshot,
};
use tokio::{
    sync::watch,
    time::{MissedTickBehavior, interval},
};

use crate::AppState;

const REFRESH: Duration = Duration::from_secs(15);

pub(crate) type SiteFeed = watch::Receiver<Option<Bytes>>;

pub(crate) fn idle() -> SiteFeed {
    watch::channel(None).1
}

pub(crate) fn site(snapshot: &StateSnapshot, clients: u32, started_at: u64) -> SiteHealth {
    SiteHealth {
        version: env!("CARGO_PKG_VERSION").to_string(),
        platform: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        started_at,
        clients,
        radios: snapshot
            .device_sets
            .iter()
            .take(MAX_HEALTH_RADIOS)
            .map(radio)
            .collect(),
    }
}

fn radio(set: &DeviceSet) -> RadioHealth {
    RadioHealth {
        label: set
            .device
            .label
            .chars()
            .take(MAX_HEALTH_LABEL_CHARS)
            .collect(),
        driver: set.device.driver.clone(),
        status: set.status,
        fault: set.fault,
        channels: u32::try_from(set.channels.len()).unwrap_or(u32::MAX),
        overruns: set.overruns,
        recording: set.recording.is_some(),
    }
}

pub(crate) struct Reporter {
    engine: Weak<Engine>,
    clients: Arc<AtomicU32>,
    started_at: u64,
    feed: watch::Sender<Option<Bytes>>,
}

impl Reporter {
    pub(crate) fn new(state: &AppState) -> Self {
        Self {
            engine: Arc::downgrade(&state.engine),
            clients: state.clients.clone(),
            started_at: unix_ms(),
            feed: watch::Sender::new(None),
        }
    }

    pub(crate) fn subscribe(&self) -> SiteFeed {
        self.feed.subscribe()
    }

    pub(crate) fn refresh(&self) -> bool {
        let Some(engine) = self.engine.upgrade() else {
            return false;
        };
        let site = site(
            &engine.snapshot(),
            self.clients.load(Ordering::Relaxed),
            self.started_at,
        );
        publish(&self.feed, &site);
        true
    }

    pub(crate) async fn run(self) {
        let mut ticker = interval(REFRESH);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if !self.refresh() {
                return;
            }
        }
    }
}

fn publish(feed: &watch::Sender<Option<Bytes>>, site: &SiteHealth) {
    let json = match serde_json::to_vec(site) {
        Ok(json) => json,
        Err(error) => {
            tracing::warn!(%error, "site health does not serialize");
            return;
        }
    };
    feed.send_if_modified(|current| {
        if current.as_deref() == Some(json.as_slice()) {
            return false;
        }
        *current = Some(Bytes::from(json));
        true
    });
}

fn unix_ms() -> u64 {
    u64::try_from(jiff::Timestamp::now().as_millisecond()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{
        Agc, Capabilities, ChannelInfo, ChannelSettings, Coherence, DcArtifact, DeviceFault,
        DeviceInfo, DeviceSetStatus, DeviceSettings, Duplex, RecordingStatus, StreamScope,
    };
    use serde_json::json;

    use super::*;

    fn capabilities() -> Capabilities {
        Capabilities {
            freq_ranges: Vec::new(),
            sample_rates: vec![2_400_000.0],
            sample_rate_ranges: Vec::new(),
            gains: Vec::new(),
            antennas: Vec::new(),
            bandwidths: Vec::new(),
            bandwidth_ranges: Vec::new(),
            bandwidth_auto: false,
            bias_tee: false,
            agc: Agc::None,
            extra: Vec::new(),
            ppm: false,
            duplex: Duplex::RxOnly,
            rx_streams: 1,
            tx_streams: 0,
            per_stream: StreamScope::default(),
            directional: None,
            dc_artifact: DcArtifact::Operator,
            hardware_sweep: false,
            coherence: Coherence::None,
            noise_source: false,
        }
    }

    fn channel(id: u32) -> ChannelInfo {
        ChannelInfo {
            id,
            stream: 0,
            node: Some("secret-node".to_owned()),
            settings: ChannelSettings::default_for("dmr").expect("dmr"),
            out_of_band: false,
            audio_recordings: Vec::new(),
            baseband_recording: None,
            network_export: None,
        }
    }

    fn radio_set(label: &str, driver: &str, status: DeviceSetStatus) -> DeviceSet {
        DeviceSet {
            id: 1,
            device: DeviceInfo {
                driver: driver.to_owned(),
                key: format!("{driver}:00000001"),
                label: label.to_owned(),
                serial: Some("00000001".to_owned()),
                profile: None,
            },
            capabilities: capabilities(),
            settings: DeviceSettings::default(),
            status,
            channels: Vec::new(),
            overruns: 0,
            clipping: Vec::new(),
            agc_gains: Vec::new(),
            error: None,
            fault: None,
            refused: None,
            recording: None,
            network_export: None,
            time_machine: None,
            scanners: Vec::new(),
            hunts: Vec::new(),
            playback: None,
            extra_lane: None,
        }
    }

    fn recording() -> RecordingStatus {
        RecordingStatus {
            file: "/home/op/recordings/secret.sigmf-data".to_owned(),
            stream: 0,
            started_at: "2026-09-29T10:00:00Z".to_owned(),
            samples: 1,
            bytes: 2,
            overruns: 0,
            error: None,
        }
    }

    fn snapshot(device_sets: Vec<DeviceSet>) -> StateSnapshot {
        StateSnapshot {
            device_sets,
            trunk_systems: Vec::new(),
            revision: 7,
        }
    }

    #[test]
    fn the_site_matches_the_contract_and_keeps_details_home() {
        let mut rtl = radio_set("RTL-SDR Blog V4", "rtlsdr", DeviceSetStatus::Running);
        rtl.channels = vec![channel(1), channel(2)];
        rtl.recording = Some(recording());
        let mut hackrf = radio_set("HackRF One", "hackrf", DeviceSetStatus::Error);
        hackrf.fault = Some(DeviceFault::Unplugged);
        hackrf.error = Some("/dev/bus/usb/001/004 vanished".to_owned());
        hackrf.overruns = 3;
        let site = site(&snapshot(vec![rtl, hackrf]), 1, 1_790_000_000_000);
        assert_eq!(
            serde_json::to_value(&site).expect("json"),
            json!({
                "version": env!("CARGO_PKG_VERSION"),
                "platform": format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
                "started_at": 1_790_000_000_000u64,
                "clients": 1,
                "radios": [
                    {"label": "RTL-SDR Blog V4", "driver": "rtlsdr", "status": "running",
                     "channels": 2, "overruns": 0, "recording": true},
                    {"label": "HackRF One", "driver": "hackrf", "status": "error",
                     "fault": "unplugged", "channels": 0, "overruns": 3, "recording": false}
                ]
            })
        );
    }

    #[test]
    fn labels_are_cut_and_radios_capped() {
        let sets = (0..MAX_HEALTH_RADIOS + 4)
            .map(|_| radio_set(&"é".repeat(80), "rtlsdr", DeviceSetStatus::Idle))
            .collect();
        let site = site(&snapshot(sets), 0, 0);
        assert_eq!(site.radios.len(), MAX_HEALTH_RADIOS);
        assert!(
            site.radios
                .iter()
                .all(|radio| radio.label == "é".repeat(MAX_HEALTH_LABEL_CHARS))
        );
        assert!(site.radios.iter().all(|radio| radio.fault.is_none()));
    }

    #[test]
    fn an_unchanged_site_does_not_wake_the_tunnel() {
        let feed = watch::Sender::new(None);
        let mut tunnel = feed.subscribe();
        let quiet = site(&snapshot(Vec::new()), 0, 5);
        publish(&feed, &quiet);
        assert!(tunnel.has_changed().expect("feed"));
        let sent = tunnel.borrow_and_update().clone().expect("site");
        assert_eq!(
            serde_json::from_slice::<SiteHealth>(&sent).expect("json"),
            quiet
        );
        publish(&feed, &quiet);
        assert!(!tunnel.has_changed().expect("feed"));
        publish(&feed, &site(&snapshot(Vec::new()), 1, 5));
        assert!(tunnel.has_changed().expect("feed"));
    }
}
