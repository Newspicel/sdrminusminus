use std::{
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};

const PACKAGE_NIX: &str = "packaging/nix/package.nix";

const LOCKFILE: &str = "web/pnpm-lock.yaml";
const MARKER: &str = "# web/pnpm-lock.yaml sha256:";
const CARGO_LOCK: &str = "Cargo.lock";
const REV_MARKER: &str = "# git rev ";
const FAKE_HASH: &str = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
const IMAGE: &str = "nixos/nix:latest";

pub fn check(root: &Path) -> Result<()> {
    let text = read_package(root)?;
    check_lockfile(&text, &lockfile_digest(root)?)?;
    check_revisions(&text, &read_lock(root)?)
}

pub fn run(root: &Path) -> Result<()> {
    let system = linux_system()?;
    let runner = Runner::find()?;
    println!("$ {} ({system})", runner.describe());
    take_pnpm_hash(&runner, root, system)?;
    take_git_hashes(&runner, root, system)
}

fn check_lockfile(text: &str, current: &str) -> Result<()> {
    let Some(recorded) = recorded_digest(text) else {
        bail!(
            "{PACKAGE_NIX} carries a pnpm deps hash with no `{MARKER}<digest>` line above it, so \
             nothing here can tell whether the hash still describes {LOCKFILE}. Take both with \
             `cargo xtask nix-hash`."
        );
    };
    ensure!(
        recorded == current,
        "{LOCKFILE} has moved since the nix pnpm deps hash was taken ({} recorded, {} now), so \
         `nix build` fetches a store the fixed-output hash does not match. Retake it with \
         `cargo xtask nix-hash`.",
        &recorded[..12],
        &current[..12],
    );
    println!("nix pnpm deps: hash taken from the current {LOCKFILE}");
    Ok(())
}

fn check_revisions(text: &str, lock: &str) -> Result<()> {
    let pins = pins(text);
    let sources = git_sources(lock);
    for source in &sources {
        ensure!(
            pins.iter().any(|pin| source.holds(&pin.key)),
            "{CARGO_LOCK} takes {} from git at {}, and {PACKAGE_NIX} pins no hash for it, so \
             `nix build` cannot vendor it. Take one with `cargo xtask nix-hash`.",
            source.name,
            &source.rev[..7.min(source.rev.len())],
        );
    }
    for pin in &pins {
        let Some(source) = sources.iter().find(|source| source.holds(&pin.key)) else {
            bail!(
                "{PACKAGE_NIX} pins a hash for `{}`, which {CARGO_LOCK} no longer takes from git. \
                 Drop the entry.",
                pin.key,
            );
        };
        let Some(rev) = &pin.rev else {
            bail!(
                "{PACKAGE_NIX} pins `{}` with no `{REV_MARKER}<rev>` line above it, so nothing \
                 here can tell whether the hash still describes the commit {CARGO_LOCK} names. \
                 Take both with `cargo xtask nix-hash`.",
                pin.key,
            );
        };
        ensure!(
            *rev == source.rev,
            "{CARGO_LOCK} has moved `{}` to {} since its nix hash was taken at {}, so `nix build` \
             fetches a commit the fixed-output hash does not match. Retake it with `cargo xtask \
             nix-hash`.",
            pin.key,
            &source.rev[..7.min(source.rev.len())],
            &rev[..7.min(rev.len())],
        );
    }
    println!("nix cargo git deps: hashes taken from the current {CARGO_LOCK}");
    Ok(())
}

fn take_pnpm_hash(runner: &Runner, root: &Path, system: &str) -> Result<()> {
    let text = read_package(root)?;
    let attribute = format!(".#packages.{system}.default.pnpmDeps");
    let updated = match runner.build(root, &attribute)? {
        Some(mismatch) => {
            println!("nix pnpm deps: {}", mismatch.hash);
            replace_hash(&text, &mismatch.hash)?
        }
        None => {
            println!("nix pnpm deps: the recorded hash already matches");
            text
        }
    };
    let updated = replace_marker(&updated, &lockfile_digest(root)?)?;
    write_package(root, &updated)
}

