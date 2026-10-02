#[cfg(target_os = "macos")]
pub fn of(pid: u32) -> Option<u32> {
    unsafe extern "C" {
        fn responsibility_get_pid_responsible_for_pid(pid: libc::pid_t) -> libc::pid_t;
    }
    let pid = libc::pid_t::try_from(pid).ok()?;
    let found = unsafe { responsibility_get_pid_responsible_for_pid(pid) };
    u32::try_from(found).ok().filter(|&found| found > 0)
}

#[cfg(not(target_os = "macos"))]
pub fn of(_: u32) -> Option<u32> {
    None
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn a_process_started_here_has_someone_responsible() {
        assert!(of(std::process::id()).is_some());
        assert_eq!(of(u32::MAX), None);
    }
}
