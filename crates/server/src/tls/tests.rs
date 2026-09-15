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
    let config = server_config(&Tls::Files { cert, key }).expect("config");
    assert_eq!(
        config.alpn_protocols,
        vec![b"h2".to_vec(), b"http/1.1".to_vec()]
    );
}

#[test]
fn a_missing_certificate_names_the_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cert = dir.path().join("absent.pem");
    let (_, key) = issue(dir.path(), "server");
    let err = server_config(&Tls::Files {
        cert: cert.clone(),
        key,
    })
    .expect_err("a missing certificate must not serve");
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
    server_config(&Tls::Files { cert, key }).expect_err("garbage must not serve");
}

#[test]
fn a_key_from_another_certificate_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (cert, _) = issue(dir.path(), "first");
    let (_, key) = issue(dir.path(), "second");
    server_config(&Tls::Files { cert, key }).expect_err("a mismatched pair must not serve");
}
