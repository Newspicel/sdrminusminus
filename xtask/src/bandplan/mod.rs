use std::{fs, path::Path, process::Command};

use anyhow::{Context, Result, bail};
use serde::Serialize;

mod bnetza;
mod cept;
mod fcc;
mod ofcom;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub(crate) struct Row {
    pub start_hz: f64,
    pub stop_hz: f64,
    pub service: &'static str,
    pub name: String,
    #[serde(skip_serializing_if = "is_true")]
    pub primary: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_step_hz: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl Row {
    pub(crate) fn new(start_hz: f64, stop_hz: f64, service: &'static str, name: String) -> Self {
        Self {
            start_hz,
            stop_hz,
            service,
            name,
            primary: true,
            reference: None,
            channel_step_hz: None,
            notes: None,
        }
    }
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_if signature"
)]
fn is_true(value: &bool) -> bool {
    *value
}

struct Source {
    generator: &'static str,
    url: &'static str,
    file: &'static str,
    kind: Kind,
}

enum Kind {
    PdfLayout,
    PdfBbox,
    Text,
}

static SOURCES: &[Source] = &[
    Source {
        generator: "ofcom",
        url: "https://static.ofcom.org.uk/static/spectrum/data/fatMapping.json",
        file: "ofcom-fat.json",
        kind: Kind::Text,
    },
    Source {
        generator: "bnetza",
        url: "https://data.bundesnetzagentur.de/Bundesnetzagentur/SharedDocs/Downloads/DE/\
               Sachgebiete/Telekommunikation/Unternehmen_Institutionen/Frequenzen/\
               20210114_frequenzplan.pdf",
        file: "bnetza-frequenzplan.pdf",
        kind: Kind::PdfLayout,
    },
    Source {
        generator: "cept",
        url: "https://efis.cept.org/reports/ReportDownloader?reportid=3",
        file: "cept-eca.csv",
        kind: Kind::Text,
    },
    Source {
        generator: "fcc",
        url: "https://transition.fcc.gov/oet/spectrum/table/fcctable.pdf",
        file: "fcc-table.pdf",
        kind: Kind::PdfBbox,
    },
];

struct Target {
    id: &'static str,
    name: &'static str,
    authority: &'static str,
    kind: &'static str,
}

pub(crate) fn run(root: &Path, offline: bool) -> Result<()> {
    let cache = root.join("target/bandplan-cache");
    fs::create_dir_all(&cache).context("bandplan cache")?;
    let out = root.join("crates/server/data/bandplan");
    let poppler = poppler_version();

    for source in SOURCES {
        let path = cache.join(source.file);
        if !path.exists() {
            if offline {
                bail!(
                    "--offline but {} is not cached; run once without it",
                    path.display()
                );
            }
            fetch(source, &path)?;
        }
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let digest = sha256(&bytes);
        let text = match source.kind {
            Kind::PdfLayout => pdftotext(&path, "-layout")?,
            Kind::PdfBbox => pdftotext(&path, "-bbox-layout")?,
            Kind::Text => String::from_utf8_lossy(&bytes).into_owned(),
        };

        let layers = match source.generator {
            "ofcom" => vec![(ofcom::TARGET, ofcom::parse(&text)?)],
            "bnetza" => vec![(bnetza::TARGET, bnetza::parse(&text)?)],
            "cept" => vec![(cept::TARGET, cept::parse(&text)?)],
            "fcc" => fcc::parse(&text)?,
            other => bail!("no importer named {other}"),
        };

        for (target, rows) in layers {
            let path = out.join(format!("{}.json", target.id));
            let version = match source.kind {
                Kind::PdfLayout | Kind::PdfBbox => poppler.as_deref(),
                Kind::Text => None,
            };
            write_layer(&path, source, target, &rows, &digest, version)?;
            println!(
                "{}: {} rows → {}",
                source.generator,
                rows.len(),
                path.display()
            );
        }
    }
    println!(
        "bandplan: regenerated. Review the diff — a table that changed shape is news, not noise."
    );
    Ok(())
}

