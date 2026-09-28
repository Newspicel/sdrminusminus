use crate::AppState;

pub mod cli;
pub(crate) mod gate;
mod mdns;
mod pairing;
mod scope;
mod sessions;
mod token;

#[derive(Default)]
pub(crate) struct Phones;

pub(crate) fn spawn_flusher(_state: &AppState) {}
