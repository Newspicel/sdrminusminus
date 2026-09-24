use std::path::Path;

use anyhow::{Context, Result};

use crate::sums::{Digests, digest, parse};

const ARCHES: [Arch; 2] = [
    Arch {
        pacman: "x86_64",
        deb: "amd64",
        triple: "x86_64-unknown-linux-gnu",
    },
    Arch {
        pacman: "aarch64",
        deb: "arm64",
        triple: "aarch64-unknown-linux-gnu",
    },
];

const SOAPY: &str = "soapysdr: radios through SoapySDR modules";

struct Arch {
    pacman: &'static str,
    deb: &'static str,
    triple: &'static str,
}

struct Package {
    name: &'static str,
    url: String,
    provides: &'static str,
    desc: &'static str,
    depends: &'static [&'static str],
    sources: Vec<Source>,
    install: String,
}

struct Source {
    arch: &'static str,
    file: String,
    url: String,
    sha256: String,
}

pub fn packages(sums: &Path, version: &str, repo: &str, out: &Path) -> Result<()> {
    let text = std::fs::read_to_string(sums).with_context(|| format!("read {}", sums.display()))?;
    let digests = parse(&text)?;
    let version = version.strip_prefix('v').unwrap_or(version);

    for package in [
        desktop(&digests, version, repo)?,
        server(&digests, version, repo)?,
    ] {
        let dir = out.join(package.name);
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        for (file, contents) in [
            ("PKGBUILD", pkgbuild(&package, version)),
            (".SRCINFO", srcinfo(&package, version)),
        ] {
            let path = dir.join(file);
            std::fs::write(&path, contents).with_context(|| format!("write {}", path.display()))?;
            println!("wrote {}", path.display());
        }
    }
    Ok(())
}

fn sources(
    digests: &Digests,
    version: &str,
    repo: &str,
    file: impl Fn(&Arch) -> String,
) -> Result<Vec<Source>> {
    ARCHES
        .iter()
        .map(|arch| {
            let file = file(arch);
            Ok(Source {
                arch: arch.pacman,
                url: format!("https://github.com/{repo}/releases/download/v{version}/{file}"),
                sha256: digest(digests, &file)?.to_string(),
                file,
            })
        })
        .collect()
}

fn desktop(digests: &Digests, version: &str, repo: &str) -> Result<Package> {
    Ok(Package {
        name: "sdrminusminus-bin",
        url: format!("https://github.com/{repo}"),
        provides: "sdrminusminus",
        desc: "Modular software-defined radio, desktop app",
        depends: &[
            "cairo",
            "dbus",
            "gcc-libs",
            "gdk-pixbuf2",
            "glib2",
            "glibc",
            "gtk3",
            "libsoup3",
            "webkit2gtk-4.1",
        ],
        sources: sources(digests, version, repo, |arch| {
            format!("sdrminusminus_{version}_{}.deb", arch.deb)
        })?,
        install: "  bsdtar -xf data.tar.gz -C \"$pkgdir\"\n  install -Dm644 \
                  \"$pkgdir/usr/lib/sdrminusminus/THIRD_PARTY_NOTICES.md\" -t \
                  \"$pkgdir/usr/share/licenses/$pkgname/\""
            .to_string(),
    })
}

fn server(digests: &Digests, version: &str, repo: &str) -> Result<Package> {
    Ok(Package {
        name: "sdrmm-bin",
        url: format!("https://github.com/{repo}"),
        provides: "sdrmm",
        desc: "Modular software-defined radio, headless server",
        depends: &["gcc-libs", "glibc"],
        sources: sources(digests, version, repo, |arch| {
            format!("sdrmm-{version}-{}.tar.gz", arch.triple)
        })?,
        install: format!(
            "  cd \"sdrmm-{version}-$CARCH-unknown-linux-gnu\"\n  install -Dm755 sdrmm \
             \"$pkgdir/usr/bin/sdrmm\"\n  install -Dm644 README.md -t \
             \"$pkgdir/usr/share/doc/sdrmm/\"\n  install -Dm644 LICENSE THIRD_PARTY_NOTICES.md -t \
             \"$pkgdir/usr/share/licenses/$pkgname/\""
        ),
    })
}

fn pkgver(version: &str) -> String {
    version.replace('-', "_")
}

