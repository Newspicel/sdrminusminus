use std::path::Path;

use sdrmm_wire::{CheckStatus, DoctorCheck, DoctorReport};

#[must_use]
pub fn collect(db_path: Option<&Path>, recordings_dir: Option<&Path>) -> DoctorReport {
    let registry = sdrmm_engine::builtin_registry(None);
    report(&registry, db_path, recordings_dir)
}

#[must_use]
pub fn report(
    registry: &sdrmm_device::DeviceRegistry,
    db_path: Option<&Path>,
    recordings_dir: Option<&Path>,
) -> DoctorReport {
    let mut checks = vec![backends_check(registry)];
    let devices = devices_check(registry);
    #[cfg(all(feature = "soapy", not(test)))]
    {
        let info = sdrmm_device_soapy::runtime_info();
        checks.push(soapy_check(&info));
    }
    checks.push(devices);
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
        let virtual_capabilities = virtual_capabilities(cfg!(debug_assertions));
        return DoctorCheck {
            id: "backends".to_string(),
            name: "Device backends".to_string(),
            status: CheckStatus::Warn,
            detail,
            hint: Some(format!(
                "this build has no hardware backend — only {virtual_capabilities}. Use a normal \
                 build, or rebuild with --features soapy."
            )),
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

fn virtual_capabilities(debug_build: bool) -> &'static str {
    if debug_build {
        "synthetic signal-generator and marker radios, plus SigMF playback through the virtual \
         driver"
    } else {
        "SigMF playback through the virtual driver"
    }
}

#[cfg(feature = "soapy")]
fn soapy_check(info: &sdrmm_device_soapy::RuntimeInfo) -> DoctorCheck {
    let module_names: Vec<String> = info
        .modules
        .iter()
        .map(|path| {
            Path::new(path)
                .file_name()
                .map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned())
        })
        .collect();
    let expected = ["rtlsdr", "hackrf"];
    let missing: Vec<&str> = expected
        .into_iter()
        .filter(|name| {
            !module_names
                .iter()
                .any(|module| module.to_ascii_lowercase().contains(name))
        })
        .collect();
    DoctorCheck {
        id: "soapy.runtime".to_string(),
        name: "SoapySDR runtime".to_string(),
        status: if missing.is_empty() {
            CheckStatus::Ok
        } else {
            CheckStatus::Warn
        },
        detail: format!(
            "core: {}\nmodule search path: {}\nloaded modules: {}",
            info.core_version,
            if info.search_paths.is_empty() {
                "(none)".to_string()
            } else {
                info.search_paths.join(", ")
            },
            if module_names.is_empty() {
                "(none)".to_string()
            } else {
                module_names.join(", ")
            }
        ),
        hint: (!missing.is_empty()).then(|| {
            format!(
                "missing bundled module(s): {}; reinstall the complete package",
                missing.join(", ")
            )
        }),
    }
}