fn take_git_hashes(runner: &Runner, root: &Path, system: &str) -> Result<()> {
    let sources = git_sources(&read_lock(root)?);
    if sources.is_empty() {
        return Ok(());
    }
    let seeded = seed_pins(&read_package(root)?, &sources)?;
    write_package(root, &seeded)?;
    let attribute = format!(".#packages.{system}.default.cargoDeps");
    for _ in 0..=sources.len() {
        let text = read_package(root)?;
        let Some(mismatch) = runner.build(root, &attribute)? else {
            let recorded = record_revisions(&text, &sources)?;
            write_package(root, &recorded)?;
            println!("nix cargo git deps: hashes taken from the current {CARGO_LOCK}");
            return Ok(());
        };
        let key = pin_key(&sources, &pins(&text), &mismatch.derivation)?;
        println!("nix cargo git deps: {key} {}", mismatch.hash);
        let updated = replace_output_hash(&text, &key, &mismatch.hash)?;
        write_package(root, &updated)?;
    }
    bail!(
        "the cargo git hashes in {PACKAGE_NIX} still do not match after taking every one of them \
         that the vendor derivation reported"
    )
}

enum Runner {
    Nix,
    Docker,
}

impl Runner {
    fn find() -> Result<Self> {
        if cfg!(target_os = "linux") && installed("nix") {
            return Ok(Self::Nix);
        }
        if reachable("docker", "info") {
            return Ok(Self::Docker);
        }
        bail!(
            "taking this hash means building the fixed-output derivation, which needs nix on \
             linux or a container runtime to hold one. Install nix, or start docker — the fetch \
             itself runs in {IMAGE}."
        )
    }

    fn describe(&self) -> &'static str {
        match self {
            Self::Nix => "nix build",
            Self::Docker => "docker run nixos/nix",
        }
    }

    fn build(&self, root: &Path, attribute: &str) -> Result<Option<Mismatch>> {
        let nix = format!(
            "nix --extra-experimental-features 'nix-command flakes' build --no-link \
             --print-out-paths '{attribute}'"
        );
        let output = match self {
            Self::Nix => Command::new("sh")
                .args(["-c", &nix])
                .current_dir(root)
                .stderr(Stdio::piped())
                .output(),
            Self::Docker => Command::new("docker")
                .args([
                    "run",
                    "--rm",
                    "-v",
                    &format!("{}:/work", root.display()),
                    "-w",
                    "/work",
                    IMAGE,
                    "sh",
                    "-c",
                    &nix,
                ])
                .current_dir(root)
                .stderr(Stdio::piped())
                .output(),
        }
        .with_context(|| format!("failed to spawn `{}`", self.describe()))?;
        if output.status.success() {
            return Ok(None);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        match mismatch(&stderr) {
            Some(found) => Ok(Some(found)),
            None => {
                bail!("`{attribute}` failed for a reason other than a hash it was given:\n{stderr}")
            }
        }
    }
}

struct Mismatch {
    derivation: String,
    hash: String,
}

struct Pin {
    key: String,
    rev: Option<String>,
    hash: String,
}

struct GitSource {
    name: String,
    rev: String,
    packages: Vec<String>,
}

impl GitSource {
    fn holds(&self, key: &str) -> bool {
        self.packages.iter().any(|package| package == key)
    }

    fn derivation(&self) -> String {
        format!("{}-{}", self.name, &self.rev[..7.min(self.rev.len())])
    }
}

fn installed(program: &str) -> bool {
    reachable(program, "--version")
}

fn reachable(program: &str, probe: &str) -> bool {
    Command::new(program)
        .arg(probe)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn linux_system() -> Result<&'static str> {
    match std::env::consts::ARCH {
        "aarch64" => Ok("aarch64-linux"),
        "x86_64" => Ok("x86_64-linux"),
        other => bail!("the flake builds aarch64-linux and x86_64-linux, not {other}"),
    }
}

fn read_package(root: &Path) -> Result<String> {
    let path = root.join(PACKAGE_NIX);
    std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))
}

fn write_package(root: &Path, text: &str) -> Result<()> {
    let path = root.join(PACKAGE_NIX);
    std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}

fn read_lock(root: &Path) -> Result<String> {
    let path = root.join(CARGO_LOCK);
    std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))
}

