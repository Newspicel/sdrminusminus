#[must_use]
pub fn server_down_detail(reason: Option<&str>) -> Option<String> {
    let trimmed = reason.map(str::trim).unwrap_or_default();
    let lower = trimmed.to_lowercase();
    let opaque = lower == "failed to fetch"
        || lower == "load failed"
        || lower.starts_with("networkerror")
        || lower.starts_with("typeerror:")
        || lower.ends_with("no response from the server")
        || lower.contains("connection refused")
        || lower.contains("error sending request");
    (!trimmed.is_empty() && !opaque).then(|| trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_what_the_transport_says_when_a_connection_is_simply_refused() {
        assert_eq!(server_down_detail(Some("Failed to fetch")), None);
        assert_eq!(server_down_detail(Some("Load failed")), None);
        assert_eq!(
            server_down_detail(Some("NetworkError when attempting to fetch resource.")),
            None
        );
        assert_eq!(server_down_detail(Some("TypeError: Failed to fetch")), None);
        assert_eq!(
            server_down_detail(Some(
                "http://127.0.0.1:1/api/workspaces: error sending request: Connection refused"
            )),
            None
        );
    }

    #[test]
    fn drops_the_proxy_empty_500() {
        assert_eq!(
            server_down_detail(Some("HTTP 500: no response from the server")),
            None
        );
    }

    #[test]
    fn keeps_a_reason_the_server_itself_gave() {
        assert_eq!(
            server_down_detail(Some("HTTP 503: starting up")).as_deref(),
            Some("HTTP 503: starting up")
        );
        assert_eq!(
            server_down_detail(Some("database is locked")).as_deref(),
            Some("database is locked")
        );
    }

    #[test]
    fn treats_blank_and_missing_alike() {
        assert_eq!(server_down_detail(None), None);
        assert_eq!(server_down_detail(Some("   ")), None);
    }
}
