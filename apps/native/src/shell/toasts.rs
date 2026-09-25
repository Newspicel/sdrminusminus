use std::collections::VecDeque;

pub const LIFETIME_MS: u64 = 12_000;
pub const STACK_LIMIT: usize = 4;
pub const LOG_LIMIT: usize = 200;
pub const LOG_MESSAGE_LIMIT: usize = 2_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Error,
    Info,
}

impl Tone {
    #[must_use]
    pub fn level(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Info => "info",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    pub id: String,
    pub tone: Tone,
    pub message: String,
    pub code: Option<String>,
    pub repeats: u32,
    pub shown_at_ms: u64,
}

impl Toast {
    #[must_use]
    pub fn tag(&self) -> String {
        match self.tone {
            Tone::Error => self.code.clone().unwrap_or_else(|| String::from("Error")),
            Tone::Info => String::from("Note"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientEvent {
    pub at: String,
    pub level: &'static str,
    pub source: &'static str,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Toasts {
    pub shown: Vec<Toast>,
    pub log: VecDeque<ClientEvent>,
    pub dropped: usize,
}

impl Toasts {
    pub fn push(&mut self, message: &str, tone: Tone, code: Option<&str>, now_ms: u64, at: &str) {
        let id = format!("{}:{message}", tone.level());
        self.record(ClientEvent {
            at: at.to_owned(),
            level: tone.level(),
            source: "toast",
            message: match code {
                Some(code) => format!("[{code}] {message}"),
                None => message.to_owned(),
            },
        });
        if let Some(at) = self.shown.iter().position(|toast| toast.id == id) {
            let mut again = self.shown.remove(at);
            again.repeats += 1;
            again.shown_at_ms = now_ms;
            self.shown.push(again);
            return;
        }
        self.shown.push(Toast {
            id,
            tone,
            message: message.to_owned(),
            code: code.map(str::to_owned),
            repeats: 0,
            shown_at_ms: now_ms,
        });
        let overflow = self.shown.len().saturating_sub(STACK_LIMIT);
        self.shown.drain(..overflow);
    }

    pub fn record(&mut self, mut event: ClientEvent) {
        if event.message.chars().count() > LOG_MESSAGE_LIMIT {
            event.message = event.message.chars().take(LOG_MESSAGE_LIMIT).collect();
        }
        self.log.push_back(event);
        while self.log.len() > LOG_LIMIT {
            self.log.pop_front();
            self.dropped += 1;
        }
    }

    pub fn dismiss(&mut self, id: &str) {
        self.shown.retain(|toast| toast.id != id);
    }

    #[must_use]
    pub fn expired(&self, now_ms: u64) -> bool {
        self.shown
            .iter()
            .any(|toast| now_ms.saturating_sub(toast.shown_at_ms) >= LIFETIME_MS)
    }

    pub fn expire(&mut self, now_ms: u64) {
        self.shown
            .retain(|toast| now_ms.saturating_sub(toast.shown_at_ms) < LIFETIME_MS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeat_merges_into_one_toast_and_counts() {
        let mut toasts = Toasts::default();
        toasts.push("boom", Tone::Error, None, 0, "t0");
        toasts.push("boom", Tone::Error, None, 10, "t1");
        toasts.push("boom", Tone::Info, None, 20, "t2");
        assert_eq!(toasts.shown.len(), 2);
        assert_eq!(toasts.shown[0].repeats, 1);
        assert_eq!(toasts.shown[0].tag(), "Error");
        assert_eq!(toasts.shown[1].tag(), "Note");
        assert_eq!(toasts.log.len(), 3);
    }

    #[test]
    fn the_server_code_names_an_error_and_lands_in_the_log() {
        let mut toasts = Toasts::default();
        toasts.push("device busy", Tone::Error, Some("engine"), 0, "t0");
        assert_eq!(toasts.shown[0].tag(), "engine");
        assert_eq!(toasts.log[0].message, "[engine] device busy");
    }

    #[test]
    fn keeps_the_newest_few_and_lets_them_age_out() {
        let mut toasts = Toasts::default();
        for at in 0..6u64 {
            toasts.push(&format!("m{at}"), Tone::Error, None, at * 1_000, "t");
        }
        assert_eq!(toasts.shown.len(), STACK_LIMIT);
        assert_eq!(toasts.shown[0].message, "m2");
        assert!(!toasts.expired(LIFETIME_MS));
        assert!(toasts.expired(LIFETIME_MS + 2_000));
        toasts.expire(LIFETIME_MS + 3_500);
        assert_eq!(toasts.shown.len(), 2);
        toasts.dismiss("error:m5");
        assert_eq!(toasts.shown.len(), 1);
    }

    #[test]
    fn the_client_log_is_capped_and_counts_what_it_dropped() {
        let mut toasts = Toasts::default();
        for at in 0..(LOG_LIMIT + 5) {
            toasts.push(&format!("m{at}"), Tone::Info, None, 0, "t");
        }
        assert_eq!(toasts.log.len(), LOG_LIMIT);
        assert_eq!(toasts.dropped, 5);
        toasts.record(ClientEvent {
            at: String::new(),
            level: "info",
            source: "test",
            message: "x".repeat(LOG_MESSAGE_LIMIT + 9),
        });
        assert_eq!(
            toasts.log.back().map(|event| event.message.len()),
            Some(LOG_MESSAGE_LIMIT)
        );
    }
}
