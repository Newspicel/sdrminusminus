//! `latest.json` — the static manifest the Tauri updater polls (PLAN §15).
//!
//! Built from the `.sig` files the bundler emits beside each updater artifact rather than from
//! a list of expected names: the signature is what the client verifies, so a platform whose
//! signature never reached the release has nothing to offer and must not appear here.
//!
//! Every rule below is strict on purpose. This file is what decides whether an installed client
//! sees a release at all, and both failure modes are silent from CI's point of view — an
//! unrecognised artifact name would drop a platform, and a duplicate would pick one at random.

use std::{
    collections::{BTreeMap, btree_map::Entry},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::Serialize;

/// The updater artifact each bundle target produces in Tauri v2, in preference order within a
/// platform: macOS tars the `.app`, while Linux reuses the AppImage and Windows reuses the
/// installer. The `.dmg` and Linux package-manager bundles are never what the updater fetches.
///
/// NSIS outranks WiX because its passive mode installs per-user with a progress bar and no
/// elevation prompt; the MSI would ask for admin rights on an update the user already accepted.
const KINDS: &[(&str, &str)] = &[
    ("darwin", ".app.tar.gz"),
    ("linux", ".AppImage"),
    ("windows", "-setup.exe"),
    ("windows", ".msi"),
];

/// `createUpdaterArtifacts: true` signs every emitted Linux package even though this app only
/// updates AppImages (a `.deb` install is deliberately left to its package manager). These are
/// known non-updater signatures rather than a bundler naming change; every other unknown suffix
/// remains an error so a new updater artifact format cannot silently disappear from the manifest.
const NON_UPDATER_KINDS: &[&str] = &[".deb", ".rpm", ".dmg"];

/// Architecture tokens as they appear in bundler output, mapped to the keys the updater matches
/// against. Longest first: `x86_64` contains `x86`, so the order is what keeps an x86_64 build
/// from being published as the 32-bit one.
const ARCHES: &[(&str, &str)] = &[
    ("x86_64", "x86_64"),
    ("aarch64", "aarch64"),
    ("amd64", "x86_64"),
    ("arm64", "aarch64"),
    ("armv7", "armv7"),
    ("i686", "i686"),
    ("x64", "x86_64"),
    ("x86", "i686"),
];

/// Mirrors the `desktop` matrix in `.github/workflows/release.yml`. Dropping one of these
/// silently strands every installed client on that platform until the next release, so a missing
/// entry fails the job instead.
const EXPECTED: &[&str] = &[
    "darwin-aarch64",
    "darwin-x86_64",
    "linux-x86_64",
    "windows-x86_64",
];

#[derive(Serialize, Debug)]
struct Manifest {
    version: String,
    platforms: BTreeMap<String, Platform>,
}

#[derive(Serialize, Debug)]
struct Platform {
    signature: String,
    url: String,
}

struct Candidate {
    rank: usize,
    file: String,
    signature: String,
}

/// Collect `<dir>/*.sig` into `latest.json`. `base_url` is the release's download prefix, which
/// differs between a tag and the rolling nightly — the GitHub layout is the workflow's business,
/// not this command's.
pub fn manifest(dir: &Path, version: &str, base_url: &str, out: Option<&Path>) -> Result<()> {
    let mut sigs = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "sig") {
            let name = path
                .file_name()
                .context("a .sig path with no file name")?
                .to_string_lossy()
                .into_owned();
            let signature = std::fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))?;
            sigs.push((name, signature));
        }
    }
    // `read_dir` order is filesystem-defined; sorting keeps a rerun byte-identical.
    sigs.sort();
    ensure!(
        !sigs.is_empty(),
        "no .sig files in {} — the desktop jobs did not produce updater artifacts, which means \
         `createUpdaterArtifacts` is off or the signing key was missing",
        dir.display()
    );

    let manifest = build(&sigs, version, base_url)?;
    let out: PathBuf = out.map_or_else(|| dir.join("latest.json"), Path::to_path_buf);
    let json = serde_json::to_string_pretty(&manifest)? + "\n";
    std::fs::write(&out, json).with_context(|| format!("write {}", out.display()))?;
    println!(
        "wrote {} ({} platforms)",
        out.display(),
        manifest.platforms.len()
    );
    Ok(())
}

fn build(sigs: &[(String, String)], version: &str, base_url: &str) -> Result<Manifest> {
    let base_url = base_url.trim_end_matches('/');
    let mut best: BTreeMap<String, Candidate> = BTreeMap::new();

    for (sig_name, signature) in sigs {
        let file = sig_name
            .strip_suffix(".sig")
            .with_context(|| format!("`{sig_name}` is not a .sig file"))?
            .to_string();
        let Some((platform, rank)) = classify(&file)? else {
            continue;
        };
        // The bundler writes the signature with a trailing newline; the client compares it raw.
        let candidate = Candidate {
            rank,
            file,
            signature: signature.trim().to_string(),
        };
        match best.entry(platform.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(candidate);
            }
            Entry::Occupied(mut slot) => {
                ensure!(
                    slot.get().rank != candidate.rank,
                    "`{}` and `{}` both claim {platform}",
                    slot.get().file,
                    candidate.file
                );
                if candidate.rank < slot.get().rank {
                    slot.insert(candidate);
                }
            }
        }
    }

    for platform in EXPECTED {
        ensure!(
            best.contains_key(*platform),
            "no updater artifact for {platform}: every installed client on it would stay on its \
             current version until the next release"
        );
    }

    Ok(Manifest {
        version: version.strip_prefix('v').unwrap_or(version).to_string(),
        platforms: best
            .into_iter()
            .map(|(platform, candidate)| {
                let url = format!("{base_url}/{}", candidate.file);
                (
                    platform,
                    Platform {
                        signature: candidate.signature,
                        url,
                    },
                )
            })
            .collect(),
    })
}

