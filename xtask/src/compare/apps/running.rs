use std::{
    fs::File,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

use super::disclaim::Disclaimed;

pub struct Running {
    handle: Handle,
    name: String,
}

enum Handle {
    Child(Child),
    Disclaimed(Disclaimed),
}

impl Running {
    pub fn spawn(mut command: Command, name: &str, log: &Path) -> Result<Self> {
        let out = File::create(log).with_context(|| format!("create {}", log.display()))?;
        let err = out.try_clone()?;
        let child = command
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            .spawn()
            .with_context(|| format!("start {name}"))?;
        Ok(Self {
            handle: Handle::Child(child),
            name: name.to_owned(),
        })
    }

    pub fn disclaimed(process: Disclaimed, name: &str) -> Self {
        Self {
            handle: Handle::Disclaimed(process),
            name: name.to_owned(),
        }
    }

    pub fn pid(&self) -> u32 {
        match &self.handle {
            Handle::Child(child) => child.id(),
            Handle::Disclaimed(process) => process.id(),
        }
    }

    pub fn alive(&mut self) -> Result<()> {
        match self.handle.exited()? {
            Some(status) => bail!("{} exited with {status}", self.name),
            None => Ok(()),
        }
    }

    pub fn wait_for(
        &mut self,
        what: &str,
        timeout: Duration,
        mut ready: impl FnMut() -> bool,
    ) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            self.alive()?;
            if ready() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(250));
        }
        bail!(
            "{} did not {what} within {} s",
            self.name,
            timeout.as_secs()
        )
    }
}

impl Handle {
    fn exited(&mut self) -> Result<Option<String>> {
        match self {
            Self::Child(child) => Ok(child.try_wait()?.map(|status| status.to_string())),
            Self::Disclaimed(process) => process.exited(),
        }
    }

    fn stop(&mut self) -> Result<()> {
        match self {
            Self::Child(child) => {
                child.kill().context("kill")?;
                child.wait().context("reap")?;
                Ok(())
            }
            Self::Disclaimed(process) => process.stop(),
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if self.handle.exited().ok().flatten().is_none()
            && let Err(err) = self.handle.stop()
        {
            eprintln!("could not stop {}: {err:#}", self.name);
        }
    }
}
