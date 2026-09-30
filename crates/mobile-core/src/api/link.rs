use sdrmm_wire::phone;

use super::MobileCore;
use crate::{
    error::CoreError,
    events::CoreEvent,
    link::{LinkCmd, merged},
    records::LinkState,
    vault::MAX_SAVED_HOSTS,
};

#[uniffi::export]
impl MobileCore {
    pub fn update_hosts(&self, server_id: String, hosts: Vec<String>) -> Result<(), CoreError> {
        if let Some(bad) = hosts.iter().find(|host| !phone::valid_pair_host(host)) {
            return Err(CoreError::InvalidLink {
                reason: format!("bad host {bad}"),
            });
        }
        let mut record = self.inner.vault.load(&server_id)?;
        let hosts_now = merged(&record.hosts, &hosts, MAX_SAVED_HOSTS);
        if hosts_now != record.hosts {
            record.hosts = hosts_now;
            self.inner.vault.store(&record)?;
        }
        if let Some(link) = self
            .inner
            .link()
            .as_ref()
            .filter(|link| link.server_id() == server_id)
        {
            link.send(LinkCmd::AddHosts(hosts));
        }
        Ok(())
    }

    pub async fn connect(&self, server_id: String) -> Result<(), CoreError> {
        let record = self.inner.vault.load(&server_id)?;
        let inner = self.inner.clone();
        self.inner
            .runtime
            .run(async move {
                let old = inner.link().take();
                if let Some(old) = old {
                    old.stop().await;
                }
                let handle = inner.start_link(record);
                let replaced = inner.link().replace(handle);
                if let Some(replaced) = replaced {
                    replaced.send(LinkCmd::Stop);
                }
                Ok(())
            })
            .await
    }

    pub fn disconnect(&self) {
        let link = self.inner.link().take();
        match link {
            Some(link) => link.send(LinkCmd::Stop),
            None => self.inner.events.emit(CoreEvent::Link {
                state: LinkState::Offline,
            }),
        }
    }

    pub fn set_foreground(&self, foreground: bool) {
        self.inner.activity.send_if_modified(|activity| {
            let changed = activity.background == foreground;
            activity.background = !foreground;
            changed
        });
    }

    pub fn network_changed(&self) {
        if let Some(link) = self.inner.link().as_ref() {
            link.send(LinkCmd::NetworkChanged);
        }
    }

