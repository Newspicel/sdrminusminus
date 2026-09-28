use super::MobileCore;
use crate::{
    error::CoreError,
    pairing::{self as flow, PairInput},
    pose::now_ms,
    records::{DiscoveredServer, PairOffer, SavedServer},
};

#[uniffi::export]
impl MobileCore {
    pub fn parse_pair_link(&self, link: String) -> Result<PairOffer, CoreError> {
        flow::offer_from_link(&link)
    }

    pub fn offer_from_discovery(
        &self,
        server: DiscoveredServer,
        code: String,
    ) -> Result<PairOffer, CoreError> {
        flow::offer_from_discovery(&server, &code)
    }

    pub async fn offer_manual(
        &self,
        address: String,
        code: String,
    ) -> Result<PairOffer, CoreError> {
        let host = flow::manual_host(&address)?;
        let net = self.inner.net.clone();
        self.inner
            .runtime
            .run(flow::offer_manual(host, code, net))
            .await
    }

    pub async fn pair(
        &self,
        offer: PairOffer,
        phone_name: String,
    ) -> Result<SavedServer, CoreError> {
        let rebind = self.rebind_token(offer.fingerprint.as_deref());
        let input = PairInput {
            offer,
            phone_name,
            platform: self.inner.config.platform,
            rebind,
            net: self.inner.net.clone(),
            now_ms: now_ms(),
        };
        let record = self.inner.runtime.run(flow::pair(input)).await?;
        self.inner.vault.store(&record)?;
        Ok(record.saved())
    }
}

impl MobileCore {
    fn rebind_token(&self, pin: Option<&str>) -> Option<String> {
        let pin = pin?;
        match self.inner.vault.servers() {
            Ok(listing) => listing
                .records
                .into_iter()
                .find(|record| record.pin == pin)
                .map(|record| record.token),
            Err(error) => {
                tracing::warn!(%error, "saved servers unreadable, pairing as a new phone");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{
        about::API_PROTOCOL,
        phone::{PHONE_SECRET_BYTES, PairResponse, Phone, PhonePlatform, PhoneToken},
    };

    use super::*;
    use crate::{
        api::testing::core,
        stub_server::{StubRequest, StubResponse, StubServer},
        vault::testing,
    };

    fn reply(phone: &str) -> PairResponse {
        PairResponse {
            phone: Phone {
                id: phone.to_owned(),
                name: "iPhone".to_owned(),
                platform: PhonePlatform::Ios,
                created_at: "2026-09-28T12:00:00Z".to_owned(),
                last_seen: None,
                online: false,
                gps_nodes: Vec::new(),
            },
            token: PhoneToken::new(phone.to_owned(), [5; PHONE_SECRET_BYTES]).encode(),
            server_id: testing::SERVER_ID.to_owned(),
            server_name: "Shack".to_owned(),
            protocol: API_PROTOCOL,
        }
    }

    #[tokio::test]
    async fn pairing_stores_the_record_and_rebinds_on_a_second_pair() {
        let stub =
            StubServer::start(|_: &StubRequest| StubResponse::json(200, &reply(testing::PHONE_ID)))
                .await;
        let (vault, core) = core();
        let offer = PairOffer {
            hosts: vec![stub.host()],
            code: "12345678".to_owned(),
            fingerprint: Some(stub.pin.clone()),
            fingerprint_short: None,
            protocol: API_PROTOCOL,
            server_name: None,
        };
        let saved = core
            .pair(offer.clone(), "iPhone".to_owned())
            .await
            .expect("paired");
        assert_eq!(saved.id, testing::SERVER_ID);
        assert_eq!(saved.hosts, [stub.host()]);
        assert!(
            vault
                .raw(&format!("server/{}", testing::SERVER_ID))
                .is_some()
        );
        assert_eq!(core.saved_servers().expect("listed"), vec![saved]);
        core.pair(offer, "iPhone".to_owned())
            .await
            .expect("paired again");
        let second: serde_json::Value =
            serde_json::from_slice(&stub.requests()[1].body).expect("json");
        assert_eq!(second["rebind"], reply(testing::PHONE_ID).token);
        assert_eq!(second["platform"], "ios");
        let offered = core
            .parse_pair_link(
                sdrmm_wire::phone::PairUri {
                    hosts: vec![stub.host()],
                    code: "12345678".to_owned(),
                    pin: stub.pin.clone(),
                    protocol: API_PROTOCOL,
                    name: None,
                }
                .to_uri(),
            )
            .expect("link");
        assert_eq!(offered.fingerprint.as_deref(), Some(stub.pin.as_str()));
        core.shutdown();
    }
}
