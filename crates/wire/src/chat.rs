use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const MAX_CHAT_URL_LEN: usize = 2_048;
pub const MAX_MATRIX_ROOM_ID_LEN: usize = 255;
pub const MAX_CHAT_TOKEN_LEN: usize = 4_096;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "service", rename_all = "snake_case")]
pub enum ChatOutputTarget {
    Discord {
        webhook_url: String,
    },
    Matrix {
        homeserver_url: String,
        room_id: String,
        access_token: String,
    },
}

impl std::fmt::Debug for ChatOutputTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Discord { .. } => formatter
                .debug_struct("Discord")
                .field("webhook_url", &"[redacted]")
                .finish(),
            Self::Matrix {
                homeserver_url,
                room_id,
                ..
            } => formatter
                .debug_struct("Matrix")
                .field("homeserver_url", homeserver_url)
                .field("room_id", room_id)
                .field("access_token", &"[redacted]")
                .finish(),
        }
    }
}

impl Default for ChatOutputTarget {
    fn default() -> Self {
        Self::Discord {
            webhook_url: String::new(),
        }
    }
}

impl ChatOutputTarget {
    #[must_use]
    pub fn configured(&self) -> bool {
        match self {
            Self::Discord { webhook_url } => !webhook_url.trim().is_empty(),
            Self::Matrix {
                homeserver_url,
                room_id,
                access_token,
            } => [homeserver_url, room_id, access_token]
                .into_iter()
                .all(|value| !value.trim().is_empty()),
        }
    }

    #[must_use]
    pub fn valid(&self) -> bool {
        match self {
            Self::Discord { webhook_url } => valid_discord_webhook(webhook_url),
            Self::Matrix {
                homeserver_url,
                room_id,
                access_token,
            } => {
                valid_https_url(homeserver_url)
                    && room_id.len() <= MAX_MATRIX_ROOM_ID_LEN
                    && access_token.len() <= MAX_CHAT_TOKEN_LEN
            }
        }
    }
}

fn valid_https_url(value: &str) -> bool {
    value.is_empty()
        || value.len() <= MAX_CHAT_URL_LEN
            && url::Url::parse(value).is_ok_and(|url| url.scheme() == "https")
}

fn valid_discord_webhook(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    if value.len() > MAX_CHAT_URL_LEN
        || url.scheme() != "https"
        || url.host_str() != Some("discord.com")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    let Some(segments) = url.path_segments() else {
        return false;
    };
    let segments: Vec<_> = segments.collect();
    match segments.as_slice() {
        ["api", "webhooks", id, token] => valid_webhook_credentials(id, token),
        ["api", version, "webhooks", id, token] => {
            valid_api_version(version) && valid_webhook_credentials(id, token)
        }
        _ => false,
    }
}

fn valid_api_version(value: &str) -> bool {
    value
        .strip_prefix('v')
        .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
}

fn valid_webhook_credentials(id: &str, token: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) && !token.is_empty()
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ChatOutputNode {
    pub target: ChatOutputTarget,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_requires_every_credential_before_delivery() {
        let matrix =
            |homeserver_url: &str, room_id: &str, access_token: &str| ChatOutputTarget::Matrix {
                homeserver_url: homeserver_url.to_owned(),
                room_id: room_id.to_owned(),
                access_token: access_token.to_owned(),
            };
        for target in [
            matrix("", "!radio:example", "secret"),
            matrix("https://matrix.example", "", "secret"),
            matrix("https://matrix.example", "!radio:example", ""),
        ] {
            assert!(!target.configured());
        }
        for target in [
            matrix(
                "https://matrix.example",
                &"r".repeat(MAX_MATRIX_ROOM_ID_LEN + 1),
                "secret",
            ),
            matrix(
                "https://matrix.example",
                "!radio:example",
                &"t".repeat(MAX_CHAT_TOKEN_LEN + 1),
            ),
        ] {
            assert!(!target.valid());
        }
        let target = matrix("https://matrix.example", "!radio:example", "secret");
        assert!(target.configured() && target.valid());
    }

    #[test]
    fn endpoints_are_bounded_https_urls() {
        assert!(ChatOutputTarget::default().valid());
        assert!(
            ChatOutputTarget::Discord {
                webhook_url: "https://discord.com/api/webhooks/1/token".to_owned(),
            }
            .valid()
        );
        assert!(
            ChatOutputTarget::Discord {
                webhook_url: "https://discord.com/api/v10/webhooks/1/token".to_owned(),
            }
            .valid()
        );
        assert!(
            !ChatOutputTarget::Discord {
                webhook_url: "ftp://discord.example/hook".to_owned(),
            }
            .valid()
        );
        assert!(
            !ChatOutputTarget::Discord {
                webhook_url: "http://discord.example/hook".to_owned(),
            }
            .valid()
        );
        assert!(
            !ChatOutputTarget::Discord {
                webhook_url: "https://".to_owned(),
            }
            .valid()
        );
        for webhook_url in [
            "https://example.com/api/webhooks/1/token",
            "https://127.0.0.1/api/webhooks/1/token",
            "https://discord.com.example/api/webhooks/1/token",
            "https://discord.com/api/channels/1/token",
        ] {
            assert!(
                !ChatOutputTarget::Discord {
                    webhook_url: webhook_url.to_owned(),
                }
                .valid()
            );
        }
    }

    #[test]
    fn debug_output_redacts_credentials() {
        let discord_secret = "discord-secret";
        let discord = ChatOutputTarget::Discord {
            webhook_url: format!("https://discord.example/webhooks/{discord_secret}"),
        };
        let discord_debug = format!("{discord:?}");
        assert!(discord_debug.contains("Discord"));
        assert!(discord_debug.contains("[redacted]"));
        assert!(!discord_debug.contains(discord_secret));

        let matrix_secret = "matrix-secret";
        let matrix = ChatOutputTarget::Matrix {
            homeserver_url: "https://matrix.example".to_owned(),
            room_id: "!radio:matrix.example".to_owned(),
            access_token: matrix_secret.to_owned(),
        };
        let matrix_debug = format!("{matrix:?}");
        assert!(matrix_debug.contains("Matrix"));
        assert!(matrix_debug.contains("https://matrix.example"));
        assert!(matrix_debug.contains("!radio:matrix.example"));
        assert!(matrix_debug.contains("[redacted]"));
        assert!(!matrix_debug.contains(matrix_secret));
    }
}
