use std::{io, path::Path};

#[derive(Debug)]
pub struct Shell;

impl sdrmm_server::NativeShell for Shell {
    fn reveal(&self, path: &Path) -> io::Result<()> {
        let shown = if path.is_dir() {
            tauri_plugin_opener::open_path(path, None::<&str>)
        } else {
            tauri_plugin_opener::reveal_item_in_dir(path)
        };
        shown.map_err(io::Error::other)
    }
}
