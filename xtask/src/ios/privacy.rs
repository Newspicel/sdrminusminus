use std::collections::BTreeSet;

use anyhow::{Result, bail};

const FILE_TIMESTAMP: &str = "NSPrivacyAccessedAPICategoryFileTimestamp";
const BOOT_TIME: &str = "NSPrivacyAccessedAPICategorySystemBootTime";
const DISK_SPACE: &str = "NSPrivacyAccessedAPICategoryDiskSpace";

const EXACT: [(&str, &str); 9] = [
    ("stat", FILE_TIMESTAMP),
    ("fstat", FILE_TIMESTAMP),
    ("lstat", FILE_TIMESTAMP),
    ("fstatat", FILE_TIMESTAMP),
    ("mach_absolute_time", BOOT_TIME),
    ("statfs", DISK_SPACE),
    ("statvfs", DISK_SPACE),
    ("fstatfs", DISK_SPACE),
    ("fstatvfs", DISK_SPACE),
];
const GETATTRLIST: &str = "getattrlist";

pub(crate) fn required_categories(undefined_symbols: &str) -> BTreeSet<&'static str> {
    undefined_symbols
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .filter_map(category)
        .collect()
}

fn category(symbol: &str) -> Option<&'static str> {
    let name = symbol.strip_prefix('_').unwrap_or(symbol);
    let name = name.split('$').next().unwrap_or(name);
    if name.starts_with(GETATTRLIST) {
        return Some(FILE_TIMESTAMP);
    }
    EXACT
        .iter()
        .find(|(exact, _)| *exact == name)
        .map(|(_, category)| *category)
}

pub(crate) fn check_manifest(required: &BTreeSet<&'static str>, manifest: &str) -> Result<()> {
    let missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|category| !manifest.contains(category))
        .collect();
    if !missing.is_empty() {
        bail!(
            "Support/PrivacyInfo.xcprivacy lacks {} used by the Rust core",
            missing.join(", ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NM: &str = "                 U _fstat$INODE64\n                 U _getattrlistbulk\n                 U _malloc\n                 U _mach_absolute_time\n";

    #[test]
    fn nm_symbols_map_to_privacy_categories() {
        let required = required_categories(NM);
        assert_eq!(
            required.into_iter().collect::<Vec<_>>(),
            vec![FILE_TIMESTAMP, BOOT_TIME]
        );
        assert!(required_categories("U _statvfs\n").contains(DISK_SPACE));
        assert!(required_categories("U _stat_other\n").is_empty());
    }

    #[test]
    fn a_missing_category_fails_the_archive() {
        let required = required_categories(NM);
        let manifest = format!("<string>{FILE_TIMESTAMP}</string>");
        let error = check_manifest(&required, &manifest).expect_err("boot time missing");
        assert!(error.to_string().contains(BOOT_TIME), "{error}");
        let full = format!("{manifest}<string>{BOOT_TIME}</string>");
        check_manifest(&required, &full).expect("complete manifest");
    }
}