fn lockfile_digest(root: &Path) -> Result<String> {
    let path = root.join(LOCKFILE);
    let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn recorded_digest(text: &str) -> Option<String> {
    let rest = text.split_once(MARKER)?.1;
    let digest: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
    (digest.len() == 64).then_some(digest)
}

fn replace_hash(text: &str, hash: &str) -> Result<String> {
    let (before, rest) = text
        .split_once("    hash = \"")
        .context("packaging/nix/package.nix declares no pnpm deps hash")?;
    let (_, after) = rest
        .split_once('"')
        .context("the pnpm deps hash is not a closed string")?;
    Ok(format!("{before}    hash = \"{hash}\"{after}"))
}

fn replace_marker(text: &str, digest: &str) -> Result<String> {
    let line = format!("    {MARKER}{digest}\n");
    match text.split_once(MARKER) {
        Some((before, rest)) => {
            let after = rest
                .split_once('\n')
                .context("the lockfile marker runs to the end of the file")?
                .1;
            let head = before
                .strip_suffix("    ")
                .context("the lockfile marker is not indented as an attribute")?;
            Ok(format!("{head}{line}{after}"))
        }
        None => {
            let anchor = "    hash = \"";
            let (before, after) = text
                .split_once(anchor)
                .context("packaging/nix/package.nix declares no pnpm deps hash")?;
            Ok(format!("{before}{line}{anchor}{after}"))
        }
    }
}

fn git_sources(lock: &str) -> Vec<GitSource> {
    let mut sources: Vec<GitSource> = Vec::new();
    for block in lock.split("[[package]]").skip(1) {
        let (Some(name), Some(version), Some(source)) = (
            field(block, "name"),
            field(block, "version"),
            field(block, "source"),
        ) else {
            continue;
        };
        let Some(url) = source.strip_prefix("git+") else {
            continue;
        };
        let Some((url, rev)) = url.split_once('#') else {
            continue;
        };
        let repo = url
            .split('?')
            .next()
            .unwrap_or(url)
            .trim_end_matches('/')
            .trim_end_matches(".git");
        let repo = repo.rsplit('/').next().unwrap_or(repo);
        let key = format!("{name}-{version}");
        match sources
            .iter_mut()
            .find(|held| held.name == repo && held.rev == rev)
        {
            Some(held) => held.packages.push(key),
            None => sources.push(GitSource {
                name: repo.to_owned(),
                rev: rev.to_owned(),
                packages: vec![key],
            }),
        }
    }
    sources
}

fn field<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let prefix = format!("{name} = \"");
    block
        .lines()
        .find_map(|line| line.strip_prefix(prefix.as_str()))
        .and_then(|rest| rest.split('"').next())
}

fn pins(text: &str) -> Vec<Pin> {
    let Ok((_, block, _)) = split_output_hashes(text) else {
        return Vec::new();
    };
    let mut pins = Vec::new();
    let mut rev = None;
    for line in block.lines() {
        let line = line.trim();
        if let Some(recorded) = line.strip_prefix(REV_MARKER) {
            rev = Some(recorded.trim().to_owned());
            continue;
        }
        let Some((key, value)) = line.split_once(" = ") else {
            continue;
        };
        let (Some(key), Some(hash)) = (quoted(key), quoted(value)) else {
            continue;
        };
        pins.push(Pin {
            key: key.to_owned(),
            rev: rev.take(),
            hash: hash.to_owned(),
        });
    }
    pins
}

fn quoted(text: &str) -> Option<&str> {
    text.trim().strip_prefix('"')?.split('"').next()
}

fn split_output_hashes(text: &str) -> Result<(&str, &str, &str)> {
    let (before, rest) = text
        .split_once("outputHashes = {")
        .context("packaging/nix/package.nix declares no cargo git output hashes")?;
    let (block, after) = rest
        .split_once("};")
        .context("the cargo git output hashes are not a closed set")?;
    Ok((before, block, after))
}

fn rewrite_pins(text: &str, pins: &[Pin]) -> Result<String> {
    let (before, _, after) = split_output_hashes(text)?;
    let body: String = pins
        .iter()
        .map(|pin| {
            let marker = pin
                .rev
                .as_ref()
                .map(|rev| format!("      {REV_MARKER}{rev}\n"))
                .unwrap_or_default();
            format!("{marker}      \"{}\" = \"{}\";\n", pin.key, pin.hash)
        })
        .collect();
    Ok(format!("{before}outputHashes = {{\n{body}    }};{after}"))
}

fn seed_pins(text: &str, sources: &[GitSource]) -> Result<String> {
    let mut pins = pins(text);
    for source in sources {
        if pins.iter().any(|pin| source.holds(&pin.key)) {
            continue;
        }
        let key = source
            .packages
            .first()
            .with_context(|| format!("{} resolves to no package", source.name))?;
        pins.push(Pin {
            key: key.clone(),
            rev: Some(source.rev.clone()),
            hash: FAKE_HASH.to_owned(),
        });
    }
    rewrite_pins(text, &pins)
}

fn record_revisions(text: &str, sources: &[GitSource]) -> Result<String> {
    let mut pins = pins(text);
    for pin in &mut pins {
        if let Some(source) = sources.iter().find(|source| source.holds(&pin.key)) {
            pin.rev = Some(source.rev.clone());
        }
    }
    rewrite_pins(text, &pins)
}

