pub mod geiger;
pub mod jitter;
pub mod loss;
pub mod mixer;
pub mod monitor;
pub mod output;
pub mod sink;
pub mod spectrogram;

mod engine;
mod player;

use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

use sdrmm_wire::AudioRoute;
use zgui::prelude::*;

use self::{engine::Engine, monitor::Taps, sink::gain_for_volume};

pub use player::player;

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
pub const TARGET_FRAMES: usize = 4_800;
pub const MAX_FRAMES: usize = 19_200;
pub const MAX_GAP_FRAMES: u64 = 19_200;
const LATENCY_CAP_MS: f64 = 500.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Health {
    pub buffered_ms: f32,
    pub trimmed_ms: f32,
    pub lost_ms: f32,
    pub underruns: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub wanted: bool,
    pub live: bool,
    pub volume: f32,
    pub muted: bool,
    pub error: Option<String>,
    pub health: Health,
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            wanted: false,
            live: false,
            volume: 1.0,
            muted: false,
            error: None,
            health: Health::default(),
        }
    }
}

impl Entry {
    #[must_use]
    pub fn gain(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            gain_for_volume(self.volume)
        }
    }
}

type Entries = Arc<HashMap<AudioRoute, Entry>>;

#[derive(Clone, Copy)]
pub struct Audio {
    entries: RwSignal<Entries>,
    engine: StoredValue<Rc<RefCell<Engine>>, LocalStorage>,
    taps: StoredValue<Rc<RefCell<Taps>>, LocalStorage>,
}

pub fn provide() -> Audio {
    let audio = Audio {
        entries: RwSignal::new(Arc::new(HashMap::new())),
        engine: StoredValue::new_local(Rc::new(RefCell::new(Engine::default()))),
        taps: StoredValue::new_local(Rc::new(RefCell::new(Taps::default()))),
    };
    provide_context(audio);
    audio
}

#[must_use]
pub fn use_audio() -> Option<Audio> {
    use_context::<Audio>()
}

impl Audio {
    #[must_use]
    pub fn entry(self, route: &AudioRoute) -> Entry {
        self.entries
            .with(|entries| entries.get(route).cloned())
            .unwrap_or_default()
    }

    fn entry_untracked(self, route: &AudioRoute) -> Entry {
        self.entries
            .with_untracked(|entries| entries.get(route).cloned())
            .unwrap_or_default()
    }

    fn edit(self, route: &AudioRoute, change: impl FnOnce(&mut Entry)) {
        let mut next = (*self.entries.get_untracked()).clone();
        let entry = next.entry(route.clone()).or_default();
        let before = entry.clone();
        change(entry);
        if *entry != before {
            self.entries.set(Arc::new(next));
        }
    }

    fn edit_all(self, mut change: impl FnMut(&AudioRoute, &mut Entry)) {
        let mut next = (*self.entries.get_untracked()).clone();
        let mut changed = false;
        for (route, entry) in &mut next {
            let before = entry.clone();
            change(route, entry);
            changed |= *entry != before;
        }
        if changed {
            self.entries.set(Arc::new(next));
        }
    }

    pub fn play(self, route: &AudioRoute) {
        self.edit(route, |entry| {
            entry.wanted = true;
            entry.error = None;
        });
    }

    pub fn stop(self, route: &AudioRoute) {
        self.edit(route, |entry| {
            entry.wanted = false;
            entry.live = false;
        });
    }

    pub fn set_volume(self, route: &AudioRoute, volume: f32) {
        self.edit(route, |entry| entry.volume = volume.clamp(0.0, 1.0));
        self.apply_gain(route);
    }

    pub fn set_muted(self, route: &AudioRoute, muted: bool) {
        self.edit(route, |entry| entry.muted = muted);
        self.apply_gain(route);
    }

    fn apply_gain(self, route: &AudioRoute) {
        let gain = self.entry_untracked(route).gain();
        self.engine
            .try_with_value(|engine| engine.borrow_mut().set_gain(route, gain));
    }

    fn fail(self, route: &AudioRoute, error: String) {
        self.edit(route, |entry| {
            entry.wanted = false;
            entry.live = false;
            entry.error = Some(error);
        });
    }

    fn wanted(self) -> Vec<AudioRoute> {
        let mut wanted: Vec<AudioRoute> = self.entries.with(|entries| {
            entries
                .iter()
                .filter(|(_, entry)| entry.wanted)
                .map(|(route, _)| route.clone())
                .collect()
        });
        wanted.sort_by(|a, b| {
            (a.device_set, a.channel, &a.fx).cmp(&(b.device_set, b.channel, &b.fx))
        });
        wanted
    }

    pub fn watch(self, route: AudioRoute, listener: impl Fn(&[f32], usize) + 'static) {
        let taps = self.taps.get_value();
        let id = taps.borrow_mut().watch(route.clone(), Rc::new(listener));
        on_cleanup_local(move || taps.borrow_mut().unwatch(&route, id));
    }

    pub fn hold_clicks(self, strength: Signal<f32>) -> Result<(), String> {
        let engine = self.engine.get_value();
        engine
            .borrow_mut()
            .hold_clicks()
            .map_err(|error| error.to_string())?;
        let driving = {
            let engine = engine.clone();
            zgui::reactive::RenderEffect::new(move |_| {
                engine.borrow_mut().set_clicks(Some(strength.get()));
            })
        };
        on_cleanup_local(move || {
            drop(driving);
            engine.borrow_mut().release_clicks();
        });
        Ok(())
    }

    #[must_use]
    pub fn latency_ms(self, route: &AudioRoute) -> f64 {
        self.engine
            .try_with_value(|engine| engine.borrow().latency_ms(route))
            .flatten()
            .map_or(0.0, |ms| ms.clamp(0.0, LATENCY_CAP_MS))
    }
}
