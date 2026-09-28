const FALLBACK_LABEL: &str = "sdrmm";
const MAX_LABEL_BYTES: usize = 63;

pub(crate) fn host_label() -> String {
    label_of(&gethostname::gethostname().to_string_lossy())
}

fn label_of(host: &str) -> String {
    let first = host.split_once('.').map_or(host, |(head, _)| head);
    let mapped: String = first
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = mapped.trim_matches('-');
    let label = trimmed[..trimmed.len().min(MAX_LABEL_BYTES)].trim_end_matches('-');
    if label.is_empty() {
        FALLBACK_LABEL.to_string()
    } else {
        label.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_labels_are_dns_safe() {
        assert_eq!(label_of("Julians-MacBook-Pro.local"), "julians-macbook-pro");
        assert_eq!(label_of("pi"), "pi");
        assert_eq!(label_of("my_host.lan"), "my-host");
        assert_eq!(label_of("-edge-"), "edge");
        assert_eq!(label_of("Ünïcode"), "n-code");
        assert_eq!(label_of(""), FALLBACK_LABEL);
        assert_eq!(label_of("---.local"), FALLBACK_LABEL);
        assert_eq!(label_of(&"a".repeat(100)).len(), MAX_LABEL_BYTES);
        assert_eq!(label_of(&format!("{}-b", "a".repeat(62))), "a".repeat(62));
    }

    #[test]
    fn this_host_has_a_label() {
        let label = host_label();
        assert!(!label.is_empty() && label.len() <= MAX_LABEL_BYTES);
        assert!(
            label
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        );
    }
}
