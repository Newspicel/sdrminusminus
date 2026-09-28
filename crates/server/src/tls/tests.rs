use std::time::SystemTime;

use super::*;

fn issue(dir: &Path, stem: &str) -> (PathBuf, PathBuf) {
    let issued = generate(&san_names(&[])).expect("generate");
    let cert_path = dir.join(format!("{stem}.pem"));
    let key_path = dir.join(format!("{stem}.key.pem"));
    std::fs::write(&cert_path, issued.cert.pem()).expect("write cert");
    std::fs::write(&key_path, issued.signing_key.serialize_pem()).expect("write key");
    (cert_path, key_path)
}

fn age(path: &Path, by: Duration) {
    let times = fs::FileTimes::new().set_modified(SystemTime::now() - by);
    fs::File::options()
        .write(true)
        .open(path)
        .expect("open")
        .set_times(times)
        .expect("set times");
}

#[test]
fn self_signed_material_survives_a_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (first, _) = self_signed(dir.path(), &[]).expect("first start");
    let (second, _) = self_signed(dir.path(), &[]).expect("second start");
    assert_eq!(first, second, "a restart minted a new certificate");
}

#[test]
fn a_certificate_near_its_expiry_is_replaced() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (first, _) = self_signed(dir.path(), &[]).expect("first start");
    age(
        &Kept::under(dir.path()).cert,
        RENEW_AFTER + Duration::from_secs(60),
    );
    let (second, _) = self_signed(dir.path(), &[]).expect("second start");
    assert_ne!(first, second, "an aged certificate was served again");
}

#[test]
fn unreadable_material_is_replaced_rather_than_fatal() {
    let dir = tempfile::tempdir().expect("tempdir");
    self_signed(dir.path(), &[]).expect("first start");
    fs::write(Kept::under(dir.path()).key, "shredded").expect("corrupt the key");
    self_signed(dir.path(), &[]).expect("second start");
}

#[test]
fn a_certificate_that_no_longer_names_this_machine_is_replaced() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (first, _) = self_signed(dir.path(), &[]).expect("first start");
    fs::write(Kept::under(dir.path()).names, "localhost").expect("move to another network");
    let (second, _) = self_signed(dir.path(), &[]).expect("second start");
    assert_ne!(first, second, "a certificate missing an address was reused");
}

#[test]
fn self_signed_names_cover_every_local_origin() {
    let names = san_names(&[]);
    for expected in ["localhost", "127.0.0.1", "::1"] {
        assert!(
            names.contains(&expected.to_owned()),
            "{expected} is missing"
        );
    }
    let mut unique = names.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), names.len(), "duplicate name in {names:?}");
}

#[cfg(unix)]
#[test]
fn the_self_signed_key_stays_private() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    self_signed(dir.path(), &[]).expect("start");
    let mode = fs::metadata(Kept::under(dir.path()).key)
        .expect("metadata")
        .permissions()
        .mode();
    assert_eq!(
        mode & 0o077,
        0,
        "key is readable beyond its owner: {mode:o}"
    );
}

#[test]
fn named_addresses_replace_the_discovered_ones() {
    let names = san_names(&["radio.example".to_owned(), "192.0.2.10".to_owned()]);
    assert_eq!(
        names,
        vec![
            "localhost",
            "127.0.0.1",
            "::1",
            "radio.example",
            "192.0.2.10"
        ]
    );
}

#[test]
fn a_named_certificate_is_reused_wherever_the_machine_runs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let named = ["radio.example".to_owned()];
    let (first, _) = self_signed(dir.path(), &named).expect("first start");
    let (second, _) = self_signed(dir.path(), &named).expect("second start");
    assert_eq!(first, second, "a restart minted a new certificate");
}

#[test]
fn a_certificate_and_key_from_files_serve() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (cert, key) = issue(dir.path(), "server");
    let served = load(&Tls::Files { cert, key }).expect("config");
    assert!(served.names.is_empty());
    assert_eq!(
        served.config.alpn_protocols,
        vec![b"h2".to_vec(), b"http/1.1".to_vec()]
    );
}

#[test]
fn a_missing_certificate_names_the_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cert = dir.path().join("absent.pem");
    let (_, key) = issue(dir.path(), "server");
    let Err(err) = load(&Tls::Files {
        cert: cert.clone(),
        key,
    }) else {
        panic!("a missing certificate must not serve");
    };
    assert!(
        err.to_string().contains(&cert.display().to_string()),
        "{err}"
    );
}

