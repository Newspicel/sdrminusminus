use super::{MobileCore, not_connected};
use crate::{
    error::CoreError,
    records::{DiscoveredServer, PairOffer, SavedServer},
};

#[uniffi::export]
impl MobileCore {
    pub fn parse_pair_link(&self, link: String) -> Result<PairOffer, CoreError> {
        not_connected(link)
    }

    pub fn offer_from_discovery(
        &self,
        server: DiscoveredServer,
        code: String,
    ) -> Result<PairOffer, CoreError> {
        not_connected((server, code))
    }

    pub async fn offer_manual(
        &self,
        address: String,
        code: String,
    ) -> Result<PairOffer, CoreError> {
        not_connected((address, code))
    }

    pub async fn pair(
        &self,
        offer: PairOffer,
        phone_name: String,
    ) -> Result<SavedServer, CoreError> {
        not_connected((offer, phone_name))
    }
}
