//! Retain and restore a real incomplete recovery without modifying its payloads.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use super::{Result, files};

const REQUIRED: [&str; 7] = [
    "resume.json",
    "model.ckpt.json",
    "runner-state.mpk",
    "replay.mpk",
    "trainer.json",
    "trainer-model.bpk",
    "trainer-optimizer.bpk",
];

#[derive(Deserialize)]
struct Stamp {
    bytes: u64,
}

#[derive(Deserialize)]
pub(super) struct Manifest {
    schema: u32,
    pub(super) iteration: usize,
    pub(super) generation: String,
    files: BTreeMap<String, Stamp>,
}

impl Manifest {
    pub(super) fn validate(&self) -> Result<()> {
        if self.schema != 1
            || !files::leaf(&self.generation)
            || !self.generation.starts_with("generation-")
            || !REQUIRED.iter().all(|name| self.files.contains_key(*name))
            || !self.files.keys().all(|name| files::leaf(name))
        {
            return Err("Unsupported recovery manifest; update the profiling workload".into());
        }
        Ok(())
    }
}

#[derive(Deserialize)]
pub(super) struct State {
    pub(super) iteration: usize,
    pub(super) phase: String,
}

pub(super) struct Capture {
    directory: PathBuf,
    total: usize,
    saved: Option<(usize, tempfile::TempDir)>,
}

impl Capture {
    pub(super) fn new(directory: &Path, total: usize) -> Self {
        Self {
            directory: directory.to_owned(),
            total,
            saved: None,
        }
    }

    pub(super) fn poll(&mut self) -> Result<()> {
        match self.try_capture() {
            Err(error)
                if error
                    .downcast_ref::<io::Error>()
                    .is_some_and(|error| error.kind() == io::ErrorKind::NotFound) =>
            {
                Ok(())
            }
            result => result,
        }
    }

    fn try_capture(&mut self) -> Result<()> {
        let manifest_path = self.directory.join("run.latest.json");
        files::regular_file(&manifest_path)?;
        let bytes = fs::read(manifest_path)?;
        let manifest: Manifest = serde_json::from_slice(&bytes)?;
        manifest.validate()?;
        if manifest.iteration >= self.total
            || self
                .saved
                .as_ref()
                .is_some_and(|(iteration, _)| *iteration >= manifest.iteration)
        {
            return Ok(());
        }
        let root = self.directory.join("run.run");
        files::regular_directory(&root)?;
        let source = root.join(&manifest.generation);
        files::regular_directory(&source)?;
        let state: State = files::read_json(&source.join("resume.json"))?;
        if state.iteration != manifest.iteration || state.phase != "Training" {
            return Ok(());
        }
        let pending = tempfile::Builder::new()
            .prefix("capture-")
            .tempdir_in(&self.directory)?;
        let destination = pending.path().join(&manifest.generation);
        fs::create_dir(&destination)?;
        for (name, stamp) in &manifest.files {
            let source = source.join(name);
            files::regular_file(&source)?;
            let copied = fs::copy(source, destination.join(name))?;
            if copied != stamp.bytes {
                return Err("recovery capture size mismatch".into());
            }
        }
        // Exact manifest bytes retain the checksums, which the actual trainer verifies
        // during recovery. No phase, iteration, paths, RNG or model state is altered.
        fs::write(pending.path().join("manifest.json"), bytes)?;
        // Replace the previous owned temporary copy only after the new one is complete.
        // Dropping that old copy keeps capture storage bounded for long profiles.
        self.saved = Some((manifest.iteration, pending));
        Ok(())
    }

    pub(super) fn restore(self) -> Result<usize> {
        let (iteration, saved) = self
            .saved
            .ok_or("no incomplete recovery was captured; refusing a completed-run no-op resume")?;
        let manifest_path = saved.path().join("manifest.json");
        let manifest: Manifest = files::read_json(&manifest_path)?;
        manifest.validate()?;
        let root = self.directory.join("run.run");
        files::regular_directory(&root)?;
        let archive = self.directory.join("first-pass-recovery");
        if archive.try_exists()? {
            return Err("first-pass recovery archive already exists".into());
        }
        // The whole tree includes journals for later model weights. Restoring into
        // a fresh tree prevents those completed cases being reused after this rewind.
        fs::rename(&root, archive)?;
        fs::create_dir(&root)?;
        files::copy_flat(
            &saved.path().join(&manifest.generation),
            &root.join(&manifest.generation),
        )?;
        let mut pending = tempfile::NamedTempFile::new_in(&self.directory)?;
        io::copy(&mut fs::File::open(manifest_path)?, pending.as_file_mut())?;
        pending.as_file().sync_all()?;
        pending
            .persist(self.directory.join("run.latest.json"))
            .map_err(|error| error.error)?;
        // Retain just this replayed capture as evidence beside the first-pass archive.
        let _retained = saved.keep();
        Ok(iteration)
    }
}