#[test]
fn a_certificate_that_is_not_pem_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (cert, key) = issue(dir.path(), "server");
    fs::write(&cert, "not a certificate").expect("overwrite");
    assert!(
        load(&Tls::Files { cert, key }).is_err(),
        "garbage must not serve"
    );
}

#[test]
fn a_key_from_another_certificate_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (cert, _) = issue(dir.path(), "first");
    let (_, key) = issue(dir.path(), "second");
    assert!(
        load(&Tls::Files { cert, key }).is_err(),
        "a mismatched pair must not serve"
    );
}

fn self_signed_in(dir: &Path, names: &[&str]) -> Served {
    load(&Tls::SelfSigned {
        dir: dir.to_path_buf(),
        names: names.iter().map(|name| (*name).to_owned()).collect(),
    })
    .expect("self-signed")
}

fn kept_certificate(dir: &Path) -> String {
    fs::read_to_string(Kept::under(dir).cert).expect("kept certificate")
}

#[test]
fn the_key_survives_a_re_mint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = self_signed_in(dir.path(), &["radio.example"]);
    let first_cert = kept_certificate(dir.path());
    let second = self_signed_in(dir.path(), &["radio.example", "192.0.2.10"]);
    assert_ne!(
        first_cert,
        kept_certificate(dir.path()),
        "new names kept the old certificate"
    );
    assert_eq!(first.pin, second.pin, "a re-mint changed the pin");
    assert_eq!(second.names, ["radio.example", "192.0.2.10"]);
}

#[test]
fn an_aged_certificate_keeps_its_pin() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = self_signed_in(dir.path(), &[]);
    age(
        &Kept::under(dir.path()).cert,
        RENEW_AFTER + Duration::from_secs(60),
    );
    assert_eq!(first.pin, self_signed_in(dir.path(), &[]).pin);
}

#[test]
fn a_lost_key_mints_a_new_pin() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = self_signed_in(dir.path(), &[]);
    fs::write(Kept::under(dir.path()).key, "shredded").expect("corrupt the key");
    let second = self_signed_in(dir.path(), &[]);
    assert_ne!(first.pin, second.pin, "a new key kept the old pin");
    fs::remove_file(Kept::under(dir.path()).key).expect("lose the key");
    assert_ne!(second.pin, self_signed_in(dir.path(), &[]).pin);
}

#[test]
fn a_key_that_does_not_match_the_certificate_mints_a_new_certificate() {
    let dir = tempfile::tempdir().expect("tempdir");
    self_signed_in(dir.path(), &["radio.example"]);
    let other = rcgen::KeyPair::generate().expect("key");
    fs::write(Kept::under(dir.path()).key, other.serialize_pem()).expect("swap the key");
    let served = self_signed_in(dir.path(), &["radio.example"]);
    assert_eq!(
        served.pin,
        sdrmm_wire::phone::hex(&Sha256::digest(other.subject_public_key_info()))
    );
}

#[test]
fn pin_is_spki_sha256() {
    let dir = tempfile::tempdir().expect("tempdir");
    let served = self_signed_in(dir.path(), &[]);
    let pem = fs::read_to_string(Kept::under(dir.path()).key).expect("key");
    let key = rcgen::KeyPair::from_pem(&pem).expect("key parses");
    let expected = sdrmm_wire::phone::hex(&Sha256::digest(key.subject_public_key_info()));
    assert_eq!(served.pin, expected);
    assert!(sdrmm_wire::phone::valid_pin(&served.pin));
    let chain = read_chain(&Kept::under(dir.path()).cert).expect("chain");
    assert_ne!(
        served.pin,
        sdrmm_wire::phone::hex(&Sha256::digest(chain[0].as_ref())),
        "the pin hashed the whole certificate"
    );
}

#[test]
fn host_local_is_a_san() {
    let names = san_names(&[]);
    assert!(
        names.contains(&crate::net::mdns_host()),
        "{names:?} misses the .local name"
    );
    let dir = tempfile::tempdir().expect("tempdir");
    let served = self_signed_in(dir.path(), &[]);
    assert!(served.names.contains(&crate::net::mdns_host()));
    for local in LOCAL_NAMES {
        assert!(!served.names.iter().any(|name| name == local), "{local}");
    }
}
