use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const MAX_OUTPUT_URL_LEN: usize = 2_048;
pub const MAX_MATRIX_ROOM_ID_LEN: usize = 255;
pub const MAX_OUTPUT_SECRET_LEN: usize = 4_096;
pub const MAX_MQTT_TOPIC_LEN: usize = 512;
pub const MAX_MQTT_USERNAME_LEN: usize = 255;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WebhookFormat {
    #[default]
    Json,
    Discord,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "service", rename_all = "snake_case")]
pub enum EventOutputTarget {
    Webhook {
        url: String,
        #[serde(default)]
        format: WebhookFormat,
    },
    Matrix {
        homeserver_url: String,
        room_id: String,
        access_token: String,
    },
    Mqtt {
        broker_url: String,
        topic: String,
        #[serde(default)]
        username: String,
        #[serde(default)]
        password: String,
    },
}

impl std::fmt::Debug for EventOutputTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Webhook { format, .. } => formatter
                .debug_struct("Webhook")
                .field("url", &"[redacted]")
                .field("format", format)
                .finish(),
            Self::Matrix {
                homeserver_url,
                room_id,
                ..
            } => formatter
                .debug_struct("Matrix")
                .field("homeserver_url", &redacted_if_credentialed(homeserver_url))
                .field("room_id", room_id)
                .field("access_token", &"[redacted]")
                .finish(),
            Self::Mqtt {
                broker_url,
                topic,
                username,
                ..
            } => formatter
                .debug_struct("Mqtt")
                .field("broker_url", &redacted_if_credentialed(broker_url))
                .field("topic", topic)
                .field("username", username)
                .field("password", &"[redacted]")
                .finish(),
        }
    }
}

impl Default for EventOutputTarget {
    fn default() -> Self {
        Self::Webhook {
            url: String::new(),
            format: WebhookFormat::Json,
        }
    }
}

impl EventOutputTarget {
    #[must_use]
    pub fn configured(&self) -> bool {
        match self {
            Self::Webhook { url, .. } => !url.trim().is_empty(),
            Self::Matrix {
                homeserver_url,
                room_id,
                access_token,
            } => [homeserver_url, room_id, access_token]
                .into_iter()
                .all(|value| !value.trim().is_empty()),
            Self::Mqtt {
                broker_url, topic, ..
            } => [broker_url, topic]
                .into_iter()
                .all(|value| !value.trim().is_empty()),
        }
    }

    #[must_use]
    pub fn valid(&self) -> bool {
        match self {
            Self::Webhook { url, .. } => valid_https_url(url),
            Self::Matrix {
                homeserver_url,
                room_id,
                access_token,
            } => {
                valid_https_url(homeserver_url)
                    && room_id.len() <= MAX_MATRIX_ROOM_ID_LEN
                    && access_token.len() <= MAX_OUTPUT_SECRET_LEN
            }
            Self::Mqtt {
                broker_url,
                topic,
                username,
                password,
            } => {
                valid_broker_url(broker_url)
                    && valid_publish_topic(topic)
                    && username.len() <= MAX_MQTT_USERNAME_LEN
                    && password.len() <= MAX_OUTPUT_SECRET_LEN
            }
        }
    }
}

fn valid_https_url(value: &str) -> bool {
    value.is_empty() || valid_url(value, &["https"])
}

fn valid_broker_url(value: &str) -> bool {
    value.is_empty() || valid_url(value, &["mqtt", "mqtts"])
}

fn valid_url(value: &str, schemes: &[&str]) -> bool {
    value.len() <= MAX_OUTPUT_URL_LEN
        && url::Url::parse(value).is_ok_and(|url| {
            schemes.contains(&url.scheme())
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
        })
}

fn valid_publish_topic(value: &str) -> bool {
    value.len() <= MAX_MQTT_TOPIC_LEN
        && !value.contains(['+', '#', '\0'])
        && !value.starts_with('/')
}

