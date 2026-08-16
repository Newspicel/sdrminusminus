use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result, ensure};

const FORMULA_TRIPLES: [&str; 4] = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
];

const CASK_ARCHES: [&str; 2] = ["aarch64", "x64"];

type Digests = BTreeMap<String, String>;

pub fn tap(sums: &Path, version: &str, repo: &str, out: &Path) -> Result<()> {
    let text = std::fs::read_to_string(sums).with_context(|| format!("read {}", sums.display()))?;
    let digests = parse(&text)?;
    let version = version.strip_prefix('v').unwrap_or(version);

    for (relative, contents) in [
        ("Formula/sdrmm.rb", formula(&digests, version, repo)?),
        ("Casks/sdrminusminus.rb", cask(&digests, version, repo)?),
    ] {
        let path = out.join(relative);
        let dir = path.parent().context("a tap path with no directory")?;
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        std::fs::write(&path, contents).with_context(|| format!("write {}", path.display()))?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

fn parse(text: &str) -> Result<Digests> {
    let mut digests = Digests::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let (digest, file) = line
            .split_once("  ")
            .with_context(|| format!("`{line}` is not a `shasum -a 256` line"))?;
        ensure!(
            digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "`{digest}` is not a SHA-256 digest"
        );
        digests.insert(file.trim().to_string(), digest.to_string());
    }
    ensure!(!digests.is_empty(), "the checksum file lists no artifact");
    Ok(digests)
}

fn digest<'a>(digests: &'a Digests, file: &str) -> Result<&'a str> {
    digests.get(file).map(String::as_str).with_context(|| {
        format!(
            "the release carries no `{file}`, so the tap would point at a download that does not \
             exist"
        )
    })
}

fn formula(digests: &Digests, version: &str, repo: &str) -> Result<String> {
    let mut archives = Vec::new();
    for triple in FORMULA_TRIPLES {
        let file = format!("sdrmm-{version}-{triple}.tar.gz");
        archives.push(format!(
            "      url \"https://github.com/{repo}/releases/download/v{version}/{file}\"\n      \
             sha256 \"{}\"",
            digest(digests, &file)?
        ));
    }
    let [mac_arm, mac_intel, linux_arm, linux_intel] = archives
        .try_into()
        .ok()
        .context("one archive per formula triple")?;

    Ok(format!(
        r##"class Sdrmm < Formula
  desc "Modular, client-server software-defined radio"
  homepage "https://github.com/{repo}"
  license "GPL-3.0-or-later"

  livecheck do
    url :stable
    strategy :github_latest
  end

  depends_on "soapysdr"

  on_macos do
    on_arm do
{mac_arm}
    end
    on_intel do
{mac_intel}
    end
  end

  on_linux do
    depends_on "patchelf" => :build

    on_arm do
{linux_arm}
    end
    on_intel do
{linux_intel}
    end
  end

  def install
    bin.install "sdrmm"
    doc.install "LICENSE", "README.md", "THIRD_PARTY_NOTICES.md"

    if OS.mac?
      MachO::Tools.add_rpath(bin/"sdrmm", formula_opt_lib("soapysdr").to_s)
      system "codesign", "--sign", "-", "--force", bin/"sdrmm"
    else
      system formula_opt_bin("patchelf")/"patchelf",
             "--set-rpath", formula_opt_lib("soapysdr"), bin/"sdrmm"
    end
  end

  def caveats
    <<~EOS
      RTL-SDR, HackRF, RTL-TCP and SpyServer receivers are built in. Other hardware is
      reached through SoapySDR modules, which install separately:
        brew install soapyremote

      Start the server on port 8080 with `sdrmm`, or in the background with
      `brew services start sdrmm`. `sdrmm --doctor` reports what this build can see.
    EOS
  end

  service do
    run [opt_bin/"sdrmm"]
    log_path var/"log/sdrmm.log"
    error_log_path var/"log/sdrmm.log"
  end

  test do
    assert_match version.to_s, shell_output("#{{bin}}/sdrmm --version")
    assert_match "SoapySDR runtime", shell_output("#{{bin}}/sdrmm --doctor")
  end
end
"##
    ))
}

