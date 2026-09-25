use std::path::PathBuf;

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub theme: String,
    pub auto_off_explained: bool,
    pub token: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PrefsFile {
    path: PathBuf,
}

impl PrefsFile {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    #[must_use]
    pub fn load(&self) -> Prefs {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
                tracing::warn!(%error, path = %self.path.display(), "unreadable preferences, starting fresh");
                Prefs::default()
            }),
            Err(_) => Prefs::default(),
        }
    }

    pub fn save(&self, prefs: &Prefs) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(prefs).context("cannot write the preferences")?;
        std::fs::write(&self.path, text)
            .with_context(|| format!("cannot save {}", self.path.display()))
    }

    pub fn update(&self, change: impl FnOnce(&mut Prefs)) -> anyhow::Result<Prefs> {
        let mut prefs = self.load();
        change(&mut prefs);
        self.save(&prefs)?;
        Ok(prefs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_reads_as_the_defaults_and_a_saved_one_reads_back() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let file = PrefsFile::new(dir.path().join("nested").join("prefs.json"));
        assert_eq!(file.load(), Prefs::default());
        let saved = file
            .update(|prefs| {
                prefs.theme = "light".to_owned();
                prefs.auto_off_explained = true;
            })
            .expect("saved");
        assert_eq!(file.load(), saved);
        assert_eq!(file.load().token, None);
    }

    #[test]
    fn a_damaged_file_starts_fresh() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("prefs.json");
        std::fs::write(&path, "{not json").expect("written");
        assert_eq!(PrefsFile::new(path).load(), Prefs::default());
    }
}
