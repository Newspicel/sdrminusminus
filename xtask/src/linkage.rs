use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, ensure};

const SYSTEM_PREFIXES: [&str; 3] = ["/usr/lib/", "/System/", "/Library/Frameworks/"];

const MAGICS: [[u8; 4]; 4] = [
    [0xcf, 0xfa, 0xed, 0xfe],
    [0xfe, 0xed, 0xfa, 0xcf],
    [0xca, 0xfe, 0xba, 0xbe],
    [0xbe, 0xba, 0xfe, 0xca],
];

pub fn check(path: &Path, external: &[String]) -> Result<()> {
    ensure!(path.exists(), "{} does not exist", path.display());
    let images = mach_o_under(path)?;
    ensure!(
        !images.is_empty(),
        "{} holds no Mach-O files — this check reads macOS artifacts",
        path.display()
    );
    let executable_dir = executable_dir(path);

    let mut edges = 0usize;
    let mut failures = Vec::new();
    for image in &images {
        let loaded = Image::read(image)?;
        let loader_dir = image.parent().unwrap_or(Path::new("."));
        for dependency in &loaded.dependencies {
            edges += 1;
            if external
                .iter()
                .any(|fragment| leaf(dependency).contains(fragment.as_str()))
            {
                continue;
            }
            if let Err(tried) = resolve(dependency, loader_dir, &executable_dir, &loaded.rpaths) {
                let tried = if dependency.starts_with('/') {
                    "an absolute path off the build machine, which no install has".to_string()
                } else if tried.is_empty() {
                    "it names no path the loader would search".to_string()
                } else {
                    format!(
                        "tried {}",
                        tried
                            .iter()
                            .map(|path| path.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                failures.push(format!(
                    "{}\n    needs {dependency}\n    {tried}",
                    image.display()
                ));
            }
        }
    }

    ensure!(
        failures.is_empty(),
        "{} would not launch — {} unresolved {}:\n  {}\n\nA dependency outside \
         /usr/lib and /System has to travel with the artifact. This is what an install sees, so \
         it fails here rather than on the first machine that is not the one that built it.",
        path.display(),
        failures.len(),
        if failures.len() == 1 {
            "dependency"
        } else {
            "dependencies"
        },
        failures.join("\n  ")
    );
    println!(
        "link closure: {} dependencies across {} Mach-O files in {} all resolve",
        edges,
        images.len(),
        path.display()
    );
    Ok(())
}

pub fn rpaths(path: &Path) -> Result<Vec<String>> {
    Ok(Image::read(path)?.rpaths)
}

fn executable_dir(path: &Path) -> PathBuf {
    if path.extension().is_some_and(|ext| ext == "app") {
        return path.join("Contents/MacOS");
    }
    if path.is_dir() {
        return path.to_path_buf();
    }
    path.parent().unwrap_or(Path::new(".")).to_path_buf()
}

fn resolve(
    dependency: &str,
    loader_dir: &Path,
    executable_dir: &Path,
    rpaths: &[String],
) -> Result<PathBuf, Vec<PathBuf>> {
    if SYSTEM_PREFIXES
        .iter()
        .any(|prefix| dependency.starts_with(prefix))
    {
        return Ok(PathBuf::from(dependency));
    }
    if let Some(rest) = dependency.strip_prefix("@rpath/") {
        let mut tried = Vec::new();
        for rpath in rpaths {
            let candidate = expand(rpath, loader_dir, executable_dir).join(rest);
            if candidate.exists() {
                return Ok(candidate);
            }
            tried.push(candidate);
        }
        return Err(tried);
    }
    if dependency.starts_with('/') {
        return Err(vec![PathBuf::from(dependency)]);
    }
    let candidate = expand(dependency, loader_dir, executable_dir);
    if candidate.exists() {
        Ok(candidate)
    } else {
        Err(vec![candidate])
    }
}

fn expand(path: &str, loader_dir: &Path, executable_dir: &Path) -> PathBuf {
    for (variable, base) in [
        ("@loader_path", loader_dir),
        ("@executable_path", executable_dir),
    ] {
        if path == variable {
            return base.to_path_buf();
        }
        if let Some(rest) = path.strip_prefix(&format!("{variable}/")) {
            return base.join(rest);
        }
    }
    PathBuf::from(path)
}

fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

struct Image {
    dependencies: Vec<String>,
    rpaths: Vec<String>,
}

impl Image {
    fn read(path: &Path) -> Result<Self> {
        let out = Command::new("otool")
            .arg("-l")
            .arg(path)
            .output()
            .with_context(|| format!("otool -l {}", path.display()))?;
        ensure!(
            out.status.success(),
            "otool -l {} failed: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(Self::parse(&String::from_utf8_lossy(&out.stdout)))
    }

    fn parse(listing: &str) -> Self {
        let mut dependencies = BTreeSet::new();
        let mut rpaths = Vec::new();
        let mut command = "";
        for line in listing.lines() {
            let line = line.trim();
            if let Some(name) = line.strip_prefix("cmd ") {
                command = name.trim();
            } else if let Some(value) = line.strip_prefix("name ") {
                if matches!(
                    command,
                    "LC_LOAD_DYLIB" | "LC_LOAD_WEAK_DYLIB" | "LC_REEXPORT_DYLIB"
                ) {
                    dependencies.insert(strip_offset(value));
                }
            } else if let Some(value) = line.strip_prefix("path ")
                && command == "LC_RPATH"
            {
                let path = strip_offset(value);
                if !rpaths.contains(&path) {
                    rpaths.push(path);
                }
            }
        }
        Self {
            dependencies: dependencies.into_iter().collect(),
            rpaths,
        }
    }
}

fn strip_offset(value: &str) -> String {
    match value.rfind(" (offset ") {
        Some(at) => value[..at].to_string(),
        None => value.trim().to_string(),
    }
}

fn mach_o_under(path: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    collect(path, &mut found)?;
    found.sort();
    Ok(found)
}

fn collect(path: &Path, found: &mut Vec<PathBuf>) -> Result<()> {
    if path.is_symlink() {
        return Ok(());
    }
    if path.is_dir() {
        for entry in std::fs::read_dir(path).with_context(|| format!("read {}", path.display()))? {
            collect(&entry?.path(), found)?;
        }
        return Ok(());
    }
    if is_mach_o(path)? {
        found.push(path.to_path_buf());
    }
    Ok(())
}

fn is_mach_o(path: &Path) -> Result<bool> {
    use std::io::Read;

    let mut head = [0u8; 4];
    let mut file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    match file.read_exact(&mut head) {
        Ok(()) => Ok(MAGICS.contains(&head)),
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(err) => Err(err).with_context(|| format!("read {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = "
Load command 8
          cmd LC_ID_DYLIB
      cmdsize 56
         name @rpath/libSoapySDR.0.8.dylib (offset 24)
Load command 9
          cmd LC_LOAD_DYLIB
      cmdsize 56
         name /usr/lib/libSystem.B.dylib (offset 24)
Load command 10
          cmd LC_LOAD_DYLIB
      cmdsize 56
         name @rpath/libiconv.2.dylib (offset 24)
Load command 11
          cmd LC_LOAD_WEAK_DYLIB
      cmdsize 56
         name @rpath/libusb-1.0.0.dylib (offset 24)
Load command 12
          cmd LC_RPATH
      cmdsize 48
         path @executable_path/../Resources/soapy/lib (offset 12)
Load command 13
          cmd LC_RPATH
      cmdsize 48
         path @loader_path/.. (offset 12)
";

    #[test]
    fn reads_what_an_image_loads_and_ignores_its_own_name() {
        let image = Image::parse(LISTING);
        assert_eq!(
            image.dependencies,
            [
                "/usr/lib/libSystem.B.dylib",
                "@rpath/libiconv.2.dylib",
                "@rpath/libusb-1.0.0.dylib",
            ]
        );
        assert_eq!(
            image.rpaths,
            ["@executable_path/../Resources/soapy/lib", "@loader_path/.."]
        );
    }

    #[test]
    fn dedupes_the_slices_of_a_universal_binary() {
        let image = Image::parse(&format!("{LISTING}{LISTING}"));
        assert_eq!(image.dependencies.len(), 3);
        assert_eq!(image.rpaths.len(), 2);
    }

    #[test]
    fn shared_cache_paths_need_no_file() {
        let system = Path::new("/nonexistent");
        assert!(resolve("/usr/lib/libSystem.B.dylib", system, system, &[]).is_ok());
        assert!(
            resolve(
                "/System/Library/Frameworks/AppKit.framework/AppKit",
                system,
                system,
                &[]
            )
            .is_ok()
        );
    }

    #[test]
    fn missing_rpath_library_reports_every_path_the_loader_would_try() {
        let dir = std::env::temp_dir();
        let tried = resolve(
            "@rpath/libiconv.2.dylib",
            &dir,
            &dir,
            &["@executable_path/../Resources/soapy/lib".to_string()],
        )
        .unwrap_err();
        assert_eq!(tried, [dir.join("../Resources/soapy/lib/libiconv.2.dylib")]);
    }

    #[test]
    fn an_rpath_library_that_is_there_resolves() {
        let dir = tempdir("linkage-rpath");
        std::fs::write(dir.join("libSoapySDR.0.8.dylib"), b"x").unwrap();
        let found = resolve(
            "@rpath/libSoapySDR.0.8.dylib",
            &dir,
            &dir,
            &["@loader_path".to_string()],
        )
        .unwrap();
        assert_eq!(found, dir.join("libSoapySDR.0.8.dylib"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_absolute_path_outside_the_system_is_the_build_machine() {
        let dir = std::env::temp_dir();
        assert!(resolve("/opt/homebrew/lib/libSoapySDR.0.8.dylib", &dir, &dir, &[]).is_err());
    }

    fn tempdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