/// `Some(platform key, preference)`, where a lower preference wins if one platform has two
/// updater artifacts; `None` for a known package the updater does not install.
fn classify(file: &str) -> Result<Option<(String, usize)>> {
    let kind = KINDS
        .iter()
        .enumerate()
        .find_map(|(rank, (os, ext))| file.ends_with(ext).then_some((*os, rank)));
    let Some((os, rank)) = kind else {
        ensure!(
            NON_UPDATER_KINDS.iter().any(|ext| file.ends_with(ext)),
            "`{file}` is not an updater artifact any bundle target emits"
        );
        return Ok(None);
    };
    let arch = ARCHES
        .iter()
        .find_map(|(token, arch)| file.contains(token).then_some(*arch))
        .with_context(|| {
            format!(
                "`{file}` names no architecture. The macOS bundler writes a bare \
                 `<product>.app.tar.gz`, so the release workflow has to rename it per target \
                 before both slices land in one release."
            )
        })?;
    Ok(Some((format!("{os}-{arch}"), rank)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://example.invalid/download/v1.2.3";

    /// Real bundler output names for the four matrix strands, macOS already renamed the way the
    /// release workflow does it.
    fn release() -> Vec<(String, String)> {
        [
            "sdr--_1.2.3_aarch64.app.tar.gz",
            "sdr--_1.2.3_x86_64.app.tar.gz",
            "sdr--_1.2.3_amd64.AppImage",
            "sdr--_1.2.3_x64-setup.exe",
        ]
        .iter()
        .map(|file| (format!("{file}.sig"), format!("sig-of-{file}\n")))
        .collect()
    }

    #[test]
    fn maps_every_matrix_strand_to_its_platform_key() {
        let manifest = build(&release(), "1.2.3", BASE).unwrap();
        assert_eq!(manifest.version, "1.2.3");
        let keys: Vec<_> = manifest.platforms.keys().map(String::as_str).collect();
        assert_eq!(keys, EXPECTED);
    }

    #[test]
    fn url_is_the_artifact_beside_the_signature() {
        let manifest = build(&release(), "1.2.3", &format!("{BASE}/")).unwrap();
        let linux = &manifest.platforms["linux-x86_64"];
        assert_eq!(linux.url, format!("{BASE}/sdr--_1.2.3_amd64.AppImage"));
        // Trimmed: a trailing newline in the .sig would fail verification on the client.
        assert_eq!(linux.signature, "sig-of-sdr--_1.2.3_amd64.AppImage");
    }

    /// Windows builds both installers, so both signatures reach the release directory.
    #[test]
    fn prefers_nsis_over_msi() {
        let mut sigs = release();
        sigs.push((
            "sdr--_1.2.3_x64_en-US.msi.sig".to_string(),
            "sig-of-msi".to_string(),
        ));
        sigs.sort();
        let manifest = build(&sigs, "1.2.3", BASE).unwrap();
        assert!(
            manifest.platforms["windows-x86_64"]
                .url
                .ends_with("-setup.exe")
        );
    }

    #[test]
    fn rejects_two_artifacts_of_the_same_kind_for_one_platform() {
        let mut sigs = release();
        sigs.push((
            "other_1.2.3_x64-setup.exe.sig".to_string(),
            "sig".to_string(),
        ));
        let err = build(&sigs, "1.2.3", BASE).unwrap_err().to_string();
        assert!(err.contains("both claim windows-x86_64"), "{err}");
    }

    /// The exact failure the macOS rename exists to prevent: unrenamed, both slices are called
    /// `sdr--.app.tar.gz` and one silently overwrites the other in the release.
    #[test]
    fn rejects_the_unrenamed_macos_artifact() {
        let sigs = vec![("sdr--.app.tar.gz.sig".to_string(), "sig".to_string())];
        let err = build(&sigs, "1.2.3", BASE).unwrap_err().to_string();
        assert!(err.contains("names no architecture"), "{err}");
    }

    #[test]
    fn rejects_a_release_missing_a_platform() {
        let sigs: Vec<_> = release()
            .into_iter()
            .filter(|(name, _)| !name.contains("x64"))
            .collect();
        let err = build(&sigs, "1.2.3", BASE).unwrap_err().to_string();
        assert!(
            err.contains("no updater artifact for windows-x86_64"),
            "{err}"
        );
    }

    /// Tauri v2 signs a `.deb` too, but a package-manager install is not an updater candidate.
    #[test]
    fn ignores_a_known_non_updater_signature() {
        let mut sigs = release();
        sigs.push(("sdr--_1.2.3_amd64.deb.sig".to_string(), "sig".to_string()));
        let manifest = build(&sigs, "1.2.3", BASE).unwrap();
        assert!(
            manifest.platforms["linux-x86_64"]
                .url
                .ends_with(".AppImage")
        );
    }

    #[test]
    fn rejects_an_unknown_signed_artifact() {
        let mut sigs = release();
        sigs.push(("sdr--_1.2.3_amd64.pkg.sig".to_string(), "sig".to_string()));
        let err = build(&sigs, "1.2.3", BASE).unwrap_err().to_string();
        assert!(err.contains("not an updater artifact"), "{err}");
    }

    #[test]
    fn strips_the_tag_prefix_from_the_version() {
        assert_eq!(build(&release(), "v1.2.3", BASE).unwrap().version, "1.2.3");
    }
}
