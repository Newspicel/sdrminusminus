use std::{
    fs::File,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

pub struct Running {
    child: Child,
    name: String,
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
            child,
            name: name.to_owned(),
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn alive(&mut self) -> Result<()> {
        match self.child.try_wait()? {
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

impl Drop for Running {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            if let Err(err) = self.child.kill() {
                eprintln!("could not stop {}: {err}", self.name);
            }
            if let Err(err) = self.child.wait() {
                eprintln!("could not reap {}: {err}", self.name);
            }
        }
    }
}
