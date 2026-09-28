use super::{MobileCore, not_connected};
use crate::{error::CoreError, events::CoreEvent, records::LinkState};

const FEATURE: &str = "Link";

#[uniffi::export]
impl MobileCore {
    pub fn update_hosts(&self, server_id: String, hosts: Vec<String>) -> Result<(), CoreError> {
        not_connected((server_id, hosts))
    }

    pub async fn connect(&self, server_id: String) -> Result<(), CoreError> {
        not_connected(server_id)
    }

    pub fn disconnect(&self) {
        self.inner.events.emit(CoreEvent::Link {
            state: LinkState::Offline,
        });
    }

    pub fn set_foreground(&self, foreground: bool) {
        self.inner.not_built(FEATURE, foreground);
    }

    pub fn network_changed(&self) {
        self.inner.not_built(FEATURE, ());
    }

    pub fn set_local_network_allowed(&self, allowed: bool) {
        self.inner.not_built(FEATURE, allowed);
    }
}