fn redacted_if_credentialed(value: &str) -> &str {
    if url::Url::parse(value)
        .is_ok_and(|url| !url.username().is_empty() || url.password().is_some())
    {
        "[redacted]"
    } else {
        value
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EventOutputNode {
    pub target: EventOutputTarget,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matrix(homeserver_url: &str, room_id: &str, access_token: &str) -> EventOutputTarget {
        EventOutputTarget::Matrix {
            homeserver_url: homeserver_url.to_owned(),
            room_id: room_id.to_owned(),
            access_token: access_token.to_owned(),
        }
    }

    fn webhook(url: &str) -> EventOutputTarget {
        EventOutputTarget::Webhook {
            url: url.to_owned(),
            format: WebhookFormat::Json,
        }
    }

    fn mqtt(broker_url: &str, topic: &str) -> EventOutputTarget {
        EventOutputTarget::Mqtt {
            broker_url: broker_url.to_owned(),
            topic: topic.to_owned(),
            username: String::new(),
            password: String::new(),
        }
    }

    #[test]
    fn matrix_requires_every_credential_before_delivery() {
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
                &"t".repeat(MAX_OUTPUT_SECRET_LEN + 1),
            ),
        ] {
            assert!(!target.valid());
        }
        let target = matrix("https://matrix.example", "!radio:example", "secret");
        assert!(target.configured() && target.valid());
        assert!(
            !matrix(
                "https://matrix-user:matrix-password@matrix.example",
                "!radio:example",
                "secret",
            )
            .valid()
        );
    }

    #[test]
    fn a_webhook_takes_any_bounded_https_endpoint() {
        assert!(EventOutputTarget::default().valid());
        assert!(!EventOutputTarget::default().configured());
        for url in [
            "https://discord.com/api/webhooks/1/token",
            "https://discord.com/api/v10/webhooks/1/token",
            "https://hooks.example.org/services/abc",
        ] {
            assert!(webhook(url).valid(), "{url}");
            assert!(webhook(url).configured(), "{url}");
        }
        for url in [
            "ftp://example.org/hook",
            "http://example.org/hook",
            "https://",
            "https://user:password@example.org/hook",
        ] {
            assert!(!webhook(url).valid(), "{url}");
        }
        assert!(
            !webhook(&format!(
                "https://example.org/{}",
                "x".repeat(MAX_OUTPUT_URL_LEN)
            ))
            .valid()
        );
    }

    #[test]
    fn the_webhook_format_survives_a_round_trip_and_defaults_to_json() {
        let discord = EventOutputTarget::Webhook {
            url: "https://discord.com/api/webhooks/1/token".to_owned(),
            format: WebhookFormat::Discord,
        };
        let encoded = serde_json::to_string(&discord).expect("encode");
        assert!(encoded.contains(r#""service":"webhook""#));
        assert!(encoded.contains(r#""format":"discord""#));
        assert_eq!(
            serde_json::from_str::<EventOutputTarget>(&encoded).expect("decode"),
            discord
        );
        assert_eq!(
            serde_json::from_str::<EventOutputTarget>(r#"{"service":"webhook","url":""}"#)
                .expect("decode"),
            EventOutputTarget::default()
        );
    }

    #[test]
    fn mqtt_needs_a_broker_and_a_publishable_topic() {
        assert!(!mqtt("", "sdrmm/events").configured());
        assert!(!mqtt("mqtts://broker.example", "").configured());
        let target = mqtt("mqtts://broker.example:8883", "sdrmm/events");
        assert!(target.configured() && target.valid());
        assert!(mqtt("mqtt://127.0.0.1:1883", "sdrmm/events").valid());
        for broker_url in [
            "https://broker.example",
            "mqtt://user:password@broker.example",
            "mqtts://",
        ] {
            assert!(!mqtt(broker_url, "sdrmm/events").valid(), "{broker_url}");
        }
        for topic in ["sdrmm/+/events", "sdrmm/#", "/sdrmm/events"] {
            assert!(!mqtt("mqtts://broker.example", topic).valid(), "{topic}");
        }
        assert!(
            !mqtt(
                "mqtts://broker.example",
                &"t".repeat(MAX_MQTT_TOPIC_LEN + 1),
            )
            .valid()
        );
        assert!(
            !EventOutputTarget::Mqtt {
                broker_url: "mqtts://broker.example".to_owned(),
                topic: "sdrmm/events".to_owned(),
                username: "radio".to_owned(),
                password: "p".repeat(MAX_OUTPUT_SECRET_LEN + 1),
            }
            .valid()
        );
    }

    #[test]
    fn debug_output_redacts_credentials() {
        let webhook_secret = "webhook-secret";
        let target = webhook(&format!("https://hooks.example/{webhook_secret}"));
        let debug = format!("{target:?}");
        assert!(debug.contains("Webhook"));
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains(webhook_secret));

        let matrix_secret = "matrix-secret";
        let target = matrix(
            "https://matrix.example",
            "!radio:matrix.example",
            matrix_secret,
        );
        let debug = format!("{target:?}");
        assert!(debug.contains("Matrix"));
        assert!(debug.contains("https://matrix.example"));
        assert!(debug.contains("!radio:matrix.example"));
        assert!(!debug.contains(matrix_secret));

        let user = "matrix-user";
        let password = "matrix-password";
        let target = matrix(
            &format!("https://{user}:{password}@matrix.example"),
            "!radio:matrix.example",
            "matrix-secret",
        );
        let debug = format!("{target:?}");
        assert!(!debug.contains(user));
        assert!(!debug.contains(password));
        assert!(debug.contains("homeserver_url: \"[redacted]\""));

        let broker_secret = "broker-secret";
        let target = EventOutputTarget::Mqtt {
            broker_url: "mqtts://broker.example".to_owned(),
            topic: "sdrmm/events".to_owned(),
            username: "radio".to_owned(),
            password: broker_secret.to_owned(),
        };
        let debug = format!("{target:?}");
        assert!(debug.contains("mqtts://broker.example"));
        assert!(debug.contains("sdrmm/events"));
        assert!(!debug.contains(broker_secret));
    }
}
