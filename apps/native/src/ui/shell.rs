use zgui::prelude::*;

use crate::shell::{
    auto_off::AutoOff, hotkeys::DEFAULT_STEP_HZ, prefs::PrefsFile, theme_choice::ThemeChoice,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    Workspaces,
    Library,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dialog {
    Shortcuts,
    About,
    Report(Option<String>),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LibraryTab {
    #[default]
    Templates,
    Presets,
    Bookmarks,
    Bands,
    Occupancy,
    Recordings,
    Field,
}

type Proceed = Box<dyn FnOnce()>;

#[derive(Clone, Copy)]
pub struct Shell {
    pub menu: RwSignal<Option<Menu>>,
    pub dialog: RwSignal<Option<Dialog>>,
    pub library_tab: RwSignal<LibraryTab>,
    pub step_hz: RwSignal<f64>,
    pub theme: RwSignal<ThemeChoice>,
    pub asking_auto_off: RwSignal<bool>,
    auto_off: StoredValue<AutoOff<Proceed>, LocalStorage>,
    prefs: StoredValue<PrefsFile, LocalStorage>,
}

impl Shell {
    pub fn new(prefs: PrefsFile) -> Self {
        let stored = prefs.load();
        Self {
            menu: RwSignal::new(None),
            dialog: RwSignal::new(None),
            library_tab: RwSignal::new(LibraryTab::default()),
            step_hz: RwSignal::new(DEFAULT_STEP_HZ),
            theme: RwSignal::new(ThemeChoice::parse(&stored.theme)),
            asking_auto_off: RwSignal::new(false),
            auto_off: StoredValue::new_local(AutoOff::new(stored.auto_off_explained)),
            prefs: StoredValue::new_local(prefs),
        }
    }

    #[must_use]
    pub fn current() -> Option<Self> {
        use_context::<Self>()
    }

    pub fn toggle_menu(self, menu: Menu) {
        self.menu.update(|open| {
            *open = if *open == Some(menu) {
                None
            } else {
                Some(menu)
            };
        });
    }

    pub fn open(self, dialog: Dialog) {
        self.menu.set(None);
        self.dialog.set(Some(dialog));
    }

    pub fn close(self) {
        self.dialog.set(None);
    }

    pub fn remember(self, change: impl FnOnce(&mut crate::shell::prefs::Prefs)) {
        if let Err(error) = self.prefs.with_value(|file| file.update(change)) {
            tracing::warn!(%error, "cannot save the preferences");
        }
    }

    pub fn set_theme(self, choice: ThemeChoice) {
        self.theme.set(choice);
        self.remember(|prefs| prefs.theme = choice.key().to_owned());
    }

    pub fn leave_auto(self, auto: bool, proceed: impl FnOnce() + 'static) {
        let now = self
            .auto_off
            .try_update_value(|gate| gate.leave(auto, Box::new(proceed)))
            .flatten();
        match now {
            Some(proceed) => proceed(),
            None => self.asking_auto_off.set(true),
        }
    }

    pub fn confirm_auto_off(self, never_again: bool) {
        self.asking_auto_off.set(false);
        let proceed = self
            .auto_off
            .try_update_value(|gate| gate.confirm(never_again))
            .flatten();
        if never_again {
            self.remember(|prefs| prefs.auto_off_explained = true);
        }
        if let Some(proceed) = proceed {
            proceed();
        }
    }

    pub fn cancel_auto_off(self) {
        self.asking_auto_off.set(false);
        self.auto_off.update_value(AutoOff::cancel);
    }
}

pub fn leave_auto(auto: bool, proceed: impl FnOnce() + 'static) {
    match Shell::current() {
        Some(shell) => shell.leave_auto(auto, proceed),
        None => proceed(),
    }
}

pub fn themed(shell: Shell) {
    let sheet = StoredValue::new_local(None::<Stylesheet>);
    let applied = zgui::reactive::RenderEffect::new(move |_| {
        let css = shell.theme.get().sheet();
        sheet.update_value(|held| match held {
            Some(installed) => installed.replace(&css),
            None => match Stylesheet::install("shell-theme", &css) {
                Some(installed) => *held = Some(installed),
                None => tracing::warn!("cannot install the theme"),
            },
        });
    });
    on_cleanup_local(move || drop(applied));
}
