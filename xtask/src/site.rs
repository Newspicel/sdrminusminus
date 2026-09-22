use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the xtask crate sits in the workspace root")
        .to_path_buf()
}

fn astro_files(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|_| panic!("read {}", dir.display())) {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            astro_files(&path, found);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "astro")
        {
            found.push(path);
        }
    }
}

fn sources(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    astro_files(&root.join("site/src"), &mut found);
    found.sort();
    found
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|_| panic!("read {}", path.display()))
}

fn name(path: &Path) -> String {
    path.file_name()
        .expect("a file name")
        .to_string_lossy()
        .into_owned()
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

fn source_of(root: &Path, path: &str) -> PathBuf {
    let public = root.join("site/public").join(path);
    if public.exists() {
        return public;
    }
    let stem = path
        .strip_suffix(".html")
        .unwrap_or_else(|| panic!("`{path}` is neither a page nor a file in site/public"));
    let page = root.join("site/src/pages").join(format!("{stem}.astro"));
    if page.is_file() {
        return page;
    }
    root.join("docs/src").join(format!("{stem}.md"))
}

#[test]
fn every_local_reference_resolves_to_a_file_the_build_publishes() {
    let root = root();
    let mut checked = 0;

    for source in sources(&root) {
        for reference in references(&read(&source)) {
            let Some(path) = reference.strip_prefix('/') else {
                continue;
            };
            let path = path.split(['#', '?']).next().unwrap_or(path);
            if path.is_empty() {
                continue;
            }
            checked += 1;

            let target = source_of(&root, path);
            assert!(
                target.exists(),
                "site/{} links to /{path}, which nothing publishes ({} is missing)",
                name(&source),
                target.display()
            );
        }
    }

    assert!(checked > 20, "only {checked} local links were checked");
}

#[test]
fn the_published_host_matches_the_site_astro_builds() {
    let root = root();
    let script = read(&root.join("scripts/build-site.sh"));
    let config = read(&root.join("site/astro.config.mjs"));
    assert_eq!(
        host(&config, "site: \"https://"),
        host(&script, "printf '"),
        "site/astro.config.mjs and the CNAME the build writes name different hosts"
    );
}

#[test]
fn the_download_button_leads_to_the_download_page() {
    let root = root();
    assert!(
        root.join("site/src/pages/download.astro").is_file(),
        "the download page is what the Download button points at"
    );
    assert!(
        read(&root.join("site/src/layouts/Page.astro"))
            .contains("class=\"get\" href=\"/download.html\""),
        "the page layout sends its Download button somewhere other than the download page"
    );
}

#[test]
fn every_demo_scene_has_a_recording() {
    let root = root();
    let scenes = read(&root.join("site/src/demo/scenes.ts"));
    let mut checked = 0;
    let mut rest = scenes.as_str();
    while let Some(start) = rest.find("{ id: \"") {
        rest = &rest[start + "{ id: \"".len()..];
        let id = &rest[..rest.find('"').expect("a closing quote")];
        checked += 1;
        assert!(
            root.join("site/public/demo")
                .join(format!("{id}.json"))
                .is_file(),
            "the {id} demo has no recording; run `pnpm --dir web demo:record`"
        );
    }
    assert!(checked > 0, "no demo scenes were found");
}