fn devices_check(registry: &sdrmm_device::DeviceRegistry) -> DoctorCheck {
    let devices = registry.probe_all();
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

#[must_use]
pub fn rate_report(registry: &sdrmm_device::DeviceRegistry) -> DoctorReport {
    let checks = registry
        .probe_all()
        .into_iter()
        .filter(|d| d.driver != "virtual")
        .map(|info| match registry.open(&info.id()) {
            Ok((_, mut device)) => {
                let rates = device.capabilities().sample_rates.clone();
                let restore = device.settings().sample_rate;
                let held: Vec<(f64, Option<f64>)> = rates
                    .iter()
                    .map(|&rate| (rate, hold_rate(device.as_mut(), rate)))
                    .collect();
                if let Some(rate) = restore {
                    let _ = hold_rate(device.as_mut(), rate);
                }
                rate_check(&info.id(), &info.label, &held)
            }
            Err(error) => DoctorCheck {
                id: format!("rates.{}", info.id()),
                name: format!("Sample rates: {}", info.label),
                status: CheckStatus::Warn,
                detail: format!("could not open: {error}"),
                hint: Some("stop anything else using the radio, then run this again".to_string()),
            },
        })
        .collect();
    DoctorReport {
        version: env!("CARGO_PKG_VERSION").to_string(),
        platform: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        checks,
    }
}

fn hold_rate(device: &mut dyn sdrmm_device::SdrDevice, rate: f64) -> Option<f64> {
    let request = sdrmm_wire::DeviceSettings {
        sample_rate: Some(rate),
        ..sdrmm_wire::DeviceSettings::default()
    };
    device.apply(&request).ok()?;
    device.settings().sample_rate
}

const RATE_TOLERANCE: f64 = 1e-6;

fn rate_check(id: &str, label: &str, held: &[(f64, Option<f64>)]) -> DoctorCheck {
    if held.is_empty() {
        return DoctorCheck {
            id: format!("rates.{id}"),
            name: format!("Sample rates: {label}"),
            status: CheckStatus::Warn,
            detail: "the driver advertises no discrete rates to check".to_string(),
            hint: None,
        };
    }
    let mut lines = Vec::with_capacity(held.len());
    let mut bad = 0usize;
    for &(asked, got) in held {
        match got {
            Some(got) if ((got - asked) / asked).abs() <= RATE_TOLERANCE => {
                lines.push(format!("{:>12.0} Hz  ok", asked));
            }
            Some(got) => {
                bad += 1;
                lines.push(format!("{asked:>12.0} Hz  HELD {got:.0} Hz"));
            }
            None => {
                bad += 1;
                lines.push(format!("{asked:>12.0} Hz  refused"));
            }
        }
    }
    DoctorCheck {
        id: format!("rates.{id}"),
        name: format!("Sample rates: {label}"),
        status: if bad == 0 {
            CheckStatus::Ok
        } else {
            CheckStatus::Fail
        },
        detail: lines.join("\n"),
        hint: (bad > 0).then(|| {
            "this radio does not run at every rate it advertises; the rates marked above are \
             the ones to avoid"
                .to_string()
        }),
    }
}

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

fn writable(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let probe = dir.join(".sdrmm-doctor-probe");
    std::fs::write(&probe, b"")?;
    std::fs::remove_file(&probe)
}

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
    fn a_rate_the_radio_does_not_hold_fails_the_check() {
        let held = [
            (2_048_000.0, Some(2_048_000.0)),
            (2_400_000.0, Some(2_286_826.0)),
            (3_200_000.0, None),
        ];
        let check = rate_check("soapy:00000001", "Generic RTL2832U", &held);
        assert_eq!(check.status, CheckStatus::Fail);
        assert!(check.detail.contains("2048000 Hz  ok"), "{}", check.detail);
        assert!(
            check.detail.contains("2400000 Hz  HELD 2286826 Hz"),
            "{}",
            check.detail
        );
        assert!(
            check.detail.contains("3200000 Hz  refused"),
            "{}",
            check.detail
        );
        assert!(check.hint.is_some());
    }

    #[test]
    fn a_radio_that_holds_every_advertised_rate_passes() {
        let held = [(2_048_000.0, Some(2_048_000.0)), (1_024_000.0, None)];
        assert_eq!(
            rate_check("x", "X", &held[..1]).status,
            CheckStatus::Ok,
            "an exact read-back is not a mismatch"
        );
        assert_eq!(rate_check("x", "X", &held).status, CheckStatus::Fail);
        assert_eq!(rate_check("x", "X", &[]).status, CheckStatus::Warn);
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
        assert_eq!(text.matches("       line one").count(), 3);
        assert_eq!(text.matches("       line two").count(), 3);
        assert!(text.contains("       → do the thing"));
    }

    #[test]
    fn path_check_reports_absence_and_writability() {
        let absent = path_check("x", "X", None, PathKind::File, "nothing persists");
        assert_eq!(absent.status, CheckStatus::Warn);
        assert!(absent.detail.contains("nothing persists"));

        let dir = tempfile::TempDir::new().expect("tempdir");
        let ok = path_check("x", "X", Some(dir.path()), PathKind::Directory, "unused");
        assert_eq!(ok.status, CheckStatus::Ok, "{}", ok.detail);

        let file = dir.path().join("sub").join("sdrmm.db");
        let ok = path_check("x", "X", Some(&file), PathKind::File, "unused");
        assert_eq!(ok.status, CheckStatus::Ok, "{}", ok.detail);
        assert!(file.parent().is_some_and(Path::exists));
        assert!(
            !file.exists(),
            "the probe must not create the database itself"
        );
    }

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

    #[test]
    fn backends_check_warns_when_only_the_virtual_driver_is_compiled_in() {
        let mut registry = sdrmm_device::DeviceRegistry::new();
        registry.register(10, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
        let check = backends_check(&registry);
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.detail.contains("virtual"));
        assert!(check.hint.is_some_and(|h| h.contains("soapy")));
    }

    #[test]
    fn virtual_backend_description_matches_the_build_policy() {
        assert_eq!(
            virtual_capabilities(true),
            "synthetic signal-generator and marker radios, plus SigMF playback through the \
             virtual driver"
        );
        assert_eq!(
            virtual_capabilities(false),
            "SigMF playback through the virtual driver"
        );
    }

    #[cfg(feature = "soapy")]
    #[test]
    fn soapy_check_reports_core_paths_modules_and_missing_baseline() {
        let check = soapy_check(&sdrmm_device_soapy::RuntimeInfo {
            core_version: "0.8.1".to_string(),
            search_paths: vec!["/app/soapy/modules0.8".to_string()],
            modules: vec!["librtlsdrSupport.so".to_string()],
        });
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.detail.contains("core: 0.8.1"));
        assert!(check.detail.contains("librtlsdrSupport.so"));
        assert!(check.hint.is_some_and(|hint| hint.contains("hackrf")));
    }
}
