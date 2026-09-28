use std::net::Ipv6Addr;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const PHONE_TOKEN_PREFIX: &str = "sdrmm-phone.";
pub const PHONE_ID_HEX: usize = 16;
pub const PHONE_SECRET_BYTES: usize = 32;
pub const PAIR_CODE_DIGITS: usize = 8;
pub const PAIR_OFFER_TTL_SECS: i64 = 300;
pub const PAIR_MAX_FAILURES: u32 = 5;
pub const PAIR_FAILURE_DELAY_MS: u64 = 1_000;
pub const MAX_PAIR_HOSTS: usize = 6;
pub const MAX_PAIR_HOST_LEN: usize = 262;
pub const MAX_PHONE_NAME_LEN: usize = 64;
pub const MAX_SERVER_NAME_LEN: usize = 64;
pub const DEFAULT_PHONE_PORT: u16 = 8443;
pub const PIN_HEX_LEN: usize = 64;
pub const PAIR_SCHEME: &str = "sdrmm";
pub const PAIR_HOST: &str = "pair";
pub const MDNS_SERVICE_TYPE: &str = "_sdrmm._tcp.local.";
pub const MDNS_TXT_VERSION: &str = "1";
pub const MDNS_TXT_KEY_VERSION: &str = "v";
pub const MDNS_TXT_KEY_PROTOCOL: &str = "p";
pub const MDNS_TXT_KEY_SERVER: &str = "id";
pub const MDNS_TXT_KEY_PIN: &str = "fp";
pub const MDNS_TXT_KEY_NAME: &str = "n";
pub const POSE_MIN_INTERVAL_MS: u64 = 50;
pub const POSE_KEEPALIVE_MS: u64 = 1_000;
pub const POSE_SILENT_AFTER_MS: u64 = 5_000;
pub const POSE_BURST: u32 = 16;
pub const POSE_RATE_HZ: u32 = 20;

