use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use rustls::{
    ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};
use sha2::{Digest, Sha256};

const MATERIAL_DIR: &str = "tls";
const CERT_FILE: &str = "self-signed.pem";
const KEY_FILE: &str = "self-signed.key.pem";
const NAMES_FILE: &str = "self-signed.names";
// Browsers distrust a server certificate valid for more than 398 days, self-signed included.
const VALID_DAYS: i64 = 397;
const RENEW_AFTER: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// Where the listener gets the certificate it presents.
///
/// Self-signed material is kept on disk: a phone that accepted the certificate once for field
/// mode keeps trusting it, where a freshly minted key on every restart would ask again each time.
/// It is minted again once the addresses it covers change, since a certificate that does not name
/// the address a client dialled is worse than an unknown one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tls {
    Files { cert: PathBuf, key: PathBuf },
    SelfSigned { dir: PathBuf, names: Vec<String> },
}

#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("cannot read the certificate chain {}: {source}", path.display())]
    Certificate {
        path: PathBuf,
        #[source]
        source: rustls::pki_types::pem::Error,
    },
    #[error("{} holds no certificate", path.display())]
    EmptyChain { path: PathBuf },
    #[error("cannot read the private key {}: {source}", path.display())]
    PrivateKey {
        path: PathBuf,
        #[source]
        source: rustls::pki_types::pem::Error,
    },
    #[error("cannot generate a self-signed certificate: {0}")]
    Generate(#[from] rcgen::Error),
    #[error("cannot write {}: {source}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the certificate and key cannot serve TLS: {0}")]
    Unusable(#[from] rustls::Error),
}

pub(crate) fn server_config(tls: &Tls) -> Result<ServerConfig, TlsError> {
    let (chain, key) = match tls {
        Tls::Files { cert, key } => (read_chain(cert)?, read_key(key)?),
        Tls::SelfSigned { dir, names } => self_signed(dir, names)?,
    };
    if let Some(leaf) = chain.first() {
        tracing::info!(sha256 = %fingerprint(leaf), "serving HTTPS");
    }
    let mut config = ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_no_client_auth()
    .with_single_cert(chain, key)?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

type Material = (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>);

fn read_chain(path: &Path) -> Result<Vec<CertificateDer<'static>>, TlsError> {
    let chain = CertificateDer::pem_file_iter(path)
        .and_then(|certs| certs.collect::<Result<Vec<_>, _>>())
        .map_err(|source| TlsError::Certificate {
            path: path.to_path_buf(),
            source,
        })?;
    if chain.is_empty() {
        return Err(TlsError::EmptyChain {
            path: path.to_path_buf(),
        });
    }
    Ok(chain)
}

fn read_key(path: &Path) -> Result<PrivateKeyDer<'static>, TlsError> {
    PrivateKeyDer::from_pem_file(path).map_err(|source| TlsError::PrivateKey {
        path: path.to_path_buf(),
        source,
    })
}

struct Kept {
    dir: PathBuf,
    cert: PathBuf,
    key: PathBuf,
    names: PathBuf,
}

impl Kept {
    fn under(dir: &Path) -> Self {
        let dir = dir.join(MATERIAL_DIR);
        Self {
            cert: dir.join(CERT_FILE),
            key: dir.join(KEY_FILE),
            names: dir.join(NAMES_FILE),
            dir,
        }
    }
}

fn self_signed(dir: &Path, asked_for: &[String]) -> Result<Material, TlsError> {
    let kept = Kept::under(dir);
    let names = san_names(asked_for);
    if fresh(&kept.cert)
        && issued_for(&kept.names, &names)
        && let Ok(chain) = read_chain(&kept.cert)
        && let Ok(key) = read_key(&kept.key)
    {
        return Ok((chain, key));
    }
    tracing::info!(names = %names.join(", "), "minting a self-signed certificate");
    store(&kept, &names, &generate(&names)?)?;
    Ok((read_chain(&kept.cert)?, read_key(&kept.key)?))
}

fn fresh(cert_path: &Path) -> bool {
    fs::metadata(cert_path)
        .and_then(|meta| meta.modified())
        .and_then(|written| written.elapsed().map_err(io::Error::other))
        .is_ok_and(|age| age < RENEW_AFTER)
}

fn issued_for(names_path: &Path, names: &[String]) -> bool {
    fs::read_to_string(names_path)
        .is_ok_and(|kept| kept.lines().eq(names.iter().map(String::as_str)))
}

fn generate(names: &[String]) -> Result<rcgen::CertifiedKey<rcgen::KeyPair>, rcgen::Error> {
    let mut params = rcgen::CertificateParams::new(names.to_vec())?;
    params.distinguished_name = distinguished_name();
    let today = jiff::Timestamp::now()
        .to_zoned(jiff::tz::TimeZone::UTC)
        .date();
    let from = today.saturating_sub(jiff::Span::new().days(1));
    let until = today.saturating_add(jiff::Span::new().days(VALID_DAYS));
    params.not_before = rcgen::date_time_ymd(
        i32::from(from.year()),
        from.month().unsigned_abs(),
        from.day().unsigned_abs(),
    );
    params.not_after = rcgen::date_time_ymd(
        i32::from(until.year()),
        until.month().unsigned_abs(),
        until.day().unsigned_abs(),
    );
    let signing_key = rcgen::KeyPair::generate()?;
    let cert = params.self_signed(&signing_key)?;
    Ok(rcgen::CertifiedKey { cert, signing_key })
}

fn distinguished_name() -> rcgen::DistinguishedName {
    let mut name = rcgen::DistinguishedName::new();
    name.push(rcgen::DnType::CommonName, "sdr--");
    name
}

/// Every name the self-signed certificate has to answer to.
///
/// Named addresses replace the discovered ones rather than adding to them: inside a container the
/// addresses this process can see are the bridge's, not the ones a browser dials, and they change
/// often enough that keeping them would mint a new certificate on most restarts.
fn san_names(asked_for: &[String]) -> Vec<String> {
    let mut names = vec![
        "localhost".to_owned(),
        "127.0.0.1".to_owned(),
        "::1".to_owned(),
    ];
    let reachable = if asked_for.is_empty() {
        crate::notices::lan_addresses()
    } else {
        asked_for.to_vec()
    };
    for name in reachable {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

fn store(
    kept: &Kept,
    names: &[String],
    issued: &rcgen::CertifiedKey<rcgen::KeyPair>,
) -> Result<(), TlsError> {
    fs::create_dir_all(&kept.dir).map_err(wrote(&kept.dir))?;
    fs::write(&kept.cert, issued.cert.pem()).map_err(wrote(&kept.cert))?;
    fs::write(&kept.names, names.join("\n")).map_err(wrote(&kept.names))?;
    write_private(&kept.key, &issued.signing_key.serialize_pem()).map_err(wrote(&kept.key))
}

fn wrote(path: &Path) -> impl FnOnce(io::Error) -> TlsError {
    let path = path.to_path_buf();
    move |source| TlsError::Write { path, source }
}

#[cfg(unix)]
fn write_private(path: &Path, pem: &str) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::write(path, pem)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn write_private(path: &Path, pem: &str) -> io::Result<()> {
    fs::write(path, pem)
}

fn fingerprint(cert: &CertificateDer<'_>) -> String {
    Sha256::digest(cert.as_ref())
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(test)]
mod tests;
