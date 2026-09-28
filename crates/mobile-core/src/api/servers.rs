use super::MobileCore;
use crate::{error::CoreError, records::Notice, records::SavedServer, vault::ServerRecord};

#[uniffi::export]
impl MobileCore {
    pub fn saved_servers(&self) -> Result<Vec<SavedServer>, CoreError> {
        let listing = self.inner.vault.servers()?;
        match listing.unreadable.len() {
            0 => {}
            1 => self
                .inner
                .notice(Notice::error("Saved server unreadable. Pair again")),
            count => self.inner.notice(Notice::error(format!(
                "{count} saved servers unreadable. Pair again"
            ))),
        }
        Ok(listing.records.iter().map(ServerRecord::saved).collect())
    }

    pub fn forget_server(&self, id: String) -> Result<(), CoreError> {
        let live = {
            let mut link = self.inner.link();
            match link.as_ref() {
                Some(handle) if handle.server_id() == id => link.take(),
                _ => None,
            }
        };
        if let Some(live) = live {
            live.forget();
        }
        self.inner.vault.delete(&id)
    }
}
