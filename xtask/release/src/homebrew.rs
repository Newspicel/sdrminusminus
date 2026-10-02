use std::path::Path;

use anyhow::{Context, Result};

use crate::sums::{Digests, digest, parse};
use crate::{DOWNLOADS, HOMEPAGE};

const CASK_ARCHES: [&str; 2] = ["aarch64", "x64"];

const CASK_RENAMES: &str = "{\n  \"sdrminusminus\": \"sdrmm-app\"\n}\n";

const TAP_MIGRATIONS: &str = "{\n  \"sdrmm\": \"homebrew/core\"\n}\n";

pub fn tap(sums: &Path, version: &str, out: &Path) -> Result<()> {
    let text = std::fs::read_to_string(sums).with_context(|| format!("read {}", sums.display()))?;
    let digests = parse(&text)?;
    let version = version.strip_prefix('v').unwrap_or(version);

    for (relative, contents) in [
        ("Casks/sdrmm-app.rb", cask(&digests, version)?),
        ("cask_renames.json", CASK_RENAMES.to_owned()),
        ("tap_migrations.json", TAP_MIGRATIONS.to_owned()),
    ] {
        let path = out.join(relative);
        let dir = path.parent().context("a tap path with no directory")?;
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        std::fs::write(&path, contents).with_context(|| format!("write {}", path.display()))?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

fn cask(digests: &Digests, version: &str) -> Result<String> {
    let mut sums = Vec::new();
    for arch in CASK_ARCHES {
        sums.push(digest(digests, &format!("SDR--_{version}_{arch}.dmg"))?);
    }
    let [arm, intel] = sums.try_into().ok().context("one dmg per cask arch")?;

    Ok(format!(
        r##"cask "sdrmm-app" do
  arch arm: "aarch64", intel: "x64"

  version "{version}"
  sha256 arm:   "{arm}",
         intel: "{intel}"

  url "{DOWNLOADS}/v#{{version}}/SDR--_#{{version}}_#{{arch}}.dmg"
  name "SDR--"
  name "sdr minus minus"
  desc "Modular, client-server software-defined radio"
  homepage "{HOMEPAGE}/"

  livecheck do
    url "{DOWNLOADS}/latest"
    regex(/v?(\d+(?:\.\d+)+)/i)
  end

  auto_updates true
  depends_on :macos

  app "SDR--.app"

  zap trash: [
    "~/Library/Application Support/dev.newspicel.sdrmm",
    "~/Library/Caches/dev.newspicel.sdrmm",
    "~/Library/HTTPStorages/dev.newspicel.sdrmm",
    "~/Library/Preferences/dev.newspicel.sdrmm.plist",
    "~/Library/Saved Application State/dev.newspicel.sdrmm.savedState",
    "~/Library/WebKit/dev.newspicel.sdrmm",
  ]
end
"##
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sums() -> String {
        let mut lines = Vec::new();
        for arch in CASK_ARCHES {
            lines.push(format!("{}  SDR--_1.2.3_{arch}.dmg", "b".repeat(64)));
        }
        lines.push(format!("{}  latest.json", "c".repeat(64)));
        lines.join("\n") + "\n"
    }

    #[test]
    fn cask_pairs_each_slice_with_its_own_digest() {
        let cask = cask(&parse(&sums()).unwrap(), "1.2.3").unwrap();
        assert!(cask.contains(&format!("arm:   \"{}\"", "b".repeat(64))));
        assert!(cask.contains(
            "url \"https://downloads.sdrmm.com/releases/v#{version}/SDR--_#{version}_#{arch}.dmg\""
        ));
        assert!(cask.contains("url \"https://downloads.sdrmm.com/releases/latest\""));
        assert!(cask.starts_with("cask \"sdrmm-app\" do"));
        assert!(cask.contains("app \"SDR--.app\""));
        assert!(cask.contains("homepage \"https://sdrmm.com/\""));
        assert!(cask.contains("depends_on :macos\n"));
    }

    #[test]
    fn a_release_missing_an_artifact_is_refused() {
        let sums = sums().replace(&format!("{}  SDR--_1.2.3_x64.dmg", "b".repeat(64)), "");
        let err = cask(&parse(&sums).unwrap(), "1.2.3")
            .unwrap_err()
            .to_string();
        assert!(err.contains("SDR--_1.2.3_x64.dmg"), "{err}");
    }

    #[test]
    fn the_tag_prefix_is_not_part_of_the_version() {
        let dir = std::env::temp_dir().join(format!("homebrew-tap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sums_path = dir.join("SHA256SUMS");
        std::fs::write(&sums_path, sums()).unwrap();

        tap(&sums_path, "v1.2.3", &dir).unwrap();
        let cask = std::fs::read_to_string(dir.join("Casks/sdrmm-app.rb")).unwrap();
        assert!(cask.contains("version \"1.2.3\""));
        let renames = std::fs::read_to_string(dir.join("cask_renames.json")).unwrap();
        assert!(renames.contains("\"sdrminusminus\": \"sdrmm-app\""));
        let migrations = std::fs::read_to_string(dir.join("tap_migrations.json")).unwrap();
        assert!(migrations.contains("\"sdrmm\": \"homebrew/core\""));
        assert!(!dir.join("Formula/sdrmm.rb").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
