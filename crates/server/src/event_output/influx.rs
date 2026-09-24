use std::collections::BTreeMap;

use reqwest::{Client, Url};

use super::{Delivery, DeliveryError, EventFacts, checked, endpoint_url};

const MAX_STRING_FIELD: usize = 256;
const MAX_FIELD_DEPTH: usize = 3;

pub(super) struct InfluxTarget<'a> {
    pub url: &'a str,
    pub bucket: &'a str,
    pub org: &'a str,
    pub token: &'a str,
}

pub(super) async fn send(
    client: &Client,
    target: InfluxTarget<'_>,
    batch: &[Delivery],
) -> Result<(), DeliveryError> {
    let base = Url::parse(target.url)
        .map_err(|error| DeliveryError::Failed(format!("InfluxDB URL: {error}")))?;
    let mut url = endpoint_url(&base, &["api", "v2", "write"])
        .map_err(|error| DeliveryError::Failed(format!("InfluxDB write URL: {error}")))?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("bucket", target.bucket);
        if !target.org.is_empty() {
            query.append_pair("org", target.org);
        }
        query.append_pair("precision", "ns");
    }
    let body = batch
        .iter()
        .map(|delivery| line(&delivery.node, &delivery.message.facts))
        .collect::<Vec<_>>()
        .join("\n");
    let mut request = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(body);
    if !target.token.is_empty() {
        request = request.header(
            reqwest::header::AUTHORIZATION,
            format!("Token {}", target.token),
        );
    }
    let response = request.send().await.map_err(|error| {
        DeliveryError::Failed(format!("InfluxDB request: {}", error.without_url()))
    })?;
    checked("InfluxDB", response).await
}

pub(super) fn line(output_node: &str, facts: &EventFacts) -> String {
    let mut text = escape_name(facts.kind, &[',', ' ']);
    for (key, value) in [
        ("output", Some(output_node.to_owned())),
        ("device_set", Some(facts.device_set.to_string())),
        ("channel", Some(facts.channel.to_string())),
        ("station", facts.station.clone()),
    ] {
        if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
            text.push(',');
            text.push_str(key);
            text.push('=');
            text.push_str(&escape_name(&value, &[',', '=', ' ']));
        }
    }
    let fields = fields(facts);
    text.push(' ');
    text.push_str(
        &fields
            .iter()
            .map(|(key, value)| format!("{}={value}", escape_name(key, &[',', '=', ' '])))
            .collect::<Vec<_>>()
            .join(","),
    );
    match facts.at.parse::<jiff::Timestamp>() {
        Ok(at) => {
            text.push(' ');
            text.push_str(&at.as_nanosecond().to_string());
        }
        Err(error) => tracing::warn!(
            %error,
            at = %facts.at,
            "InfluxDB point has an unreadable time, the server stamps it on arrival"
        ),
    }
    text
}

fn fields(facts: &EventFacts) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    if let Some(data) = facts.record.pointer("/event/data") {
        flatten(data, String::new(), 0, &mut fields);
    }
    fields.insert("freq_hz".to_owned(), float(facts.freq_hz));
    fields.insert("summary".to_owned(), string(&facts.summary));
    fields
}

fn flatten(
    value: &serde_json::Value,
    key: String,
    depth: usize,
    fields: &mut BTreeMap<String, String>,
) {
    match value {
        serde_json::Value::Object(map) if depth < MAX_FIELD_DEPTH => {
            for (name, value) in map {
                let key = if key.is_empty() {
                    name.clone()
                } else {
                    format!("{key}.{name}")
                };
                flatten(value, key, depth + 1, fields);
            }
        }
        serde_json::Value::Number(number) if !key.is_empty() => {
            if let Some(number) = number.as_f64().filter(|number| number.is_finite()) {
                fields.insert(key, float(number));
            }
        }
        serde_json::Value::Bool(flag) if !key.is_empty() => {
            fields.insert(key, flag.to_string());
        }
        serde_json::Value::String(text)
            if !key.is_empty() && text.chars().count() <= MAX_STRING_FIELD =>
        {
            fields.insert(key, string(text));
        }
        _ => {}
    }
}

fn float(value: f64) -> String {
    let text = value.to_string();
    if text.contains(['.', 'e', 'E']) {
        text
    } else {
        format!("{text}.0")
    }
}

fn string(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        match character {
            '"' | '\\' => {
                quoted.push('\\');
                quoted.push(character);
            }
            '\n' | '\r' => quoted.push(' '),
            _ => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

fn escape_name(value: &str, special: &[char]) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        if special.contains(&character) || character == '\\' {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}
