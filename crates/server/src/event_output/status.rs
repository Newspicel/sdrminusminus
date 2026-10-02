use std::collections::HashMap;

use sdrmm_engine::Engine;
use sdrmm_wire::{EventOutputTarget, ServerEvent, event_output::EventOutputStatus};
use tokio::sync::watch;

use super::Binding;

struct Entry {
    target: EventOutputTarget,
    status: EventOutputStatus,
}

#[derive(Clone)]
pub(super) struct Statuses {
    entries: watch::Sender<HashMap<String, Entry>>,
}

impl Default for Statuses {
    fn default() -> Self {
        Self {
            entries: watch::Sender::new(HashMap::new()),
        }
    }
}

impl Statuses {
    pub fn configure(&self, bindings: &[Binding]) {
        self.entries.send_modify(|entries| {
            entries.retain(|node, entry| {
                bindings
                    .iter()
                    .any(|binding| binding.node == *node && binding.target == entry.target)
            });
            for binding in bindings.iter().filter(|binding| reports(&binding.target)) {
                entries
                    .entry(binding.node.clone())
                    .or_insert_with(|| Entry {
                        target: binding.target.clone(),
                        status: EventOutputStatus {
                            node: binding.node.clone(),
                            ..EventOutputStatus::default()
                        },
                    });
            }
        });
    }

    pub fn delivered(&self, node: &str, count: usize) {
        self.update(node, |status| {
            status.delivered = status.delivered.saturating_add(count as u64);
            status.error = None;
        });
    }

    pub fn failed(&self, node: &str, count: usize, error: String) {
        self.update(node, |status| {
            status.failed = status.failed.saturating_add(count as u64);
            status.error = Some(error);
        });
    }

    pub fn lost(&self, count: u64) {
        self.entries.send_modify(|entries| {
            for entry in entries.values_mut() {
                entry.status.error = Some(format!("Missed {count} decoder events"));
            }
        });
    }

    pub fn publish(&self, engine: &Engine) {
        for entry in self.entries.borrow().values() {
            engine.emit_event(ServerEvent::EventOutputStatus(entry.status.clone()));
        }
    }

    #[cfg(test)]
    pub fn get(&self, node: &str) -> Option<EventOutputStatus> {
        self.entries
            .borrow()
            .get(node)
            .map(|entry| entry.status.clone())
    }

    fn update(&self, node: &str, change: impl FnOnce(&mut EventOutputStatus)) {
        self.entries.send_if_modified(|entries| {
            entries
                .get_mut(node)
                .map(|entry| change(&mut entry.status))
                .is_some()
        });
    }
}

fn reports(target: &EventOutputTarget) -> bool {
    !matches!(target, EventOutputTarget::Beast { .. })
}
