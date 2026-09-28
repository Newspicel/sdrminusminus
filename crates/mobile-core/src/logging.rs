use std::{
    fmt::Write as _,
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

use tracing::{Event, Level, Metadata, Subscriber, field::Field};
use tracing_subscriber::{
    Layer,
    layer::{Context, SubscriberExt as _},
};

const OWN_TARGET: &str = "sdrmm";

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}

#[uniffi::export(foreign)]
pub trait LogListener: Send + Sync {
    fn on_log(&self, level: LogLevel, target: String, message: String);
}

static LISTENER: Mutex<Option<Arc<dyn LogListener>>> = Mutex::new(None);
static INSTALLED: OnceLock<bool> = OnceLock::new();

pub(crate) fn install() -> bool {
    *INSTALLED.get_or_init(|| {
        tracing::subscriber::set_global_default(tracing_subscriber::registry().with(Bridge)).is_ok()
    })
}

pub(crate) fn set_listener(listener: Option<Arc<dyn LogListener>>) {
    *LISTENER.lock().unwrap_or_else(PoisonError::into_inner) = listener;
}

fn listener() -> Option<Arc<dyn LogListener>> {
    LISTENER
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

fn level(metadata: &Metadata<'_>) -> Option<LogLevel> {
    match *metadata.level() {
        Level::ERROR => Some(LogLevel::Error),
        Level::WARN => Some(LogLevel::Warn),
        Level::INFO => Some(LogLevel::Info),
        Level::DEBUG if metadata.target().starts_with(OWN_TARGET) => Some(LogLevel::Debug),
        _ => None,
    }
}

struct Bridge;

impl<S: Subscriber> Layer<S> for Bridge {
    fn enabled(&self, metadata: &Metadata<'_>, _context: Context<'_, S>) -> bool {
        level(metadata).is_some()
    }

    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let Some(level) = level(event.metadata()) else {
            return;
        };
        let Some(listener) = listener() else {
            return;
        };
        let mut message = Message::default();
        event.record(&mut message);
        listener.on_log(level, event.metadata().target().to_owned(), message.text);
    }
}

#[derive(Default)]
struct Message {
    text: String,
}

impl tracing::field::Visit for Message {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.append(field, format_args!("{value}"));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.append(field, format_args!("{value:?}"));
    }
}

impl Message {
    fn append(&mut self, field: &Field, value: std::fmt::Arguments<'_>) {
        if !self.text.is_empty() {
            self.text.push(' ');
        }
        let _ = if field.name() == "message" {
            write!(self.text, "{value}")
        } else {
            write!(self.text, "{}={value}", field.name())
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Collect {
        lines: Mutex<Vec<(LogLevel, String, String)>>,
    }

    impl LogListener for Collect {
        fn on_log(&self, level: LogLevel, target: String, message: String) {
            self.lines
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((level, target, message));
        }
    }

    impl Collect {
        fn find(&self, marker: &str) -> Vec<(LogLevel, String, String)> {
            self.lines
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .filter(|(_, _, message)| message.contains(marker))
                .cloned()
                .collect()
        }
    }

    #[test]
    fn the_listener_gets_core_logs_with_their_fields() {
        assert!(install());
        let collect = Arc::new(Collect::default());
        set_listener(Some(collect.clone()));
        tracing::warn!(target: "sdrmm_mobile_core::link", attempt = 3, "marker-7a retry");
        tracing::debug!(target: "sdrmm_mobile_core::link", "marker-7a detail");
        tracing::debug!(target: "hyper::proto", "marker-7a noise");
        tracing::trace!(target: "sdrmm_mobile_core::link", "marker-7a trace");
        assert_eq!(
            collect.find("marker-7a"),
            vec![
                (
                    LogLevel::Warn,
                    "sdrmm_mobile_core::link".to_owned(),
                    "marker-7a retry attempt=3".to_owned()
                ),
                (
                    LogLevel::Debug,
                    "sdrmm_mobile_core::link".to_owned(),
                    "marker-7a detail".to_owned()
                ),
            ]
        );
    }
}
