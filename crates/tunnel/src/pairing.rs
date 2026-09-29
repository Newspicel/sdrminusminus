use std::time::Duration;

use base64::Engine as _;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use crate::identity::{DeviceKey, IdentityError};

pub const MAX_NAME_CHARS: usize = 64;
const FALLBACK_NAME: &str = "SDR--";
const MIN_INTERVAL: Duration = Duration::from_secs(1);
const SLOW_DOWN: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const DEVICE_ID_LEN: usize = 26;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartRequest {
    pub public_key: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Started {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub interval: u64,
    pub expires_in: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollRequest {
    pub device_code: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PollResponse {
    Pending,
    Approved {
        device_id: String,
        relay_url: String,
    },
    Expired,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paired {
    pub device_id: String,
    pub relay_url: url::Url,
}

#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    #[error("app address: {0}")]
    Address(String),
    #[error("could not reach the app: {0}")]
    Unreachable(String),
    #[error("the app refused: {0}")]
    Refused(String),
    #[error("the code expired")]
    Expired,
    #[error("the app sent an unusable answer: {0}")]
    Answer(String),
    #[error(transparent)]
    Identity(#[from] IdentityError),
}

enum PollFailure {
    SlowDown,
    Transient(String),
    Fatal(PairingError),
}

#[derive(Deserialize)]
struct Refusal {
    error: String,
}

pub fn device_name(raw: &str) -> String {
    let name: String = raw
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME_CHARS)
        .collect();
    let name = name.trim();
    if name.is_empty() {
        FALLBACK_NAME.to_string()
    } else {
        name.to_string()
    }
}

pub fn public_key_text(key: &DeviceKey) -> Result<String, IdentityError> {
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.public_key()?))
}

#[derive(Debug)]
pub struct Pairing {
    client: reqwest::Client,
    poll: url::Url,
    started: Started,
    deadline: Instant,
}

impl Pairing {
    pub async fn start(app: &url::Url, key: &DeviceKey, name: &str) -> Result<Self, PairingError> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| PairingError::Unreachable(error.to_string()))?;
        let request = StartRequest {
            public_key: public_key_text(key)?,
            name: device_name(name),
        };
        let response = client
            .post(endpoint(app, "api/pair/start")?)
            .json(&request)
            .send()
            .await
            .map_err(|error| PairingError::Unreachable(error.to_string()))?;
        if !response.status().is_success() {
            return Err(refused(response).await);
        }
        let started: Started = response
            .json()
            .await
            .map_err(|error| PairingError::Answer(error.to_string()))?;
        let deadline = Instant::now() + Duration::from_secs(started.expires_in);
        Ok(Self {
            client,
            poll: endpoint(app, "api/pair/poll")?,
            started,
            deadline,
        })
    }

    pub fn started(&self) -> &Started {
        &self.started
    }

    pub async fn wait(self) -> Result<Paired, PairingError> {
        let mut interval = Duration::from_secs(self.started.interval).max(MIN_INTERVAL);
        let request = PollRequest {
            device_code: self.started.device_code.clone(),
        };
        loop {
            tokio::time::sleep(interval).await;
            if Instant::now() >= self.deadline {
                return Err(PairingError::Expired);
            }
            match self.poll_once(&request).await {
                Ok(PollResponse::Pending) => {}
                Ok(PollResponse::Approved {
                    device_id,
                    relay_url,
                }) => return paired(device_id, &relay_url),
                Ok(PollResponse::Expired) => return Err(PairingError::Expired),
                Err(PollFailure::SlowDown) => interval += SLOW_DOWN,
                Err(PollFailure::Transient(error)) => {
                    tracing::debug!(%error, "pairing poll failed; trying again");
                }
                Err(PollFailure::Fatal(error)) => return Err(error),
            }
        }
    }

    async fn poll_once(&self, request: &PollRequest) -> Result<PollResponse, PollFailure> {
        let response = self
            .client
            .post(self.poll.clone())
            .json(request)
            .send()
            .await
            .map_err(|error| PollFailure::Transient(error.to_string()))?;
        match response.status() {
            StatusCode::TOO_MANY_REQUESTS => Err(PollFailure::SlowDown),
            StatusCode::GONE => Ok(PollResponse::Expired),
            status if status.is_success() => response
                .json()
                .await
                .map_err(|error| PollFailure::Fatal(PairingError::Answer(error.to_string()))),
            status if status.is_server_error() => {
                Err(PollFailure::Transient(format!("HTTP {status}")))
            }
            _ => Err(PollFailure::Fatal(refused(response).await)),
        }
    }
}

fn endpoint(app: &url::Url, path: &str) -> Result<url::Url, PairingError> {
    app.join(path)
        .map_err(|error| PairingError::Address(format!("{app}: {error}")))
}

async fn refused(response: reqwest::Response) -> PairingError {
    let status = response.status();
    let message = match response.json::<Refusal>().await {
        Ok(refusal) => refusal.error,
        Err(_) => format!("HTTP {status}"),
    };
    PairingError::Refused(message)
}

fn paired(device_id: String, relay_url: &str) -> Result<Paired, PairingError> {
    let well_formed = device_id.len() == DEVICE_ID_LEN
        && device_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase());
    if !well_formed {
        return Err(PairingError::Answer(format!("device id {device_id:?}")));
    }
    let relay_url: url::Url = relay_url
        .parse()
        .map_err(|error| PairingError::Answer(format!("relay address {relay_url}: {error}")))?;
    if !matches!(relay_url.scheme(), "ws" | "wss") {
        return Err(PairingError::Answer(format!(
            "relay address {relay_url} is not a WebSocket address"
        )));
    }
    Ok(Paired {
        device_id,
        relay_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_cut_and_never_empty() {
        assert_eq!(device_name("  shack pi \n"), "shack pi");
        assert_eq!(device_name("a\u{7}b"), "ab");
        assert_eq!(device_name("   "), FALLBACK_NAME);
        assert_eq!(
            device_name(&"x".repeat(100)).chars().count(),
            MAX_NAME_CHARS
        );
    }

    #[test]
    fn poll_answers_parse_by_status() {
        let pending: PollResponse =
            serde_json::from_str(r#"{"status":"pending"}"#).expect("pending");
        assert_eq!(pending, PollResponse::Pending);
        let approved: PollResponse = serde_json::from_str(
            r#"{"status":"approved","device_id":"abc","relay_url":"wss://sdrmm.link/v1/device/abc"}"#,
        )
        .expect("approved");
        assert_eq!(
            approved,
            PollResponse::Approved {
                device_id: "abc".to_string(),
                relay_url: "wss://sdrmm.link/v1/device/abc".to_string(),
            }
        );
    }

    #[test]
    fn only_well_formed_approvals_are_accepted() {
        let id = "0123456789abcdefghjkmnpqrs";
        assert!(paired(id.to_string(), "wss://sdrmm.link/v1/device/x").is_ok());
        assert!(paired("short".to_string(), "wss://sdrmm.link/v1/device/x").is_err());
        assert!(paired(id.to_uppercase(), "wss://sdrmm.link/v1/device/x").is_err());
        assert!(paired(id.to_string(), "https://sdrmm.link/").is_err());
        assert!(paired(id.to_string(), "not a url").is_err());
    }
}
