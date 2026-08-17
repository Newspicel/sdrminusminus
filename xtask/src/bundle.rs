use std::path::Path;

use anyhow::{Context, Result, ensure};

const CONFIG: &str = "apps/desktop/tauri.conf.json";

pub fn check_resources(root: &Path) -> Result<()> {
    let path = root.join(CONFIG);
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let config: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    let resources = config
        .pointer("/bundle/resources")
        .and_then(serde_json::Value::as_object)
        .with_context(|| format!("{} has no bundle.resources map", path.display()))?;
    let sources: Vec<&str> = resources.keys().map(String::as_str).collect();
    if let Some((outer, inner)) = overlapping(&sources) {
        anyhow::bail!(
            "{} maps {inner} and its ancestor {outer} as separate resources. The Windows \
             bundlers key their resource table by source path, so a file reachable through two \
             mappings is installed to one of the two destinations, chosen by the hash order of \
             the map — which is how SoapySDR.dll stopped landing beside the executable.",
            path.display()
        );
    }
    ensure!(
        sources.contains(&"resources/soapy/bin"),
        "{} no longer stages the Soapy runtime libraries beside the executable, which is where \
         the Windows loader resolves the ones the binary imports",
        path.display()
    );
    let crate_dir = path.parent().context("config has no parent")?;
    for source in &sources {
        ensure!(
            source.contains('*') || crate_dir.join(source).exists(),
            "{} maps {source}, which does not exist — every desktop build resolves this map, so \
             a staged-only path has to keep a placeholder in the tree",
            path.display()
        );
    }
    println!(
        "bundle resources: {} disjoint sources in {CONFIG}",
        sources.len()
    );
    Ok(())
}

fn overlapping<'a>(sources: &[&'a str]) -> Option<(&'a str, &'a str)> {
    for outer in sources {
        for inner in sources {
            if outer != inner && inner.starts_with(&format!("{}/", outer.trim_end_matches('/'))) {
                return Some((outer, inner));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_and_a_path_inside_it_overlap() {
        assert_eq!(
            overlapping(&["resources/soapy", "resources/soapy/bin"]),
            Some(("resources/soapy", "resources/soapy/bin"))
        );
    }

    #[test]
    fn siblings_and_look_alike_prefixes_do_not() {
        assert_eq!(
            overlapping(&[
                "resources/soapy/bin",
                "resources/soapy/lib",
                "resources/soapy/licenses",
                "../../THIRD_PARTY_NOTICES.md",
            ]),
            None
        );
    }

    #[test]
    fn the_desktop_bundle_stages_every_file_exactly_once() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask has a parent");
        check_resources(root).expect("bundle resources");
    }
}
