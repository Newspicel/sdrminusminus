//! The startup update check. Entirely Rust-side, on purpose: the WebView is pointed at the
//! embedded server's origin, so reaching the updater from JS would mean granting IPC to a
//! remote URL — the one thing the shell is built to avoid. `UpdaterExt` is a Rust API on
//! `AppHandle` and needs no capability, so the frontend stays unaware this exists.
use anyhow::Result;
use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

/// Fire-and-forget: an unreachable or slow endpoint must not delay the window, and a failed
/// check is a log line rather than a startup error — the app works fine on the version it has.
pub fn spawn(app: &AppHandle) {
    if let Some(reason) = unsupported() {
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

/// The dialog's callback is delivered on the main thread, so the answer comes back over a
/// channel: blocking this task on the dialog would block the thread the dialog needs.
async fn prompt(app: &AppHandle, version: &str) -> bool {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .message(format!(
            "sdr-- {version} is available.\n\nInstalling restarts the app, which stops any \
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

/// Linux updates rewrite the running AppImage in place, which the updater locates through
/// `APPIMAGE`. A `.deb` install has no such file and is owned by the package manager, so this
/// is a skip rather than a failure to put in front of the user.
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
