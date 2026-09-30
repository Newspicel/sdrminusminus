use super::MobileCore;
use crate::{
    error::CoreError,
    events::{CoreEvent, EventQueue},
    link::{Session, rest::RestError},
    records::{Notice, SavedServer},
    vault::ServerRecord,
};

#[uniffi::export]
impl MobileCore {
    pub fn saved_servers(&self) -> Result<Vec<SavedServer>, CoreError> {
        let listing = self.inner.vault.servers()?;
        if self.inner.newly_unreadable(&listing.unreadable) {
            self.inner.notice(unreadable(listing.unreadable.len()));
        }
        Ok(listing.records.iter().map(ServerRecord::saved).collect())
    }

    pub fn forget_server(&self, id: String) -> Result<(), CoreError> {
        if let Some(live) = self.inner.take_link(&id) {
            live.retire();
            let phone = self.inner.vault.load(&id).map(|record| record.phone_id);
            let session = self
                .inner
                .session()
                .filter(|session| phone.as_ref().is_ok_and(|phone| *phone == session.phone_id));
            let events = self.inner.events.clone();
            self.inner.runtime.spawn(async move {
                live.stop().await;
                if let Some(session) = session {
                    unpair(&session, &events).await;
                }
            });
        }
        self.inner.vault.delete(&id)
    }
}

async fn unpair(session: &Session, events: &EventQueue) {
    match session.api.unpair().await {
        Ok(()) | Err(RestError::Status { status: 401, .. }) => {}
        Err(error) => {
            tracing::warn!(%error, "phone not removed on the server");
            events.emit(CoreEvent::Notice {
                notice: Notice::warn("Not removed on server"),
            });
        }
    }
}

fn unreadable(count: usize) -> Notice {
    match count {
        1 => Notice::error("Saved server unreadable. Pair again"),
        count => Notice::error(format!("{count} saved servers unreadable. Pair again")),
    }
}
