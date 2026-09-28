use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use rcgen::PublicKeyData;
use rustls::{
    ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};
use sha2::{Digest, Sha256};

const MATERIAL_DIR: &str = "tls";
const CERT_FILE: &str = "self-signed.pem";
const KEY_FILE: &str = "self-signed.key.pem";
const NAMES_FILE: &str = "self-signed.names";
const VALID_DAYS: i64 = 397;
const RENEW_AFTER: Duration = Duration::from_secs(365 * 24 * 60 * 60);
const LOCAL_NAMES: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

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
    #[error("cannot pin the certificate: {0}")]
    Pin(String),
}

pub(crate) struct Served {
    pub(crate) config: Arc<ServerConfig>,
    pub(crate) pin: String,
    pub(crate) names: Vec<String>,
}

pub(crate) fn load(tls: &Tls) -> Result<Served, TlsError> {
    let (chain, key, names) = match tls {
        Tls::Files { cert, key } => (read_chain(cert)?, read_key(key)?, Vec::new()),
        Tls::SelfSigned { dir, names } => {
            let names = san_names(names);
            let (chain, key) = kept_or_minted(&Kept::under(dir), &names)?;
            (chain, key, reachable_names(names))
        }
    };
    let Some(leaf) = chain.first() else {
        return Err(TlsError::Pin("no certificate to pin".to_owned()));
    };
    let pin = spki_pin(leaf)?;
    tracing::info!(sha256 = %fingerprint(leaf), pin = %pin, "serving HTTPS");
    let mut config = ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_no_client_auth()
    .with_single_cert(chain, key)?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(Served {
        config: Arc::new(config),
        pin,
        names,
    })
}

pub(crate) fn spki_pin(cert: &CertificateDer<'_>) -> Result<String, TlsError> {
    sdrmm_wire::phone::spki_pin(cert.as_ref()).map_err(|error| TlsError::Pin(error.to_string()))
}

fn reachable_names(names: Vec<String>) -> Vec<String> {
    names
        .into_iter()
        .filter(|name| !LOCAL_NAMES.contains(&name.as_str()))
        .collect()
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

#[cfg(test)]
fn self_signed(dir: &Path, asked_for: &[String]) -> Result<Material, TlsError> {
    kept_or_minted(&Kept::under(dir), &san_names(asked_for))
}

fn kept_or_minted(kept: &Kept, names: &[String]) -> Result<Material, TlsError> {
    let key = kept_key(&kept.key);
    if let Some(key) = &key
        && fresh(&kept.cert)
        && issued_for(&kept.names, names)
        && let Ok(chain) = read_chain(&kept.cert)
        && certifies(&chain, key)
        && let Ok(der) = read_key(&kept.key)
    {
        return Ok((chain, der));
    }
    let key = match key {
        Some(key) => key,
        None => rcgen::KeyPair::generate()?,
    };
    tracing::info!(names = %names.join(", "), "minting a self-signed certificate");
    let cert = certificate(names, &key)?;
    store(kept, names, &cert, &key)?;
    Ok((read_chain(&kept.cert)?, read_key(&kept.key)?))
}

fn kept_key(path: &Path) -> Option<rcgen::KeyPair> {
    let pem = match fs::read_to_string(path) {
        Ok(pem) => pem,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            tracing::info!("new TLS key");
            return None;
        }
        Err(error) => {
            tracing::warn!(%error, "new TLS key: phones must pair again");
            return None;
        }
    };
    match rcgen::KeyPair::from_pem(&pem) {
        Ok(key) => Some(key),
        Err(error) => {
            tracing::warn!(%error, "new TLS key: phones must pair again");
            None
        }
    }
}

fn certifies(chain: &[CertificateDer<'static>], key: &rcgen::KeyPair) -> bool {
    let wanted = sdrmm_wire::phone::hex(&Sha256::digest(key.subject_public_key_info()));
    chain
        .first()
        .and_then(|leaf| spki_pin(leaf).ok())
        .is_some_and(|pin| pin == wanted)
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

#[cfg(test)]
fn generate(names: &[String]) -> Result<rcgen::CertifiedKey<rcgen::KeyPair>, rcgen::Error> {
    let signing_key = rcgen::KeyPair::generate()?;
    let cert = certificate(names, &signing_key)?;
    Ok(rcgen::CertifiedKey { cert, signing_key })
}

fn certificate(names: &[String], key: &rcgen::KeyPair) -> Result<rcgen::Certificate, rcgen::Error> {
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
    params.self_signed(key)
}

fn distinguished_name() -> rcgen::DistinguishedName {
    let mut name = rcgen::DistinguishedName::new();
    name.push(rcgen::DnType::CommonName, "SDR--");
    name
}

fn san_names(asked_for: &[String]) -> Vec<String> {
    let mut names: Vec<String> = LOCAL_NAMES.iter().map(|name| (*name).to_owned()).collect();
    let reachable = if asked_for.is_empty() {
        let mut discovered = crate::net::lan_addresses();
        discovered.push(crate::net::mdns_host());
        discovered
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
    cert: &rcgen::Certificate,
    key: &rcgen::KeyPair,
) -> Result<(), TlsError> {
    fs::create_dir_all(&kept.dir).map_err(wrote(&kept.dir))?;
    fs::write(&kept.cert, cert.pem()).map_err(wrote(&kept.cert))?;
    fs::write(&kept.names, names.join("\n")).map_err(wrote(&kept.names))?;
    write_private(&kept.key, &key.serialize_pem()).map_err(wrote(&kept.key))
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
