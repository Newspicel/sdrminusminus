use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail, ensure};
use sdrmm_wire::{Attribution, ComponentSource};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod android;

pub(crate) use android::{ANDROID_NOTICES, notices as android_notices};

pub const NOTICES_JSON: &str = "crates/server/data/notices.json";
pub const NOTICES_MARKDOWN: &str = "THIRD_PARTY_NOTICES.md";
pub const MOBILE_NOTICES_JSON: &str = "crates/mobile-core/data/notices.json";

const NOT_DISTRIBUTED: &[&str] = &["xtask"];

const MAX_LICENSE_BYTES: u64 = 256 * 1024;

struct Native {
    name: &'static str,
    license: &'static str,
    url: &'static str,
    files: &'static [&'static str],
}

const NATIVE: &[Native] = &[
    Native {
        name: "DPDFNet",
        license: "Apache-2.0",
        url: "https://github.com/ceva-ip/DPDFNet",
        files: &[],
    },
    Native {
        name: "FFmpeg 9.0.1",
        license: "LGPL-2.1-or-later",
        url: "https://ffmpeg.org/",
        files: &["FFmpeg-LGPL-2.1.txt"],
    },
    Native {
        name: "SoapySDR",
        license: "BSL-1.0",
        url: "https://github.com/pothosware/SoapySDR",
        files: &[],
    },
    Native {
        name: "libairspy",
        license: "BSD-3-Clause",
        url: "https://github.com/airspy/airspyone_host",
        files: &["libairspy-BSD-3-Clause.txt"],
    },
    Native {
        name: "libairspyhf",
        license: "BSD-3-Clause",
        url: "https://github.com/airspy/airspyhf",
        files: &["libairspyhf-BSD-3-Clause.txt"],
    },
    Native {
        name: "hackrf-nusb 0.3.0",
        license: "MIT OR Apache-2.0",
        url: "https://github.com/bastibl/hackrf-nusb",
        files: &["hackrf-nusb-MIT.txt"],
    },
    Native {
        name: "hackrf.h (libhackrf API)",
        license: "BSD-3-Clause",
        url: "https://github.com/greatscottgadgets/hackrf",
        files: &["HackRF-BSD-3-Clause.txt"],
    },
    Native {
        name: "dmrconfig",
        license: "BSD-3-Clause",
        url: "https://github.com/OpenRTX/dmrconfig",
        files: &["BSD-3-Clause-dmrconfig.txt"],
    },
    Native {
        name: "mbelib",
        license: "ISC",
        url: "https://github.com/szechyjs/mbelib",
        files: &["mbelib-ISC.txt"],
    },
    Native {
        name: "codec2 FDMDV modem",
        license: "LGPL-2.1-only",
        url: "https://github.com/drowe67/codec2",
        files: &["codec2-LGPL-2.1.txt"],
    },
    Native {
        name: "xng",
        license: "MIT OR Apache-2.0",
        url: "https://github.com/airframesio/xng",
        files: &["xng-MIT.txt"],
    },
];

#[derive(Debug, Serialize)]
struct NoticesDocument {
    license: String,
    license_text: String,
    repository: String,
    components: Vec<Attribution>,
    texts: BTreeMap<String, String>,
}

pub fn run(root: &Path, pnpm: &str) -> Result<()> {
    let document = harvest(root, pnpm)?;

    let json = serde_json::to_string_pretty(&document).context("serialize notices")?;
    write(&root.join(NOTICES_JSON), &format!("{json}\n"))?;
    write(&root.join(NOTICES_MARKDOWN), &markdown(&document))?;
    println!(
        "notices: {} components, {} license texts",
        document.components.len(),
        document.texts.len()
    );
    write(&root.join(MOBILE_NOTICES_JSON), &mobile_notices(root)?)?;
    write(&root.join(ANDROID_NOTICES), &android_notices(root)?)?;
    Ok(())
}

pub(crate) fn mobile_notices(root: &Path) -> Result<String> {
    let shipped = crate::mobile::phone_packages(root)?;
    let metadata = cargo_metadata(root)?;
    let members: HashSet<&str> = metadata
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect();
    let mut pool = TextPool::default();
    let mut components = Vec::new();
    for package in &metadata.packages {
        if members.contains(package.id.as_str())
            || !shipped.contains(&(package.name.clone(), package.version.clone()))
        {
            continue;
        }
        components.push(rust_attribution(package, &mut pool)?);
    }
    ensure!(
        !components.is_empty(),
        "the phone build has no dependencies at all: the generator is broken"
    );
    components.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.version.cmp(&b.version))
    });
    let document = NoticesDocument {
        components,
        texts: pool.texts,
        ..own_license(root)?
    };
    let json = serde_json::to_string_pretty(&document).context("serialize mobile notices")?;
    Ok(format!("{json}\n"))
}

