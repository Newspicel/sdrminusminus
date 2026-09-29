use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use futures::StreamExt;
use reqwest::{
    Client, StatusCode,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use sdrmm_engine::Engine;
use sdrmm_mobile_core::{
    CoreConfig, CoreError, CoreEvent, LinkState, MobileCore, PairOffer, Platform, SavedServer,
    SecretVault, VaultError,
};
use sdrmm_server::{Config, ServerHandle, ServerOptions, tls::Tls};
use sdrmm_wire::{
    ChannelNode, CreateWorkspaceRequest, CreatedRowId, DeviceNode, DeviceRef, GpsNode, HuntNode,
    NodeBody, PairUri, PairingOffer, PatchEdge, PatchGraph, PatchNode, PhoneAccess,
    PhoneAccessStatus, PhonesResponse, PortRef, Position, PositionSource, RackLayout, ServerEvent,
    WorkspaceSnapshot,
};
use tempfile::TempDir;
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, handshake::client::generate_key, protocol::Role},
};

pub const ADMIN_TOKEN: &str = "admin";
pub const PHONE_NAME: &str = "Test phone";
pub const PATIENCE: Duration = Duration::from_secs(15);
pub const HUNT: &str = "hunt";
pub const GPS: &str = "car";

pub struct TestServer {
    pub base: String,
    pub phone_port: u16,
    pub admin: Client,
    engine: Arc<Engine>,
    _handle: ServerHandle,
    _dir: TempDir,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.engine.shutdown();
    }
}

pub async fn spawn_server() -> TestServer {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = sdrmm_device::DeviceRegistry::new();
    registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
    let engine = Engine::with_registry(registry, None);
    let handle = sdrmm_server::serve(
        Config {
            bind: "127.0.0.1:0".parse().expect("bind"),
            db_path: Some(dir.path().join("sdrmm.db")),
            tls: Some(Tls::SelfSigned {
                dir: dir.path().to_path_buf(),
                names: vec!["127.0.0.1".to_owned()],
            }),
            options: ServerOptions {
                token: Some(ADMIN_TOKEN.to_owned()),
                ..ServerOptions::default()
            },
        },
        engine.clone(),
    )
    .await
    .expect("server starts");
    let mut server = TestServer {
        base: format!("https://{}", handle.local_addr),
        phone_port: 0,
        admin: admin_client(),
        engine,
        _handle: handle,
        _dir: dir,
    };
    server.phone_port = server.allow_phones().await;
    server
}

fn admin_client() -> Client {
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {ADMIN_TOKEN}")).expect("header"),
    );
    Client::builder()
        .danger_accept_invalid_certs(true)
        .tls_info(true)
        .http1_only()
        .default_headers(headers)
        .build()
        .expect("admin client")
}

fn free_port() -> u16 {
    std::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, 0))
        .expect("bind")
        .local_addr()
        .expect("address")
        .port()
}

impl TestServer {
    async fn allow_phones(&self) -> u16 {
        let port = free_port();
        let response = self
            .admin
            .put(format!("{}/api/phones/access", self.base))
            .json(&PhoneAccess {
                enabled: true,
                port,
            })
            .send()
            .await
            .expect("phone access");
        assert_eq!(response.status(), StatusCode::OK);
        let status: PhoneAccessStatus = response.json().await.expect("access status");
        let endpoint = status.endpoint.expect("the phone listener is the endpoint");
        assert_eq!((endpoint.port, endpoint.dedicated), (port, true));
        port
    }

    pub fn phone_host(&self) -> String {
        format!("127.0.0.1:{}", self.phone_port)
    }

    pub async fn offer(&self) -> PairingOffer {
        let response = self
            .admin
            .post(format!("{}/api/phones/offers", self.base))
            .json(&serde_json::json!({}))
            .send()
            .await
            .expect("offer");
        assert_eq!(response.status(), StatusCode::CREATED);
        response.json().await.expect("offer body")
    }

    pub fn local_link(&self, offer: &PairingOffer) -> String {
        let mut link = PairUri::parse(&offer.uri).expect("the offer carries a pairing link");
        assert_eq!(link.pin, offer.endpoint.pin);
        link.hosts = vec![self.phone_host()];
        link.to_uri()
    }

    pub async fn presented_pin(&self) -> String {
        let response = self
            .admin
            .get(format!("https://{}/api/about", self.phone_host()))
            .send()
            .await
            .expect("about");
        let certificate = response
            .extensions()
            .get::<reqwest::tls::TlsInfo>()
            .and_then(reqwest::tls::TlsInfo::peer_certificate)
            .expect("peer certificate")
            .to_vec();
        sdrmm_wire::phone::spki_pin(&certificate).expect("pin")
    }