const MAX_HOST_NAME_LEN: usize = 253;
const KEY_CHECK_CHARS: usize = 20;
const GROUP_LEN: usize = 4;
const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PhonePlatform {
    Ios,
    Android,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Phone {
    pub id: String,
    pub name: String,
    pub platform: PhonePlatform,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<String>,
    pub online: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gps_nodes: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PhoneAccess {
    pub enabled: bool,
    pub port: u16,
}

impl Default for PhoneAccess {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_PHONE_PORT,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PhoneListenerState {
    Off,
    On { port: u16 },
    Failed { port: u16, reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MdnsState {
    Off,
    On { instance: String },
    Failed { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PhoneEndpoint {
    pub port: u16,
    pub hosts: Vec<String>,
    pub pin: String,
    pub key_check: String,
    pub dedicated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PhoneAccessStatus {
    pub access: PhoneAccess,
    pub listener: PhoneListenerState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<PhoneEndpoint>,
    pub mdns: MdnsState,
    pub protocol: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PhonesResponse {
    pub phones: Vec<Phone>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer: Option<PairingOfferStatus>,
    pub access: PhoneAccessStatus,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CreateOfferRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PairingOffer {
    pub id: String,
    pub code: String,
    pub uri: String,
    pub key_check: String,
    pub expires_at: String,
    pub endpoint: PhoneEndpoint,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OfferState {
    Live,
    Used { phone: String },
    Burned,
    Expired,
    Cancelled,
    Superseded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PairingOfferStatus {
    pub id: String,
    pub state: OfferState,
    pub expires_at: String,
    pub failures: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PairRequest {
    pub code: String,
    pub name: String,
    pub platform: PhonePlatform,
    pub protocol: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rebind: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PairResponse {
    pub phone: Phone,
    pub token: String,
    pub server_id: String,
    pub server_name: String,
    pub protocol: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RenamePhoneRequest {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PhoneSelf {
    pub phone: Phone,
    pub server_id: String,
    pub server_name: String,
}

#[derive(Clone, PartialEq, Eq)]
pub struct PhoneToken {
    pub phone: String,
    secret: [u8; PHONE_SECRET_BYTES],
}

impl PhoneToken {
    #[must_use]
    pub fn new(phone: String, secret: [u8; PHONE_SECRET_BYTES]) -> Self {
        Self { phone, secret }
    }

    #[must_use]
    pub fn secret(&self) -> &[u8; PHONE_SECRET_BYTES] {
        &self.secret
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let (phone, secret) = text.strip_prefix(PHONE_TOKEN_PREFIX)?.split_once('.')?;
        if !valid_phone_id(phone) || secret.len() != PHONE_SECRET_BYTES * 2 {
            return None;
        }
        let secret = unhex(secret)?.try_into().ok()?;
        Some(Self {
            phone: phone.to_owned(),
            secret,
        })
    }

    #[must_use]
    pub fn encode(&self) -> String {
        format!("{PHONE_TOKEN_PREFIX}{}.{}", self.phone, hex(&self.secret))
    }
}

impl std::fmt::Debug for PhoneToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhoneToken")
            .field("phone", &self.phone)
            .field("secret", &format_args!("<redacted>"))
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairUri {
    pub hosts: Vec<String>,
    pub code: String,
    pub pin: String,
    pub protocol: u32,
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PairUriError {
    #[error("not a pairing link")]
    NotPairing,
    #[error("no host")]
    NoHost,
    #[error("too many hosts")]
    TooManyHosts,
    #[error("bad host {0}")]
    Host(String),
    #[error("bad code")]
    Code,
    #[error("bad key")]
    Pin,
    #[error("bad protocol")]
    Protocol,
    #[error("bad name")]
    Name,
    #[error("{0} given twice")]
    Duplicate(&'static str),
}

impl PairUri {
    #[must_use]
    pub fn to_uri(&self) -> String {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        for host in &self.hosts {
            query.append_pair("h", host);
        }
        query
            .append_pair("c", &self.code)
            .append_pair("fp", &self.pin)
            .append_pair("p", &self.protocol.to_string());
        if let Some(name) = &self.name {
            query.append_pair("n", name);
        }
        format!("{PAIR_SCHEME}://{PAIR_HOST}?{}", query.finish())
    }

    pub fn parse(text: &str) -> Result<Self, PairUriError> {
        let url = url::Url::parse(text).map_err(|_| PairUriError::NotPairing)?;
        if !is_pairing_link(&url) {
            return Err(PairUriError::NotPairing);
        }
        let mut fields = PairFields::default();
        for (key, value) in url.query_pairs() {
            fields.take(&key, value.into_owned())?;
        }
        fields.finish()
    }
}

fn is_pairing_link(url: &url::Url) -> bool {
    url.scheme() == PAIR_SCHEME
        && url.host_str() == Some(PAIR_HOST)
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(url.path(), "" | "/")
}

#[derive(Default)]
struct PairFields {
    hosts: Vec<String>,
    code: Option<String>,
    pin: Option<String>,
    protocol: Option<String>,
    name: Option<String>,
}

impl PairFields {
    fn take(&mut self, key: &str, value: String) -> Result<(), PairUriError> {
        let (slot, key) = match key {
            "h" => return self.add_host(value),
            "c" => (&mut self.code, "c"),
            "fp" => (&mut self.pin, "fp"),
            "p" => (&mut self.protocol, "p"),
            "n" => (&mut self.name, "n"),
            _ => return Ok(()),
        };
        if slot.replace(value).is_some() {
            return Err(PairUriError::Duplicate(key));
        }
        Ok(())
    }

    fn add_host(&mut self, host: String) -> Result<(), PairUriError> {
        if !valid_pair_host(&host) {
            return Err(PairUriError::Host(host));
        }
        if self.hosts.contains(&host) {
            return Ok(());
        }
        if self.hosts.len() == MAX_PAIR_HOSTS {
            return Err(PairUriError::TooManyHosts);
        }
        self.hosts.push(host);
        Ok(())
    }

    fn finish(self) -> Result<PairUri, PairUriError> {
        if self.hosts.is_empty() {
            return Err(PairUriError::NoHost);
        }
        let code = self
            .code
            .filter(|code| valid_pair_code(code))
            .ok_or(PairUriError::Code)?;
        let pin = self
            .pin
            .filter(|pin| valid_pin(pin))
            .ok_or(PairUriError::Pin)?;
        let protocol = self
            .protocol
            .as_deref()
            .and_then(parse_protocol)
            .ok_or(PairUriError::Protocol)?;
        if self
            .name
            .as_deref()
            .is_some_and(|name| !valid_label(name, MAX_SERVER_NAME_LEN))
        {
            return Err(PairUriError::Name);
        }
        Ok(PairUri {
            hosts: self.hosts,
            code,
            pin,
            protocol,
            name: self.name,
        })
    }
}

fn parse_protocol(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

#[must_use]
pub fn valid_phone_id(id: &str) -> bool {
    id.strip_prefix('p')
        .is_some_and(|digits| digits.len() == PHONE_ID_HEX && is_lower_hex(digits))
}

#[must_use]
pub fn valid_phone_name(name: &str) -> bool {
    valid_label(name, MAX_PHONE_NAME_LEN)
}

#[must_use]
pub fn valid_pin(pin: &str) -> bool {
    pin.len() == PIN_HEX_LEN && is_lower_hex(pin)
}

#[must_use]
pub fn valid_pair_code(code: &str) -> bool {
    code.len() == PAIR_CODE_DIGITS && code.bytes().all(|byte| byte.is_ascii_digit())
}

#[must_use]
pub fn valid_pair_host(host: &str) -> bool {
    if host.len() > MAX_PAIR_HOST_LEN {
        return false;
    }
    match host.strip_prefix('[') {
        Some(bracketed) => bracketed
            .split_once("]:")
            .is_some_and(|(address, port)| address.parse::<Ipv6Addr>().is_ok() && valid_port(port)),
        None => host
            .split_once(':')
            .is_some_and(|(name, port)| valid_host_name(name) && valid_port(port)),
    }
}

fn valid_host_name(name: &str) -> bool {
    (1..=MAX_HOST_NAME_LEN).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
}

fn valid_port(port: &str) -> bool {
    !port.starts_with('0')
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|port| port != 0)
}

fn valid_label(text: &str, max_chars: usize) -> bool {
    text.trim() == text
        && (1..=max_chars).contains(&text.chars().count())
        && !text.chars().any(char::is_control)
}

fn is_lower_hex(text: &str) -> bool {
    text.bytes().all(|byte| nibble(byte).is_some())
}

#[must_use]
pub fn key_check(pin: &str) -> String {
    grouped(
        pin.chars()
            .take(KEY_CHECK_CHARS)
            .map(|digit| digit.to_ascii_uppercase()),
    )
}

#[cfg(feature = "pin")]
pub fn spki_pin(cert_der: &[u8]) -> Result<String, PinError> {
    use sha2::Digest;
    let der = rustls_pki_types::CertificateDer::from(cert_der);
    let cert = webpki::EndEntityCert::try_from(&der).map_err(|_| PinError::Parse)?;
    Ok(hex(&sha2::Sha256::digest(cert.subject_public_key_info())))
}

#[cfg(feature = "pin")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PinError {
    #[error("certificate does not parse")]
    Parse,
}

#[must_use]
pub fn group_code(code: &str) -> String {
    grouped(code.chars())
}

fn grouped(chars: impl Iterator<Item = char>) -> String {
    let mut out = String::new();
    for (index, digit) in chars.enumerate() {
        if index > 0 && index.is_multiple_of(GROUP_LEN) {
            out.push(' ');
        }
        out.push(digit);
    }
    out
}

#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

#[must_use]
pub fn unhex(text: &str) -> Option<Vec<u8>> {
    let (pairs, rest) = text.as_bytes().as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    pairs
        .iter()
        .map(|[high, low]| Some((nibble(*high)? << 4) | nibble(*low)?))
        .collect()
}

fn nibble(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
