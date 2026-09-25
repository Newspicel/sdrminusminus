use std::path::PathBuf;

use sdrmm_wire::decode::BroadcastData;
use zgui::prelude::*;

use crate::store::Store;

pub const SHEET: &str = css!(
    r#"
.dk-pane { flex-direction: column; gap: 8px; min-width: 0; }
.dk-line { flex-direction: row; flex-wrap: wrap; align-items: baseline; gap: 4px 12px; min-width: 0; }
.dk-big { font-family: var(--mono); font-size: 20px; letter-spacing: 0.04em; color: var(--ink); }
.dk-mid { font-family: var(--mono); font-size: 14px; color: var(--ink); }
.dk-dim { color: var(--ink-dim); font-size: 11px; }
.dk-num { font-family: var(--mono); font-size: 11px; color: var(--ink-dim); }
.dk-accent { color: var(--accent); }
.dk-danger { color: var(--danger); }
.dk-push { margin-left: auto; }
.dk-chip {
    padding: 1px 6px;
    border: 1px solid var(--line);
    border-radius: 4px;
    font-family: var(--mono);
    font-size: 10px;
    color: var(--ink-dim);
}
.dk-chip.on { border-color: var(--accent); color: var(--accent); }
.dk-chip.off { opacity: 0.5; }
.dk-box {
    padding: 4px 8px;
    border: 1px solid var(--line);
    border-radius: 5px;
    background-color: var(--panel);
    font-family: var(--mono);
    font-size: 12px;
    color: var(--ink);
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
}
.dk-pre {
    padding: 4px 8px;
    border: 1px solid var(--line);
    border-radius: 5px;
    background-color: var(--panel);
    font-family: var(--mono);
    font-size: 11px;
    color: var(--ink);
    white-space: pre-wrap;
    overflow: auto;
    max-height: 260px;
    min-height: 80px;
}
.dk-alert {
    padding: 4px 8px;
    border: 1px solid var(--danger);
    border-radius: 5px;
    color: var(--danger);
    font-size: 11px;
    align-items: center;
    gap: 8px;
}
.dk-table { flex-direction: column; min-width: 0; font-family: var(--mono); font-size: 11px; }
.dk-tr { flex-direction: row; gap: 8px; padding: 2px 0; border-bottom: 1px solid var(--line); min-width: 0; }
.dk-th { color: var(--ink-faint); font-size: 9px; letter-spacing: 0.09em; text-transform: uppercase; }
.dk-td { flex: 1 1 0; min-width: 0; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; color: var(--ink); }
.dk-td.wide { flex: 2 1 0; }
.dk-td.dim { color: var(--ink-dim); }
.dk-td.fade { color: var(--ink-dim); opacity: 0.5; }
.dk-sort:hover { color: var(--accent); }
.dk-fields { flex-direction: column; gap: 1px; font-size: 11px; }
.dk-field { flex-direction: row; gap: 10px; min-width: 0; }
.dk-field__name { flex: 0 0 132px; color: var(--ink-dim); }
.dk-field__value { flex: 1 1 auto; min-width: 0; color: var(--ink); font-family: var(--mono); }
.dk-thumbs { flex-direction: row; flex-wrap: wrap; gap: 6px; }
.dk-thumb { padding: 2px; border: 1px solid var(--line); border-radius: 4px; }
.dk-thumb.on { border-color: var(--accent); }
.dk-thumb image { width: 64px; height: 48px; object-fit: contain; background-color: black; }
.dk-picture { width: 100%; max-height: 360px; object-fit: contain; background-color: black; border-radius: 4px; }
.dk-object { max-width: 100%; max-height: 320px; object-fit: contain; }
.dk-more { color: var(--ink-faint); font-size: 10px; }
"#
);

pub fn install() {
    install_stylesheet("kit-decoders", SHEET);
}

pub fn no_pan() -> Attrs {
    zgui_flow::view::NodeCx::<(), ()>::no_pan()
}

pub fn fields_view(fields: Vec<(&'static str, String)>) -> impl IntoView {
    let rows: Vec<_> = fields
        .into_iter()
        .map(|(name, value)| {
            view! {
                row(class = "dk-field") {
                    text(class = "dk-field__name") {{name}}
                    text(class = "dk-field__value") {{value}}
                }
            }
        })
        .collect();
    view! { column(class = "dk-fields") {{rows}} }
}

pub fn alert(message: String, dismiss: impl Fn() + 'static) -> impl IntoView {
    view! {
        row(class = "dk-alert") {
            text {{message}}
            spacer()
            control(class = "btn", on:click:stop = move |_| dismiss()) {"Dismiss"}
        }
    }
}

fn downloads() -> PathBuf {
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(std::env::temp_dir)
}

#[must_use]
pub fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\') {
                '_'
            } else {
                c
            }
        })
        .collect();
    if cleaned.trim().is_empty() || cleaned == "." || cleaned == ".." {
        "download".to_owned()
    } else {
        cleaned
    }
}

pub fn save_bytes(store: Store, name: String, bytes: Vec<u8>) {
    let target = downloads().join(safe_name(&name));
    zgui::task::spawn_local(async move {
        let shown = target.display().to_string();
        let written = zgui::task::blocking(move || std::fs::write(&target, bytes)).await;
        match written {
            Ok(()) => store.say(format!("Saved {shown}")),
            Err(error) => store.say(format!("Cannot save {shown}: {error}")),
        }
    });
}

pub fn save_download(store: Store, path: String, name: String) {
    zgui::task::spawn_local(async move {
        match store.api().bytes(&path).await {
            Ok(bytes) => save_bytes(store, name, bytes),
            Err(error) => store.say(format!("Download failed: {error}")),
        }
    });
}

#[must_use]
pub fn is_picture(media_type: &str) -> bool {
    matches!(
        media_type,
        "image/jpeg" | "image/png" | "image/gif" | "image/bmp"
    )
}

pub fn broadcast_data_view(store: Store, data: &BroadcastData) -> impl IntoView + use<> {
    let picture = is_picture(&data.media_type).then(|| {
        let bytes = zgui_image::ImageBytes::new(data.bytes.clone());
        let url = bytes.url();
        on_cleanup_local(move || drop(bytes));
        AnyView::new(
            view! { image(class = "dk-object", src = Some(url), alt = Some(data.name.clone())) },
        )
    });
    let name = data.name.clone();
    let bytes = data.bytes.clone();
    let label = format!("Save {}", data.name);
    view! {
        column(class = "dk-pane") {
            {picture}
            row {
                control(
                    class = "btn",
                    on:click:stop = move |_| save_bytes(store, name.clone(), bytes.clone())
                ) {{label}}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_name_cannot_leave_the_download_folder() {
        assert_eq!(safe_name("slide.jpg"), "slide.jpg");
        assert_eq!(safe_name("../../etc/passwd"), ".._.._etc_passwd");
        assert_eq!(safe_name("a\u{7}b"), "a_b");
        assert_eq!(safe_name(".."), "download");
        assert_eq!(safe_name(""), "download");
    }

    #[test]
    fn only_pictures_render_inline() {
        assert!(is_picture("image/png"));
        assert!(!is_picture("text/html"));
    }
}
