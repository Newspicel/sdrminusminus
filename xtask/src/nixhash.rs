use std::{
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};

const PACKAGE_NIX: &str = "packaging/nix/package.nix";

const LOCKFILE: &str = "web/pnpm-lock.yaml";
const MARKER: &str = "# web/pnpm-lock.yaml sha256:";
const IMAGE: &str = "nixos/nix:latest";

pub fn check(root: &Path) -> Result<()> {
    let text = read_package(root)?;
    let current = lockfile_digest(root)?;
    let Some(recorded) = recorded_digest(&text) else {
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

pub fn run(root: &Path) -> Result<()> {
    let system = linux_system()?;
    let runner = Runner::find()?;
    println!("$ {} ({system})", runner.describe());
    let text = read_package(root)?;
    let updated = match runner.build(root, system)? {
        Some(hash) => {
            println!("nix pnpm deps: {hash}");
            replace_hash(&text, &hash)?
        }
        None => {
            println!("nix pnpm deps: the recorded hash already matches");
            text
        }
    };
    let updated = replace_marker(&updated, &lockfile_digest(root)?)?;
    let path = root.join(PACKAGE_NIX);
    std::fs::write(&path, updated).with_context(|| format!("write {}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
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

    fn build(&self, root: &Path, system: &str) -> Result<Option<String>> {
        let attribute = format!(".#packages.{system}.default.pnpmDeps");
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
            Some(hash) => Ok(Some(hash)),
            None => bail!(
                "the pnpm deps derivation failed for a reason other than the hash it was given:\n\
                 {stderr}"
            ),
        }
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

fn mismatch(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .find(|line| line.trim_start().starts_with("got:"))?
        .split_whitespace()
        .find(|word| word.starts_with("sha256-"))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACKAGE: &str = "  pnpmDeps = fetchPnpmDeps {\n    fetcherVersion = 4;\n    hash = \
                           \"sha256-old=\";\n  };\n  pnpmRoot = \"web\";\n";

    #[test]
    fn takes_the_hash_the_derivation_reported() {
        let stderr = "error: hash mismatch in fixed-output derivation '/nix/store/x.drv':\n         \
                      specified: sha256-aaa=\n            got:    sha256-bbb=\n";
        assert_eq!(mismatch(stderr).as_deref(), Some("sha256-bbb="));
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
}