fn write_layer(
    path: &Path,
    source: &Source,
    target: &Target,
    rows: &[Row],
    sha256: &str,
    poppler: Option<&str>,
) -> Result<()> {
    let mut provenance = serde_json::Map::new();
    provenance.insert("generator".into(), source.generator.into());
    provenance.insert("url".into(), source.url.into());
    provenance.insert("sha256".into(), sha256.into());
    if let Some(poppler) = poppler {
        provenance.insert("pdftotext".into(), poppler.into());
    }
    let doc = serde_json::json!({
        "id": target.id,
        "name": target.name,
        "authority": target.authority,
        "source": source.url,
        "kind": target.kind,
        "provenance": provenance,
        "entries": rows,
    });
    fs::write(path, format!("{}\n", serde_json::to_string_pretty(&doc)?))
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn fetch(source: &Source, to: &Path) -> Result<()> {
    println!("fetching {}", source.url);
    let status = curl(source.url, to, None)?;
    if status.success() {
        return Ok(());
    }
    if source.generator != "cept" {
        bail!("curl failed for {}", source.url);
    }

    let der = to.with_extension("ca.der");
    let pem = to.with_extension("ca.pem");
    let status = curl(
        "https://cacerts.digicert.com/RapidSSLTLSRSACAG1.crt",
        &der,
        None,
    )?;
    if !status.success() {
        bail!("curl failed for the EFIS TLS intermediate");
    }
    fs::write(&pem, pem_certificate(&fs::read(&der)?))?;
    let status = curl(source.url, to, Some(&pem))?;
    if !status.success() {
        bail!("curl failed for {}", source.url);
    }
    Ok(())
}

fn curl(url: &str, to: &Path, ca: Option<&Path>) -> Result<std::process::ExitStatus> {
    let mut command = Command::new("curl");
    command
        .args(["-fsSL", "--max-time", "600", "-o"])
        .arg(to)
        .arg(url);
    if let Some(ca) = ca {
        command.arg("--cacert").arg(ca);
    }
    command
        .status()
        .context("curl not found — it is how this fetches its sources")
}

fn pdftotext(path: &Path, mode: &str) -> Result<String> {
    let out = Command::new("pdftotext")
        .arg(mode)
        .arg(path)
        .arg("-")
        .output()
        .context(
            "pdftotext not found. It is poppler: `apt install poppler-utils` or \
             `brew install poppler`. Only the importers need it — not the build, not the server",
        )?;
    if !out.status.success() {
        bail!("pdftotext failed on {}", path.display());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn pem_certificate(der: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(der.len().div_ceil(3) * 4);
    for chunk in der.chunks(3) {
        let bits = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        encoded.push(char::from(ALPHABET[((bits >> 18) & 63) as usize]));
        encoded.push(char::from(ALPHABET[((bits >> 12) & 63) as usize]));
        encoded.push(if chunk.len() > 1 {
            char::from(ALPHABET[((bits >> 6) & 63) as usize])
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            char::from(ALPHABET[(bits & 63) as usize])
        } else {
            '='
        });
    }
    let body = encoded
        .as_bytes()
        .chunks(64)
        .map(|line| String::from_utf8_lossy(line))
        .collect::<Vec<_>>()
        .join("\n");
    format!("-----BEGIN CERTIFICATE-----\n{body}\n-----END CERTIFICATE-----\n")
}

fn poppler_version() -> Option<String> {
    let out = Command::new("pdftotext").arg("-v").output().ok()?;
    let text = String::from_utf8_lossy(&out.stderr);
    text.lines().next().map(|line| line.trim().to_string())
}

fn sha256(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a_2f98,
        0x7137_4491,
        0xb5c0_fbcf,
        0xe9b5_dba5,
        0x3956_c25b,
        0x59f1_11f1,
        0x923f_82a4,
        0xab1c_5ed5,
        0xd807_aa98,
        0x1283_5b01,
        0x2431_85be,
        0x550c_7dc3,
        0x72be_5d74,
        0x80de_b1fe,
        0x9bdc_06a7,
        0xc19b_f174,
        0xe49b_69c1,
        0xefbe_4786,
        0x0fc1_9dc6,
        0x240c_a1cc,
        0x2de9_2c6f,
        0x4a74_84aa,
        0x5cb0_a9dc,
        0x76f9_88da,
        0x983e_5152,
        0xa831_c66d,
        0xb003_27c8,
        0xbf59_7fc7,
        0xc6e0_0bf3,
        0xd5a7_9147,
        0x06ca_6351,
        0x1429_2967,
        0x27b7_0a85,
        0x2e1b_2138,
        0x4d2c_6dfc,
        0x5338_0d13,
        0x650a_7354,
        0x766a_0abb,
        0x81c2_c92e,
        0x9272_2c85,
        0xa2bf_e8a1,
        0xa81a_664b,
        0xc24b_8b70,
        0xc76c_51a3,
        0xd192_e819,
        0xd699_0624,
        0xf40e_3585,
        0x106a_a070,
        0x19a4_c116,
        0x1e37_6c08,
        0x2748_774c,
        0x34b0_bcb5,
        0x391c_0cb3,
        0x4ed8_aa4a,
        0x5b9c_ca4f,
        0x682e_6ff3,
        0x748f_82ee,
        0x78a5_636f,
        0x84c8_7814,
        0x8cc7_0208,
        0x90be_fffa,
        0xa450_6ceb,
        0xbef9_a3f7,
        0xc671_78f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];

    let mut message = bytes.to_vec();
    let bits = (bytes.len() as u64) * 8;
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bits.to_be_bytes());

    for chunk in message.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (at, word) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[at] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for at in 16..64 {
            let s0 = w[at - 15].rotate_right(7) ^ w[at - 15].rotate_right(18) ^ (w[at - 15] >> 3);
            let s1 = w[at - 2].rotate_right(17) ^ w[at - 2].rotate_right(19) ^ (w[at - 2] >> 10);
            w[at] = w[at - 16]
                .wrapping_add(s0)
                .wrapping_add(w[at - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for at in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[at])
                .wrapping_add(w[at]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, value) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(value);
        }
    }
    h.iter().map(|word| format!("{word:08x}")).collect()
}

pub(crate) fn service_of(
    name: &str,
    table: &[(&str, &str)],
    unmapped: &mut Vec<String>,
) -> &'static str {
    const CATEGORIES: [&str; 10] = [
        "amateur",
        "broadcast",
        "aeronautical",
        "maritime",
        "mobile",
        "satellite",
        "navigation",
        "science",
        "ism",
        "other",
    ];
    let haystack = name.to_lowercase();
    for (needle, category) in table {
        if haystack.contains(needle) {
            return CATEGORIES
                .iter()
                .find(|known| *known == category)
                .copied()
                .unwrap_or("other");
        }
    }
    if !unmapped.iter().any(|seen| seen == name) {
        unmapped.push(name.to_string());
    }
    "other"
}

