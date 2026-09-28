use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result, bail};

pub(crate) const ANDROID_NOTICES: &str = "apps/android/app/src/main/assets/NOTICES.txt";
const VERSION_CATALOG: &str = "apps/android/gradle/libs.versions.toml";
const LICENSE_DIR: &str = "packaging/licenses";
const APACHE: &str = "Apache-2.0.txt";

struct Component {
    name: &'static str,
    version: Option<&'static str>,
    license: &'static str,
    url: &'static str,
    note: Option<&'static str>,
    text: Option<&'static str>,
}

const COMPONENTS: &[Component] = &[
    Component {
        name: "Jetpack Compose",
        version: Some("composeBom"),
        license: "Apache-2.0",
        url: "https://developer.android.com/jetpack/androidx/releases/compose",
        note: None,
        text: None,
    },
    Component {
        name: "AndroidX Activity, Core, Lifecycle, Navigation 3, DataStore",
        version: None,
        license: "Apache-2.0",
        url: "https://developer.android.com/jetpack/androidx",
        note: None,
        text: None,
    },
    Component {
        name: "CameraX",
        version: Some("camerax"),
        license: "Apache-2.0",
        url: "https://developer.android.com/jetpack/androidx/releases/camera",
        note: None,
        text: None,
    },
    Component {
        name: "ZXing",
        version: Some("zxing"),
        license: "Apache-2.0",
        url: "https://github.com/zxing/zxing",
        note: None,
        text: None,
    },
    Component {
        name: "MapLibre Native",
        version: Some("maplibre"),
        license: "BSD-2-Clause",
        url: "https://github.com/maplibre/maplibre-native",
        note: Some("Its notices, including the components it bundles, follow below."),
        text: Some("MapLibre-Native-LICENSES.core.md"),
    },
    Component {
        name: "Car App Library",
        version: Some("carApp"),
        license: "Apache-2.0",
        url: "https://developer.android.com/jetpack/androidx/releases/car-app",
        note: None,
        text: None,
    },
    Component {
        name: "JNA",
        version: Some("jna"),
        license: "Apache-2.0",
        url: "https://github.com/java-native-access/jna",
        note: Some(
            "Offered under LGPL-2.1-or-later or Apache-2.0. SDR-- uses it under Apache-2.0.",
        ),
        text: None,
    },
    Component {
        name: "kotlinx-coroutines",
        version: Some("coroutines"),
        license: "Apache-2.0",
        url: "https://github.com/Kotlin/kotlinx.coroutines",
        note: None,
        text: None,
    },
    Component {
        name: "Material Symbols",
        version: None,
        license: "Apache-2.0",
        url: "https://github.com/google/material-design-icons",
        note: Some("App icons."),
        text: None,
    },
    Component {
        name: "Kotlin standard library",
        version: Some("kotlin"),
        license: "Apache-2.0",
        url: "https://github.com/JetBrains/kotlin",
        note: None,
        text: None,
    },
    Component {
        name: "MapLibre Java (GeoJSON, Turf)",
        version: None,
        license: "MIT",
        url: "https://github.com/maplibre/maplibre-java",
        note: None,
        text: Some("MapLibre-Java-MIT.txt"),
    },
    Component {
        name: "MapLibre Gestures",
        version: None,
        license: "BSD-2-Clause",
        url: "https://github.com/maplibre/maplibre-gestures-android",
        note: None,
        text: Some("MapLibre-Gestures-BSD-2-Clause.md"),
    },
    Component {
        name: "OkHttp, Okio, Timber",
        version: None,
        license: "Apache-2.0",
        url: "https://square.github.io/okhttp/",
        note: Some("Pulled in by MapLibre Native."),
        text: None,
    },
    Component {
        name: "Guava, Gson, Dagger, javax.inject, Jakarta Inject, JSpecify, JSR 305, and the \
               AutoValue, Error Prone, J2ObjC and JetBrains annotations",
        version: None,
        license: "Apache-2.0",
        url: "https://github.com/google/guava",
        note: Some("Pulled in by AndroidX and MapLibre Native."),
        text: None,
    },
    Component {
        name: "Checker Framework qualifiers",
        version: None,
        license: "MIT",
        url: "https://github.com/typetools/checker-framework",
        note: None,
        text: Some("checker-qual-MIT.txt"),
    },
];

