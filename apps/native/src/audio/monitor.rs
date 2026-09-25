use std::{collections::HashMap, rc::Rc};

use sdrmm_wire::AudioRoute;

pub type PcmListener = Rc<dyn Fn(&[f32], usize)>;

#[derive(Default)]
pub struct Taps {
    next: u64,
    listeners: HashMap<AudioRoute, Vec<(u64, PcmListener)>>,
}

impl Taps {
    pub fn watch(&mut self, route: AudioRoute, listener: PcmListener) -> u64 {
        self.next += 1;
        self.listeners
            .entry(route)
            .or_default()
            .push((self.next, listener));
        self.next
    }

    pub fn unwatch(&mut self, route: &AudioRoute, id: u64) {
        if let Some(held) = self.listeners.get_mut(route) {
            held.retain(|(held, _)| *held != id);
            if held.is_empty() {
                self.listeners.remove(route);
            }
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn is_watched(&self, route: &AudioRoute) -> bool {
        self.listeners.contains_key(route)
    }

    #[must_use]
    pub fn listeners(&self, route: &AudioRoute) -> Vec<PcmListener> {
        self.listeners
            .get(route)
            .map(|held| held.iter().map(|(_, listener)| listener.clone()).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn route(channel: u32, fx: &[&str]) -> AudioRoute {
        AudioRoute {
            device_set: 1,
            channel,
            fx: fx.iter().map(|node| (*node).to_owned()).collect(),
        }
    }

    fn publish(taps: &Taps, route: &AudioRoute, pcm: &[f32], channels: usize) {
        for listener in taps.listeners(route) {
            listener(pcm, channels);
        }
    }

    #[test]
    fn delivers_a_block_to_every_watcher_of_that_channel_with_its_layout() {
        let mut taps = Taps::default();
        let seen = Rc::new(RefCell::new(Vec::new()));
        for _ in 0..2 {
            let seen = seen.clone();
            taps.watch(
                route(7, &[]),
                Rc::new(move |pcm, channels| seen.borrow_mut().push((pcm.to_vec(), channels))),
            );
        }
        publish(&taps, &route(7, &[]), &[0.5, -0.5], 2);
        assert_eq!(
            *seen.borrow(),
            vec![(vec![0.5, -0.5], 2), (vec![0.5, -0.5], 2)]
        );
    }

    #[test]
    fn keeps_channels_apart_and_stops_once_the_watcher_lets_go() {
        let mut taps = Taps::default();
        let seen = Rc::new(RefCell::new(0));
        let counter = seen.clone();
        let id = taps.watch(
            route(7, &[]),
            Rc::new(move |_, _| *counter.borrow_mut() += 1),
        );
        publish(&taps, &route(8, &[]), &[1.0], 1);
        publish(&taps, &route(7, &[]), &[1.0], 1);
        assert!(taps.is_watched(&route(7, &[])));
        taps.unwatch(&route(7, &[]), id);
        publish(&taps, &route(7, &[]), &[1.0], 1);
        assert_eq!(*seen.borrow(), 1);
        assert!(!taps.is_watched(&route(7, &[])));
    }

    #[test]
    fn stays_watched_while_any_watcher_is_left() {
        let mut taps = Taps::default();
        let first = taps.watch(route(7, &[]), Rc::new(|_, _| {}));
        taps.watch(route(7, &[]), Rc::new(|_, _| {}));
        taps.unwatch(&route(7, &[]), first);
        assert!(taps.is_watched(&route(7, &[])));
    }
}