pub(crate) fn report_unmapped(generator: &str, unmapped: &[String]) {
    if unmapped.is_empty() {
        return;
    }
    println!(
        "{generator}: {} service name(s) fell through to `other` — extend the mapping table:",
        unmapped.len()
    );
    for name in unmapped {
        println!("    {name}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_the_published_vectors() {
        assert_eq!(
            sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn an_unmapped_service_falls_back_and_is_reported() {
        let table = [("maritime", "maritime")];
        let mut unmapped = Vec::new();
        assert_eq!(
            service_of("MARITIME MOBILE", &table, &mut unmapped),
            "maritime"
        );
        assert!(unmapped.is_empty());
        assert_eq!(
            service_of("QUANTUM TELEPATHY", &table, &mut unmapped),
            "other"
        );
        assert_eq!(unmapped, vec!["QUANTUM TELEPATHY"]);
        service_of("QUANTUM TELEPATHY", &table, &mut unmapped);
        assert_eq!(unmapped.len(), 1);
    }

    #[test]
    fn der_certificate_is_wrapped_as_standard_pem() {
        assert_eq!(
            pem_certificate(b"abcde"),
            "-----BEGIN CERTIFICATE-----\nYWJjZGU=\n-----END CERTIFICATE-----\n"
        );
    }
}
