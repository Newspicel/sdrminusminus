use std::{
    io::Read,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use sdrmm_device::DeviceError;
use sdrmm_wire::DeviceInfo;
use serde::{Deserialize, Serialize};

const PROBE_FLAG: &str = "--sdrmm-probe-soapy";
const CHILD_MARKER: &str = "SDRMM_SOAPY_PROBE_CHILD";
const MODE: &str = "SDRMM_SOAPY_PROBE";
const IN_PROCESS: &str = "in-process";
const TIMEOUT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(10);
const STDERR_TAIL: usize = 400;

static HELPER: AtomicBool = AtomicBool::new(false);

/// Answers a probe request and exits when this process was spawned as one, and otherwise marks
/// the running executable as a probe helper so later enumerations run in a child process.
///
/// Call it before parsing arguments: vendor SoapySDR modules open USB devices while enumerating
/// and a faulty one aborts or segfaults the process it runs in.
pub fn enable_isolated_probes() {
    HELPER.store(true, Ordering::Relaxed);
    let mut args = std::env::args_os().skip(1);
    if args.next().is_none_or(|flag| flag != PROBE_FLAG) {
        return;
    }
    let scope = Scope::from_arg(args.next().unwrap_or_default().to_string_lossy().as_ref());
    let filter = args
        .next()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    if scope == Scope::Deep {
        // SAFETY: this runs before the argument parsing that starts the rest of the program, so
        // no other thread exists yet and no SoapySDR call has loaded a module.
        unsafe { crate::runtime::load_network_modules() };
    }
    std::process::exit(run_child(&filter));
}

/// How far a search may reach.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Scope {
    /// Only what is attached to this machine: milliseconds, and safe to run on a hotplug tick.
    Fast,
    /// Network discovery as well, which vendor modules answer in seconds.
    Deep,
}

impl Scope {
    const fn as_arg(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Deep => "deep",
        }
    }

    fn from_arg(arg: &str) -> Self {
        if arg == Self::Deep.as_arg() {
            Self::Deep
        } else {
            Self::Fast
        }
    }
}

fn run_child(filter: &str) -> i32 {
    let found = match crate::enumerate_serialized(filter) {
        Ok(found) => found,
        Err(error) => {
            eprintln!("soapy enumerate: {error}");
            return 1;
        }
    };
    let reply: Vec<Found> = found.iter().map(Found::new).collect();
    match serde_json::to_string(&reply) {
        Ok(json) => {
            println!("{json}");
            0
        }
        Err(error) => {
            eprintln!("soapy probe encode: {error}");
            1
        }
    }
}

fn isolated() -> bool {
    HELPER.load(Ordering::Relaxed)
        && std::env::var_os(CHILD_MARKER).is_none()
        && !std::env::var(MODE).is_ok_and(|mode| mode == IN_PROCESS)
}

/// A device as the search found it: what the rest of the engine sees, and the arguments that
/// reopen exactly this radio without searching again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Found {
    pub(crate) info: DeviceInfo,
    pub(crate) args: String,
}

impl Found {
    fn new(args: &soapysdr::Args) -> Self {
        Self {
            info: crate::device_info(args),
            args: args.to_string(),
        }
    }

    pub(crate) fn is_driver(&self, driver: &str) -> bool {
        soapysdr::Args::from(self.args.as_str())
            .get("driver")
            .is_some_and(|found| found.eq_ignore_ascii_case(driver))
    }
}

/// Searches for devices matching `filter`, out of process where the running executable supports it.
///
/// A process that enumerates in-process cannot choose which modules are loaded — the first search
/// loads them all — so there `scope` has nothing left to decide.
pub(crate) fn devices(filter: &str, scope: Scope) -> Result<Vec<Found>, DeviceError> {
    if isolated() {
        return spawn(filter, scope);
    }
    Ok(crate::enumerate_serialized(filter)
        .map_err(|error| DeviceError::Io(format!("soapy enumerate: {error}")))?
        .iter()
        .map(Found::new)
        .collect())
}

fn spawn(filter: &str, scope: Scope) -> Result<Vec<Found>, DeviceError> {
    let exe = std::env::current_exe().map_err(|error| {
        DeviceError::Io(format!(
            "soapy probe: cannot locate this executable: {error}"
        ))
    })?;
    run(&exe, filter, scope, TIMEOUT)
}

fn run(
    exe: &std::path::Path,
    filter: &str,
    scope: Scope,
    timeout: Duration,
) -> Result<Vec<Found>, DeviceError> {
    let mut child = Command::new(exe)
        .arg(PROBE_FLAG)
        .arg(scope.as_arg())
        .arg(filter)
        .env(CHILD_MARKER, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| DeviceError::Io(format!("soapy probe: cannot spawn helper: {error}")))?;

    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);
    // A killed helper can leave a grandchild holding the pipes open, so its output is only
    // collected once the helper is known to have exited.
    let Some(status) = wait(&mut child, timeout)? else {
        return Err(DeviceError::Io(format!(
            "soapy probe helper timed out after {timeout:?} and was killed"
        )));
    };
    let stdout = stdout.map(join).unwrap_or_default();
    let stderr = stderr.map(join).unwrap_or_default();

    if !status.success() {
        return Err(DeviceError::Io(format!(
            "soapy probe helper failed ({status}): {}",
            tail(&stderr)
        )));
    }
    serde_json::from_str(stdout.trim())
        .map_err(|error| DeviceError::Io(format!("soapy probe: unreadable reply: {error}")))
}