const MAP_DATA: &[(&str, &str, &str)] = &[
    (
        "OpenFreeMap",
        "https://openfreemap.org",
        "Map tiles and styles served by OpenFreeMap.",
    ),
    (
        "OpenMapTiles",
        "https://openmaptiles.org",
        "Tile schema and styles © OpenMapTiles, BSD-3-Clause code and CC-BY-4.0 design.",
    ),
    (
        "OpenStreetMap",
        "https://www.openstreetmap.org/copyright",
        "Map data © OpenStreetMap contributors, available under the Open Database License.",
    ),
];

pub(crate) fn notices(root: &Path) -> Result<String> {
    let catalog = std::fs::read_to_string(root.join(VERSION_CATALOG))
        .with_context(|| format!("read {VERSION_CATALOG}"))?;
    let versions = catalog_versions(&catalog);
    let license = |file: &str| -> Result<String> {
        let path = root.join(LICENSE_DIR).join(file);
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        Ok(text.trim_start_matches(['\n', '\r']).trim_end().to_owned())
    };
    let mut out = String::from(
        "SDR-- for Android ships these components beside the Rust core listed above.\n\n",
    );
    for component in COMPONENTS {
        out.push_str(&component_block(component, &versions)?);
        out.push_str("\n\n");
    }
    for (name, url, note) in MAP_DATA {
        out.push_str(&format!("{name}\n{url}\n{note}\n\n"));
    }
    out.push_str("Apache License 2.0\n\n");
    out.push_str(&license(APACHE)?);
    for component in COMPONENTS {
        if let Some(file) = component.text {
            out.push_str(&format!("\n\n{} license\n\n", component.name));
            out.push_str(&license(file)?);
        }
    }
    out.push('\n');
    Ok(out)
}

fn component_block(component: &Component, versions: &BTreeMap<String, String>) -> Result<String> {
    let title = match component.version {
        Some(key) => {
            let Some(version) = versions.get(key) else {
                bail!(
                    "{VERSION_CATALOG} names no version `{key}` for {}",
                    component.name
                );
            };
            format!("{} {version}", component.name)
        }
        None => component.name.to_owned(),
    };
    let mut block = format!("{title}\n{}\n{}", component.license, component.url);
    if let Some(note) = component.note {
        block.push('\n');
        block.push_str(note);
    }
    Ok(block)
}

fn catalog_versions(catalog: &str) -> BTreeMap<String, String> {
    let mut versions = BTreeMap::new();
    let mut inside = false;
    for line in catalog.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == "[versions]";
            continue;
        }
        if !inside {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        versions.insert(key.trim().to_owned(), value.to_owned());
    }
    versions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_versions_come_from_the_versions_table_only() {
        let catalog = "[versions]\nzxing = \"3.5.4\"\njna = \"5.19.1\"\n\n[libraries]\nzxing = { module = \"x\" }\n";
        let versions = catalog_versions(catalog);
        assert_eq!(versions.get("zxing").map(String::as_str), Some("3.5.4"));
        assert_eq!(versions.get("jna").map(String::as_str), Some("5.19.1"));
        assert_eq!(versions.len(), 2);
    }

    #[test]
    fn a_missing_catalog_version_is_an_error() {
        let error = component_block(&COMPONENTS[3], &BTreeMap::new())
            .expect_err("missing version refused")
            .to_string();
        assert!(error.contains("`zxing`"), "{error}");
    }

    #[test]
    fn android_notices_name_every_bundled_component() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root");
        let text = notices(root).expect("notices build");
        for name in [
            "Jetpack Compose",
            "AndroidX",
            "CameraX",
            "ZXing",
            "MapLibre Native",
            "Car App Library",
            "JNA",
            "kotlinx-coroutines",
            "OpenFreeMap",
            "OpenMapTiles",
            "OpenStreetMap",
            "Apache License",
            "BSD 2-Clause License",
            "MapLibre Gestures license",
            "Checker Framework qualifiers license",
            "OkHttp",
            "Material Symbols",
        ] {
            assert!(text.contains(name), "{name} missing");
        }
        assert!(!text.contains('\u{2014}'));
    }
}
