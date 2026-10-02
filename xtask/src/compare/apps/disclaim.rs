#[cfg(target_os = "macos")]
pub use macos::{Disclaimed, spawn};
#[cfg(not(target_os = "macos"))]
pub use other::{Disclaimed, spawn};

#[cfg(not(target_os = "macos"))]
mod other {
    use std::{ffi::OsStr, path::Path};

    use anyhow::{Result, bail};

    pub struct Disclaimed;

    impl Disclaimed {
        pub fn id(&self) -> u32 {
            0
        }

        pub fn exited(&mut self) -> Result<Option<String>> {
            Ok(None)
        }

        pub fn stop(&mut self) -> Result<()> {
            Ok(())
        }
    }

    pub fn spawn(_: &Path, _: &[&OsStr], _: &[(&str, &OsStr)], _: &Path) -> Result<Disclaimed> {
        bail!("a process owning its own helpers can only be started on macOS")
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::{
        ffi::{CString, OsStr, OsString},
        fs::File,
        mem::MaybeUninit,
        os::{fd::AsRawFd, unix::ffi::OsStrExt},
        path::Path,
        ptr,
    };

    use anyhow::{Context, Result, ensure};

    unsafe extern "C" {
        fn responsibility_spawnattrs_setdisclaim(
            attr: *mut libc::posix_spawnattr_t,
            disclaim: libc::c_int,
        ) -> libc::c_int;
    }

    pub struct Disclaimed {
        pid: libc::pid_t,
        status: Option<libc::c_int>,
    }

    pub fn spawn(
        program: &Path,
        args: &[&OsStr],
        env: &[(&str, &OsStr)],
        log: &Path,
    ) -> Result<Disclaimed> {
        let log = File::create(log).with_context(|| format!("create {}", log.display()))?;
        let path = c_string(program.as_os_str())?;
        let rest = args
            .iter()
            .map(|arg| c_string(arg))
            .collect::<Result<Vec<_>>>()?;
        let mut argv: Vec<*const libc::c_char> = vec![path.as_ptr()];
        argv.extend(rest.iter().map(|arg| arg.as_ptr()));
        argv.push(ptr::null());
        let vars = environment(env)?;
        let mut envp: Vec<*const libc::c_char> = vars.iter().map(|var| var.as_ptr()).collect();
        envp.push(ptr::null());
        let mut actions = Actions::new()?;
        actions.stdio(log.as_raw_fd())?;
        let mut attr = Attr::new()?;
        attr.disclaim()?;
        let mut pid = 0;
        let code = unsafe {
            libc::posix_spawn(
                &mut pid,
                path.as_ptr(),
                &actions.0,
                &attr.0,
                argv.as_ptr().cast(),
                envp.as_ptr().cast(),
            )
        };
        ensure!(code == 0, "posix_spawn {}: error {code}", program.display());
        Ok(Disclaimed { pid, status: None })
    }

    impl Disclaimed {
        pub fn id(&self) -> u32 {
            self.pid.unsigned_abs()
        }

        pub fn exited(&mut self) -> Result<Option<String>> {
            if self.status.is_none() {
                self.reap(libc::WNOHANG)?;
            }
            Ok(self.status.map(describe))
        }

        pub fn stop(&mut self) -> Result<()> {
            if self.status.is_some() {
                return Ok(());
            }
            ensure!(
                unsafe { libc::kill(self.pid, libc::SIGKILL) } == 0,
                "kill {}: {}",
                self.pid,
                std::io::Error::last_os_error()
            );
            self.reap(0)
        }

        fn reap(&mut self, options: libc::c_int) -> Result<()> {
            let mut status = 0;
            let found = unsafe { libc::waitpid(self.pid, &mut status, options) };
            ensure!(
                found >= 0,
                "waitpid {}: {}",
                self.pid,
                std::io::Error::last_os_error()
            );
            if found == self.pid {
                self.status = Some(status);
            }
            Ok(())
        }
    }

    fn describe(status: libc::c_int) -> String {
        if libc::WIFEXITED(status) {
            format!("exit status {}", libc::WEXITSTATUS(status))
        } else {
            format!("signal {}", libc::WTERMSIG(status))
        }
    }

    fn environment(overrides: &[(&str, &OsStr)]) -> Result<Vec<CString>> {
        let mut vars: Vec<(OsString, OsString)> = std::env::vars_os()
            .filter(|(key, _)| overrides.iter().all(|(name, _)| key != name))
            .collect();
        vars.extend(
            overrides
                .iter()
                .map(|(name, value)| (OsString::from(name), value.to_os_string())),
        );
        vars.iter()
            .map(|(key, value)| {
                let mut pair = key.clone();
                pair.push("=");
                pair.push(value);
                c_string(&pair)
            })
            .collect()
    }

    fn c_string(text: &OsStr) -> Result<CString> {
        CString::new(text.as_bytes()).context("a NUL byte in a spawn argument")
    }

    fn checked(code: libc::c_int, what: &str) -> Result<()> {
        ensure!(code == 0, "{what}: error {code}");
        Ok(())
    }

    struct Actions(libc::posix_spawn_file_actions_t);

    impl Actions {
        fn new() -> Result<Self> {
            let mut actions = MaybeUninit::uninit();
            checked(
                unsafe { libc::posix_spawn_file_actions_init(actions.as_mut_ptr()) },
                "posix_spawn_file_actions_init",
            )?;
            Ok(Self(unsafe { actions.assume_init() }))
        }

        fn stdio(&mut self, log: libc::c_int) -> Result<()> {
            let null = c"/dev/null";
            checked(
                unsafe {
                    libc::posix_spawn_file_actions_addopen(
                        &mut self.0,
                        0,
                        null.as_ptr(),
                        libc::O_RDONLY,
                        0,
                    )
                },
                "redirect stdin",
            )?;
            for fd in [1, 2] {
                checked(
                    unsafe { libc::posix_spawn_file_actions_adddup2(&mut self.0, log, fd) },
                    "redirect output",
                )?;
            }
            Ok(())
        }
    }

    impl Drop for Actions {
        fn drop(&mut self) {
            unsafe { libc::posix_spawn_file_actions_destroy(&mut self.0) };
        }
    }

    struct Attr(libc::posix_spawnattr_t);

    impl Attr {
        fn new() -> Result<Self> {
            let mut attr = MaybeUninit::uninit();
            checked(
                unsafe { libc::posix_spawnattr_init(attr.as_mut_ptr()) },
                "posix_spawnattr_init",
            )?;
            Ok(Self(unsafe { attr.assume_init() }))
        }

        fn disclaim(&mut self) -> Result<()> {
            let mut defaults = MaybeUninit::uninit();
            unsafe { libc::sigfillset(defaults.as_mut_ptr()) };
            let flags = libc::POSIX_SPAWN_SETSIGDEF | libc::POSIX_SPAWN_CLOEXEC_DEFAULT;
            checked(
                unsafe { libc::posix_spawnattr_setsigdefault(&mut self.0, defaults.as_ptr()) },
                "posix_spawnattr_setsigdefault",
            )?;
            checked(
                unsafe { libc::posix_spawnattr_setflags(&mut self.0, flags as libc::c_short) },
                "posix_spawnattr_setflags",
            )?;
            checked(
                unsafe { responsibility_spawnattrs_setdisclaim(&mut self.0, 1) },
                "responsibility_spawnattrs_setdisclaim",
            )
        }
    }

    impl Drop for Attr {
        fn drop(&mut self) {
            unsafe { libc::posix_spawnattr_destroy(&mut self.0) };
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::compare::apps::responsible;

        #[test]
        fn a_disclaimed_process_is_responsible_for_itself() {
            let dir = tempfile::tempdir().unwrap();
            let log = dir.path().join("log");
            let mut child = spawn(Path::new("/bin/sleep"), &[OsStr::new("30")], &[], &log).unwrap();
            assert_eq!(child.exited().unwrap(), None);
            assert_eq!(responsible::of(child.id()), Some(child.id()));
            child.stop().unwrap();
            assert_eq!(child.exited().unwrap().as_deref(), Some("signal 9"));
        }
    }
}
