//! Environment diagnostics (PLAN §15: `sdrmm --doctor` prints what's found — backends, USB
//! permissions, paths). Split in two on purpose: [`collect`] does the I/O and is untestable
//! without hardware, [`render`] is a pure function over the report and is not (PLAN §14: no
//! hardware in CI, ever). The same [`DoctorReport`] is served as `GET /api/doctor`, so the
//! CLI and the web UI never disagree about what is wrong.

use std::path::Path;

use sdrmm_wire::{CheckStatus, DoctorCheck, DoctorReport};

/// Build the report: compiled backends, device probe, storage paths, USB permissions.
///
/// Must not run while a server is live in the same process: probing enumerates every backend,
/// and `device-soapy` documents that overlapping enumerates crash inside libusb. The CLI path
/// exits before `serve()`; the REST path reuses the engine's own registry instead of building
/// a second one.
#[must_use]
pub fn collect(db_path: Option<&Path>, recordings_dir: Option<&Path>) -> DoctorReport {
    let registry = sdrmm_engine::builtin_registry(None);
    report(&registry, db_path, recordings_dir)
}

/// [`collect`] against an existing registry — what the server uses so it never enumerates
/// twice at once.
#[must_use]
pub fn report(
    registry: &sdrmm_device::DeviceRegistry,
    db_path: Option<&Path>,
    recordings_dir: Option<&Path>,
) -> DoctorReport {
    let mut checks = vec![backends_check(registry)];
    checks.push(devices_check(registry));
    checks.extend(usb_checks());
    checks.push(path_check(
        "storage.db",
        "Database",
        db_path,
        PathKind::File,
        "presets, bookmarks, recordings index and decoder log are kept in memory and lost on \
         exit",
    ));
    checks.push(path_check(
        "storage.recordings",
        "Recordings directory",
        recordings_dir,
        PathKind::Directory,
        "recording is disabled",
    ));
    DoctorReport {
        version: env!("CARGO_PKG_VERSION").to_string(),
        platform: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        checks,
    }
}

/// Which backends this build can open devices through. Derived from the registry, so it
/// cannot drift from what the server actually registers.
fn backends_check(registry: &sdrmm_device::DeviceRegistry) -> DoctorCheck {
    let mut ids: Vec<&str> = registry
        .driver_ids()
        .into_iter()
        .map(|(_, id)| id)
        .collect();
    ids.sort_unstable();
    let hardware: Vec<&str> = ids.iter().copied().filter(|id| *id != "virtual").collect();
    let detail = format!("compiled backends: {}", ids.join(", "));
    if hardware.is_empty() {
        return DoctorCheck {
            id: "backends".to_string(),
            name: "Device backends".to_string(),
            status: CheckStatus::Warn,
            detail,
            hint: Some(
                "this build has no hardware backend — only the signal generator and SigMF \
                 playback. Rebuild with --features rtl-native,hackrf-native (or soapy)."
                    .to_string(),
            ),
        };
    }
    DoctorCheck {
        id: "backends".to_string(),
        name: "Device backends".to_string(),
        status: CheckStatus::Ok,
        detail,
        hint: None,
    }
}

fn devices_check(registry: &sdrmm_device::DeviceRegistry) -> DoctorCheck {
    let devices = registry.probe_all();
    // The virtual driver always probes at least the signal generator, so "only virtual" is
    // the honest way to say "no hardware was found".
    let hardware: Vec<String> = devices
        .iter()
        .filter(|d| d.driver != "virtual")
        .map(|d| match &d.serial {
            Some(serial) => format!("{} [{}] serial {serial}", d.id(), d.label),
            None => format!("{} [{}]", d.id(), d.label),
        })
        .collect();
    if hardware.is_empty() {
        return DoctorCheck {
            id: "devices".to_string(),
            name: "Devices found".to_string(),
            status: CheckStatus::Warn,
            detail: format!("no hardware; {} virtual device(s)", devices.len()),
            hint: Some(
                "check the USB connection, then the USB permission line below; \
                 `lsusb`/`system_profiler SPUSBDataType` shows whether the OS sees it at all"
                    .to_string(),
            ),
        };
    }
    DoctorCheck {
        id: "devices".to_string(),
        name: "Devices found".to_string(),
        status: CheckStatus::Ok,
        detail: hardware.join("\n"),
        hint: None,
    }
}

