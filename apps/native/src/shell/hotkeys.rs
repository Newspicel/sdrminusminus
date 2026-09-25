use sdrmm_wire::channel::{MAX_SQUELCH_AUTO_MARGIN_DB, MIN_SQUELCH_AUTO_MARGIN_DB, Squelch};

pub const TUNE_STEPS_HZ: [f64; 8] = [
    10.0,
    100.0,
    1_000.0,
    5_000.0,
    12_500.0,
    25_000.0,
    100_000.0,
    1_000_000.0,
];
pub const DEFAULT_STEP_HZ: f64 = 100_000.0;
pub const DEFAULT_SQUELCH_DB: f32 = -60.0;
pub const SQUELCH_RANGE_DB: (f32, f32) = (-120.0, 0.0);
pub const SQUELCH_NUDGE_DB: f32 = 2.0;
pub const ANALOG_MODES: [&str; 4] = ["nfm", "wfm", "am", "ssb"];

pub const BINDINGS: &[(&str, &str)] = &[
    ("\u{2190} \u{2192}", "Tune down / up one step"),
    ("Shift \u{2190} \u{2192}", "Tune ten steps"),
    ("[ ]", "Smaller / larger tune step"),
    (", .", "Previous / next channel"),
    ("m / M", "Cycle the channel's analog mode"),
    ("- / + =", "Squelch down / up 2 dB"),
    ("s", "Squelch on / off"),
    ("1 \u{2013} 9", "Select the nth node"),
    ("p", "Pin / unpin on the rack"),
    ("v", "Swap patch and rack"),
    ("z", "Full screen face, Esc returns"),
    ("Ctrl / \u{2318} Z", "Undo"),
    ("Ctrl / \u{2318} Shift Z", "Redo (Ctrl / \u{2318} Y too)"),
    ("Backspace", "Delete the selected node or wire"),
    ("?", "This list"),
    ("Esc", "Close an overlay or menu, or drop the selection"),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Chord {
    pub ctrl: bool,
    pub meta: bool,
    pub alt: bool,
    pub shift: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Tune(i32),
    StepBy(i32),
    CycleMode(i32),
    Squelch(f32),
    ToggleSquelch,
    SelectChannel(i32),
    SelectNode(usize),
    TogglePin,
    ToggleView,
    ToggleFull,
    Undo,
    Redo,
    ShowShortcuts,
}

#[must_use]
pub fn history_step(key: &str, chord: Chord) -> Option<Action> {
    if chord.alt || !(chord.ctrl || chord.meta) {
        return None;
    }
    match key.to_lowercase().as_str() {
        "z" if chord.shift => Some(Action::Redo),
        "z" => Some(Action::Undo),
        "y" => Some(Action::Redo),
        _ => None,
    }
}

#[must_use]
pub fn action(key: &str, chord: Chord) -> Option<Action> {
    if let Some(step) = history_step(key, chord) {
        return Some(step);
    }
    if chord.ctrl || chord.meta || chord.alt {
        return None;
    }
    let tens = if chord.shift { 10 } else { 1 };
    Some(match key {
        "ArrowLeft" => Action::Tune(-tens),
        "ArrowRight" => Action::Tune(tens),
        "[" => Action::StepBy(-1),
        "]" => Action::StepBy(1),
        "," => Action::SelectChannel(-1),
        "." => Action::SelectChannel(1),
        "m" => Action::CycleMode(if chord.shift { -1 } else { 1 }),
        "M" => Action::CycleMode(-1),
        "-" => Action::Squelch(-SQUELCH_NUDGE_DB),
        "=" | "+" => Action::Squelch(SQUELCH_NUDGE_DB),
        "s" => Action::ToggleSquelch,
        "p" => Action::TogglePin,
        "v" => Action::ToggleView,
        "z" => Action::ToggleFull,
        "?" => Action::ShowShortcuts,
        digit => {
            let index = digit
                .parse::<usize>()
                .ok()
                .filter(|n| (1..=9).contains(n))?;
            Action::SelectNode(index - 1)
        }
    })
}

#[must_use]
pub fn stepped(current: f64, direction: i32) -> f64 {
    let at = TUNE_STEPS_HZ
        .iter()
        .position(|step| (*step - current).abs() < f64::EPSILON)
        .map_or(-1, |at| at as i64);
    let next = (at + i64::from(direction)).clamp(0, TUNE_STEPS_HZ.len() as i64 - 1);
    usize::try_from(next)
        .ok()
        .and_then(|next| TUNE_STEPS_HZ.get(next).copied())
        .unwrap_or(current)
}

#[must_use]
pub fn next_analog_mode(current: &str, direction: i32) -> &'static str {
    let Some(at) = ANALOG_MODES.iter().position(|mode| *mode == current) else {
        return ANALOG_MODES[0];
    };
    let length = ANALOG_MODES.len() as i32;
    let next = (at as i32 + direction).rem_euclid(length);
    ANALOG_MODES[next as usize]
}

