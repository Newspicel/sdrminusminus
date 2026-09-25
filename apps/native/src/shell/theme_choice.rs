#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeChoice {
    #[default]
    System,
    Dark,
    Light,
}

pub const CYCLE: [ThemeChoice; 3] = [ThemeChoice::System, ThemeChoice::Dark, ThemeChoice::Light];

const LIGHT_TOKENS: &str = "
    --bg: oklch(0.955 0.003 240);
    --panel: oklch(0.99 0.002 240);
    --panel-2: oklch(0.935 0.005 240);
    --panel-3: oklch(0.975 0.003 240);
    --line: oklch(0.88 0.006 240);
    --line-strong: oklch(0.68 0.01 240);
    --ink: oklch(0.24 0.012 240);
    --ink-dim: oklch(0.44 0.012 240);
    --ink-faint: oklch(0.55 0.012 240);
    --accent: oklch(0.52 0.12 225);
    --accent-dim: oklch(0.64 0.1 222);
    --danger: oklch(0.51 0.19 27);
    --ok: oklch(0.5 0.115 155);
";

impl ThemeChoice {
    #[must_use]
    pub fn next(self) -> Self {
        let at = CYCLE.iter().position(|choice| *choice == self).unwrap_or(0);
        CYCLE[(at + 1) % CYCLE.len()]
    }

    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    #[must_use]
    pub fn parse(stored: &str) -> Self {
        CYCLE
            .into_iter()
            .find(|choice| choice.key() == stored)
            .unwrap_or_default()
    }

    #[must_use]
    pub fn sheet(self) -> String {
        match self {
            Self::Dark => String::new(),
            Self::Light => format!(":root:root {{{LIGHT_TOKENS}}}"),
            Self::System => {
                format!("@media (prefers-color-scheme: light) {{ :root:root {{{LIGHT_TOKENS}}} }}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_every_choice_and_comes_back() {
        let mut walked = Vec::new();
        let mut choice = ThemeChoice::System;
        for _ in 0..CYCLE.len() {
            walked.push(choice);
            choice = choice.next();
        }
        assert_eq!(walked, CYCLE);
        assert_eq!(choice, ThemeChoice::System);
    }

    #[test]
    fn reads_back_what_it_stored_and_defaults_to_the_system() {
        for choice in CYCLE {
            assert_eq!(ThemeChoice::parse(choice.key()), choice);
        }
        assert_eq!(ThemeChoice::parse("sepia"), ThemeChoice::System);
    }

    #[test]
    fn dark_is_the_base_sheet_and_the_system_follows_the_desktop() {
        assert!(ThemeChoice::Dark.sheet().is_empty());
        assert!(ThemeChoice::Light.sheet().contains("--bg"));
        assert!(
            ThemeChoice::System
                .sheet()
                .contains("prefers-color-scheme: light")
        );
    }
}