/// USB access is the single most common reason a plugged-in SDR does not appear. Only Linux
/// has a rule to point at; macOS grants USB access to any process.
fn usb_checks() -> Vec<DoctorCheck> {
    #[cfg(target_os = "linux")]
    {
        const RULES: &[&str] = &[
            "/etc/udev/rules.d/rtl-sdr.rules",
            "/lib/udev/rules.d/rtl-sdr.rules",
            "/usr/lib/udev/rules.d/rtl-sdr.rules",
            "/etc/udev/rules.d/53-hackrf.rules",
            "/lib/udev/rules.d/53-hackrf.rules",
            "/usr/lib/udev/rules.d/53-hackrf.rules",
        ];
        let found: Vec<&str> = RULES
            .iter()
            .copied()
            .filter(|p| Path::new(p).exists())
            .collect();
        let root = unsafe { libc::geteuid() } == 0;
        if !found.is_empty() || root {
            return vec![DoctorCheck {
                id: "usb.permissions".to_string(),
                name: "USB permissions".to_string(),
                status: CheckStatus::Ok,
                detail: if found.is_empty() {
                    "running as root".to_string()
                } else {
                    format!("udev rules: {}", found.join(", "))
                },
                hint: None,
            }];
        }
        vec![DoctorCheck {
            id: "usb.permissions".to_string(),
            name: "USB permissions".to_string(),
            status: CheckStatus::Warn,
            detail: "no RTL-SDR or HackRF udev rules found and not running as root".to_string(),
            hint: Some(
                "install the vendor udev rules (rtl-sdr and hackrf packages ship them), then \
                 `sudo udevadm control --reload-rules && sudo udevadm trigger` and replug the \
                 device. Without them the device enumerates but cannot be opened."
                    .to_string(),
            ),
        }]
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

/// Whether a configured path names a file or the directory itself. Never inferred from the
/// extension: `--db /data/sdrmm` would be read as a directory and the probe would *create* one
/// where the database belongs, and a recordings directory called `sdr.captures` would be read
/// as a file and its parent probed instead. The caller always knows which it is.
#[derive(Clone, Copy)]
enum PathKind {
    File,
    Directory,
}

fn path_check(
    id: &str,
    name: &str,
    path: Option<&Path>,
    kind: PathKind,
    absent_consequence: &str,
) -> DoctorCheck {
    let Some(path) = path else {
        return DoctorCheck {
            id: id.to_string(),
            name: name.to_string(),
            status: CheckStatus::Warn,
            detail: format!("not configured — {absent_consequence}"),
            hint: None,
        };
    };
    // The directory is what has to be writable: the file itself may not exist yet, and the
    // engine creates the recordings directory on the first recording.
    let dir = match kind {
        PathKind::File => path.parent().unwrap_or(path),
        PathKind::Directory => path,
    };
    let (status, detail, hint) = match writable(dir) {
        Ok(()) => (CheckStatus::Ok, path.display().to_string(), None),
        Err(e) => (
            CheckStatus::Fail,
            format!("{} is not writable: {e}", dir.display()),
            Some("fix the directory's ownership, or pass an explicit path".to_string()),
        ),
    };
    DoctorCheck {
        id: id.to_string(),
        name: name.to_string(),
        status,
        detail,
        hint,
    }
}

/// Probe writability by actually creating and removing a file: permission bits alone lie on
/// read-only mounts, ACLs and container overlays.
fn writable(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let probe = dir.join(".sdrmm-doctor-probe");
    std::fs::write(&probe, b"")?;
    std::fs::remove_file(&probe)
}

/// Render for a terminal. Pure, so it is the part that gets tested.
#[must_use]
pub fn render(report: &DoctorReport) -> String {
    let mut out = format!("sdr-- {} ({})\n\n", report.version, report.platform);
    for check in &report.checks {
        let mark = match check.status {
            CheckStatus::Ok => "ok  ",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "FAIL",
        };
        out.push_str(&format!("[{mark}] {}\n", check.name));
        for line in check.detail.lines() {
            out.push_str(&format!("       {line}\n"));
        }
        if let Some(hint) = &check.hint {
            out.push_str(&format!("       → {hint}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(id: &str, status: CheckStatus, hint: Option<&str>) -> DoctorCheck {
        DoctorCheck {
            id: id.to_string(),
            name: format!("Check {id}"),
            status,
            detail: "line one\nline two".to_string(),
            hint: hint.map(str::to_string),
        }
    }

    #[test]
    fn render_lists_every_check_with_its_detail_lines_and_hints() {
        let report = DoctorReport {
            version: "1.2.3".to_string(),
            platform: "linux/aarch64".to_string(),
            checks: vec![
                check("a", CheckStatus::Ok, None),
                check("b", CheckStatus::Warn, Some("do the thing")),
                check("c", CheckStatus::Fail, None),
            ],
        };
        let text = render(&report);
        assert!(text.starts_with("sdr-- 1.2.3 (linux/aarch64)"));
        assert!(text.contains("[ok  ] Check a"));
        assert!(text.contains("[warn] Check b"));
        assert!(text.contains("[FAIL] Check c"));
        // Multi-line details must stay aligned under their check, not collapse into one line.
        assert_eq!(text.matches("       line one").count(), 3);
        assert_eq!(text.matches("       line two").count(), 3);
        assert!(text.contains("       → do the thing"));
    }

    /// A missing path is a warning with the consequence spelled out, not a bare "none".
    #[test]
    fn path_check_reports_absence_and_writability() {
        let absent = path_check("x", "X", None, PathKind::File, "nothing persists");
        assert_eq!(absent.status, CheckStatus::Warn);
        assert!(absent.detail.contains("nothing persists"));

        let dir = tempfile::TempDir::new().expect("tempdir");
        let ok = path_check("x", "X", Some(dir.path()), PathKind::Directory, "unused");
        assert_eq!(ok.status, CheckStatus::Ok, "{}", ok.detail);

        // A file path is judged by the directory that has to hold it — the file itself is
        // created later, on first use.
        let file = dir.path().join("sub").join("sdrmm.db");
        let ok = path_check("x", "X", Some(&file), PathKind::File, "unused");
        assert_eq!(ok.status, CheckStatus::Ok, "{}", ok.detail);
        assert!(file.parent().is_some_and(Path::exists));
        assert!(
            !file.exists(),
            "the probe must not create the database itself"
        );
    }

    /// The old heuristic keyed on the extension: an extension-less database path was read as a
    /// directory and *created* as one, and a dotted recordings directory had its parent probed.
    #[test]
    fn path_kind_is_told_not_guessed() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let db = root.path().join("sdrmm");
        let ok = path_check("db", "DB", Some(&db), PathKind::File, "unused");
        assert_eq!(ok.status, CheckStatus::Ok, "{}", ok.detail);
        assert!(
            !db.exists(),
            "an extension-less db path must not become a directory"
        );

        let recordings = root.path().join("sdr.captures");
        let ok = path_check(
            "rec",
            "Rec",
            Some(&recordings),
            PathKind::Directory,
            "unused",
        );
        assert_eq!(ok.status, CheckStatus::Ok, "{}", ok.detail);
        assert!(
            recordings.is_dir(),
            "a dotted directory must be probed itself"
        );
    }

    /// A build with only the virtual driver can decode recordings but cannot receive; that
    /// has to read as a warning with a way out, not as "all good".
    #[test]
    fn backends_check_warns_when_only_the_virtual_driver_is_compiled_in() {
        let mut registry = sdrmm_device::DeviceRegistry::new();
        registry.register(10, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
        let check = backends_check(&registry);
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.detail.contains("virtual"));
        assert!(check.hint.is_some_and(|h| h.contains("rtl-native")));
    }
}