    pub async fn phones(&self) -> PhonesResponse {
        self.admin
            .get(format!("{}/api/phones", self.base))
            .send()
            .await
            .expect("phones")
            .json()
            .await
            .expect("phones body")
    }

    pub async fn revoke(&self, phone: &str) {
        let response = self
            .admin
            .delete(format!("{}/api/phones/{phone}", self.base))
            .send()
            .await
            .expect("revoke");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    pub async fn create_workspace(&self, name: &str, graph: PatchGraph) -> i64 {
        let response = self
            .admin
            .post(format!("{}/api/workspaces", self.base))
            .json(&CreateWorkspaceRequest {
                name: name.to_owned(),
                snapshot: Some(WorkspaceSnapshot::new(graph, RackLayout::default())),
            })
            .send()
            .await
            .expect("create workspace");
        assert_eq!(response.status(), StatusCode::OK);
        let created: CreatedRowId = response.json().await.expect("workspace id");
        created.id
    }

    pub async fn activate(&self, workspace: i64) {
        for step in ["activate", "apply"] {
            let response = self
                .admin
                .post(format!("{}/api/workspaces/{workspace}/{step}", self.base))
                .send()
                .await
                .expect(step);
            assert!(
                response.status().is_success(),
                "{step}: {} {}",
                response.status(),
                response.text().await.unwrap_or_default()
            );
        }
    }

    pub async fn act(&self, node: &str, action: serde_json::Value) -> (StatusCode, String) {
        let response = self
            .admin
            .post(format!("{}/api/missions/{node}/actions", self.base))
            .json(&action)
            .send()
            .await
            .expect("mission action");
        let status = response.status();
        (status, response.text().await.expect("action body"))
    }

    pub async fn admin_ws(&self) -> AdminSocket {
        let response = self
            .admin
            .get(format!("{}/api/ws", self.base))
            .header("connection", "upgrade")
            .header("upgrade", "websocket")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", generate_key())
            .send()
            .await
            .expect("upgrade request");
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        let upgraded = response.upgrade().await.expect("upgraded");
        AdminSocket {
            socket: WebSocketStream::from_raw_socket(upgraded, Role::Client, None).await,
        }
    }
}

pub struct AdminSocket {
    socket: WebSocketStream<reqwest::Upgraded>,
}

impl AdminSocket {
    pub async fn wait_for<T>(
        &mut self,
        what: &str,
        mut pick: impl FnMut(ServerEvent) -> Option<T>,
    ) -> T {
        let found = tokio::time::timeout(PATIENCE, async {
            while let Some(frame) = self.socket.next().await {
                let Message::Text(text) = frame.expect("admin socket frame") else {
                    continue;
                };
                let Ok(event) = serde_json::from_str::<ServerEvent>(&text) else {
                    continue;
                };
                if let Some(found) = pick(event) {
                    return Some(found);
                }
            }
            None
        })
        .await;
        match found {
            Ok(Some(found)) => found,
            Ok(None) => panic!("the admin socket closed before {what}"),
            Err(_) => panic!("no {what} on the admin socket within {PATIENCE:?}"),
        }
    }
}

#[derive(Default)]
pub struct MemoryVault {
    items: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemoryVault {
    fn items(&self) -> MutexGuard<'_, HashMap<String, Vec<u8>>> {
        self.items.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn record(&self, server: &str) -> serde_json::Value {
        let items = self.items();
        let bytes = items
            .get(&format!("server/{server}"))
            .expect("a stored server record");
        serde_json::from_slice(bytes).expect("record json")
    }

    pub fn replace_record(&self, server: &str, record: &serde_json::Value) {
        self.items().insert(
            format!("server/{server}"),
            serde_json::to_vec(record).expect("record json"),
        );
    }
}

impl SecretVault for MemoryVault {
    fn load(&self, key: String) -> Result<Option<Vec<u8>>, VaultError> {
        Ok(self.items().get(&key).cloned())
    }

    fn store(&self, key: String, value: Vec<u8>) -> Result<(), VaultError> {
        self.items().insert(key, value);
        Ok(())
    }

    fn delete(&self, key: String) -> Result<(), VaultError> {
        self.items().remove(&key);
        Ok(())
    }

