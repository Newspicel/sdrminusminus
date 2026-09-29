use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

const VISUAL_CPP_RUNTIME: [&str; 6] = [
    "vcruntime",
    "msvcp",
    "concrt",
    "vccorlib",
    "vcomp",
    "ucrtbased",
];
const API_SETS: [&str; 2] = ["api-ms-win-", "ext-ms-win-"];
const IMPORTS: usize = 1;
const DELAY_IMPORTS: usize = 13;
const SECTION_HEADER: usize = 40;

pub fn is_pe(head: &[u8; 4]) -> bool {
    head.starts_with(b"MZ")
}

pub fn check(path: &Path, images: &[PathBuf], external: &[String]) -> Result<()> {
    let system = std::env::var_os("SystemRoot").map(|root| PathBuf::from(root).join("System32"));
    let mut edges = 0usize;
    let mut failures = Vec::new();
    for image in images {
        let data = std::fs::read(image).with_context(|| format!("read {}", image.display()))?;
        let dir = image.parent().unwrap_or(Path::new("."));
        for dll in imports(&data).with_context(|| format!("parse {}", image.display()))? {
            edges += 1;
            if let Some(reason) = unresolved(&dll, dir, system.as_deref(), external) {
                failures.push(format!(
                    "{}\n    needs {dll}\n    {reason}",
                    image.display()
                ));
            }
        }
    }
    ensure!(
        failures.is_empty(),
        "{} would not launch on a clean Windows: {} unresolved:\n  {}",
        path.display(),
        failures.len(),
        failures.join("\n  ")
    );
    println!(
        "link closure: {edges} imports across {} PE files in {} all resolve",
        images.len(),
        path.display()
    );
    Ok(())
}

fn unresolved(
    dll: &str,
    dir: &Path,
    system: Option<&Path>,
    external: &[String],
) -> Option<&'static str> {
    let name = dll.to_ascii_lowercase();
    if dir.join(dll).is_file() {
        None
    } else if VISUAL_CPP_RUNTIME
        .iter()
        .any(|prefix| name.starts_with(prefix))
    {
        Some("the Visual C++ runtime is not part of Windows: link it statically")
    } else if API_SETS.iter().any(|prefix| name.starts_with(prefix))
        || external
            .iter()
            .any(|fragment| name.contains(&fragment.to_ascii_lowercase()))
        || system.is_some_and(|system| system.join(dll).is_file())
    {
        None
    } else {
        Some("neither shipped next to it nor in System32")
    }
}

fn imports(data: &[u8]) -> Result<Vec<String>> {
    let signature = u32_at(data, 0x3c)? as usize;
    ensure!(
        data.get(signature..signature + 4) == Some(b"PE\0\0".as_slice()),
        "no PE signature"
    );
    let coff = signature + 4;
    let optional = coff + 20;
    let table = optional + usize::from(u16_at(data, coff + 16)?);
    let (count_at, directories) = match u16_at(data, optional)? {
        0x10b => (optional + 92, optional + 96),
        0x20b => (optional + 108, optional + 112),
        magic => bail!("unknown optional header magic {magic:#x}"),
    };
    let image = Image {
        data,
        sections: (0..usize::from(u16_at(data, coff + 2)?))
            .map(|index| Section::read(data, table + index * SECTION_HEADER))
            .collect::<Result<_>>()?,
    };
    let count = u32_at(data, count_at)? as usize;
    let mut names = Vec::new();
    for (directory, stride, name_at) in [(IMPORTS, 20, 12), (DELAY_IMPORTS, 32, 4)] {
        if directory < count {
            let rva = u32_at(data, directories + directory * 8)?;
            names.extend(image.names(rva, stride, name_at)?);
        }
    }
    Ok(names)
}

struct Section {
    address: u32,
    size: u32,
    offset: u32,
}

impl Section {
    fn read(data: &[u8], at: usize) -> Result<Self> {
        Ok(Self {
            size: u32_at(data, at + 8)?.max(u32_at(data, at + 16)?),
            address: u32_at(data, at + 12)?,
            offset: u32_at(data, at + 20)?,
        })
    }
}

struct Image<'a> {
    data: &'a [u8],
    sections: Vec<Section>,
}

impl Image<'_> {
    fn offset(&self, rva: u32) -> Result<usize> {
        self.sections
            .iter()
            .find(|section| {
                (section.address..section.address.saturating_add(section.size)).contains(&rva)
            })
            .map(|section| section.offset as usize + (rva - section.address) as usize)
            .with_context(|| format!("RVA {rva:#x} lies in no section"))
    }

    fn names(&self, rva: u32, stride: usize, name_at: usize) -> Result<Vec<String>> {
        let mut names = Vec::new();
        if rva == 0 {
            return Ok(names);
        }
        let mut descriptor = self.offset(rva)?;
        loop {
            let name = u32_at(self.data, descriptor + name_at)?;
            if name == 0 {
                return Ok(names);
            }
            names.push(self.string(name)?);
            descriptor += stride;
        }
    }

    fn string(&self, rva: u32) -> Result<String> {
        let bytes = self
            .data
            .get(self.offset(rva)?..)
            .context("name past the end of the file")?;
        let end = bytes
            .iter()
            .position(|&byte| byte == 0)
            .context("unterminated name")?;
        Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }
}