fn quoted(items: &[&str]) -> String {
    items
        .iter()
        .map(|item| format!("'{item}'"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn pkgbuild(package: &Package, version: &str) -> String {
    let mut arch_lines = String::new();
    for source in &package.sources {
        arch_lines.push_str(&format!(
            "source_{arch}=('{file}::{url}')\nsha256sums_{arch}=('{sha}')\n",
            arch = source.arch,
            file = source.file,
            url = source.url,
            sha = source.sha256,
        ));
    }
    let arches: Vec<&str> = package.sources.iter().map(|source| source.arch).collect();
    format!(
        "pkgname={name}\npkgver={pkgver}\npkgrel=1\npkgdesc='{desc}'\narch=({arches})\n\
         url='{url}'\nlicense=('GPL-3.0-or-later')\ndepends=({depends})\noptdepends=('{SOAPY}')\n\
         provides=('{provides}')\nconflicts=('{provides}')\noptions=('!strip' '!debug')\n\
         {arch_lines}\npackage() {{\n{install}\n}}\n",
        name = package.name,
        pkgver = pkgver(version),
        desc = package.desc,
        arches = quoted(&arches),
        url = package.url,
        depends = quoted(package.depends),
        provides = package.provides,
        install = package.install,
    )
}

fn srcinfo(package: &Package, version: &str) -> String {
    let mut lines = vec![
        format!("pkgbase = {}", package.name),
        format!("\tpkgdesc = {}", package.desc),
        format!("\tpkgver = {}", pkgver(version)),
        "\tpkgrel = 1".to_string(),
        format!("\turl = {}", package.url),
    ];
    lines.extend(
        package
            .sources
            .iter()
            .map(|source| format!("\tarch = {}", source.arch)),
    );
    lines.push("\tlicense = GPL-3.0-or-later".to_string());
    lines.extend(
        package
            .depends
            .iter()
            .map(|dep| format!("\tdepends = {dep}")),
    );
    lines.push(format!("\toptdepends = {SOAPY}"));
    lines.push(format!("\tprovides = {}", package.provides));
    lines.push(format!("\tconflicts = {}", package.provides));
    lines.push("\toptions = !strip".to_string());
    lines.push("\toptions = !debug".to_string());
    for source in &package.sources {
        lines.push(format!(
            "\tsource_{} = {}::{}",
            source.arch, source.file, source.url
        ));
        lines.push(format!("\tsha256sums_{} = {}", source.arch, source.sha256));
    }
    lines.push(String::new());
    lines.push(format!("pkgname = {}", package.name));
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPO: &str = "Newspicel/sdrminusminus";

    fn sums() -> String {
        let mut lines = Vec::new();
        for arch in ARCHES {
            lines.push(format!(
                "{}  sdrminusminus_1.2.3_{}.deb",
                "d".repeat(64),
                arch.deb
            ));
            lines.push(format!(
                "{}  sdrmm-1.2.3-{}.tar.gz",
                "e".repeat(64),
                arch.triple
            ));
        }
        lines.join("\n") + "\n"
    }

    #[test]
    fn desktop_package_pairs_each_arch_with_its_deb() {
        let package = desktop(&parse(&sums()).unwrap(), "1.2.3", REPO).unwrap();
        let pkgbuild = pkgbuild(&package, "1.2.3");
        assert!(pkgbuild.contains(&format!(
            "source_x86_64=('sdrminusminus_1.2.3_amd64.deb::https://github.com/{REPO}/releases/download/v1.2.3/sdrminusminus_1.2.3_amd64.deb')\nsha256sums_x86_64=('{}')",
            "d".repeat(64)
        )), "{pkgbuild}");
        assert!(pkgbuild.contains("sdrminusminus_1.2.3_arm64.deb"));
        assert!(pkgbuild.contains("depends=('cairo' "));
    }

    #[test]
    fn srcinfo_mirrors_the_pkgbuild() {
        let package = server(&parse(&sums()).unwrap(), "1.2.3", REPO).unwrap();
        let srcinfo = srcinfo(&package, "1.2.3");
        assert!(srcinfo.starts_with("pkgbase = sdrmm-bin\n"));
        assert!(srcinfo.ends_with("\npkgname = sdrmm-bin\n"));
        assert!(srcinfo.contains("\tarch = aarch64\n"));
        assert!(srcinfo.contains(&format!("\tsha256sums_aarch64 = {}\n", "e".repeat(64))));
        assert!(
            pkgbuild(&package, "1.2.3").contains("cd \"sdrmm-1.2.3-$CARCH-unknown-linux-gnu\"")
        );
    }

    #[test]
    fn a_prerelease_version_is_a_valid_pkgver() {
        assert_eq!(pkgver("1.6.0-rc.1"), "1.6.0_rc.1");
    }

    #[test]
    fn a_release_missing_an_arch_is_refused() {
        let sums = sums().replace(
            &format!(
                "{}  sdrmm-1.2.3-aarch64-unknown-linux-gnu.tar.gz",
                "e".repeat(64)
            ),
            "",
        );
        let err = server(&parse(&sums).unwrap(), "1.2.3", REPO)
            .err()
            .unwrap()
            .to_string();
        assert!(
            err.contains("sdrmm-1.2.3-aarch64-unknown-linux-gnu.tar.gz"),
            "{err}"
        );
    }
}