fn replace_output_hash(text: &str, key: &str, hash: &str) -> Result<String> {
    let mut pins = pins(text);
    let pin = pins
        .iter_mut()
        .find(|pin| pin.key == key)
        .with_context(|| format!("{PACKAGE_NIX} pins no hash for `{key}`"))?;
    pin.hash = hash.to_owned();
    rewrite_pins(text, &pins)
}

fn pin_key(sources: &[GitSource], pins: &[Pin], derivation: &str) -> Result<String> {
    let source = sources
        .iter()
        .find(|source| source.derivation() == derivation)
        .with_context(|| {
            format!(
                "`{derivation}` is not one of the git dependencies {CARGO_LOCK} names, so there \
                 is no hash in {PACKAGE_NIX} to take for it"
            )
        })?;
    pins.iter()
        .find(|pin| source.holds(&pin.key))
        .map(|pin| pin.key.clone())
        .or_else(|| source.packages.first().cloned())
        .with_context(|| format!("{} resolves to no package", source.name))
}

fn mismatch(stderr: &str) -> Option<Mismatch> {
    let mut lines = stderr
        .lines()
        .skip_while(|line| !line.contains("hash mismatch in fixed-output derivation"));
    let derivation = derivation_name(lines.next()?)?;
    let hash = lines
        .find(|line| line.trim_start().starts_with("got:"))?
        .split_whitespace()
        .find(|word| word.starts_with("sha256-"))?
        .to_owned();
    Some(Mismatch { derivation, hash })
}

