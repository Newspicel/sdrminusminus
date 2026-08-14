//! `cargo xtask licenses` — the attribution a release owes, harvested from the build that
//! produces it.
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail, ensure};
use sdrmm_wire::{Attribution, ComponentSource};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Compiled into `crates/server` and served at `/api/about`.
pub const NOTICES_JSON: &str = "crates/server/data/notices.json";
/// The repository-facing rendering of the same harvest.
pub const NOTICES_MARKDOWN: &str = "THIRD_PARTY_NOTICES.md";

/// Workspace members that are not distributed, and whose dependencies are therefore nobody's
/// attribution problem. `xtask` is a build tool that no artifact contains.
const NOT_DISTRIBUTED: &[&str] = &["xtask"];

/// A license file bigger than this is not a license file. The largest real one in the tree is
/// the GPL at ~35 KB, so the cap only ever catches something that has gone wrong.
const MAX_LICENSE_BYTES: u64 = 256 * 1024;

/// Where a component needs more than its SPDX id to be understood. Keyed by component name.
///
/// Every entry here is a case where reading the id alone would leave a user with the wrong
/// idea — a separately licensed copyleft library, or a permissive license over a patented
/// algorithm. Nothing goes in this table that the id already says.
const NOTES: &[(&str, &str)] = &[
    (
        "codec2",
        "LGPL-2.1-only, statically linked into the binary. The LGPL permits this under any \
         outer license provided users can relink the executable against a modified Codec2; \
         sdr-- satisfies that by publishing its complete source, which is the \"work that uses \
         the library\" LGPL-2.1 §6 asks for.",
    ),
    (
        "blip25-vocoder",
        "MIT, but the AMBE+2 vocoder it implements is covered by patents held by Digital Voice \
         Systems, Inc. in some jurisdictions. A software license grants no patent rights it \
         does not hold — check your own position before distributing DMR voice decoding.",
    ),
    (
        "cssparser",
        "MPL-2.0. File-level copyleft: modifications to the crate's own files must be published, \
         which reaches nothing in sdr--.",
    ),
    (
        "selectors",
        "MPL-2.0. File-level copyleft: modifications to the crate's own files must be published, \
         which reaches nothing in sdr--.",
    ),
    (
        "option-ext",
        "MPL-2.0. File-level copyleft: modifications to the crate's own files must be published, \
         which reaches nothing in sdr--.",
    ),
    (
        "serialport",
        "MPL-2.0. File-level copyleft: modifications to the crate's own files must be published, \
         which reaches nothing in sdr--.",
    ),
];

/// One curated hardware-layer component.
struct Native {
    name: &'static str,
    license: &'static str,
    url: &'static str,
    note: Option<&'static str>,
    /// File names under `packaging/soapy/licenses` holding this component's text, where one is
    /// committed. Empty means the text travels only in the installer's own `soapy/licenses`.
    files: &'static [&'static str],
}

