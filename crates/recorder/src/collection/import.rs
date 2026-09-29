use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use super::{
    CollectionFile, CollectionReader, collection_path, holds, lane_stem, stream_name,
    tmp_collection_path, write_collection,
};
use crate::{
    SigmfError, data_path, holds_recording,
    import::{ArchivedCollection, Imported, Member, sanitize},
    meta_path, tmp_meta_path,
};

pub(crate) fn import_collection(
    dir: &Path,
    archive: ArchivedCollection,
) -> Result<Imported, SigmfError> {
    let name = sanitize(&archive.name)?;
    let mut file: CollectionFile = serde_json::from_slice(&archive.collection)?;
    let lanes = in_stream_order(&file, archive.lanes)?;
    fs::create_dir_all(dir)?;
    let stem = claim_free(dir, &name, lanes.len())?;
    match land(&stem, &mut file, &lanes) {
        Ok(samples) => Ok(Imported { stem, samples }),
        Err(err) => {
            discard(&stem, lanes.len());
            Err(err)
        }
    }
}

fn in_stream_order(
    file: &CollectionFile,
    mut members: Vec<Member>,
) -> Result<Vec<Member>, SigmfError> {
    if file.collection.streams.is_empty() {
        return Err(SigmfError::Malformed(
            "a collection needs at least one stream".to_owned(),
        ));
    }
    let mut lanes = Vec::with_capacity(file.collection.streams.len());
    for stream in &file.collection.streams {
        let index = members
            .iter()
            .position(|member| member.name == stream.name)
            .ok_or_else(|| {
                SigmfError::Malformed(format!("the archive lacks lane `{}`", stream.name))
            })?;
        lanes.push(members.swap_remove(index));
    }
    match members.first() {
        None => Ok(lanes),
        Some(stray) => Err(SigmfError::Malformed(format!(
            "`{}` is not a lane of the collection",
            stray.name
        ))),
    }
}

fn claim_free(dir: &Path, name: &str, lanes: usize) -> Result<PathBuf, SigmfError> {
    let mut candidate = dir.join(name);
    let mut n = 2;
    loop {
        let taken = holds(&candidate)
            || holds_recording(&candidate)
            || (0..lanes).any(|lane| holds_recording(&lane_stem(&candidate, lane)));
        if !taken {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(tmp_collection_path(&candidate))
            {
                Ok(_) => return Ok(candidate),
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
                Err(err) => return Err(err.into()),
            }
        }
        candidate = dir.join(format!("{name}-{n}"));
        n += 1;
    }
}

fn land(stem: &Path, file: &mut CollectionFile, lanes: &[Member]) -> Result<u64, SigmfError> {
    for (index, (stream, member)) in file.collection.streams.iter_mut().zip(lanes).enumerate() {
        let lane = lane_stem(stem, index);
        fs::write(data_path(&lane), &member.data)?;
        fs::write(tmp_meta_path(&lane), &member.meta)?;
        stream.name = stream_name(&lane)?;
    }
    for index in 0..lanes.len() {
        let lane = lane_stem(stem, index);
        fs::rename(tmp_meta_path(&lane), meta_path(&lane))?;
    }
    let tmp = tmp_collection_path(stem);
    write_collection(fs::File::create(&tmp)?, file)?;
    fs::rename(&tmp, collection_path(stem))?;
    Ok(CollectionReader::open(stem)?.total_samples())
}

fn discard(stem: &Path, lanes: usize) {
    for index in 0..lanes {
        let lane = lane_stem(stem, index);
        for path in [data_path(&lane), tmp_meta_path(&lane), meta_path(&lane)] {
            let _ = fs::remove_file(path);
        }
    }
    let _ = fs::remove_file(collection_path(stem));
    let _ = fs::remove_file(tmp_collection_path(stem));
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::{
        Export, ExportKind, Stored, annotate,
        import::{Archive, import_archive, read_archive},
        library::tests::collection,
        scan_library,
    };

    fn archived(stem: &Path) -> Vec<u8> {
        let mut export = Export::open(stem, ExportKind::SigmfArchive).expect("export");
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut export, &mut bytes).expect("read");
        bytes
    }

    fn unpacked(stem: &Path) -> ArchivedCollection {
        match read_archive(archived(stem).as_slice()).expect("reads") {
            Archive::Collection(collection) => collection,
            Archive::Pair(_) => panic!("a collection"),
        }
    }

    fn files(dir: &Path) -> usize {
        fs::read_dir(dir).expect("dir").count()
    }

    #[test]
    fn a_downloaded_collection_uploads_whole_under_a_free_name() {
        let source = TempDir::new().expect("temp");
        let stem = collection(source.path(), "bank", 3, 40);
        annotate(&stem, Some("Roof"), &["df".to_owned()], None).expect("annotate");
        let library = TempDir::new().expect("temp");
        collection(library.path(), "bank", 2, 8);

        let imported = import_archive(library.path(), archived(&stem).as_slice()).expect("imports");

        assert_eq!(imported.stem, library.path().join("bank-2"));
        assert_eq!(imported.samples, 40);
        assert_eq!(
            scan_library(library.path()).expect("scan"),
            [
                Stored::Collection(library.path().join("bank")),
                Stored::Collection(imported.stem.clone()),
            ]
        );
        let reader = CollectionReader::open(&imported.stem).expect("plays");
        assert_eq!(reader.lanes(), 3);
        assert_eq!(reader.notes().name.as_deref(), Some("Roof"));
        assert_eq!(
            fs::read(meta_path(&lane_stem(&imported.stem, 2))).expect("lane meta"),
            fs::read(meta_path(&lane_stem(&stem, 2))).expect("source meta")
        );
    }

    #[test]
    fn a_collection_missing_a_lane_leaves_nothing_behind() {
        let source = TempDir::new().expect("temp");
        let stem = collection(source.path(), "bank", 3, 16);
        let library = TempDir::new().expect("temp");
        let mut short = unpacked(&stem);
        short.lanes.remove(1);

        let err = import_collection(library.path(), short).expect_err("a lane is missing");

        assert!(err.to_string().contains("bank-lane1"), "{err}");
        assert_eq!(files(library.path()), 0);
    }

    #[test]
    fn a_lane_that_fails_its_hash_leaves_nothing_behind() {
        let source = TempDir::new().expect("temp");
        let stem = collection(source.path(), "bank", 2, 16);
        let library = TempDir::new().expect("temp");
        let mut tampered = unpacked(&stem);
        tampered.lanes[1].meta.push(b' ');

        let err = import_collection(library.path(), tampered).expect_err("the hash differs");

        assert!(matches!(err, SigmfError::Malformed(_)), "{err}");
        assert_eq!(files(library.path()), 0);
    }

    #[test]
    fn a_lane_the_collection_does_not_name_is_refused() {
        let source = TempDir::new().expect("temp");
        let stem = collection(source.path(), "bank", 2, 16);
        let library = TempDir::new().expect("temp");
        let mut padded = unpacked(&stem);
        let mut stray = unpacked(&stem).lanes.remove(0);
        stray.name = "other".to_owned();
        padded.lanes.push(stray);

        assert!(matches!(
            import_collection(library.path(), padded),
            Err(SigmfError::Malformed(_))
        ));
        assert_eq!(files(library.path()), 0);
    }
}
