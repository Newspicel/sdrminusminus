use crate::store::Store;

pub struct Picked {
    pub name: String,
    pub bytes: Vec<u8>,
}

pub async fn open_text(kind: &str, extensions: &[&str]) -> Option<String> {
    let file = rfd::AsyncFileDialog::new()
        .add_filter(kind, extensions)
        .pick_file()
        .await?;
    Some(String::from_utf8_lossy(&file.read().await).into_owned())
}

pub async fn open_many(kind: &str, extensions: &[&str]) -> Vec<Picked> {
    let Some(files) = rfd::AsyncFileDialog::new()
        .add_filter(kind, extensions)
        .pick_files()
        .await
    else {
        return Vec::new();
    };
    let mut picked = Vec::with_capacity(files.len());
    for file in files {
        picked.push(Picked {
            name: file.file_name(),
            bytes: file.read().await,
        });
    }
    picked
}

pub async fn save(store: Store, name: &str, bytes: &[u8]) {
    let Some(file) = rfd::AsyncFileDialog::new()
        .set_file_name(name)
        .save_file()
        .await
    else {
        return;
    };
    match file.write(bytes).await {
        Ok(()) => store.note(format!("Saved {}", file.file_name())),
        Err(error) => store.fail("Cannot save the file", &error.into()),
    }
}

pub async fn download(store: Store, path: &str, name: &str) {
    match store.api().bytes(path).await {
        Ok(bytes) => save(store, name, &bytes).await,
        Err(error) => store.fail("Cannot download", &error),
    }
}

pub fn open_link(store: Store, url: &str) {
    if let Err(error) = open::that_detached(url) {
        store.fail("Cannot open the link", &error.into());
    }
}