fn u16_at(data: &[u8], at: usize) -> Result<u16> {
    let bytes = data
        .get(at..at + 2)
        .and_then(|bytes| bytes.try_into().ok())
        .context("truncated PE header")?;
    Ok(u16::from_le_bytes(bytes))
}

fn u32_at(data: &[u8], at: usize) -> Result<u32> {
    let bytes = data
        .get(at..at + 4)
        .and_then(|bytes| bytes.try_into().ok())
        .context("truncated PE header")?;
    Ok(u32::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPTIONAL: usize = 0x58;
    const TABLE: usize = OPTIONAL + 240;
    const RAW: usize = 0x200;
    const RVA: u32 = 0x1000;

    fn put(data: &mut [u8], at: usize, bytes: &[u8]) {
        data[at..at + bytes.len()].copy_from_slice(bytes);
    }

    fn image(magic: u16, imports: &[&str], delayed: &[&str]) -> Vec<u8> {
        let (count_at, directories) = if magic == 0x20b { (108, 112) } else { (92, 96) };
        let mut data = vec![0u8; RAW + 0x400];
        put(&mut data, 0, b"MZ");
        put(&mut data, 0x3c, &0x40u32.to_le_bytes());
        put(&mut data, 0x40, b"PE\0\0");
        put(&mut data, 0x46, &1u16.to_le_bytes());
        put(&mut data, 0x54, &240u16.to_le_bytes());
        put(&mut data, OPTIONAL, &magic.to_le_bytes());
        put(&mut data, OPTIONAL + count_at, &16u32.to_le_bytes());
        put(&mut data, TABLE + 8, &0x400u32.to_le_bytes());
        put(&mut data, TABLE + 12, &RVA.to_le_bytes());
        put(&mut data, TABLE + 20, &(RAW as u32).to_le_bytes());
        let delay_table = (imports.len() + 1) * 20;
        let mut text = delay_table + (delayed.len() + 1) * 32;
        for (directory, table, stride, name_at, names) in [
            (IMPORTS, 0, 20, 12, imports),
            (DELAY_IMPORTS, delay_table, 32, 4, delayed),
        ] {
            let entry = OPTIONAL + directories + directory * 8;
            put(&mut data, entry, &(RVA + table as u32).to_le_bytes());
            for (index, name) in names.iter().enumerate() {
                let descriptor = RAW + table + index * stride + name_at;
                put(&mut data, descriptor, &(RVA + text as u32).to_le_bytes());
                put(&mut data, RAW + text, name.as_bytes());
                text += name.len() + 1;
            }
        }
        data
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pe-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn direct_and_delayed_imports_are_read_from_both_formats() {
        for magic in [0x10b, 0x20b] {
            let data = image(
                magic,
                &["KERNEL32.dll", "avcodec-63.dll"],
                &["VCRUNTIME140.dll"],
            );
            assert_eq!(
                imports(&data).unwrap(),
                ["KERNEL32.dll", "avcodec-63.dll", "VCRUNTIME140.dll"]
            );
        }
    }

    #[test]
    fn a_file_without_a_pe_signature_is_refused() {
        let mut data = image(0x20b, &[], &[]);
        put(&mut data, 0x40, b"NE\0\0");
        assert!(imports(&data).is_err());
    }

    #[test]
    fn the_visual_cpp_runtime_resolves_only_when_shipped() {
        let dir = scratch("runtime");
        assert!(unresolved("VCRUNTIME140.dll", &dir, Some(&dir), &[]).is_some());
        assert!(unresolved("msvcp140.dll", &dir, Some(&dir), &[]).is_some());
        std::fs::write(dir.join("VCRUNTIME140.dll"), b"").unwrap();
        assert!(unresolved("VCRUNTIME140.dll", &dir, None, &[]).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn api_sets_system_and_external_libraries_resolve() {
        let dir = scratch("system");
        let system = dir.join("System32");
        std::fs::create_dir_all(&system).unwrap();
        std::fs::write(system.join("KERNEL32.dll"), b"").unwrap();
        let external = ["soapysdr".to_string()];
        assert!(unresolved("api-ms-win-crt-heap-l1-1-0.dll", &dir, None, &[]).is_none());
        assert!(unresolved("KERNEL32.dll", &dir, Some(&system), &[]).is_none());
        assert!(unresolved("SoapySDR.dll", &dir, None, &external).is_none());
        assert!(unresolved("SoapySDR.dll", &dir, Some(&system), &[]).is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_installer_needing_the_runtime_is_refused() {
        let dir = scratch("installer");
        let exe = dir.join("sdrmm-desktop.exe");
        std::fs::write(
            &exe,
            image(0x20b, &["avcodec-63.dll", "VCRUNTIME140.dll"], &[]),
        )
        .unwrap();
        std::fs::write(dir.join("avcodec-63.dll"), b"").unwrap();
        let err = check(&dir, &[exe], &[]).unwrap_err().to_string();
        assert!(err.contains("needs VCRUNTIME140.dll"), "{err}");
        assert!(!err.contains("needs avcodec-63.dll"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
