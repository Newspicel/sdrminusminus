use super::MobileCore;
use crate::{
    error::CoreError,
    pairing::{self as flow, PairInput},
    pose::now_ms,
    records::{DiscoveredServer, PairOffer, SavedServer},
    vault::ServerRecord,
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
        let previous = self.paired_before(offer.fingerprint.as_deref());
        let held = previous
            .as_ref()
            .and_then(|record| self.inner.take_link(&record.server_id));
        let resume = held.is_some();
        let input = PairInput {
            offer,
            phone_name,
            platform: self.inner.config.platform,
            rebind: previous.as_ref().map(|record| record.token.clone()),
            net: self.inner.net.clone(),
            now_ms: now_ms(),
        };
        let paired = self
            .inner
            .runtime
            .run(async move {
                if let Some(link) = held {
                    link.retire_and_stop().await;
                }
                flow::pair(input).await
            })
            .await;
        let record = match paired {
            Ok(record) => record,
            Err(error) => {
                if let Some(previous) = previous.filter(|_| resume) {
                    self.inner.resume_link(previous);
                }
                return Err(error);
            }
        };
        let replaced = self.retire_live(&record.server_id).await;
        let stored = self.inner.vault.store(&record);
        let restart = match (replaced, previous) {
            (true, _) => Some(record.clone()),
            (false, Some(previous)) if resume => Some(resumed(record.clone(), previous)),
            (false, _) => None,
        };
        if let Some(restart) = restart {
            self.inner.resume_link(restart);
        }
        stored?;
        Ok(record.saved())
    }
}

fn resumed(record: ServerRecord, previous: ServerRecord) -> ServerRecord {
    if previous.server_id == record.server_id {
        record
    } else {
        previous
    }
}

impl MobileCore {
    async fn retire_live(&self, server_id: &str) -> bool {
        let Some(link) = self.inner.take_link(server_id) else {
            return false;
        };
        let retired = self
            .inner
            .runtime
            .run(async move {
                link.retire_and_stop().await;
                Ok(())
            })
            .await;
        if let Err(error) = retired {
            tracing::warn!(%error, "old link not stopped");
        }
        true
    }

    fn paired_before(&self, pin: Option<&str>) -> Option<ServerRecord> {
        let pin = pin?;
        match self.inner.vault.servers() {
            Ok(listing) => listing.records.into_iter().find(|record| record.pin == pin),
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
