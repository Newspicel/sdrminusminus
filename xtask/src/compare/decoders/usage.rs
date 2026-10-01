use anyhow::Result;

#[cfg(unix)]
fn seconds(who: libc::c_int) -> Result<f64> {
    use anyhow::ensure;
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    let status = unsafe { libc::getrusage(who, usage.as_mut_ptr()) };
    ensure!(status == 0, "getrusage failed");
    let usage = unsafe { usage.assume_init() };
    let time = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 * 1e-6;
    Ok(time(usage.ru_utime) + time(usage.ru_stime))
}

#[cfg(unix)]
pub fn own() -> Result<f64> {
    seconds(libc::RUSAGE_SELF)
}

#[cfg(unix)]
pub fn children() -> Result<f64> {
    seconds(libc::RUSAGE_CHILDREN)
}

#[cfg(not(unix))]
pub fn own() -> Result<f64> {
    anyhow::bail!("CPU time is only measured on Unix")
}

#[cfg(not(unix))]
pub fn children() -> Result<f64> {
    anyhow::bail!("CPU time is only measured on Unix")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_time_grows_with_work() {
        let before = own().expect("usage");
        let mut sum = 0u64;
        for i in 0..20_000_000u64 {
            sum = std::hint::black_box(sum.wrapping_add(i * i));
        }
        assert!(own().expect("usage") > before, "{sum}");
    }
}
