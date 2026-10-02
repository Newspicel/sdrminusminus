use std::time::Duration;

use anyhow::{Context, Result};

use super::running::Running;

#[cfg(target_os = "macos")]
use macos::{Control, find, raise};
#[cfg(not(target_os = "macos"))]
use other::{Control, find, raise};

const TIMEOUT: Duration = Duration::from_secs(60);
const PLAY: Control = Control::toggle("Play");
const STOP: Control = Control::toggle("Stop");
const RESUME: Control = Control::button("Resume audio");

pub fn listen(running: &mut Running, receivers: usize) -> Result<()> {
    let pid = running.pid();
    let mut seen = String::from("nothing");
    let shown = running.wait_for("show every input on the speaker", TIMEOUT, || {
        let found = find(pid, PLAY);
        seen = match &found {
            Ok(found) => format!("{} Play toggles", found.len()),
            Err(err) => format!("{err:#}"),
        };
        found.is_ok_and(|found| found.len() == receivers)
    });
    shown.with_context(|| format!("wanted {receivers} Play toggles, saw {seen}"))?;
    raise(pid)?;
    for control in find(pid, PLAY)? {
        control.press()?;
    }
    running.wait_for("play every input", TIMEOUT, || {
        find(pid, STOP).is_ok_and(|found| found.len() == receivers)
            && find(pid, RESUME).is_ok_and(|found| found.is_empty())
    })
}

#[cfg(not(target_os = "macos"))]
mod other {
    use anyhow::{Result, bail};

    pub struct Control;

    pub struct Found;

    impl Control {
        pub const fn toggle(_: &'static str) -> Self {
            Self
        }

        pub const fn button(_: &'static str) -> Self {
            Self
        }
    }

    impl Found {
        pub fn press(&self) -> Result<()> {
            bail!("pressing a control needs macOS Accessibility")
        }
    }

    pub fn find(_: u32, _: Control) -> Result<Vec<Found>> {
        bail!("finding a control needs macOS Accessibility")
    }

    pub fn raise(_: u32) -> Result<()> {
        bail!("raising a window needs macOS Accessibility")
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ptr::{self, NonNull};

    use anyhow::{Context, Result, bail, ensure};
    use objc2_application_services::{AXError, AXUIElement};
    use objc2_core_foundation::{CFArray, CFBoolean, CFRetained, CFString, CFType};

    #[derive(Clone, Copy)]
    pub struct Control {
        role: &'static str,
        label: &'static str,
    }

    pub struct Found(CFRetained<AXUIElement>);

    impl Control {
        pub const fn toggle(label: &'static str) -> Self {
            Self {
                role: "AXCheckBox",
                label,
            }
        }

        pub const fn button(label: &'static str) -> Self {
            Self {
                role: "AXButton",
                label,
            }
        }

        fn matches(self, element: &AXUIElement) -> bool {
            text(element, "AXRole").as_deref() == Some(self.role)
                && ["AXDescription", "AXTitle"]
                    .iter()
                    .any(|name| text(element, name).as_deref() == Some(self.label))
        }
    }

    impl Found {
        pub fn press(&self) -> Result<()> {
            let error = unsafe { self.0.perform_action(&CFString::from_static_str("AXPress")) };
            ensure!(error == AXError::Success, "AXPress failed: {error:?}");
            Ok(())
        }
    }

    pub fn raise(pid: u32) -> Result<()> {
        let app = application(pid)?;
        let error = unsafe {
            app.set_attribute_value(
                &CFString::from_static_str("AXFrontmost"),
                CFBoolean::new(true),
            )
        };
        ensure!(
            error == AXError::Success,
            "raising the app failed: {error:?}"
        );
        Ok(())
    }

    pub fn find(pid: u32, control: Control) -> Result<Vec<Found>> {
        let app = application(pid)?;
        let mut found = Vec::new();
        let mut queue = children(&app)?;
        while let Some(element) = queue.pop() {
            if control.matches(&element) {
                found.push(Found(element.clone()));
            }
            queue.extend(children(&element).unwrap_or_default());
        }
        Ok(found)
    }

    fn application(pid: u32) -> Result<CFRetained<AXUIElement>> {
        ensure!(
            unsafe { objc2_application_services::AXIsProcessTrusted() },
            "grant this terminal Accessibility access in System Settings"
        );
        let pid = libc::pid_t::try_from(pid).context("pid out of range")?;
        Ok(unsafe { AXUIElement::new_application(pid) })
    }

    fn children(element: &AXUIElement) -> Result<Vec<CFRetained<AXUIElement>>> {
        let Some(value) = attribute(element, "AXChildren")? else {
            return Ok(Vec::new());
        };
        let Ok(array) = value.downcast::<CFArray>() else {
            bail!("AXChildren is not a list");
        };
        let array: CFRetained<CFArray<CFType>> = unsafe { CFRetained::cast_unchecked(array) };
        Ok(array
            .iter()
            .filter_map(|child| child.downcast::<AXUIElement>().ok())
            .collect())
    }

    fn text(element: &AXUIElement, name: &str) -> Option<String> {
        let value = attribute(element, name).ok()??;
        value
            .downcast::<CFString>()
            .ok()
            .map(|text| text.to_string())
    }

    fn attribute(element: &AXUIElement, name: &str) -> Result<Option<CFRetained<CFType>>> {
        let name = CFString::from_str(name);
        let mut value: *const CFType = ptr::null();
        let error = unsafe { element.copy_attribute_value(&name, NonNull::from(&mut value)) };
        match error {
            AXError::Success => {
                Ok(NonNull::new(value.cast_mut())
                    .map(|value| unsafe { CFRetained::from_raw(value) }))
            }
            AXError::NoValue | AXError::AttributeUnsupported => Ok(None),
            other => bail!("reading {name} failed: {other:?}"),
        }
    }
}
