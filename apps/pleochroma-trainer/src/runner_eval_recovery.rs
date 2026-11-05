//! Durable validation cases and finer-grained continuation records.
//!
//! The caller owns the directory exclusively. A case is published only after its payload is
//! synced; temporary files left by interruption are never read as completed work. The original
//! version-3 whole-case records remain readable. Separate versioned fragments preserve passes,
//! episode steps and teacher searches without rewriting training replay or model weights.

use std::{
    fs,
    io::{Cursor, ErrorKind, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{DesignResult, EvalJob, ObjectiveWeights, RunnerError};

const VERSION: u32 = 3;

/// Changing policy execution changes episode decisions even with identical checkpoint weights.
const POLICY_MATH: &str = "selected-backend-batched-forward-v1";

const PART_VERSION: u32 = 1;

#[path = "runner_eval_reference_journal.rs"]
mod references;

pub(super) struct Journal {
    directory: PathBuf,
    identity_stamp: u64,
    reference_directory: Option<PathBuf>,
}

/// One exact evaluation case, with small independently committed continuation payloads.
#[derive(Clone)]
pub(super) struct CaseJournal {
    directory: PathBuf,
    identity_stamp: u64,
    index: usize,
    entry: usize,
    weights: String,
}

#[derive(Serialize)]
struct Identity<'a> {
    version: u32,
    trainer_version: &'static str,
    observation_layout: u32,
    policy_backend: &'static str,
    policy_math: &'static str,
    environment_math: &'static str,
    network: &'a pleochroma_ai::model::GemstoneSetAttentionNetwork,
    config: String,
    corpus: String,
    entries: &'a [usize],
    tasks: Vec<(usize, String)>,
    fidelity: super::EvalFidelity,
    seed: u64,
    sampled: bool,
    oracle: bool,
    noise: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseRecord {
    version: u32,
    identity_stamp: u64,
    index: usize,
    entry: usize,
    weights: String,
    payload: Vec<u8>,
    payload_stamp: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartRecord {
    version: u32,
    identity_stamp: u64,
    index: usize,
    entry: usize,
    weights: String,
    part: String,
    payload: Vec<u8>,
    payload_stamp: u64,
}

impl Journal {
    pub(super) fn open(
        directory: &Path,
        job: EvalJob<'_>,
        entries: &[usize],
        tasks: &[(usize, Option<ObjectiveWeights>)],
    ) -> Result<Self, RunnerError> {
        let mut config = job.cfg.clone();
        // Requested Auto/explicit selection is irrelevant; bind the resolved backend below.
        config.device = super::super::DeviceChoice::Auto;
        let identity = Identity {
            version: VERSION,
            trainer_version: env!("CARGO_PKG_VERSION"),
            observation_layout: pleochroma_ai::env::OBS_LAYOUT_VERSION,
            policy_backend: job.backend,
            policy_math: POLICY_MATH,
            environment_math: if job.execution.is_some() {
                pleochroma_ai::env::CUDA_ENVIRONMENT_MATH
            } else {
                "cpu-env-v1"
            },
            network: job.net,
            config: format!("{config:?}"),
            corpus: job.split.recovery_identity(),
            entries,
            tasks: tasks
                .iter()
                .map(|(entry, weights)| (*entry, format!("{weights:?}")))
                .collect(),
            fidelity: job.fidelity,
            seed: job.seed,
            sampled: job.passes.sampled,
            oracle: job.passes.oracle,
            noise: job.passes.noise,
        };
        let mut journal = Self::open_identity(directory, &encode(&identity)?)?;
        // Compare the complete corpus once per validation job, rather than copying it into
        // every step/probe fragment or serializing thousands of designs once per case.
        journal.reference_directory = Some(references::open_corpus(directory, &identity.corpus)?);
        Ok(journal)
    }

    fn open_identity(directory: &Path, identity: &[u8]) -> Result<Self, RunnerError> {
        ensure_directory(directory)?;
        let path = directory.join("identity.mpk");
        match regular_bytes(&path)? {
            Some(saved) if saved == identity => {}
            Some(_) => {
                return Err(failure(
                    "open",
                    "job identity/backend/math changed, old journal version, or corrupt identity; refusing completed-case reuse (use a fresh validation journal directory)",
                ));
            }
            None => {
                for entry in fs::read_dir(directory).map_err(|e| failure("read directory", e))? {
                    let entry = entry.map_err(|e| failure("inspect directory entry", e))?;
                    if !entry.file_name().to_string_lossy().starts_with("pending-") {
                        return Err(failure(
                            "open",
                            "journal identity is missing beside existing files",
                        ));
                    }
                }
                atomic_bytes(&path, identity)?;
                #[cfg(unix)]
                if let Some(parent) = directory.parent() {
                    sync_directory(parent)?;
                }
            }
        }
        Ok(Self {
            directory: directory.to_owned(),
            identity_stamp: stamp(identity),
            reference_directory: None,
        })
    }

    pub(super) fn case(
        &self,
        index: usize,
        entry: usize,
        weights: Option<ObjectiveWeights>,
    ) -> CaseJournal {
        CaseJournal {
            directory: self.directory.clone(),
            identity_stamp: self.identity_stamp,
            index,
            entry,
            weights: format!("{weights:?}"),
        }
    }

    /// An exact model-independent case shared by validation iterations of this run.
    pub(super) fn reference_case(
        &self,
        job: EvalJob<'_>,
        entry: usize,
        weights: Option<ObjectiveWeights>,
    ) -> Result<CaseJournal, RunnerError> {
        let root = match &self.reference_directory {
            Some(directory) => directory.clone(),
            None => references::open_corpus(&self.directory, &job.split.recovery_identity())?,
        };
        let identity = references::identity(job, entry, weights)?;
        let directory = root.join(format!("case-{:016x}", stamp(&identity)));
        // The hash is only a path discriminator. Exact identity bytes guard collisions.
        let salted = job.seed ^ (entry as u64).wrapping_mul(0x9E37_79B9);
        let resolved = weights.unwrap_or_else(|| super::draw_weights(job.cfg.objectives, salted));
        Ok(Self::open_identity(&directory, &identity)?.case(0, entry, Some(resolved)))
    }

    pub(super) fn load(
        &self,
        index: usize,
        entry: usize,
        weights: Option<ObjectiveWeights>,
    ) -> Result<Option<DesignResult>, RunnerError> {
        let Some(bytes) = regular_bytes(&self.path(index))? else {
            return Ok(None);
        };
        let record: CaseRecord = decode(&bytes)?;
        if record.version != VERSION
            || record.identity_stamp != self.identity_stamp
            || record.index != index
            || record.entry != entry
            || record.weights != format!("{weights:?}")
            || record.payload_stamp != stamp(&record.payload)
        {
            return Err(failure(
                "load case",
                format!("case {index} is incompatible or corrupt"),
            ));
        }
        decode(&record.payload).map(Some)
    }

    pub(super) fn save(
        &self,
        index: usize,
        entry: usize,
        weights: Option<ObjectiveWeights>,
        result: &DesignResult,
    ) -> Result<(), RunnerError> {
        let payload = encode(result)?;
        let record = CaseRecord {
            version: VERSION,
            identity_stamp: self.identity_stamp,
            index,
            entry,
            weights: format!("{weights:?}"),
            payload_stamp: stamp(&payload),
            payload,
        };
        atomic_bytes(&self.path(index), &encode(&record)?)
    }

    fn path(&self, index: usize) -> PathBuf {
        self.directory.join(format!("case-{index:06}.mpk"))
    }
}

impl CaseJournal {
    pub(super) fn load<T: DeserializeOwned>(&self, part: &str) -> Result<Option<T>, RunnerError> {
        let path = self.path(part)?;
        let Some(bytes) = regular_bytes(&path)? else {
            return Ok(None);
        };
        let record: PartRecord = decode(&bytes)?;
        if record.version != PART_VERSION
            || record.identity_stamp != self.identity_stamp
            || record.index != self.index
            || record.entry != self.entry
            || record.weights != self.weights
            || record.part != part
            || record.payload_stamp != stamp(&record.payload)
        {
            return Err(failure(
                "load continuation",
                format!("case {} part {part} is incompatible or corrupt", self.index),
            ));
        }
        decode(&record.payload).map(Some)
    }

    pub(super) fn save<T: Serialize>(&self, part: &str, value: &T) -> Result<(), RunnerError> {
        let path = self.path(part)?;
        let payload = encode(value)?;
        let record = PartRecord {
            version: PART_VERSION,
            identity_stamp: self.identity_stamp,
            index: self.index,
            entry: self.entry,
            weights: self.weights.clone(),
            part: part.to_owned(),
            payload_stamp: stamp(&payload),
            payload,
        };
        atomic_bytes(&path, &encode(&record)?)
    }

    fn path(&self, part: &str) -> Result<PathBuf, RunnerError> {
        if part.is_empty()
            || part.len() > 96
            || !part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(failure(
                "continuation path",
                "invalid continuation part name",
            ));
        }
        Ok(self
            .directory
            .join(format!("case-{:06}.part-{part}.mpk", self.index)))
    }
}

/// Create directories without traversing an existing symlink/reparse point.
fn ensure_directory(path: &Path) -> Result<(), RunnerError> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        ensure_directory(parent)?;
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(failure("open", "journal path is not a regular directory")),
        Err(error) if error.kind() == ErrorKind::NotFound => match fs::create_dir(path) {
            Ok(()) => {
                #[cfg(unix)]
                if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    sync_directory(parent)?;
                }
                Ok(())
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(path)
                    .map_err(|e| failure("inspect created directory", e))?;
                if metadata.is_dir() && !metadata.file_type().is_symlink() {
                    Ok(())
                } else {
                    Err(failure("open", "journal path is not a regular directory"))
                }
            }
            Err(error) => Err(failure("create directory", error)),
        },
        Err(error) => Err(failure("inspect directory", error)),
    }
}

