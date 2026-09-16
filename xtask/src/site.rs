use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the xtask crate sits in the workspace root")
        .to_path_buf()
}

fn site_files(root: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(root.join("site"))
        .expect("read site/")
        .map(|entry| entry.expect("a site/ entry").path())
        .filter(|path| path.is_file())
        .collect();
    found.sort();
    found
}

fn pages(root: &Path) -> Vec<PathBuf> {
    site_files(root)
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "html")
        })
        .collect()
}

fn read(page: &Path) -> String {
    std::fs::read_to_string(page).unwrap_or_else(|_| panic!("read {}", page.display()))
}

fn name(page: &Path) -> String {
    page.file_name()
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
    if let Some(shot) = path.strip_prefix("screens/") {
        return root.join("assets/screenshots").join(shot);
    }
    if path == "icon.svg" {
        return root.join("assets/icon.svg");
    }
    let alongside = root.join("site").join(path);
    if alongside.is_file() {
        return alongside;
    }
    let chapter = path
        .strip_suffix(".html")
        .unwrap_or_else(|| panic!("`{path}` is neither a book page nor a file in site/"));
    root.join("docs/src").join(format!("{chapter}.md"))
}

#[test]
fn every_local_reference_resolves_to_a_file_the_build_publishes() {
    let root = root();
    let mut checked = 0;

    for page in pages(&root) {
        let html = read(&page);
        for reference in references(&html) {
            let Some(path) = reference.strip_prefix('/') else {
                continue;
            };
            let path = path.split('#').next().unwrap_or(path);
            if path.is_empty() {
                continue;
            }
            checked += 1;

            let source = source_of(&root, path);
            assert!(
                source.exists(),
                "site/{} links to /{path}, which nothing publishes ({} is missing)",
                name(&page),
                source.display()
            );
        }
    }

    assert!(checked > 20, "only {checked} local links were checked");
}

#[test]
fn the_published_host_matches_the_page_that_claims_it() {
    let root = root();
    let script =
        std::fs::read_to_string(root.join("scripts/build-site.sh")).expect("read build-site.sh");
    let published = host(&script, "printf '");

    for page in pages(&root) {
        let html = read(&page);
        assert_eq!(
            host(&html, "<link rel=\"canonical\" href=\"https://"),
            published,
            "site/{} and the CNAME the build writes name different hosts",
            name(&page)
        );
    }
}

#[test]
fn the_build_copies_every_file_kept_in_site() {
    let root = root();
    let script =
        std::fs::read_to_string(root.join("scripts/build-site.sh")).expect("read build-site.sh");

    for file in site_files(&root) {
        let extension = file
            .extension()
            .unwrap_or_else(|| panic!("site/{} has no extension", name(&file)))
            .to_string_lossy();
        assert!(
            script.contains(&format!("site/*.{extension} ")),
            "build-site.sh publishes no .{extension} from site/, so site/{} never reaches the site",
            name(&file)
        );
    }
}

#[test]
fn the_download_button_leads_to_the_download_page() {
    let root = root();
    assert!(
        root.join("site/download.html").is_file(),
        "the download page is what the Download button points at"
    );

    for page in pages(&root) {
        let html = read(&page);
        assert!(
            html.contains("class=\"get\" href=\"/download.html\""),
            "site/{} sends its Download button somewhere other than the download page",
            name(&page)
        );
    }
}