fn derivation_name(line: &str) -> Option<String> {
    let path = line.split('\'').nth(1)?;
    let file = path.rsplit('/').next()?;
    let (_, name) = file.strip_suffix(".drv")?.split_once('-')?;
    Some(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACKAGE: &str = "  pnpmDeps = fetchPnpmDeps {\n    fetcherVersion = 4;\n    hash = \
                           \"sha256-old=\";\n  };\n  pnpmRoot = \"web\";\n";

    const PINNED: &str = "  cargoLock = {\n    lockFile = ../../Cargo.lock;\n    outputHashes = \
                          {\n      # git rev aaaaaaaabbbbbbbbccccccccddddddddeeeeeeee\n      \
                          \"soapysdr-0.5.1\" = \"sha256-one=\";\n    };\n  };\n";

    const LOCK: &str = "[[package]]\nname = \"soapysdr\"\nversion = \"0.5.1\"\nsource = \
                        \"git+https://github.com/Newspicel/rust-soapysdr?rev=aaaaaaaabbbbbbbbccccccccddddddddeeeeeeee#aaaaaaaabbbbbbbbccccccccddddddddeeeeeeee\"\n\n\
                        [[package]]\nname = \"soapysdr-sys\"\nversion = \"0.8.1\"\nsource = \
                        \"git+https://github.com/Newspicel/rust-soapysdr?rev=aaaaaaaabbbbbbbbccccccccddddddddeeeeeeee#aaaaaaaabbbbbbbbccccccccddddddddeeeeeeee\"\n\n\
                        [[package]]\nname = \"serde\"\nversion = \"1.0.0\"\nsource = \
                        \"registry+https://github.com/rust-lang/crates.io-index\"\n";

    #[test]
    fn takes_the_hash_and_the_derivation_the_fetch_reported() {
        let stderr = "error: hash mismatch in fixed-output derivation \
                      '/nix/store/r13z84g47j98ai2zab4qim268m918kwm-rust-soapysdr-fc09ef2.drv':\n  \
                      specified: sha256-aaa=\n            got:    sha256-bbb=\n";
        let found = mismatch(stderr).expect("mismatch");
        assert_eq!(found.derivation, "rust-soapysdr-fc09ef2");
        assert_eq!(found.hash, "sha256-bbb=");
    }

    #[test]
    fn reads_no_hash_out_of_a_build_that_failed_for_another_reason() {
        assert!(
            mismatch("error: builder for '/nix/store/x.drv' failed with exit code 1").is_none()
        );
    }

    #[test]
    fn writes_the_marker_above_the_hash_and_then_keeps_it_there() {
        let once = replace_marker(PACKAGE, &"a".repeat(64)).expect("marker");
        assert_eq!(
            recorded_digest(&once).as_deref(),
            Some("a".repeat(64).as_str())
        );
        let twice = replace_marker(&once, &"b".repeat(64)).expect("marker");
        assert_eq!(
            recorded_digest(&twice).as_deref(),
            Some("b".repeat(64).as_str())
        );
        assert_eq!(twice.matches(MARKER).count(), 1);
        assert!(twice.contains("    hash = \"sha256-old=\";"));
    }

    #[test]
    fn replaces_the_hash_and_nothing_around_it() {
        let updated = replace_hash(PACKAGE, "sha256-new=").expect("hash");
        assert!(updated.contains("    hash = \"sha256-new=\";"));
        assert!(updated.contains("fetcherVersion = 4;"));
        assert!(updated.ends_with("  pnpmRoot = \"web\";\n"));
    }

    #[test]
    fn reads_no_digest_out_of_a_file_that_carries_none() {
        assert!(recorded_digest(PACKAGE).is_none());
        assert!(recorded_digest(&format!("    {MARKER}abc\n")).is_none());
    }

    #[test]
    fn gathers_a_repository_crates_under_the_one_commit_it_is_fetched_at() {
        let sources = git_sources(LOCK);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].name, "rust-soapysdr");
        assert_eq!(sources[0].rev, "aaaaaaaabbbbbbbbccccccccddddddddeeeeeeee");
        assert_eq!(
            sources[0].packages,
            ["soapysdr-0.5.1", "soapysdr-sys-0.8.1"]
        );
        assert_eq!(sources[0].derivation(), "rust-soapysdr-aaaaaaa");
    }

    #[test]
    fn accepts_a_pin_taken_at_the_commit_the_lock_names() {
        check_revisions(PINNED, LOCK).expect("pinned");
    }

    #[test]
    fn refuses_a_pin_taken_at_an_older_commit() {
        let moved = LOCK.replace("aaaaaaaa", "ffffffff");
        let error = check_revisions(PINNED, &moved).expect_err("moved");
        assert!(error.to_string().contains("cargo xtask nix-hash"));
        assert!(error.to_string().contains("soapysdr-0.5.1"));
    }

    #[test]
    fn refuses_a_pin_with_no_recorded_commit() {
        let bare = PINNED.replace(
            "      # git rev aaaaaaaabbbbbbbbccccccccddddddddeeeeeeee\n",
            "",
        );
        let error = check_revisions(&bare, LOCK).expect_err("bare");
        assert!(error.to_string().contains(REV_MARKER));
    }

    #[test]
    fn refuses_a_git_dependency_nothing_pins() {
        let error = check_revisions("outputHashes = {\n    };\n", LOCK).expect_err("unpinned");
        assert!(error.to_string().contains("rust-soapysdr"));
    }

    #[test]
    fn refuses_a_pin_the_lock_no_longer_takes_from_git() {
        let error = check_revisions(PINNED, "").expect_err("stale");
        assert!(error.to_string().contains("Drop the entry"));
    }

    #[test]
    fn seeds_a_pin_for_a_git_dependency_that_has_none() {
        let seeded = seed_pins("    outputHashes = {\n    };\n", &git_sources(LOCK)).expect("seed");
        assert!(seeded.contains("      # git rev aaaaaaaabbbbbbbbccccccccddddddddeeeeeeee\n"));
        assert!(seeded.contains(&format!("      \"soapysdr-0.5.1\" = \"{FAKE_HASH}\";\n")));
    }

    #[test]
    fn replaces_one_pins_hash_and_keeps_its_commit() {
        let updated = replace_output_hash(PINNED, "soapysdr-0.5.1", "sha256-two=").expect("hash");
        assert!(updated.contains("      \"soapysdr-0.5.1\" = \"sha256-two=\";\n"));
        assert!(updated.contains("      # git rev aaaaaaaabbbbbbbbccccccccddddddddeeeeeeee\n"));
        assert!(updated.contains("    lockFile = ../../Cargo.lock;\n"));
        assert!(updated.ends_with("  };\n"));
    }

    #[test]
    fn records_the_commit_the_lock_moved_to() {
        let moved = LOCK.replace("aaaaaaaa", "ffffffff");
        let recorded = record_revisions(PINNED, &git_sources(&moved)).expect("record");
        check_revisions(&recorded, &moved).expect("recorded");
        assert_eq!(recorded.matches(REV_MARKER).count(), 1);
    }

    #[test]
    fn names_the_pin_a_failing_fetch_belongs_to() {
        let sources = git_sources(LOCK);
        let key = pin_key(&sources, &pins(PINNED), "rust-soapysdr-aaaaaaa").expect("key");
        assert_eq!(key, "soapysdr-0.5.1");
        assert!(pin_key(&sources, &pins(PINNED), "xng-6a768a2").is_err());
    }
}
