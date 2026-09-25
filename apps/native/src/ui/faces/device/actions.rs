use sdrmm_wire::{
    CreateDeviceSetRequest, CreatedId, device::DeviceInfo, patch::NodeBody, state::StateSnapshot,
};
use zgui::prelude::*;

use crate::store::Store;

pub fn edit_body(store: Store, node: String, edit: impl FnOnce(&mut NodeBody) + 'static) {
    store.edit_graph(move |graph| {
        if let Some(found) = graph.nodes.iter_mut().find(|found| found.id == node) {
            edit(&mut found.body);
        }
    });
}

pub async fn create_set(store: Store, device_id: String) -> anyhow::Result<u32> {
    let created: CreatedId = store
        .api()
        .post("/api/devicesets", &CreateDeviceSetRequest { device_id })
        .await?;
    Ok(created.id)
}

pub async fn opened_device(store: Store, set: u32) -> anyhow::Result<Option<DeviceInfo>> {
    let state: StateSnapshot = store.api().get("/api/state").await?;
    Ok(state
        .device_sets
        .into_iter()
        .find(|found| found.id == set)
        .map(|found| found.device))
}

#[derive(Clone, Copy)]
pub struct Busy {
    pub busy: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
}

impl Busy {
    pub fn new() -> Self {
        Self {
            busy: RwSignal::new(false),
            error: RwSignal::new(None),
        }
    }

    pub fn run(self, store: Store, work: impl Future<Output = anyhow::Result<()>> + 'static) {
        self.busy.set(true);
        self.error.set(None);
        zgui::task::spawn_local(async move {
            let outcome = work.await;
            self.busy.try_set(false);
            if let Err(error) = outcome {
                let said = format!("{error:#}");
                self.error.try_set(Some(said.clone()));
                store.say(said);
            }
            store.refresh_state();
        });
    }
}

pub fn release(
    store: Store,
    node: String,
    busy: Busy,
    unbind: impl FnOnce(&mut NodeBody) + 'static,
) {
    let set = store.device_set_of(&node);
    busy.run(store, async move {
        if let Some(set) = set {
            store
                .api()
                .delete(&format!("/api/devicesets/{set}"))
                .await?;
        }
        edit_body(store, node, unbind);
        Ok(())
    });
}