fn cask(digests: &Digests, version: &str, repo: &str) -> Result<String> {
    let mut sums = Vec::new();
    for arch in CASK_ARCHES {
        sums.push(digest(digests, &format!("sdr--_{version}_{arch}.dmg"))?);
    }
    let [arm, intel] = sums.try_into().ok().context("one dmg per cask arch")?;

    Ok(format!(
        r##"cask "sdrminusminus" do
  arch arm: "aarch64", intel: "x64"

  version "{version}"
  sha256 arm:   "{arm}",
         intel: "{intel}"

  url "https://github.com/{repo}/releases/download/v#{{version}}/sdr--_#{{version}}_#{{arch}}.dmg"
  name "sdr--"
  name "sdr minus minus"
  desc "Modular, client-server software-defined radio"
  homepage "https://github.com/{repo}"

  livecheck do
    url :url
    strategy :github_latest
  end

  auto_updates true
  depends_on macos: :catalina

  app "sdr--.app"

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

    const REPO: &str = "Newspicel/sdrminusminus";

    fn sums() -> String {
        let mut lines = Vec::new();
        for triple in FORMULA_TRIPLES {
            lines.push(format!("{}  sdrmm-1.2.3-{triple}.tar.gz", "a".repeat(64)));
        }
        for arch in CASK_ARCHES {
            lines.push(format!("{}  sdr--_1.2.3_{arch}.dmg", "b".repeat(64)));
        }
        lines.push(format!("{}  latest.json", "c".repeat(64)));
        lines.join("\n") + "\n"
    }

    #[test]
    fn formula_carries_a_download_for_every_supported_target() {
        let formula = formula(&parse(&sums()).unwrap(), "1.2.3", REPO).unwrap();
        for triple in FORMULA_TRIPLES {
            assert!(
                formula.contains(&format!(
                    "download/v1.2.3/sdrmm-1.2.3-{triple}.tar.gz\"\n      sha256"
                )),
                "{triple} is missing from:\n{formula}"
            );
        }
        assert_eq!(formula.matches(&"a".repeat(64)).count(), 4);
    }

    #[test]
    fn cask_pairs_each_slice_with_its_own_digest() {
        let cask = cask(&parse(&sums()).unwrap(), "1.2.3", REPO).unwrap();
        assert!(cask.contains(&format!("arm:   \"{}\"", "b".repeat(64))));
        assert!(cask.contains("sdr--_#{version}_#{arch}.dmg"));
        assert!(cask.contains("app \"sdr--.app\""));
    }

    #[test]
    fn a_release_missing_an_artifact_is_refused() {
        let sums = sums().replace(&format!("{}  sdr--_1.2.3_x64.dmg", "b".repeat(64)), "");
        let err = cask(&parse(&sums).unwrap(), "1.2.3", REPO)
            .unwrap_err()
            .to_string();
        assert!(err.contains("sdr--_1.2.3_x64.dmg"), "{err}");
    }

    #[test]
    fn a_truncated_digest_is_refused() {
        let err = parse("abc  sdrmm-1.2.3-aarch64-apple-darwin.tar.gz")
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a SHA-256 digest"), "{err}");
    }

    #[test]
    fn the_tag_prefix_is_not_part_of_the_version() {
        let dir = std::env::temp_dir().join(format!("homebrew-tap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sums_path = dir.join("SHA256SUMS");
        std::fs::write(&sums_path, sums()).unwrap();

        tap(&sums_path, "v1.2.3", REPO, &dir).unwrap();
        let cask = std::fs::read_to_string(dir.join("Casks/sdrminusminus.rb")).unwrap();
        assert!(cask.contains("version \"1.2.3\""));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
