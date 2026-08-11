//! The docs-row rule (MODEM-PLAN §5 item 8): `crates/modem/CATALOG.md` and the committed
//! measurement artifacts must move together, so the gate checks them against each other in
//! both directions. An artifact no catalog row names is a measurement nobody can find — it
//! stops gating anything the moment it is forgotten; a catalog path with no file behind it is
//! a claim with nothing backing it. Same failure shape as the toolchain pins: two things
//! updated by different hands that must change together, where divergence produces no error
//! on its own.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

const CATALOG: &str = "crates/modem/CATALOG.md";

/// The directories whose files are committed measurement artifacts. A new baselines
/// directory joins this list — that is what puts its contents under the rule.
const BASELINE_DIRS: &[&str] = &["crates/modem/baselines", "crates/channels/baselines"];

pub fn check(root: &Path) -> Result<()> {
    let catalog =
        std::fs::read_to_string(root.join(CATALOG)).with_context(|| format!("read {CATALOG}"))?;

    for dir in BASELINE_DIRS {
        for file in files_under(&root.join(dir))? {
            let rel = file.strip_prefix(root).unwrap_or(&file);
            let name = file
                .file_name()
                .and_then(|name| name.to_str())
                .with_context(|| format!("non-utf8 file name under {dir}"))?;
            ensure!(
                catalog.contains(name),
                "{} is committed but {CATALOG} never names it. Every baseline artifact needs \
                 its catalog row (MODEM-PLAN §5 item 8) — add the row, or delete the artifact \
                 if the measurement is gone.",
                rel.display()
            );
        }
    }

    for reference in referenced_paths(&catalog) {
        ensure!(
            root.join(reference).exists(),
            "{CATALOG} references `{reference}`, which does not exist. Baseline references \
             are workspace-relative paths written exactly as committed — fix the path, or \
             restore the artifact: a row pointing at nothing gates nothing."
        );
    }
    Ok(())
}

/// All regular files under `dir`, recursively, in a stable order so the first failure named
/// is always the same one. Hidden entries are skipped: a Finder `.DS_Store` dropped next to
/// the artifacts must not demand a catalog row.
fn files_under(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = std::fs::read_dir(&current)
            .with_context(|| format!("read baseline directory {}", current.display()))?;
        for entry in entries {
            let entry = entry.with_context(|| format!("read entry in {}", current.display()))?;
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Every token in the catalog that claims to be a baseline artifact. The catalog writes
/// artifact paths workspace-relative and verbatim (its own header states the convention), so
/// recognising a claim needs no markdown parsing: split prose and table punctuation away and
/// keep what contains `baselines/`. A trailing `.`, `:` or `/` is sentence punctuation or a
/// directory reference, never part of a committed file name.
fn referenced_paths(catalog: &str) -> impl Iterator<Item = &str> {
    catalog
        .split(|c: char| c.is_whitespace() || "`|()[]<>\"',;*".contains(c))
        .map(|token| token.trim_end_matches(['.', ':', '/']))
        .filter(|token| token.contains("baselines/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same call `xtask check` makes, against the committed tree — so an artifact added
    /// without its row fails plain `cargo test` too, not only the full gate.
    #[test]
    fn the_committed_tree_is_consistent() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask has a parent");
        check(root).unwrap();
    }

    /// A throwaway workspace shape under the temp dir: both baseline directories plus a
    /// catalog with the given text. Dropped with its files.
    struct Tree {
        root: PathBuf,
    }

    impl Tree {
        fn new(name: &str, catalog: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("sdrmm-xtask-catalog-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            for dir in BASELINE_DIRS {
                std::fs::create_dir_all(root.join(dir)).unwrap();
            }
            std::fs::write(root.join(CATALOG), catalog).unwrap();
            Self { root }
        }

        fn file(&self, rel: &str) {
            std::fs::write(self.root.join(rel), "{}").unwrap();
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn an_artifact_without_a_row_fails() {
        let tree = Tree::new("orphan", "# no rows here\n");
        tree.file("crates/modem/baselines/orphan.json");
        let err = check(&tree.root).expect_err("an undocumented artifact must fail the rule");
        assert!(err.to_string().contains("orphan.json"), "{err}");
    }

    #[test]
    fn a_reference_to_a_missing_artifact_fails() {
        let tree = Tree::new(
            "ghost",
            "the curve lives at `crates/modem/baselines/ghost.json`.\n",
        );
        let err = check(&tree.root).expect_err("a dangling reference must fail the rule");
        assert!(err.to_string().contains("ghost.json"), "{err}");
    }

    #[test]
    fn prose_punctuation_directories_and_hidden_files_pass() {
        let tree = Tree::new(
            "clean",
            "See `crates/modem/baselines/real.json`, kept under `crates/modem/baselines/` \
             (and | table | cells: crates/channels/baselines/dmr/real.json).\n",
        );
        tree.file("crates/modem/baselines/real.json");
        std::fs::create_dir_all(tree.root.join("crates/channels/baselines/dmr")).unwrap();
        tree.file("crates/channels/baselines/dmr/real.json");
        tree.file("crates/modem/baselines/.hidden");
        check(&tree.root).unwrap();
    }
}
