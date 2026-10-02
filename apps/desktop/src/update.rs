use anyhow::Result;
use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

const RELEASE: Option<&str> = option_env!("SDRMM_RELEASE");
const RELEASES: &str = "https://github.com/Newspicel/sdrmm/releases";

pub fn spawn(app: &AppHandle) {
    if let Some(skip) = skipped() {
        tracing::info!("update check skipped: {}", skip.reason());
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = check(&app).await {
            tracing::warn!("update check failed: {e:#}");
        }
    });
}

async fn check(app: &AppHandle) -> Result<()> {
    let Some(update) = app.updater()?.check().await? else {
        tracing::debug!("no update available");
        return Ok(());
    };
    tracing::info!(
        "update available: {} -> {}",
        update.current_version,
        update.version
    );
    if !prompt(app, &update.version).await {
        return Ok(());
    }
    update.download_and_install(|_, _| {}, || {}).await?;
    app.restart()
}

pub async fn update_now(app: AppHandle) {
    match skipped() {
        Some(Skip::Packaged) => {
            tracing::info!("update skipped: {}", Skip::Packaged.reason());
            use_package_manager(&app).await;
        }
        Some(Skip::Unreleased) => {
            tracing::info!("update skipped: {}", Skip::Unreleased.reason());
            open_releases();
        }
        None => {
            let installed = install(&app).await.unwrap_or_else(|e| {
                tracing::warn!("update failed: {e:#}");
                false
            });
            if !installed {
                open_releases();
            }
        }
    }
    app.exit(0);
}

fn open_releases() {
    if let Err(e) = tauri_plugin_opener::open_url(RELEASES, None::<&str>) {
        tracing::warn!("could not open {RELEASES}: {e}");
    }
}

async fn use_package_manager(app: &AppHandle) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .message("Update SDR-- with your package manager.")
        .title("Update")
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::Ok)
        .show(move |_| {
            let _ = tx.send(());
        });
    let _ = rx.await;
}

async fn install(app: &AppHandle) -> Result<bool> {
    let Some(update) = app.updater()?.check().await? else {
        return Ok(false);
    };
    update.download_and_install(|_, _| {}, || {}).await?;
    app.restart()
}

async fn prompt(app: &AppHandle, version: &str) -> bool {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .message(format!(
            "SDR-- {version} is available.\n\nInstalling restarts the app, which stops any \
             recording or stream that is running."
        ))
        .title("Update available")
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Install and restart".to_string(),
            "Later".to_string(),
        ))
        .show(move |install| {
            let _ = tx.send(install);
        });
    rx.await.unwrap_or(false)
}

#[derive(Debug, PartialEq, Eq)]
enum Skip {
    Unreleased,
    Packaged,
}

impl Skip {
    fn reason(&self) -> &'static str {
        match self {
            Self::Unreleased => {
                "not built from a release tag; dev and nightly builds never replace themselves"
            }
            Self::Packaged => "installed by a package manager, which owns updates",
        }
    }
}

fn skipped() -> Option<Skip> {
    skip(RELEASE, packaged())
}

fn skip(release: Option<&str>, packaged: bool) -> Option<Skip> {
    if release.is_none_or(str::is_empty) {
        return Some(Skip::Unreleased);
    }
    packaged.then_some(Skip::Packaged)
}

#[cfg(target_os = "linux")]
fn packaged() -> bool {
    std::env::var_os("APPIMAGE").is_none()
}

#[cfg(not(target_os = "linux"))]
fn packaged() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreleased_builds_never_update() {
        assert_eq!(skip(None, false), Some(Skip::Unreleased));
        assert_eq!(skip(Some(""), true), Some(Skip::Unreleased));
    }

    #[test]
    fn packaged_releases_defer_to_the_package_manager() {
        assert_eq!(skip(Some("1"), true), Some(Skip::Packaged));
    }

    #[test]
    fn standalone_releases_update_themselves() {
        assert_eq!(skip(Some("1"), false), None);
    }
}
