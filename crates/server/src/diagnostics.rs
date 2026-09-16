use std::{
    borrow::Cow,
    collections::VecDeque,
    fmt::Debug,
    net::IpAddr,
    sync::{
        LazyLock, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

use sdrmm_wire::{LogLevel, LogLine, MAX_LOG_LINES, MAX_LOG_MESSAGE_LEN};
use tracing::{
    Event, Level, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context};

pub struct LogRing {
    lines: Mutex<VecDeque<LogLine>>,
    dropped: AtomicU64,
    capacity: usize,
}

impl LogRing {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            lines: Mutex::new(VecDeque::with_capacity(capacity)),
            dropped: AtomicU64::new(0),
            capacity: capacity.max(1),
        }
    }

    pub fn push(&self, line: LogLine) {
        let mut lines = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
        while lines.len() >= self.capacity {
            lines.pop_front();
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        lines.push_back(line);
    }

    #[must_use]
    pub fn lines(&self) -> Vec<LogLine> {
        self.lines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn clear(&self) {
        self.lines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.dropped.store(0, Ordering::Relaxed);
    }
}

static LOG: LazyLock<LogRing> = LazyLock::new(|| LogRing::new(MAX_LOG_LINES));

#[must_use]
pub fn log() -> &'static LogRing {
    &LOG
}

static SECRETS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Registers a string the log must never carry. Called with the shared token at startup, before
/// any request can put it in a message.
pub fn hide_secret(secret: &str) {
    if secret.is_empty() {
        return;
    }
    let mut secrets = SECRETS.lock().unwrap_or_else(PoisonError::into_inner);
    if !secrets.iter().any(|known| known == secret) {
        secrets.push(secret.to_string());
    }
}

pub struct RingLayer;

#[must_use]
pub fn layer() -> RingLayer {
    RingLayer
}

/// The one tracing setup both front ends use: the console the operator may be watching, and the
/// ring a bug report reads. A desktop build has no console at all, which is the whole reason the
/// ring exists.
pub fn install_tracing() -> Result<(), tracing_subscriber::util::TryInitError> {
    use tracing_subscriber::prelude::*;

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sdrmm=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .with(layer())
        .try_init()
}

impl<S: Subscriber> Layer<S> for RingLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = EventVisitor::default();
        event.record(&mut visitor);
        let metadata = event.metadata();
        LOG.push(LogLine {
            at: jiff::Timestamp::now().to_string(),
            level: level_of(*metadata.level()),
            target: metadata.target().to_string(),
            message: truncate(&redact_text(&visitor.finish()), MAX_LOG_MESSAGE_LEN),
        });
    }
}

const fn level_of(level: Level) -> LogLevel {
    match level {
        Level::ERROR => LogLevel::Error,
        Level::WARN => LogLevel::Warn,
        Level::INFO => LogLevel::Info,
        Level::DEBUG => LogLevel::Debug,
        Level::TRACE => LogLevel::Trace,
    }
}

#[derive(Default)]
struct EventVisitor {
    message: String,
    fields: String,
}

impl EventVisitor {
    fn write(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
            return;
        }
        if !self.fields.is_empty() {
            self.fields.push(' ');
        }
        self.fields.push_str(field.name());
        self.fields.push('=');
        self.fields.push_str(&redact_field(field.name(), value));
    }

    fn finish(self) -> String {
        match (self.message.is_empty(), self.fields.is_empty()) {
            (true, _) => self.fields,
            (false, true) => self.message,
            (false, false) => format!("{} {}", self.message, self.fields),
        }
    }
}

impl Visit for EventVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.write(field, value);
    }

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.write(field, &format!("{value:?}"));
    }
}

const POSITION_FIELDS: &[&str] = &[
    "lat",
    "latitude",
    "lon",
    "lng",
    "longitude",
    "alt",
    "altitude",
    "grid",
    "locator",
    "maidenhead",
];

fn redact_field<'a>(name: &str, value: &'a str) -> Cow<'a, str> {
    if POSITION_FIELDS.contains(&name) {
        return Cow::Borrowed("<redacted>");
    }
    Cow::Owned(redact_text(value))
}

/// Strips what a log line is not allowed to publish: the shared token, the operator's home
/// directory, and any address that is not the loopback.
#[must_use]
pub fn redact_text(text: &str) -> String {
    mask_addresses(&mask_home(&mask_secrets(text)))
}

fn mask_secrets(text: &str) -> String {
    let secrets = SECRETS.lock().unwrap_or_else(PoisonError::into_inner);
    secrets.iter().fold(text.to_string(), |masked, secret| {
        masked.replace(secret.as_str(), "<redacted>")
    })
}

fn mask_home(text: &str) -> String {
    match home_dir() {
        Some(home) if !home.is_empty() => text.replace(home.as_str(), "~"),
        _ => text.to_string(),
    }
}

fn home_dir() -> Option<String> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var(key)
        .ok()
        .map(|home| home.trim_end_matches(['/', '\\']).to_string())
        .filter(|home| home.len() > 1)
}

const fn address_char(ch: char) -> bool {
    ch.is_ascii_hexdigit() || ch == '.' || ch == ':'
}

fn mask_addresses(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    for ch in text.chars() {
        if address_char(ch) {
            run.push(ch);
            continue;
        }
        flush_run(&mut out, &mut run);
        out.push(ch);
    }
    flush_run(&mut out, &mut run);
    out
}

fn flush_run(out: &mut String, run: &mut String) {
    if run.is_empty() {
        return;
    }
    match mask_address(run) {
        Some(masked) => out.push_str(&masked),
        None => out.push_str(run),
    }
    run.clear();
}

fn mask_address(candidate: &str) -> Option<String> {
    if let Ok(ip) = candidate.parse::<IpAddr>() {
        return identifying(ip).then(|| "<ip>".to_string());
    }
    let (head, port) = candidate.rsplit_once(':')?;
    if port.is_empty() || !port.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let ip = head.trim_matches(['[', ']']).parse::<IpAddr>().ok()?;
    identifying(ip).then(|| format!("<ip>:{port}"))
}

fn identifying(ip: IpAddr) -> bool {
    !ip.is_loopback() && !ip.is_unspecified()
}

fn truncate(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests;
