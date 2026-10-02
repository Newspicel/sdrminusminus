use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};

pub type Digests = BTreeMap<String, String>;

pub fn parse(text: &str) -> Result<Digests> {
    let mut digests = Digests::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let (digest, file) = line
            .split_once("  ")
            .with_context(|| format!("`{line}` is not a `shasum -a 256` line"))?;
        ensure!(
            digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "`{digest}` is not a SHA-256 digest"
        );
        digests.insert(file.trim().to_string(), digest.to_string());
    }
    ensure!(!digests.is_empty(), "the checksum file lists no artifact");
    Ok(digests)
}

pub fn digest<'a>(digests: &'a Digests, file: &str) -> Result<&'a str> {
    digests.get(file).map(String::as_str).with_context(|| {
        format!(
            "the release carries no `{file}`, so the package would point at a download that does not \
             exist"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_truncated_digest_is_refused() {
        let err = parse("abc  sdrmm-1.2.3-aarch64-apple-darwin.tar.gz")
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a SHA-256 digest"), "{err}");
    }
}