fn own_license(root: &Path) -> Result<NoticesDocument> {
    let license_text = std::fs::read_to_string(root.join("LICENSE")).context("read LICENSE")?;
    Ok(NoticesDocument {
        license: "AGPL-3.0-or-later".to_string(),
        license_text: normalize(&license_text),
        repository: "https://github.com/newspicel/sdrmm".to_string(),
        components: Vec::new(),
        texts: BTreeMap::new(),
    })
}

fn write(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(path, contents).with_context(|| format!("write {}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}

fn harvest(root: &Path, pnpm: &str) -> Result<NoticesDocument> {
    let mut pool = TextPool::default();
    let mut components = Vec::new();
    components.extend(rust_components(root, &mut pool)?);
    components.extend(web_components(root, pnpm, &mut pool)?);
    components.extend(native_components(root, &mut pool)?);

    ensure!(
        !components.is_empty(),
        "harvested no components at all: the generator is broken, and committing this would \
         replace the notices with an empty file"
    );

    components.sort_by(|a, b| {
        order(a.source)
            .cmp(&order(b.source))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.version.cmp(&b.version))
    });

    Ok(NoticesDocument {
        components,
        texts: pool.texts,
        ..own_license(root)?
    })
}

const fn order(source: ComponentSource) -> u8 {
    match source {
        ComponentSource::Rust => 0,
        ComponentSource::Web => 1,
        ComponentSource::Native => 2,
    }
}

#[derive(Default)]
struct TextPool {
    texts: BTreeMap<String, String>,
}

impl TextPool {
    fn intern(&mut self, text: &str) -> Option<String> {
        let text = normalize(text);
        if text.is_empty() {
            return None;
        }
        let id: String = Sha256::digest(text.as_bytes())
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.texts.entry(id.clone()).or_insert(text);
        Some(id)
    }
}

fn normalize(text: &str) -> String {
    let mut out = text.replace("\r\n", "\n");
    out.truncate(out.trim_end().len());
    out
}

fn license_files(dir: &Path, pool: &mut TextPool) -> Result<Vec<String>> {
    const PREFIXES: &[&str] = &["license", "licence", "copying", "unlicense", "notice"];

    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("read {}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if !PREFIXES.iter().any(|prefix| name.starts_with(prefix)) {
            continue;
        }
        let metadata = entry
            .metadata()
            .with_context(|| format!("stat {}", entry.path().display()))?;
        if !metadata.is_file() || metadata.len() > MAX_LICENSE_BYTES {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(entry.path())
            && let Some(id) = pool.intern(&text)
        {
            ids.push(id);
        }
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

#[derive(Debug, Deserialize)]
struct Metadata {
    packages: Vec<MetaPackage>,
    workspace_members: Vec<String>,
    resolve: MetaResolve,
}

#[derive(Debug, Deserialize)]
struct MetaPackage {
    id: String,
    name: String,
    version: String,
    license: Option<String>,
    repository: Option<String>,
    manifest_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct MetaResolve {
    nodes: Vec<MetaNode>,
}

#[derive(Debug, Deserialize)]
struct MetaNode {
    id: String,
    deps: Vec<MetaDep>,
}

#[derive(Debug, Deserialize)]
struct MetaDep {
    pkg: String,
    dep_kinds: Vec<MetaDepKind>,
}

#[derive(Debug, Deserialize)]
struct MetaDepKind {
    kind: Option<String>,
}

fn cargo_metadata(root: &Path) -> Result<Metadata> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--all-features"])
        .current_dir(root)
        .output()
        .context("failed to spawn `cargo metadata`")?;
    if !output.status.success() {
        bail!(
            "`cargo metadata` failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout).context("parse `cargo metadata` output")
}

fn rust_components(root: &Path, pool: &mut TextPool) -> Result<Vec<Attribution>> {
    let metadata = cargo_metadata(root)?;

    let packages: HashMap<&str, &MetaPackage> = metadata
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect();
    let nodes: HashMap<&str, &MetaNode> = metadata
        .resolve
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();

    let members: HashSet<&str> = metadata
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect();
    let roots: Vec<&str> = members
        .iter()
        .copied()
        .filter(|id| {
            packages
                .get(id)
                .is_none_or(|package| !NOT_DISTRIBUTED.contains(&package.name.as_str()))
        })
        .collect();

    let mut seen: HashSet<&str> = HashSet::new();
    let mut stack = roots;
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(node) = nodes.get(id) else { continue };
        for dep in &node.deps {
            if dep
                .dep_kinds
                .iter()
                .all(|kind| kind.kind.as_deref() == Some("dev"))
            {
                continue;
            }
            stack.push(dep.pkg.as_str());
        }
    }

    let mut components = Vec::new();
    for id in seen {
        if members.contains(id) {
            continue;
        }
        let Some(package) = packages.get(id) else {
            continue;
        };
        components.push(rust_attribution(package, pool)?);
    }
    Ok(components)
}

fn rust_attribution(package: &MetaPackage, pool: &mut TextPool) -> Result<Attribution> {
    let dir = package
        .manifest_path
        .parent()
        .with_context(|| format!("{} has no manifest directory", package.name))?;
    Ok(Attribution {
        name: package.name.clone(),
        version: Some(package.version.clone()),
        license: package
            .license
            .clone()
            .unwrap_or_else(|| "See the crate's own license file".to_string()),
        source: ComponentSource::Rust,
        url: package.repository.clone(),
        texts: license_files(dir, pool)?,
    })
}

#[derive(Debug, Deserialize)]
struct PnpmPackage {
    name: String,
    #[serde(default)]
    versions: Vec<String>,
    #[serde(default)]
    paths: Vec<PathBuf>,
    license: String,
    #[serde(default)]
    homepage: Option<String>,
}

fn web_components(root: &Path, pnpm: &str, pool: &mut TextPool) -> Result<Vec<Attribution>> {
    let output = Command::new(pnpm)
        .args(["--dir", "web", "licenses", "list", "--json", "--prod"])
        .current_dir(root)
        .output()
        .with_context(|| format!("failed to spawn `{pnpm}` (is pnpm installed?)"))?;
    if !output.status.success() {
        bail!(
            "`{pnpm} licenses list` failed with {}: {}\nRun `pnpm --dir web install` first.",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let listing: BTreeMap<String, Vec<PnpmPackage>> =
        serde_json::from_slice(&output.stdout).context("parse `pnpm licenses list` output")?;

    let mut components = Vec::new();
    for package in listing.into_values().flatten() {
        let mut texts = Vec::new();
        for path in &package.paths {
            texts.extend(license_files(path, pool)?);
        }
        texts.sort_unstable();
        texts.dedup();
        components.push(Attribution {
            name: package.name,
            version: (!package.versions.is_empty()).then(|| package.versions.join(", ")),
            license: package.license,
            source: ComponentSource::Web,
            url: package.homepage,
            texts,
        });
    }
    Ok(components)
}

fn markdown(document: &NoticesDocument) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    out.push_str(
        "# Third-party notices\n\n\
         <!-- Generated by `cargo xtask licenses`. -->\n\n\
         SDR-- is licensed under AGPL-3.0-or-later: see [`LICENSE`](LICENSE).\n\n",
    );

    for source in [
        ComponentSource::Rust,
        ComponentSource::Web,
        ComponentSource::Native,
    ] {
        let rows: Vec<&Attribution> = document
            .components
            .iter()
            .filter(|component| component.source == source)
            .collect();
        if rows.is_empty() {
            continue;
        }
        let _ = writeln!(out, "## {} ({})\n", source.label(), rows.len());
        out.push_str("| Component | Version | License |\n| --- | --- | --- |\n");
        for row in rows {
            let name = match &row.url {
                Some(url) => format!("[{}]({url})", row.name),
                None => row.name.clone(),
            };
            let _ = writeln!(
                out,
                "| {name} | {} | {} |",
                row.version.as_deref().unwrap_or("-"),
                row.license
            );
        }
        out.push('\n');
    }
    out
}

fn native_components(root: &Path, pool: &mut TextPool) -> Result<Vec<Attribution>> {
    let dir = root.join("packaging/licenses");
    let mut components = Vec::new();
    for native in NATIVE {
        let mut texts = Vec::new();
        for file in native.files {
            let path = dir.join(file);
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("read {} for {}", path.display(), native.name))?;
            let id = pool.intern(&text).with_context(|| {
                format!(
                    "{} is empty: {} would ship with no notice",
                    path.display(),
                    native.name
                )
            })?;
            texts.push(id);
        }
        texts.sort_unstable();
        texts.dedup();
        components.push(Attribution {
            name: native.name.to_string(),
            version: None,
            license: native.license.to_string(),
            source: ComponentSource::Native,
            url: Some(native.url.to_string()),
            texts,
        });
    }
    Ok(components)
}
