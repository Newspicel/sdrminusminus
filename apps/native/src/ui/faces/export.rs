use std::path::PathBuf;

use zgui::prelude::*;

use crate::{
    binding,
    store::Store,
    ui::kit_audio::{self, button},
};

#[derive(Clone, Copy)]
enum Format {
    Csv,
    Json,
}

impl Format {
    const fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Json => "json",
        }
    }
}

#[must_use]
fn export_path(format: Format, sink: &str) -> String {
    format!(
        "/api/decoderlog/export/{}?sink={}",
        format.extension(),
        encode(sink)
    )
}

#[must_use]
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn target(format: Format, sink: &str) -> PathBuf {
    let folder = dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(std::env::temp_dir);
    folder.join(format!(
        "decoder-log-{}.{}",
        encode(sink),
        format.extension()
    ))
}

fn download(store: Store, format: Format, sink: String) {
    zgui::task::spawn_local(async move {
        let bytes = match store.api().bytes(&export_path(format, &sink)).await {
            Ok(bytes) => bytes,
            Err(error) => {
                store.say(format!("cannot export the log: {error}"));
                return;
            }
        };
        let path = target(format, &sink);
        match std::fs::write(&path, bytes) {
            Ok(()) => store.say(format!("Saved {}", path.display())),
            Err(error) => store.say(format!("cannot save {}: {error}", path.display())),
        }
    });
}

pub fn face(store: Store, node: String) -> impl IntoView {
    kit_audio::install();
    let wired = {
        let node = node.clone();
        Signal::derive(move || !binding::sources_of(&store.graph.get(), &node, "events").is_empty())
    };
    let idle = Signal::derive(move || !wired.get());
    let offer = move |format: Format, label: &'static str| {
        let sink = node.clone();
        button(
            move || String::from(label),
            Signal::stored(false),
            idle,
            move || download(store, format, sink.clone()),
        )
    };
    view! {
        column(class = "face") {
            text(class = "hint") {{move || if wired.get() { "Every logged row, as one file" } else { "Wire decoders in" }}}
            row(class = "face__foot") {
                {offer(Format::Csv, "CSV")}
                {offer(Format::Json, "JSON")}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_export_asks_for_the_rows_of_its_own_sink() {
        assert_eq!(
            export_path(Format::Csv, "export-1"),
            "/api/decoderlog/export/csv?sink=export-1"
        );
        assert_eq!(
            export_path(Format::Json, "a b&c"),
            "/api/decoderlog/export/json?sink=a%20b%26c"
        );
    }

    #[test]
    fn a_saved_file_is_named_by_sink_and_format() {
        let path = target(Format::Csv, "export-1");
        assert!(path.ends_with("decoder-log-export-1.csv"));
    }
}
