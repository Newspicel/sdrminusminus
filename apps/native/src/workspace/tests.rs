use std::sync::Arc;

use sdrmm_engine::Engine;
use sdrmm_wire::patch::{NodeBody, PatchNode, Position, RackCell, RackSlot};

use super::*;

struct Server {
    api: Api,
    engine: Arc<Engine>,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    async fn new() -> Self {
        let mut registry = sdrmm_device::DeviceRegistry::new();
        registry.register(0, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
        let engine = Engine::with_registry(registry, None);
        let router = sdrmm_server::router(
            engine.clone(),
            sdrmm_server::Store::open(None).expect("in-memory store"),
            &sdrmm_server::ServerOptions::default(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let task =
            tokio::spawn(async move { axum::serve(listener, router).await.expect("server") });
        Self {
            api: Api::new(format!("http://{address}")).expect("client"),
            engine,
            task,
        }
    }

    async fn workspace(&self) -> WorkspaceDetail {
        let id = self
            .api
            .create_workspace("Native test")
            .await
            .expect("create workspace");
        let detail = self.api.workspace(id).await.expect("workspace");
        let mut snapshot = WorkspaceSnapshot::empty();
        snapshot.graph.nodes.push(node("scope"));
        snapshot.settings.band_region = Some("iaru-1".into());
        snapshot.settings.band_ruler = false;
        snapshot.rack.slots.push(RackSlot {
            node: "scope".into(),
            cell: RackCell {
                x: 0,
                y: 0,
                w: 4,
                h: 3,
            },
        });
        self.api
            .save_workspace(id, detail.info.revision, snapshot)
            .await
            .expect("seed");
        self.api.workspace(id).await.expect("seeded workspace")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
        self.engine.shutdown();
    }
}

fn node(id: &str) -> PatchNode {
    PatchNode {
        id: id.into(),
        body: NodeBody::Scope,
        position: Position { x: 0.0, y: 0.0 },
        size: None,
        label: None,
    }
}

#[tokio::test]
async fn queued_edits_preserve_settings_and_rack_and_undo_in_order() {
    let server = Server::new().await;
    let detail = server.workspace().await;
    let (session, worker) = Session::new(server.api.clone(), detail.clone());
    let task = tokio::spawn(worker.run());
    let mut graph = detail.snapshot.graph.clone();
    graph.nodes[0].position.x = 123.0;
    let first = session.save(graph.clone());
    graph.nodes.push(node("scope2"));
    let second = session.save(graph.clone());
    let undo = session.step(true);
    let redo = session.step(false);
    let (first, second, undo, redo) = tokio::join!(first, second, undo, redo);
    let first = first.expect("first save");
    let second = second.expect("second save");
    let undo = undo.expect("undo");
    let redo = redo.expect("redo");
    assert!(second.info.revision > first.info.revision);
    assert_eq!(second.snapshot.graph, graph);
    assert_eq!(undo.snapshot, first.snapshot);
    assert_eq!(redo.snapshot, second.snapshot);
    assert_eq!(redo.snapshot.settings, detail.snapshot.settings);
    assert_eq!(redo.snapshot.rack, detail.snapshot.rack);
    graph.nodes.retain(|node| node.id != "scope");
    let removed = session.save(graph).await.expect("remove racked node");
    assert!(removed.snapshot.rack.slots.is_empty());
    assert_eq!(removed.snapshot.settings, detail.snapshot.settings);
    drop(session);
    task.await.expect("writer shutdown");
}

#[tokio::test]
async fn an_external_edit_is_reported_without_overwriting_it() {
    let server = Server::new().await;
    let detail = server.workspace().await;
    let (session, worker) = Session::new(server.api.clone(), detail.clone());
    let task = tokio::spawn(worker.run());
    let mut external = detail.snapshot.clone();
    external.settings.band_ruler = true;
    server
        .api
        .save_workspace(detail.info.id, detail.info.revision, external.clone())
        .await
        .expect("external edit");
    let error = session
        .save(detail.snapshot.graph)
        .await
        .expect_err("revision conflict");
    assert!(error.to_string().contains("409"));
    let current = server
        .api
        .workspace(detail.info.id)
        .await
        .expect("current workspace");
    assert_eq!(current.snapshot, external);
    drop(session);
    task.await.expect("writer shutdown");
}

#[tokio::test]
async fn decoder_controls_change_the_virtual_radio_through_the_native_client() {
    let server = Server::new().await;
    let detail = server.workspace().await;
    let id = detail.info.id;
    let graph = crate::starter::graph(&server.api.devices().await.expect("virtual devices"));
    let (session, worker) = Session::new(server.api.clone(), detail);
    let task = tokio::spawn(worker.run());
    session.save(graph).await.expect("save starter patch");
    server.api.activate_workspace(id).await.expect("activate");
    let report = server.api.apply_workspace(id).await.expect("apply");
    assert!(report.refused.is_empty(), "{:?}", report.refused);
    let state = server.api.state().await.expect("state");
    let set = state.device_sets.first().expect("device set");
    assert_eq!(set.device.driver, "virtual");
    let channel = set.channels.first().expect("channel");
    let descriptors = server.api.channel_types().await.expect("descriptors");
    let limits = &descriptors
        .iter()
        .find(|descriptor| descriptor.type_id == "nfm")
        .expect("NFM")
        .limits;
    let fields = crate::params::fields("nfm").expect("controls");
    let bandwidth = fields
        .iter()
        .find(|field| field.name == "bandwidth_hz")
        .expect("bandwidth control");
    let mut settings = channel.settings.clone();
    settings.params = crate::params::edited(
        &settings.params,
        bandwidth,
        serde_json::json!(25000.0),
        limits,
    )
    .expect("edit bandwidth");
    let first = session.channel(set.id, channel.id, settings.clone());
    let compander = fields
        .iter()
        .find(|field| field.name == "compander")
        .expect("compander control");
    settings.params =
        crate::params::edited(&settings.params, compander, serde_json::json!(true), limits)
            .expect("edit compander");
    let second = session.channel(set.id, channel.id, settings.clone());
    first.await.expect("first edit");
    second.await.expect("second edit");
    let tone = fields
        .iter()
        .find(|field| field.name == "tone_mode")
        .expect("tone control");
    settings.params =
        crate::params::edited(&settings.params, tone, serde_json::json!("ctcss"), limits)
            .expect("select tone");
    assert!(crate::params::visible(&settings.params, "ctcss_hz"));
    assert!(!crate::params::visible(&settings.params, "dcs_code"));
    session
        .channel(set.id, channel.id, settings.clone())
        .await
        .expect("enable tone squelch");
    let current = server.api.state().await.expect("updated state");
    assert_eq!(current.device_sets[0].channels[0].settings, settings);
    drop(session);
    task.await.expect("writer shutdown");
}
