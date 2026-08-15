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
            Self::Discord { webhook_url } => valid_url(webhook_url),
            Self::Matrix {
                homeserver_url,
                room_id,
                access_token,
            } => {
                valid_url(homeserver_url)
                    && room_id.len() <= MAX_MATRIX_ROOM_ID_LEN
                    && access_token.len() <= MAX_CHAT_TOKEN_LEN
            }
        }
    }
}

fn valid_url(value: &str) -> bool {
    value.is_empty()
        || value.len() <= MAX_CHAT_URL_LEN
            && url::Url::parse(value).is_ok_and(|url| url.scheme() == "https")
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
        let mut target = ChatOutputTarget::Matrix {
            homeserver_url: "https://matrix.example".to_owned(),
            room_id: "!radio:example".to_owned(),
            access_token: String::new(),
        };
        assert!(!target.configured());
        if let ChatOutputTarget::Matrix { access_token, .. } = &mut target {
            *access_token = "secret".to_owned();
        }
        assert!(target.configured());
        assert!(target.valid());
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