    fn keys(&self) -> Result<Vec<String>, VaultError> {
        Ok(self.items().keys().cloned().collect())
    }
}

pub struct TestPhone {
    pub core: Arc<MobileCore>,
    pub vault: Arc<MemoryVault>,
    _dir: TempDir,
}

impl Drop for TestPhone {
    fn drop(&mut self) {
        self.core.shutdown();
    }
}

impl TestPhone {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = Arc::new(MemoryVault::default());
        let core = MobileCore::new(
            CoreConfig {
                app_version: "1.0".to_owned(),
                platform: Platform::Ios,
                device_model: "iPhone18,1".to_owned(),
                data_dir: dir.path().display().to_string(),
            },
            vault.clone(),
        )
        .expect("core starts");
        Self {
            core,
            vault,
            _dir: dir,
        }
    }

    pub async fn offer(&self, server: &TestServer) -> PairOffer {
        let offer = server.offer().await;
        self.core
            .parse_pair_link(server.local_link(&offer))
            .expect("the link parses")
    }

    pub async fn pair(&self, server: &TestServer) -> Result<SavedServer, CoreError> {
        let offer = self.offer(server).await;
        self.core.pair(offer, PHONE_NAME.to_owned()).await
    }

    pub async fn go_live(&self, server: &TestServer) -> SavedServer {
        let saved = self.pair(server).await.expect("paired");
        self.core.connect(saved.id.clone()).await.expect("connect");
        self.wait_for("online", |event| {
            matches!(
                event,
                CoreEvent::Link {
                    state: LinkState::Online { .. }
                }
            )
            .then_some(())
        })
        .await;
        saved
    }

    pub async fn wait_for<T>(&self, what: &str, pick: impl FnMut(CoreEvent) -> Option<T>) -> T {
        self.wait_within(PATIENCE, what, pick)
            .await
            .unwrap_or_else(|seen| panic!("no {what} within {PATIENCE:?}, saw {seen:?}"))
    }

    pub async fn wait_within<T>(
        &self,
        patience: Duration,
        what: &str,
        mut pick: impl FnMut(CoreEvent) -> Option<T>,
    ) -> Result<T, Vec<CoreEvent>> {
        let mut seen = Vec::new();
        let found = tokio::time::timeout(patience, async {
            while let Some(event) = self.core.next_event().await {
                if let Some(found) = pick(event.clone()) {
                    return Some(found);
                }
                seen.push(event);
            }
            None
        })
        .await;
        match found {
            Ok(Some(found)) => Ok(found),
            Ok(None) => panic!("the core shut down before {what}"),
            Err(_) => Err(seen),
        }
    }
}

fn node(id: &str, body: NodeBody) -> PatchNode {
    PatchNode {
        id: id.to_owned(),
        body,
        position: Position { x: 0.0, y: 0.0 },
        size: None,
        label: None,
    }
}

fn wire(from: (&str, &str), to: (&str, &str)) -> PatchEdge {
    let port = |(node, port): (&str, &str)| PortRef {
        node: node.to_owned(),
        port: port.to_owned(),
    };
    PatchEdge {
        from: port(from),
        to: port(to),
    }
}

fn phone_gps(phone: &str) -> PatchNode {
    node(
        GPS,
        NodeBody::Gps(GpsNode {
            source: Some(PositionSource::Phone {
                phone: phone.to_owned(),
            }),
        }),
    )
}

pub fn gps_graph(phone: &str) -> PatchGraph {
    PatchGraph {
        nodes: vec![phone_gps(phone)],
        edges: Vec::new(),
    }
}

pub fn hunt_graph(phone: Option<&str>) -> PatchGraph {
    let mut graph = PatchGraph {
        nodes: vec![
            node(
                "radio",
                NodeBody::Device(DeviceNode {
                    device: Some(DeviceRef {
                        backend: "virtual".to_owned(),
                        serial: None,
                        key: Some("band".to_owned()),
                    }),
                    locked_streams: Vec::new(),
                }),
            ),
            node(
                "voice",
                NodeBody::Channel(ChannelNode {
                    channel_type: "nfm".to_owned(),
                    record_calls: false,
                    tuning_locked: false,
                }),
            ),
            node(HUNT, NodeBody::Hunt(HuntNode::default())),
        ],
        edges: vec![
            wire(("radio", "iq"), ("voice", "iq")),
            wire((HUNT, "control"), ("voice", "control")),
        ],
    };
    if let Some(phone) = phone {
        graph.nodes.push(phone_gps(phone));
        graph
            .edges
            .push(wire((GPS, "position"), (HUNT, "position")));
    }
    graph
}
