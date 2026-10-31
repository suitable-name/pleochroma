//! Transactional storage for the CLI recovery bundle.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter, Read, Write},
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{RESUME_SCHEMA, ResumeResult};

/// Every generation must contain a matching runner, replay, model and optimizer snapshot.
const REQUIRED_PAYLOADS: [&str; 7] = [
    "resume.json",
    "model.ckpt.json",
    "runner-state.mpk",
    "replay.mpk",
    "trainer.json",
    "trainer-model.bpk",
    "trainer-optimizer.bpk",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct FileStamp {
    bytes: u64,
    /// Accidental-corruption detection, not a cryptographic authenticity claim.
    fnv1a: u64,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    schema: u32,
    iteration: usize,
    generation: String,
    previous: Option<String>,
    files: BTreeMap<String, FileStamp>,
}

pub(super) fn read_json<T: DeserializeOwned>(path: &Path) -> ResumeResult<T> {
    Ok(serde_json::from_reader(BufReader::new(File::open(path)?))?)
}

pub(super) fn write_json(path: &Path, value: &impl Serialize) -> ResumeResult<()> {
    let mut writer = BufWriter::new(File::create(path)?);
    serde_json::to_writer(&mut writer, value)?;
    writer.flush()?;
    writer.get_ref().sync_all()?;
    Ok(())
}

pub(super) fn publish(
    pending: tempfile::TempDir,
    root: &Path,
    manifest_path: &Path,
    iteration: usize,
) -> ResumeResult<()> {
    let previous: Option<Manifest> = if manifest_path.is_file() {
        Some(read_json(manifest_path)?)
    } else {
        None
    };
    let mut files = BTreeMap::new();
    for item in fs::read_dir(pending.path())? {
        let item = item?;
        if !item.file_type()?.is_file() {
            return Err("recovery bundle may contain only regular payload files".into());
        }
        let name = item
            .file_name()
            .into_string()
            .map_err(|_| "non-UTF-8 recovery payload name")?;
        // Windows FlushFileBuffers requires a writable handle. Opening read-only can make
        // every otherwise valid checkpoint publication fail with access denied.
        OpenOptions::new()
            .write(true)
            .open(item.path())?
            .sync_all()?;
        files.insert(name, stamp(&item.path())?);
    }
    for required in REQUIRED_PAYLOADS {
        if !files.contains_key(required) {
            return Err(format!("recovery bundle is missing {required}").into());
        }
    }
    #[cfg(unix)]
    sync_directory(pending.path())?;
    let generation = pending
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid recovery generation name")?
        .to_owned();
    validate_generation(&generation)?;
    let manifest = Manifest {
        schema: RESUME_SCHEMA,
        iteration,
        generation,
        previous: previous.as_ref().map(|old| old.generation.clone()),
        files,
    };
    // Keeping the directory before publication can leave an orphan after a crash, but never
    // a manifest pointing at a directory removed by TempDir's destructor.
    let _committed = pending.keep();
    #[cfg(unix)]
    sync_directory(root)?;
    atomic_json(manifest_path, &manifest)?;
    if let Some(old) = previous.and_then(|old| old.previous)
        && let Err(error) = remove_old_generation(root, &old, &manifest)
    {
        eprintln!("warning: latest recovery is committed; old snapshot cleanup failed: {error}");
    }
    Ok(())
}

pub(super) fn read_committed(root: &Path, manifest_path: &Path) -> ResumeResult<PathBuf> {
    let manifest: Manifest = read_json(manifest_path)?;
    if manifest.schema != RESUME_SCHEMA {
        return Err(format!("unsupported latest-checkpoint schema {}", manifest.schema).into());
    }
    validate_generation(&manifest.generation)?;
    let directory = root.join(&manifest.generation);
    if !fs::symlink_metadata(&directory)?.is_dir() {
        return Err("recovery generation is not a regular directory".into());
    }
    for required in REQUIRED_PAYLOADS {
        if !manifest.files.contains_key(required) {
            return Err(format!("latest recovery does not include {required}").into());
        }
    }
    for (name, expected) in &manifest.files {
        validate_leaf(name)?;
        let path = directory.join(name);
        if !fs::symlink_metadata(&path)?.is_file() || stamp(&path)? != *expected {
            return Err(format!(
                "recovery payload {} is corrupt or incomplete",
                path.display()
            )
            .into());
        }
    }
    // Configuration and payload must belong to the same committed iteration.
    #[derive(Deserialize)]
    struct Counter {
        iteration: usize,
    }
    let counter: Counter = read_json(&directory.join("resume.json"))?;
    if counter.iteration != manifest.iteration {
        return Err("latest recovery manifest and resume state disagree on iteration".into());
    }
    Ok(directory)
}

fn atomic_json(path: &Path, value: &impl Serialize) -> ResumeResult<()> {
    let parent = path
        .parent()
        .ok_or("latest checkpoint needs a parent directory")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut writer = BufWriter::new(file.as_file_mut());
        serde_json::to_writer(&mut writer, value)?;
        writer.flush()?;
    }
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    #[cfg(unix)]
    sync_directory(parent)?;
    Ok(())
}

fn stamp(path: &Path) -> std::io::Result<FileStamp> {
    let mut file = BufReader::new(File::open(path)?);
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut bytes = 0u64;
    let mut buffer = [0u8; 8192];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        for byte in &buffer[..count] {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    Ok(FileStamp { bytes, fnv1a: hash })
}

fn validate_leaf(name: &str) -> ResumeResult<()> {
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
        || name.contains(['/', '\\'])
    {
        return Err("recovery manifest contains an invalid file name".into());
    }
    Ok(())
}

fn validate_generation(name: &str) -> ResumeResult<()> {
    validate_leaf(name)?;
    if !name.starts_with("generation-") {
        return Err("invalid recovery generation name".into());
    }
    Ok(())
}

fn remove_old_generation(root: &Path, name: &str, live: &Manifest) -> ResumeResult<()> {
    validate_generation(name)?;
    if name == live.generation || live.previous.as_deref() == Some(name) {
        return Err("refusing to remove a retained recovery generation".into());
    }
    let path = root.join(name);
    if !path.exists() {
        return Ok(());
    }
    if !fs::symlink_metadata(&path)?.is_dir()
        || fs::canonicalize(&path)?.parent() != Some(fs::canonicalize(root)?.as_path())
    {
        return Err("old recovery generation is not a direct child of the run directory".into());
    }
    // Only remove flat generated payloads; never recursively follow an unexpected subtree.
    let entries = fs::read_dir(&path)?.collect::<Result<Vec<_>, _>>()?;
    if entries
        .iter()
        .any(|entry| entry.file_type().map_or(true, |kind| !kind.is_file()))
    {
        return Err("old recovery generation contains unexpected non-file entries".into());
    }
    for entry in entries {
        fs::remove_file(entry.path())?;
    }
    fs::remove_dir(path)?;
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}
