use anyhow::Result;
use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

const RELEASE: Option<&str> = option_env!("SDRMM_RELEASE");

pub fn spawn(app: &AppHandle) {
    if let Some(reason) = skipped() {
        tracing::info!("update check skipped: {reason}");
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

fn skipped() -> Option<&'static str> {
    if RELEASE.is_none_or(str::is_empty) {
        return Some(
            "not built from a release tag; dev and nightly builds never replace themselves",
        );
    }
    unsupported()
}

#[cfg(target_os = "linux")]
fn unsupported() -> Option<&'static str> {
    std::env::var_os("APPIMAGE")
        .is_none()
        .then_some("not running as an AppImage; .deb installs update via the package manager")
}

#[cfg(not(target_os = "linux"))]
fn unsupported() -> Option<&'static str> {
    None
}
