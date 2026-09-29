use std::{
    io,
    path::{Path, PathBuf},
};

use sdrmm_server::StoreError;
use tauri::AppHandle;
use tauri_plugin_dialog::{
    DialogExt, MessageDialogButtons, MessageDialogKind, MessageDialogResult,
};

use crate::update;

const UPDATE: &str = "Update";
const DELETE: &str = "Delete database";
const QUIT: &str = "Quit";

pub fn ask(app: &AppHandle, db: PathBuf, error: &StoreError) {
    let handle = app.clone();
    app.dialog()
        .message(format!("{error}\n\n{}", db.display()))
        .title("Database is newer")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::YesNoCancelCustom(
            UPDATE.to_string(),
            DELETE.to_string(),
            QUIT.to_string(),
        ))
        .show_with_result(move |choice| act(&handle, &db, &choice));
}

fn act(app: &AppHandle, db: &Path, choice: &MessageDialogResult) {
    match choice {
        MessageDialogResult::Custom(label) if label == UPDATE => {
            tauri::async_runtime::spawn(update::update_now(app.clone()));
        }
        MessageDialogResult::Custom(label) if label == DELETE => match delete(db) {
            Ok(()) => app.restart(),
            Err(e) => {
                tracing::error!("could not delete {}: {e}", db.display());
                app.exit(1);
            }
        },
        _ => app.exit(0),
    }
}

fn delete(db: &Path) -> io::Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let mut file = db.as_os_str().to_owned();
        file.push(suffix);
        match std::fs::remove_file(&file) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_removes_the_database_and_its_journal() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("sdrmm.db");
        std::fs::write(&db, b"db").unwrap();
        std::fs::write(dir.path().join("sdrmm.db-wal"), b"wal").unwrap();
        delete(&db).unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
