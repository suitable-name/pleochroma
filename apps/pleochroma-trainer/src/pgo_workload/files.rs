//! Flat immutable snapshot copies and profiling artifact I/O.

use std::{
    fs,
    io::{BufReader, BufWriter, Write},
    path::Path,
};

use serde::{Serialize, de::DeserializeOwned};

use super::Result;

pub(super) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    regular_file(path)?;
    let reader = BufReader::new(fs::File::open(path)?);
    Ok(serde_json::from_reader(reader)?)
}

pub(super) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().ok_or("output needs a parent directory")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        serde_json::to_writer_pretty(&mut writer, value)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

pub(super) fn regular_file(path: &Path) -> Result<()> {
    // symlink_metadata preserves NotFound so capture can retry normal pruning races.
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(format!("Not a regular file: {}", path.display()).into());
    }
    Ok(())
}

pub(super) fn regular_directory(path: &Path) -> Result<()> {
    if !fs::symlink_metadata(path)?.file_type().is_dir() {
        return Err(format!("Not a regular directory: {}", path.display()).into());
    }
    Ok(())
}

pub(super) fn leaf(name: &str) -> bool {
    !matches!(name, "" | "." | "..")
        && !name.contains(['/', '\\', ':'])
        && Path::new(name).components().count() == 1
}

pub(super) fn copy_flat(source: &Path, destination: &Path) -> Result<()> {
    regular_directory(source)?;
    fs::create_dir(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        regular_file(&entry.path())?;
        fs::copy(entry.path(), destination.join(entry.file_name()))?;
    }
    Ok(())
}

pub(super) fn archive_json(directory: &Path) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with("run") && name.ends_with(".json") {
            regular_file(&entry.path())?;
            let destination = directory.join(format!("first-pass-{name}"));
            if destination.try_exists()? {
                return Err("first-pass profiling archive already exists".into());
            }
            fs::copy(entry.path(), destination)?;
        }
    }
    Ok(())
}

pub(super) fn text(path: &Path) -> Result<String> {
    Ok(path
        .to_str()
        .ok_or("trainer CLI requires UTF-8 paths")?
        .to_owned())
}