/// The hardware layer. Versions are deliberately absent: these arrive from radioconda packages
/// pinned at packaging time, and writing a version here would assert something this generator
/// cannot read.
const NATIVE: &[Native] = &[
    Native {
        name: "SoapySDR",
        license: "BSL-1.0",
        url: "https://github.com/pothosware/SoapySDR",
        note: None,
        files: &[],
    },
    Native {
        name: "SoapyRTLSDR",
        license: "BSL-1.0",
        url: "https://github.com/pothosware/SoapyRTLSDR",
        note: None,
        files: &[],
    },
    Native {
        name: "rtl-sdr (librtlsdr)",
        license: "GPL-2.0-or-later",
        url: "https://gitea.osmocom.org/sdr/rtl-sdr",
        note: Some(
            "Shipped in installers and container images as a SoapySDR module, loaded at runtime \
             through SoapySDR's own plugin API. sdr-- neither links it nor derives from it, so \
             the GPL applies to that library and not to this product.",
        ),
        files: &[],
    },
    Native {
        name: "SoapyHackRF",
        license: "MIT",
        url: "https://github.com/pothosware/SoapyHackRF",
        note: None,
        files: &["SoapyHackRF-MIT.txt"],
    },
    Native {
        name: "hackrf (libhackrf)",
        license: "GPL-2.0-or-later",
        url: "https://github.com/greatscottgadgets/hackrf",
        note: Some(
            "Loaded at runtime as a SoapySDR module, on the same terms as librtlsdr. The public \
             API declarations in `hackrf.h` are BSD-3-Clause.",
        ),
        files: &["HackRF-GPL-2.0-or-later.txt", "HackRF-BSD-3-Clause.txt"],
    },
    Native {
        name: "SoapySDRPlay3",
        license: "MIT",
        url: "https://github.com/pothosware/SoapySDRPlay3",
        note: Some(
            "Not bundled. The SDRplay API it needs is commercial software licensed for use with \
             genuine SDRplay hardware, so operators install the vendor API and this module \
             themselves.",
        ),
        files: &[],
    },
    Native {
        name: "Airspy, AirspyHF, bladeRF, LimeSuite, libiio/PlutoSDR, SoapyRemote",
        license: "See the bundled package metadata",
        url: "https://github.com/pothosware",
        note: Some(
            "Resolved from platform packages at packaging time, so the exact versions and \
             licenses are whatever each installer pinned. `packaging/soapy/stage-unix.sh` copies \
             every one of their license texts and package manifests into `soapy/licenses` inside \
             the bundle; that directory, not this row, is the authoritative record for a given \
             release.",
        ),
        files: &[],
    },
];

#[derive(Debug, Serialize)]
struct NoticesDocument {
    license: String,
    license_text: String,
    repository: String,
    components: Vec<Attribution>,
    /// Content-addressed license texts, shared across every component that ships an identical
    /// copy. Several hundred crates offer Apache-2.0 byte-for-byte; each MIT copy differs in
    /// its copyright line and stays distinct.
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
    Ok(())
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
        "harvested no components at all — the generator is broken, and committing this would \
         replace the notices with an empty file"
    );

    components.sort_by(|a, b| {
        order(a.source)
            .cmp(&order(b.source))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.version.cmp(&b.version))
    });
    // Native rows carry their own notes from [`NATIVE`]; [`NOTES`] annotates the harvested
    // rows, which have none of their own. Assigning rather than overwriting keeps the two
    // tables from silently erasing each other.
    for component in &mut components {
        if let Some((_, note)) = NOTES.iter().find(|(name, _)| *name == component.name) {
            component.note = Some((*note).to_string());
        }
    }

    let license_text = std::fs::read_to_string(root.join("LICENSE")).context("read LICENSE")?;
    Ok(NoticesDocument {
        license: "GPL-3.0-or-later".to_string(),
        license_text: normalize(&license_text),
        repository: "https://github.com/newspicel/sdrminusminus".to_string(),
        components,
        texts: pool.texts,
    })
}

const fn order(source: ComponentSource) -> u8 {
    match source {
        ComponentSource::Rust => 0,
        ComponentSource::Web => 1,
        ComponentSource::Native => 2,
    }
}

/// License texts, interned by content so identical copies are stored once.
#[derive(Default)]
struct TextPool {
    texts: BTreeMap<String, String>,
}

impl TextPool {
    /// Returns the id the text is addressed by, or `None` if there is no text to address.
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

/// Line endings and trailing whitespace are checkout artifacts, not license content. Normalizing
/// them is what stops a Windows clone from hashing every text differently and rewriting the
/// whole document.
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
        // A non-UTF-8 "license" is not a license text; skipping it is more honest than
        // lossily transcoding somebody's copyright line.
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
    /// `null` for a normal dependency, `"dev"` or `"build"` otherwise.
    kind: Option<String>,
}

