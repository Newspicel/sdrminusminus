use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the xtask crate sits in the workspace root")
        .to_path_buf()
}

fn references(html: &str) -> Vec<String> {
    let mut found = Vec::new();
    for attribute in ["href=\"", "src=\""] {
        let mut rest = html;
        while let Some(start) = rest.find(attribute) {
            rest = &rest[start + attribute.len()..];
            let Some(end) = rest.find('"') else { break };
            found.push(rest[..end].to_string());
            rest = &rest[end..];
        }
    }
    found
}

fn host(line_source: &str, marker: &str) -> String {
    let start = line_source
        .find(marker)
        .map(|at| at + marker.len())
        .unwrap_or_else(|| panic!("{marker} is missing"));
    line_source[start..]
        .split(|char: char| !(char.is_ascii_alphanumeric() || char == '.' || char == '-'))
        .find(|piece| !piece.is_empty())
        .expect("a host after the marker")
        .to_string()
}

#[test]
fn every_local_reference_resolves_to_a_file_the_build_publishes() {
    let root = root();
    let html = std::fs::read_to_string(root.join("site/index.html")).expect("read site/index.html");
    let mut checked = 0;

    for reference in references(&html) {
        let Some(path) = reference.strip_prefix('/') else {
            continue;
        };
        let path = path.split('#').next().unwrap_or(path);
        if path.is_empty() {
            continue;
        }
        checked += 1;

        let source = if let Some(shot) = path.strip_prefix("screens/") {
            root.join("assets/screenshots").join(shot)
        } else if path == "icon.svg" {
            root.join("assets/icon.svg")
        } else {
            let chapter = path
                .strip_suffix(".html")
                .unwrap_or_else(|| panic!("`{path}` is not a book page"));
            root.join("docs/src").join(format!("{chapter}.md"))
        };

        assert!(
            source.exists(),
            "site/index.html links to /{path}, which nothing publishes ({} is missing)",
            source.display()
        );
    }

    assert!(checked > 10, "only {checked} local links were checked");
}

#[test]
fn the_published_host_matches_the_page_that_claims_it() {
    let root = root();
    let html = std::fs::read_to_string(root.join("site/index.html")).expect("read site/index.html");
    let script =
        std::fs::read_to_string(root.join("scripts/build-site.sh")).expect("read build-site.sh");

    assert_eq!(
        host(&html, "<link rel=\"canonical\" href=\"https://"),
        host(&script, "printf '"),
        "the canonical URL and the CNAME the build writes name different hosts"
    );
}
