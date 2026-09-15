use std::path::Path;

const MODE: &str = "SDRMM_LINUX_GRAPHICS";
const DMABUF: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";
const COMPOSITING: &str = "WEBKIT_DISABLE_COMPOSITING_MODE";
const NVIDIA_MODULE: &str = "/sys/module/nvidia";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Auto,
    Safe,
    Off,
}

impl std::str::FromStr for Mode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "safe" => Ok(Self::Safe),
            "off" => Ok(Self::Off),
            other => Err(other.to_owned()),
        }
    }
}

fn disabled_renderers(mode: Mode, nvidia: bool) -> &'static [&'static str] {
    match mode {
        Mode::Off => &[],
        Mode::Safe => &[DMABUF, COMPOSITING],
        Mode::Auto if nvidia => &[DMABUF],
        Mode::Auto => &[],
    }
}

fn mode() -> Mode {
    let Ok(value) = std::env::var(MODE) else {
        return Mode::default();
    };
    value.parse().unwrap_or_else(|unknown| {
        tracing::warn!("{MODE}={unknown} is not one of auto, safe, off; reading it as auto");
        Mode::default()
    })
}

/// # Safety
/// The caller must invoke this during single-threaded process startup, before any other thread
/// can read or write the process environment and before the first webview is built, which is what
/// reads these.
pub unsafe fn configure() {
    if !cfg!(target_os = "linux") {
        return;
    }
    let mode = mode();
    let mut applied = Vec::new();
    for name in disabled_renderers(mode, Path::new(NVIDIA_MODULE).is_dir()) {
        if std::env::var_os(name).is_some() {
            continue;
        }
        unsafe { std::env::set_var(name, "1") };
        applied.push(*name);
    }
    tracing::info!(?mode, ?applied, "linux webview rendering");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_leaves_a_working_stack_accelerated() {
        assert!(disabled_renderers(Mode::Auto, false).is_empty());
    }

    #[test]
    fn auto_drops_the_dmabuf_renderer_for_a_loaded_nvidia_module() {
        assert_eq!(disabled_renderers(Mode::Auto, true), [DMABUF]);
    }

    #[test]
    fn safe_drops_compositing_as_well() {
        assert_eq!(disabled_renderers(Mode::Safe, false), [DMABUF, COMPOSITING]);
    }

    #[test]
    fn off_keeps_every_path_even_where_auto_would_act() {
        assert!(disabled_renderers(Mode::Off, true).is_empty());
    }

    #[test]
    fn mode_reads_the_documented_spellings() {
        assert_eq!(" AUTO ".parse(), Ok(Mode::Auto));
        assert_eq!("safe".parse(), Ok(Mode::Safe));
        assert_eq!("off".parse(), Ok(Mode::Off));
        assert_eq!("fast".parse::<Mode>(), Err("fast".to_owned()));
    }
}