fn wait(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<Option<std::process::ExitStatus>, DeviceError> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(Some(status)),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(None);
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(error) => {
                return Err(DeviceError::Io(format!("soapy probe: {error}")));
            }
        }
    }
}

fn drain(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = pipe.read_to_string(&mut text);
        text
    })
}

fn join(handle: std::thread::JoinHandle<String>) -> String {
    handle.join().unwrap_or_default()
}

fn tail(stderr: &str) -> String {
    let trimmed = stderr.trim();
    match trimmed.char_indices().nth_back(STDERR_TAIL - 1) {
        Some((at, _)) if at > 0 => format!("…{}", &trimmed[at..]),
        _ => trimmed.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_library_without_a_helper_enumerates_in_process() {
        assert!(
            !isolated(),
            "a process that never called enable_isolated_probes has no helper to spawn"
        );
    }

    #[test]
    fn a_scope_survives_the_trip_to_the_helper_and_back() {
        for scope in [Scope::Fast, Scope::Deep] {
            assert_eq!(Scope::from_arg(scope.as_arg()), scope);
        }
    }

    #[test]
    fn a_helper_asked_for_nothing_in_particular_searches_only_this_machine() {
        assert_eq!(Scope::from_arg(""), Scope::Fast);
        assert_eq!(Scope::from_arg("everything"), Scope::Fast);
    }

    #[cfg(unix)]
    #[test]
    fn the_helper_is_told_which_scope_to_search() {
        let (_dir, path) = helper("echo \"$2 $3\" >&2\nexit 1");
        let error = run(&path, "driver=remote", Scope::Deep, Duration::from_secs(5))
            .expect_err("this helper answers with its arguments and fails");
        let message = error.to_string();
        assert!(message.contains("deep driver=remote"), "{message}");
    }

    #[test]
    fn short_helper_errors_are_reported_whole() {
        assert_eq!(tail("  rtlsdr open failed\n"), "rtlsdr open failed");
    }

    #[test]
    fn long_helper_errors_keep_their_end() {
        let noise = "x".repeat(STDERR_TAIL * 2);
        let reported = tail(&format!("{noise}boom"));
        assert!(reported.starts_with('…'), "{reported}");
        assert!(reported.ends_with("boom"), "{reported}");
        assert_eq!(reported.chars().count(), STDERR_TAIL + 1);
    }

    #[cfg(unix)]
    fn helper(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("helper");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write helper");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        (dir, path)
    }

    #[cfg(unix)]
    #[test]
    fn a_helper_reply_carries_the_device_and_the_arguments_that_reopen_it() {
        let reply = serde_json::to_string(&vec![Found {
            info: DeviceInfo {
                driver: "soapy".to_string(),
                key: "00000001".to_string(),
                label: "NESDR SMArt v5".to_string(),
                serial: Some("00000001".to_string()),
                profile: None,
            },
            args: "driver=rtlsdr, serial=00000001".to_string(),
        }])
        .expect("encode");
        let (_dir, path) = helper(&format!("echo '{reply}'"));
        let found = run(&path, "", Scope::Fast, Duration::from_secs(5)).expect("probe");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].info.key, "00000001");
        assert_eq!(found[0].args, "driver=rtlsdr, serial=00000001");
    }

    #[test]
    fn probe_arguments_survive_the_trip_back_into_soapy() {
        let original = soapysdr::Args::from("driver=rtlsdr, serial=00000001, label=NESDR");
        let reopened = soapysdr::Args::from(original.to_string().as_str());
        for key in ["driver", "serial", "label"] {
            assert_eq!(
                reopened.get(key),
                original.get(key),
                "`{key}` must survive the child reply"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_helper_killed_by_a_vendor_driver_reports_instead_of_taking_us_with_it() {
        let (_dir, path) = helper("echo 'rtlsdr open failed' >&2\nkill -SEGV $$");
        let error =
            run(&path, "", Scope::Fast, Duration::from_secs(5)).expect_err("probe must fail");
        let message = error.to_string();
        assert!(message.contains("probe helper failed"), "{message}");
        assert!(message.contains("rtlsdr open failed"), "{message}");
    }

    #[cfg(unix)]
    #[test]
    fn a_wedged_helper_is_killed_and_reported() {
        let (_dir, path) = helper("sleep 30");
        let start = Instant::now();
        let error =
            run(&path, "", Scope::Fast, Duration::from_millis(200)).expect_err("probe must fail");
        assert!(error.to_string().contains("timed out"), "{error}");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "kill took too long"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_helper_that_answers_with_noise_is_an_error_not_a_panic() {
        let (_dir, path) = helper("echo not-json");
        let error =
            run(&path, "", Scope::Fast, Duration::from_secs(5)).expect_err("probe must fail");
        assert!(error.to_string().contains("unreadable reply"), "{error}");
    }

    #[test]
    fn a_reply_round_trips_through_the_child_encoding() {
        let found = vec![Found {
            info: DeviceInfo {
                driver: "soapy".to_string(),
                key: "00000001".to_string(),
                label: "Generic RTL2832U".to_string(),
                serial: Some("00000001".to_string()),
                profile: None,
            },
            args: "driver=rtlsdr, serial=00000001".to_string(),
        }];
        let json = serde_json::to_string(&found).expect("encode");
        let decoded: Vec<Found> = serde_json::from_str(&json).expect("decode");
        assert_eq!(decoded, found);
    }
}
