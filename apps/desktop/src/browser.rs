use tauri::{
    Runtime, Url,
    webview::{NewWindowFeatures, NewWindowResponse},
};

pub fn open_in_browser<R: Runtime>(url: Url, _: NewWindowFeatures) -> NewWindowResponse<R> {
    if !is_web(&url) {
        tracing::warn!("refused to open {url}");
    } else if let Err(e) = tauri_plugin_opener::open_url(url.as_str(), None::<&str>) {
        tracing::warn!("could not open {url}: {e}");
    }
    NewWindowResponse::Deny
}

fn is_web(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn web(url: &str) -> bool {
        is_web(&url.parse().unwrap())
    }

    #[test]
    fn opens_only_web_links() {
        assert!(web("https://discord.gg/dYaRyGwBNw"));
        assert!(web("http://example.com"));
        assert!(!web("file:///etc/passwd"));
        assert!(!web("javascript:alert(1)"));
    }
}
