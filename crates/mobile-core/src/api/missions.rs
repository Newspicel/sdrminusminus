use super::MobileCore;
use crate::{
    error::CoreError,
    missions::{
        listing::{Route, route},
        reducer::Input,
        views::MissionCommand,
    },
};

#[uniffi::export]
impl MobileCore {
    pub async fn refresh_missions(&self) -> Result<(), CoreError> {
        let session = self.inner.session().ok_or(CoreError::NotConnected)?;
        let hub = self.inner.missions.clone();
        self.inner
            .runtime
            .run(async move {
                match session.api.missions().await {
                    Ok(response) => {
                        hub.apply(Input::Listing(Box::new(response))).await;
                        Ok(())
                    }
                    Err(error) => {
                        session.check(&error);
                        Err(error.into_core(&session.host))
                    }
                }
            })
            .await
    }

    pub async fn switch_workspace(&self, id: String) -> Result<(), CoreError> {
        let workspace = id.parse::<i64>().map_err(|_| CoreError::Refused {
            message: "Bad workspace".to_owned(),
        })?;
        let session = self.inner.session().ok_or(CoreError::NotConnected)?;
        let hub = self.inner.missions.clone();
        self.inner
            .runtime
            .run(async move {
                match session.api.switch_workspace(workspace).await {
                    Ok(response) => {
                        hub.apply(Input::Listing(Box::new(response))).await;
                        Ok(())
                    }
                    Err(error) => {
                        session.check(&error);
                        Err(error.into_core(&session.host))
                    }
                }
            })
            .await
    }

    pub fn open_mission(&self, id: String) -> Result<(), CoreError> {
        if self.inner.session().is_none() {
            return Err(CoreError::NotConnected);
        }
        if self.inner.missions.shared().find(&id).is_none() {
            return Err(CoreError::NoMission);
        }
        self.inner.missions.open(Some(id));
        Ok(())
    }

    pub fn close_mission(&self) {
        self.inner.missions.open(None);
    }

    pub async fn send(&self, command: MissionCommand) -> Result<(), CoreError> {
        let shared = self.inner.missions.shared();
        let entry = shared
            .open
            .as_deref()
            .and_then(|id| shared.find(id))
            .ok_or(CoreError::NoMission)?;
        let (node, action) = match route(entry, command)? {
            Route::TargetMode(mode) => {
                self.inner.missions.send(Input::TargetMode(mode));
                return Ok(());
            }
            Route::Server { node, action } => (node, action),
        };
        let session = self.inner.session().ok_or(CoreError::NotConnected)?;
        let hub = self.inner.missions.clone();
        self.inner
            .runtime
            .run(async move {
                match session.api.act(node, action).await {
                    Ok(response) => {
                        hub.apply(Input::Acted(Box::new(response.mission))).await;
                        Ok(())
                    }
                    Err(error) => {
                        session.check(&error);
                        Err(error.into_core(&session.host))
                    }
                }
            })
            .await
    }
}