fn encode(value: &impl Serialize) -> Result<Vec<u8>, RunnerError> {
    let mut bytes = Vec::new();
    value
        .serialize(&mut rmp_serde::Serializer::new(&mut bytes).with_struct_map())
        .map_err(|e| failure("encode", e))?;
    Ok(bytes)
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, RunnerError> {
    let mut cursor = Cursor::new(bytes);
    let value = T::deserialize(&mut rmp_serde::Deserializer::new(&mut cursor))
        .map_err(|e| failure("decode", e))?;
    if cursor.position() != bytes.len() as u64 {
        return Err(failure("decode", "unexpected trailing bytes"));
    }
    Ok(value)
}

fn regular_bytes(path: &Path) -> Result<Option<Vec<u8>>, RunnerError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => fs::read(path)
            .map(Some)
            .map_err(|e| failure("read payload", e)),
        Ok(_) => Err(failure(
            "read payload",
            "journal payload is not a regular file",
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(failure("inspect payload", error)),
    }
}

fn atomic_bytes(path: &Path, bytes: &[u8]) -> Result<(), RunnerError> {
    let parent = path
        .parent()
        .ok_or_else(|| failure("save", "missing parent directory"))?;
    ensure_directory(parent)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err(failure("save", "journal payload is not a regular file")),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(failure("inspect destination", error)),
    }
    let mut temporary = tempfile::Builder::new()
        .prefix("pending-")
        .tempfile_in(parent)
        .map_err(|e| failure("create temporary payload", e))?;
    temporary
        .write_all(bytes)
        .map_err(|e| failure("write payload", e))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|e| failure("sync payload", e))?;
    temporary
        .persist(path)
        .map_err(|e| failure("publish payload", e.error))?;
    #[cfg(unix)]
    sync_directory(parent)?;
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), RunnerError> {
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|e| failure("sync directory", e))
}

/// Detects accidental corruption; full job identity is compared byte-for-byte separately.
fn stamp(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn failure(operation: &str, error: impl std::fmt::Display) -> RunnerError {
    RunnerError::Data(format!("validation recovery {operation}: {error}"))
}

#[cfg(test)]
#[path = "runner_eval_recovery_tests.rs"]
mod tests;