    pub fn set_local_network_allowed(&self, allowed: bool) {
        self.inner.net.allow_local(allowed);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex, PoisonError},
        time::Duration,
    };

    use futures::{StreamExt, future::BoxFuture};
    use sdrmm_wire::{
        about::{API_PROTOCOL, AboutResponse},
        mission::{
            ChannelTarget, HuntMission, Mission, MissionActionResponse, MissionBody,
            MissionControl, MissionWorkspace, MissionsResponse, PositionLink,
        },
        phone::{Phone, PhonePlatform, PhoneSelf},
        ws::ServerEvent,
    };
    use tokio_tungstenite::tungstenite::Message;

    use crate::{
        api::{MobileCore, testing::core},
        error::CoreError,
        events::CoreEvent,
        missions::views::MissionCommand,
        records::{
            HeadingMode, LinkState, LocationSample, MagAccuracy, MotionFrame, MotionSample, Mount,
            PoseSettings,
        },
        stub_server::{StubHandler, StubRequest, StubResponse, StubServer, StubSocket},
        vault::{ServerRecord, testing},
    };

    #[derive(Default)]
    struct Shack {
        texts: Arc<Mutex<Vec<String>>>,
    }

    fn hunt_mission(controls: &[MissionControl]) -> Mission {
        Mission {
            node: "hunt1".to_owned(),
            label: "Fox".to_owned(),
            ready: true,
            problems: Vec::new(),
            controls: controls.to_vec(),
            body: MissionBody::Hunt(HuntMission {
                target: Some(ChannelTarget {
                    device_set: 1,
                    channel: 3,
                    channel_node: "ch1".to_owned(),
                    channel_type: "nfm".to_owned(),
                    frequency_hz: 145.5e6,
                    bandwidth_hz: 12_500.0,
                }),
                status: None,
                clicks: true,
                position: Some(PositionLink {
                    node: "gps1".to_owned(),
                    phone: Some(testing::PHONE_ID.to_owned()),
                }),
                triangulations: Vec::new(),
            }),
        }
    }

    impl StubHandler for Shack {
        fn http(&self, request: &StubRequest) -> StubResponse {
            match (request.method.as_str(), request.path()) {
                ("GET", "/api/about") => StubResponse::json(
                    200,
                    &AboutResponse {
                        name: "SDR--".to_owned(),
                        version: "0.9.0".to_owned(),
                        protocol: API_PROTOCOL,
                        server_id: testing::SERVER_ID.to_owned(),
                        server_name: "Shack".to_owned(),
                        license: String::new(),
                        license_text: String::new(),
                        repository: String::new(),
                        components: Vec::new(),
                        reveal: false,
                    },
                ),
                ("GET", "/api/missions") => StubResponse::json(
                    200,
                    &MissionsResponse {
                        revision: 1,
                        workspace: Some(MissionWorkspace {
                            id: 1,
                            name: "Field".to_owned(),
                        }),
                        workspaces: Vec::new(),
                        missions: vec![hunt_mission(&[MissionControl::StartHunt])],
                        truncated: 0,
                    },
                ),
                ("GET", "/api/phones/self") => StubResponse::json(
                    200,
                    &PhoneSelf {
                        phone: Phone {
                            id: testing::PHONE_ID.to_owned(),
                            name: "iPhone".to_owned(),
                            platform: PhonePlatform::Ios,
                            created_at: "2026-09-28T12:00:00Z".to_owned(),
                            last_seen: None,
                            online: true,
                            gps_nodes: vec!["gps1".to_owned()],
                        },
                        server_id: testing::SERVER_ID.to_owned(),
                        server_name: "Shack".to_owned(),
                    },
                ),
                ("POST", "/api/missions/hunt1/actions") => StubResponse::json(
                    200,
                    &MissionActionResponse {
                        mission: hunt_mission(&[MissionControl::StopHunt]),
                    },
                ),
                _ => StubResponse::error(404, "no route"),
            }
        }

        fn socket(&self, _request: StubRequest, mut socket: StubSocket) -> BoxFuture<'static, ()> {
            let texts = self.texts.clone();
            Box::pin(async move {
                let hello = ServerEvent::Hello {
                    revision: 1,
                    protocol: API_PROTOCOL,
                    phone: Some(testing::PHONE_ID.to_owned()),
                };
                if let Ok(text) = serde_json::to_string(&hello) {
                    let _ = futures::SinkExt::send(&mut socket, Message::text(text)).await;
                }
                while let Some(Ok(message)) = socket.next().await {
                    if let Message::Text(text) = message {
                        texts
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push(text.to_string());
                    }
                }
            })
        }
    }

    async fn wait_for(core: &MobileCore, wanted: impl Fn(&CoreEvent) -> bool) -> CoreEvent {
        loop {
            let event = tokio::time::timeout(Duration::from_secs(10), core.next_event())
                .await
                .expect("in time")
                .expect("an event");
            if wanted(&event) {
                return event;
            }
        }
    }

    #[tokio::test]
    async fn a_saved_server_goes_online_lists_missions_and_takes_poses() {
        let shack = Shack::default();
        let texts = shack.texts.clone();
        let stub = StubServer::start(shack).await;
        let (vault, core) = core();
        let record = ServerRecord {
            hosts: vec![stub.host()],
            pin: stub.pin.clone(),
            ..testing::record()
        };
        vault.put(
            &record.key(),
            Ok(serde_json::to_vec(&record).expect("json")),
        );
        core.set_pose_settings(PoseSettings {
            heading_mode: HeadingMode::Compass,
            mount: Mount::Flat,
            mount_offset_deg: 0.0,
            share_pose: true,
        });
        core.connect(testing::SERVER_ID.to_owned())
            .await
            .expect("started");
        wait_for(&core, |event| {
            matches!(event, CoreEvent::Link { state: LinkState::Online { server } } if server == "Shack")
        })
        .await;
        wait_for(
            &core,
            |event| matches!(event, CoreEvent::Missions { view } if view.missions.len() == 1),
        )
        .await;
        core.open_mission("hunt1".to_owned()).expect("opened");
        wait_for(
            &core,
            |event| matches!(event, CoreEvent::Hunt { view } if !view.running),
        )
        .await;
        core.send(MissionCommand::StartHunt).await.expect("sent");
        wait_for(
            &core,
            |event| matches!(event, CoreEvent::Hunt { view } if view.running),
        )
        .await;
        assert!(stub.requests().iter().any(|request| {
            request.path() == "/api/missions/hunt1/actions"
                && request.body == br#"{"action":"start_hunt"}"#
        }));
        assert_eq!(
            core.send(MissionCommand::Calibrate).await,
            Err(CoreError::Refused {
                message: "Not a control of this mission".to_owned()
            })
        );
        let now = crate::pose::now_ms();
        core.push_location(LocationSample {
            t_unix_ms: now,
            lat: 52.52,
            lon: 13.405,
            alt_m: None,
            h_acc_m: 4.0,
            v_acc_m: None,
            speed_mps: Some(0.0),
            speed_acc_mps: None,
            course_deg: None,
            course_acc_deg: None,
        });
        let half = -(90.0f64 + 30.0).to_radians() / 2.0;
        core.push_motion(MotionSample {
            t_unix_ms: now + 5,
            frame: MotionFrame::TrueNorth,
            qw: half.cos(),
            qx: 0.0,
            qy: 0.0,
            qz: half.sin(),
            rot_x: 0.0,
            rot_y: 0.0,
            rot_z: 0.0,
            grav_x: 0.0,
            grav_y: 0.0,
            grav_z: -1.0,
            heading_deg: None,
            mag_accuracy: MagAccuracy::High,
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut heading = None;
        while heading.is_none() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
            heading = texts
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .filter_map(|text| serde_json::from_str::<serde_json::Value>(text).ok())
                .filter(|value| value["type"] == "PublishPose")
                .find_map(|value| value["data"]["fix"]["heading_deg"].as_f64());
        }
        assert!(
            heading.is_some_and(|deg| (deg - 30.0).abs() < 0.5),
            "{heading:?}"
        );
        core.disconnect();
        wait_for(&core, |event| {
            matches!(
                event,
                CoreEvent::Link {
                    state: LinkState::Offline
                }
            )
        })
        .await;
        core.connect(testing::SERVER_ID.to_owned())
            .await
            .expect("started again");
        wait_for(&core, |event| {
            matches!(
                event,
                CoreEvent::Link {
                    state: LinkState::Online { .. }
                }
            )
        })
        .await;
        core.forget_server(testing::SERVER_ID.to_owned())
            .expect("forgotten");
        wait_for(&core, |event| {
            matches!(
                event,
                CoreEvent::Link {
                    state: LinkState::Offline
                }
            )
        })
        .await;
        wait_for(&core, |event| {
            matches!(event, CoreEvent::Notice { notice } if notice.text == "Not removed on server")
        })
        .await;
        assert!(
            stub.requests()
                .iter()
                .any(|request| request.method == "DELETE" && request.path() == "/api/phones/self")
        );
        assert!(core.saved_servers().expect("listed").is_empty());
        core.shutdown();
    }

    #[test]
    fn update_hosts_adds_resolved_addresses_and_keeps_the_pin() {
        let (vault, core) = core();
        let record = testing::record();
        vault.put(
            &record.key(),
            Ok(serde_json::to_vec(&record).expect("json")),
        );
        core.update_hosts(
            testing::SERVER_ID.to_owned(),
            vec!["10.0.0.9:8443".to_owned(), "192.168.1.20:8443".to_owned()],
        )
        .expect("merged");
        let saved = core.saved_servers().expect("listed");
        assert_eq!(
            saved[0].hosts,
            ["192.168.1.20:8443", "10.0.0.9:8443", "[fe80::1]:8443"]
        );
        let raw = vault.raw(&record.key()).expect("stored");
        let stored: serde_json::Value = serde_json::from_slice(&raw).expect("json");
        assert_eq!(stored["pin"], testing::PIN);
        assert_eq!(stored["token"], testing::token());
        let full: Vec<String> = (1..=8).map(|last| format!("10.0.1.{last}:8443")).collect();
        vault.put(
            &record.key(),
            Ok(serde_json::to_vec(&ServerRecord {
                hosts: full.clone(),
                ..record.clone()
            })
            .expect("json")),
        );
        core.update_hosts(
            testing::SERVER_ID.to_owned(),
            vec!["10.0.2.1:8443".to_owned()],
        )
        .expect("merged");
        let hosts = &core.saved_servers().expect("listed")[0].hosts;
        assert_eq!(hosts.len(), 8);
        assert_eq!(hosts[..2], [full[0].clone(), "10.0.2.1:8443".to_owned()]);
        assert!(!hosts.contains(&full[7]));
        assert_eq!(
            core.update_hosts(testing::SERVER_ID.to_owned(), vec!["pi.local".to_owned()]),
            Err(CoreError::InvalidLink {
                reason: "bad host pi.local".to_owned()
            })
        );
        assert!(core.update_hosts("0a".to_owned(), Vec::new()).is_err());
        core.shutdown();
    }
}