#[must_use]
pub fn nudged_squelch(squelch: &Squelch, delta_db: f32) -> Squelch {
    match squelch {
        Squelch::Auto { margin_db } => Squelch::Auto {
            margin_db: (margin_db + delta_db)
                .clamp(MIN_SQUELCH_AUTO_MARGIN_DB, MAX_SQUELCH_AUTO_MARGIN_DB),
        },
        Squelch::Manual { level_db } => Squelch::Manual {
            level_db: (level_db + delta_db).clamp(SQUELCH_RANGE_DB.0, SQUELCH_RANGE_DB.1),
        },
        Squelch::Off => Squelch::Manual {
            level_db: (DEFAULT_SQUELCH_DB + delta_db).clamp(SQUELCH_RANGE_DB.0, SQUELCH_RANGE_DB.1),
        },
    }
}

#[must_use]
pub fn toggled_squelch(squelch: &Squelch) -> Squelch {
    if squelch.is_off() {
        Squelch::Manual {
            level_db: DEFAULT_SQUELCH_DB,
        }
    } else {
        Squelch::Off
    }
}

#[must_use]
pub fn cycled(ids: &[String], selected: Option<&str>, direction: i32) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    let length = ids.len() as i32;
    let at = selected
        .and_then(|selected| ids.iter().position(|id| id == selected))
        .map_or(-1, |at| at as i32);
    let next = (at + direction + length).rem_euclid(length);
    ids.get(next as usize).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(ctrl: bool, meta: bool, alt: bool, shift: bool) -> Chord {
        Chord {
            ctrl,
            meta,
            alt,
            shift,
        }
    }

    #[test]
    fn takes_both_platforms_spelling_of_undo_and_redo() {
        assert_eq!(
            history_step("z", chord(false, true, false, false)),
            Some(Action::Undo)
        );
        assert_eq!(
            history_step("z", chord(true, false, false, false)),
            Some(Action::Undo)
        );
        assert_eq!(
            history_step("z", chord(false, true, false, true)),
            Some(Action::Redo)
        );
        assert_eq!(
            history_step("y", chord(true, false, false, false)),
            Some(Action::Redo)
        );
    }

    #[test]
    fn reads_the_shifted_capital_as_the_same_key() {
        assert_eq!(
            history_step("Z", chord(true, false, false, true)),
            Some(Action::Redo)
        );
        assert_eq!(
            history_step("Y", chord(false, true, false, false)),
            Some(Action::Redo)
        );
    }

    #[test]
    fn leaves_every_other_chord_alone() {
        assert_eq!(history_step("z", Chord::default()), None);
        assert_eq!(history_step("z", chord(false, true, true, false)), None);
        assert_eq!(history_step("s", chord(false, true, false, false)), None);
        assert_eq!(action("v", chord(true, false, false, false)), None);
    }

    #[test]
    fn maps_the_plain_keys() {
        let shift = chord(false, false, false, true);
        assert_eq!(
            action("ArrowLeft", Chord::default()),
            Some(Action::Tune(-1))
        );
        assert_eq!(action("ArrowRight", shift), Some(Action::Tune(10)));
        assert_eq!(action("M", shift), Some(Action::CycleMode(-1)));
        assert_eq!(action("3", Chord::default()), Some(Action::SelectNode(2)));
        assert_eq!(action("0", Chord::default()), None);
        assert_eq!(action("q", Chord::default()), None);
        assert_eq!(action("?", shift), Some(Action::ShowShortcuts));
    }

    #[test]
    fn walks_the_tune_steps_and_stops_at_the_ends() {
        assert_eq!(stepped(DEFAULT_STEP_HZ, 1), 1_000_000.0);
        assert_eq!(stepped(1_000_000.0, 1), 1_000_000.0);
        assert_eq!(stepped(10.0, -1), 10.0);
        assert_eq!(stepped(12_500.0, -1), 5_000.0);
    }

    #[test]
    fn cycles_the_analog_modes_and_starts_a_digital_one_on_nfm() {
        assert_eq!(next_analog_mode("nfm", 1), "wfm");
        assert_eq!(next_analog_mode("nfm", -1), "ssb");
        assert_eq!(next_analog_mode("ssb", 1), "nfm");
        assert_eq!(next_analog_mode("dmr", 1), "nfm");
    }

    #[test]
    fn nudges_the_squelch_within_its_range() {
        assert_eq!(
            nudged_squelch(&Squelch::Off, 2.0),
            Squelch::Manual { level_db: -58.0 }
        );
        assert_eq!(
            nudged_squelch(&Squelch::Manual { level_db: -1.0 }, 2.0),
            Squelch::Manual { level_db: 0.0 }
        );
        assert_eq!(
            nudged_squelch(&Squelch::Auto { margin_db: 3.0 }, -2.0),
            Squelch::Auto {
                margin_db: MIN_SQUELCH_AUTO_MARGIN_DB
            }
        );
        assert_eq!(
            toggled_squelch(&Squelch::Off),
            Squelch::Manual {
                level_db: DEFAULT_SQUELCH_DB
            }
        );
        assert_eq!(
            toggled_squelch(&Squelch::Auto { margin_db: 6.0 }),
            Squelch::Off
        );
    }

    #[test]
    fn cycles_through_ids_and_wraps() {
        let ids = vec!["a".to_owned(), "b".to_owned()];
        assert_eq!(cycled(&ids, None, 1).as_deref(), Some("a"));
        assert_eq!(cycled(&ids, Some("b"), 1).as_deref(), Some("a"));
        assert_eq!(cycled(&ids, Some("a"), -1).as_deref(), Some("b"));
        assert_eq!(cycled(&[], None, 1), None);
    }
}
