use super::{MobileCore, not_connected};
use crate::{error::CoreError, missions::views::MissionCommand};

const FEATURE: &str = "Missions";

#[uniffi::export]
impl MobileCore {
    pub async fn refresh_missions(&self) -> Result<(), CoreError> {
        not_connected(())
    }

    pub async fn switch_workspace(&self, id: String) -> Result<(), CoreError> {
        not_connected(id)
    }

    pub fn open_mission(&self, id: String) -> Result<(), CoreError> {
        not_connected(id)
    }

    pub fn close_mission(&self) {
        self.inner.not_built(FEATURE, ());
    }

    pub async fn send(&self, command: MissionCommand) -> Result<(), CoreError> {
        not_connected(command)
    }
}