fn rust_components(root: &Path, pool: &mut TextPool) -> Result<Vec<Attribution>> {
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
    let metadata: Metadata =
        serde_json::from_slice(&output.stdout).context("parse `cargo metadata` output")?;

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
            // A crate pulled in only as a dev-dependency ships nowhere. One that is *also* a
            // normal or build dependency does, so the test is "every edge is dev", not "any".
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
        let dir = package
            .manifest_path
            .parent()
            .with_context(|| format!("{} has no manifest directory", package.name))?;
        components.push(Attribution {
            name: package.name.clone(),
            version: Some(package.version.clone()),
            license: package
                .license
                .clone()
                .unwrap_or_else(|| "See the crate's own license file".to_string()),
            source: ComponentSource::Rust,
            url: package.repository.clone(),
            texts: license_files(dir, pool)?,
            note: None,
        });
    }
    Ok(components)
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

/// The npm packages bundled into the built UI.
///
/// `--prod` is the whole point: Vite, Biome and Playwright are how the bundle is built and
/// checked, not what is in it, and listing them would pad the notices with a few hundred
/// packages no user ever receives.
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
            note: None,
        });
    }
    Ok(components)
}

/// The repository-facing rendering: what a reader with no binary to run gets.
///
/// The noted components come first and the bulk tables after, because the ordering is the
/// message. Six hundred permissive rows say nothing a reader needs to act on; the handful with
/// a copyleft obligation or a patent behind them do, and burying those in alphabetical order
/// among the rest is how a notices file becomes something nobody reads.
fn markdown(document: &NoticesDocument) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    out.push_str(
        "# Third-party notices\n\n\
         <!-- Generated by `cargo xtask licenses`. Do not edit by hand: `cargo xtask check` \
         regenerates this file and fails on any difference. -->\n\n\
         sdr-- itself is licensed under the GNU General Public License, version 3 or later — \
         see [`LICENSE`](LICENSE).\n\n\
         This file lists every third-party component a release distributes: crates compiled into \
         the binaries, npm packages bundled into the web UI, and the SoapySDR hardware libraries \
         shipped alongside them. Dev-only tooling is excluded, because a test harness and a \
         bundler are how a release is built rather than part of one.\n\n\
         The full license texts are distributed with the software, not merely referenced by it. \
         They are compiled into the server and readable in the app under **About**, served at \
         `GET /api/about`, and stored in \
         [`crates/server/data/notices.json`](crates/server/data/notices.json). Installers \
         additionally carry each hardware package's own texts and manifests in \
         `soapy/licenses`.\n\n",
    );

    let noted: Vec<&Attribution> = document
        .components
        .iter()
        .filter(|component| component.note.is_some())
        .collect();
    if !noted.is_empty() {
        out.push_str(
            "## Components that need more than their SPDX id\n\n\
             Everything else in this file is a permissive license that asks only for \
             attribution, which the notices above provide. These do not.\n\n",
        );
        for component in noted {
            let note = component.note.as_deref().unwrap_or_default();
            let _ = writeln!(
                out,
                "**{}** — {}\n\n{note}\n",
                component.name, component.license
            );
        }
    }

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
                row.version.as_deref().unwrap_or("—"),
                row.license
            );
        }
        out.push('\n');
    }
    out
}

/// The curated hardware layer, with the license texts that are committed for it.
///
/// Each row takes only its own texts. Attaching the whole directory to every row would make
/// the HackRF GPL look like it covers SoapySDR, which is the single most misleading thing this
/// file could say.
fn native_components(root: &Path, pool: &mut TextPool) -> Result<Vec<Attribution>> {
    let dir = root.join("packaging/soapy/licenses");
    let mut components = Vec::new();
    for native in NATIVE {
        let mut texts = Vec::new();
        for file in native.files {
            let path = dir.join(file);
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("read {} for {}", path.display(), native.name))?;
            let id = pool.intern(&text).with_context(|| {
                format!(
                    "{} is empty — {} would ship with no notice",
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
            note: native.note.map(str::to_string),
        });
    }
    Ok(components)
}
