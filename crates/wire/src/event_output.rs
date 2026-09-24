use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const MAX_OUTPUT_URL_LEN: usize = 2_048;
pub const MAX_MATRIX_ROOM_ID_LEN: usize = 255;
pub const MAX_OUTPUT_SECRET_LEN: usize = 4_096;
pub const MAX_MQTT_TOPIC_LEN: usize = 512;
pub const MAX_MQTT_USERNAME_LEN: usize = 255;
pub const MAX_SQL_IDENTIFIER_LEN: usize = 63;
pub const MAX_INFLUX_NAME_LEN: usize = 255;
pub const DEFAULT_POSTGRES_TABLE: &str = "sdrmm_events";

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
    Beast {
        address: String,
        #[serde(default)]
        enabled: bool,
    },
    Tunnel {
        interface: String,
        #[schema(value_type = String, format = "ipv4")]
        address: std::net::Ipv4Addr,
        prefix: u8,
    },
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
    Postgres {
        url: String,
        table: String,
        #[serde(default)]
        username: String,
        #[serde(default)]
        password: String,
    },
    Influx {
        url: String,
        bucket: String,
        #[serde(default)]
        org: String,
        #[serde(default)]
        token: String,
    },
}

impl std::fmt::Debug for EventOutputTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Beast { address, .. } => formatter
                .debug_struct("Beast")
                .field("address", address)
                .finish(),
            Self::Tunnel {
                interface,
                address,
                prefix,
            } => formatter
                .debug_struct("Tunnel")
                .field("interface", interface)
                .field("address", address)
                .field("prefix", prefix)
                .finish(),
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
            Self::Postgres {
                url,
                table,
                username,
                ..
            } => formatter
                .debug_struct("Postgres")
                .field("url", &redacted_if_credentialed(url))
                .field("table", table)
                .field("username", username)
                .field("password", &"[redacted]")
                .finish(),
            Self::Influx {
                url, bucket, org, ..
            } => formatter
                .debug_struct("Influx")
                .field("url", &redacted_if_credentialed(url))
                .field("bucket", bucket)
                .field("org", org)
                .field("token", &"[redacted]")
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
            Self::Beast { address, enabled } => *enabled && !address.is_empty(),
            Self::Tunnel { interface, .. } => !interface.is_empty(),
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
            Self::Postgres {
                url,
                table,
                username,
                ..
            } => [url, table, username]
                .into_iter()
                .all(|value| !value.trim().is_empty()),
            Self::Influx { url, bucket, .. } => [url, bucket]
                .into_iter()
                .all(|value| !value.trim().is_empty()),
        }
    }

    #[must_use]
    pub fn valid(&self) -> bool {
        match self {
            Self::Beast { address, .. } => {
                address.is_empty()
                    || (address.len() <= crate::MAX_NETWORK_ADDRESS_LEN
                        && address
                            .parse::<std::net::SocketAddr>()
                            .is_ok_and(|address| address.port() != 0))
            }
            Self::Tunnel {
                interface,
                address,
                prefix,
            } => {
                interface.len() <= 15
                    && interface
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                    && *prefix <= 32
                    && !address.is_unspecified()
                    && !address.is_multicast()
                    && !address.is_broadcast()
            }
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
            Self::Postgres {
                url,
                table,
                username,
                password,
            } => {
                valid_postgres_url(url)
                    && (table.is_empty() || valid_sql_identifier(table))
                    && username.len() <= MAX_SQL_IDENTIFIER_LEN
                    && password.len() <= MAX_OUTPUT_SECRET_LEN
            }
            Self::Influx {
                url,
                bucket,
                org,
                token,
            } => {
                valid_influx_url(url)
                    && valid_influx_name(bucket)
                    && valid_influx_name(org)
                    && token.len() <= MAX_OUTPUT_SECRET_LEN
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

fn valid_postgres_url(value: &str) -> bool {
    value.is_empty() || valid_url(value, &["postgres", "postgresql"])
}

fn valid_influx_url(value: &str) -> bool {
    value.is_empty() || valid_url(value, &["http", "https"])
}

#[must_use]
pub fn valid_sql_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    value.len() <= MAX_SQL_IDENTIFIER_LEN
        && bytes
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first == b'_')
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn valid_influx_name(value: &str) -> bool {
    value.len() <= MAX_INFLUX_NAME_LEN && !value.chars().any(char::is_control)
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct BeastExportStatus {
    pub node: String,
    pub address: String,
    pub listening: bool,
    pub clients: u32,
    pub frames: u64,
    pub error: Option<String>,
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
    fn beast_requires_an_explicit_listener_and_enable() {
        for address in ["127.0.0.1:30005", "0.0.0.0:30005", "[::1]:30005"] {
            let target = EventOutputTarget::Beast {
                address: address.to_owned(),
                enabled: true,
            };
            assert!(target.valid() && target.configured());
            assert_eq!(
                serde_json::from_str::<EventOutputTarget>(&serde_json::to_string(&target).unwrap())
                    .unwrap(),
                target
            );
        }
        for address in ["127.0.0.1:0", "localhost", "::1:30005", "host:30005"] {
            assert!(
                !EventOutputTarget::Beast {
                    address: address.to_owned(),
                    enabled: true
                }
                .valid()
            );
        }
        assert!(
            !EventOutputTarget::Beast {
                address: "127.0.0.1:30005".to_owned(),
                enabled: false
            }
            .configured()
        );
    }

    #[test]
    fn tunnel_configuration_is_bounded_and_round_trips() {
        let target = EventOutputTarget::Tunnel {
            interface: "utun7".to_owned(),
            address: "10.23.0.1".parse().unwrap(),
            prefix: 24,
        };
        assert!(target.valid() && target.configured());
        assert_eq!(
            serde_json::from_str::<EventOutputTarget>(&serde_json::to_string(&target).unwrap())
                .unwrap(),
            target
        );
        for interface in ["../tun", "tun 0", "1234567890123456"] {
            let target = EventOutputTarget::Tunnel {
                interface: interface.to_owned(),
                address: "10.23.0.1".parse().unwrap(),
                prefix: 24,
            };
            assert!(!target.valid());
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

    fn postgres(url: &str, table: &str, username: &str) -> EventOutputTarget {
        EventOutputTarget::Postgres {
            url: url.to_owned(),
            table: table.to_owned(),
            username: username.to_owned(),
            password: String::new(),
        }
    }

    fn influx(url: &str, bucket: &str) -> EventOutputTarget {
        EventOutputTarget::Influx {
            url: url.to_owned(),
            bucket: bucket.to_owned(),
            org: String::new(),
            token: String::new(),
        }
    }

    #[test]
    fn postgres_needs_a_server_a_plain_table_and_a_user() {
        let target = postgres("postgres://db.example:5432/radio", "sdrmm_events", "radio");
        assert!(target.valid() && target.configured());
        assert!(postgres("postgresql://127.0.0.1/radio?sslmode=require", "t", "radio").valid());
        assert!(!postgres("", "sdrmm_events", "radio").configured());
        assert!(!postgres("postgres://db.example/radio", "", "radio").configured());
        assert!(!postgres("postgres://db.example/radio", "sdrmm_events", "").configured());
        for url in [
            "https://db.example/radio",
            "postgres://radio:secret@db.example/radio",
            "postgres://",
        ] {
            assert!(!postgres(url, "sdrmm_events", "radio").valid(), "{url}");
        }
        for table in [
            "Events",
            "1events",
            "events;drop",
            "public.events",
            "\"events\"",
        ] {
            assert!(
                !postgres("postgres://db.example/radio", table, "radio").valid(),
                "{table}"
            );
        }
        assert!(
            !postgres(
                "postgres://db.example/radio",
                &"t".repeat(MAX_SQL_IDENTIFIER_LEN + 1),
                "radio"
            )
            .valid()
        );
        assert_eq!(
            serde_json::from_str::<EventOutputTarget>(&serde_json::to_string(&target).unwrap())
                .unwrap(),
            target
        );
    }

    #[test]
    fn influx_needs_a_server_and_a_bucket() {
        let target = influx("http://127.0.0.1:8086", "radio");
        assert!(target.valid() && target.configured());
        assert!(influx("https://influx.example", "radio").valid());
        assert!(!influx("", "radio").configured());
        assert!(!influx("http://127.0.0.1:8086", "").configured());
        for url in [
            "influx://127.0.0.1",
            "https://user:secret@influx.example",
            "https://",
        ] {
            assert!(!influx(url, "radio").valid(), "{url}");
        }
        assert!(!influx("https://influx.example", "ra\ndio").valid());
        assert!(
            !influx(
                "https://influx.example",
                &"b".repeat(MAX_INFLUX_NAME_LEN + 1)
            )
            .valid()
        );
        assert!(
            !EventOutputTarget::Influx {
                url: "https://influx.example".to_owned(),
                bucket: "radio".to_owned(),
                org: String::new(),
                token: "t".repeat(MAX_OUTPUT_SECRET_LEN + 1),
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

        let database_secret = "database-secret";
        let target = EventOutputTarget::Postgres {
            url: "postgres://db.example/radio".to_owned(),
            table: "sdrmm_events".to_owned(),
            username: "radio".to_owned(),
            password: database_secret.to_owned(),
        };
        let debug = format!("{target:?}");
        assert!(debug.contains("postgres://db.example/radio"));
        assert!(!debug.contains(database_secret));

        let token = "influx-token";
        let target = EventOutputTarget::Influx {
            url: "https://influx.example".to_owned(),
            bucket: "radio".to_owned(),
            org: "home".to_owned(),
            token: token.to_owned(),
        };
        let debug = format!("{target:?}");
        assert!(debug.contains("radio"));
        assert!(!debug.contains(token));
    }
}
