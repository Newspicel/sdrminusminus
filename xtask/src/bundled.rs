use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

pub fn libraries(root: &Path, target: Option<&str>) -> Result<Vec<PathBuf>> {
    let triple = target.map_or_else(crate::host_triple, |triple| Ok(triple.to_owned()))?;
    match crate::media_dir(root, target)? {
        Some(media) => media_runtime(&media, &triple),
        None => Ok(Vec::new()),
    }
}

fn media_runtime(media: &Path, triple: &str) -> Result<Vec<PathBuf>> {
    let dir = if triple.contains("windows") {
        media.join("bin")
    } else {
        media.join("lib")
    };
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry?.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if is_runtime_name(&name, triple) {
            found.push(path);
        }
    }
    found.sort();
    anyhow::ensure!(
        !found.is_empty(),
        "{} holds no shared FFmpeg libraries",
        dir.display()
    );
    Ok(found)
}

fn is_runtime_name(name: &str, triple: &str) -> bool {
    let major = |rest: &str| !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit());
    if triple.contains("windows") {
        name.ends_with(".dll")
    } else if triple.contains("apple") {
        name.strip_prefix("lib")
            .and_then(|name| name.strip_suffix(".dylib"))
            .and_then(|stem| stem.split_once('.'))
            .is_some_and(|(_, version)| major(version))
    } else {
        name.strip_prefix("lib")
            .and_then(|name| name.split_once(".so."))
            .is_some_and(|(_, version)| major(version))
    }
}

pub fn stage(libraries: &[PathBuf], dir: &Path) -> Result<()> {
    for library in libraries {
        let name = library.file_name().context("library has no file name")?;
        std::fs::copy(library, dir.join(name))
            .with_context(|| format!("cannot stage {}", library.display()))?;
    }
    Ok(())
}

pub fn desktop_config_for(root: &Path, target: Option<&str>) -> Result<String> {
    let triple = target.map_or_else(crate::host_triple, |triple| Ok(triple.to_owned()))?;
    Ok(desktop_config(&libraries(root, target)?, &triple))
}

pub fn write_desktop_config(root: &Path, target: Option<&str>, out: &Path) -> Result<()> {
    std::fs::write(out, desktop_config_for(root, target)?)
        .with_context(|| format!("write {}", out.display()))
}

fn desktop_config(libraries: &[PathBuf], triple: &str) -> String {
    let sources = libraries
        .iter()
        .map(|path| path.to_string_lossy().into_owned());
    let bundle = if triple.contains("apple") {
        json!({ "macOS": { "frameworks": sources.collect::<Vec<_>>() } })
    } else {
        let resources: Map<String, Value> = libraries
            .iter()
            .zip(sources)
            .map(|(path, source)| {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                (source, Value::from(name.into_owned()))
            })
            .collect();
        json!({ "resources": resources })
    };
    json!({ "bundle": bundle }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_names_the_loader_asks_for_are_shipped() {
        let mac = "aarch64-apple-darwin";
        assert!(is_runtime_name("libavcodec.63.dylib", mac));
        assert!(!is_runtime_name("libavcodec.dylib", mac));
        assert!(!is_runtime_name("libavcodec.63.1.101.dylib", mac));
        let linux = "x86_64-unknown-linux-gnu";
        assert!(is_runtime_name("libavcodec.so.63", linux));
        assert!(!is_runtime_name("libavcodec.so", linux));
        assert!(!is_runtime_name("libavcodec.so.63.1.101", linux));
        assert!(is_runtime_name("avcodec-63.dll", "x86_64-pc-windows-msvc"));
    }

    #[test]
    fn macos_bundles_libraries_as_frameworks() {
        let libraries = [PathBuf::from("/t/libavcodec.63.dylib")];
        let config = desktop_config(&libraries, "aarch64-apple-darwin");
        let value: Value = serde_json::from_str(&config).unwrap();
        assert_eq!(
            value["bundle"]["macOS"]["frameworks"][0],
            "/t/libavcodec.63.dylib"
        );
    }

    #[test]
    fn windows_and_linux_install_them_as_resources() {
        for (triple, name) in [
            ("x86_64-pc-windows-msvc", "avcodec-63.dll"),
            ("x86_64-unknown-linux-gnu", "libavcodec.so.63"),
        ] {
            let source = format!("/t/{name}");
            let config = desktop_config(&[PathBuf::from(&source)], triple);
            let value: Value = serde_json::from_str(&config).unwrap();
            assert_eq!(value["bundle"]["resources"][&source], name);
        }
    }
}
