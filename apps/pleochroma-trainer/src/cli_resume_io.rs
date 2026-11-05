//! Transactional storage for the CLI recovery bundle.
//!
//! A generation directory is written, every payload is synced and stamped, and only then is the
//! small manifest replaced. The first manifests stamped every payload byte-wise with FNV-1a;
//! new ones stamp word-wise, and each stamp names its hash by its field ([`FileStamp`]). They
//! also carry the stamps of the previous generation, so a damaged latest generation falls back
//! to it (loudly, never silently). Both kinds verify; the additions are optional fields of
//! schema 1, which the PGO workload (`pgo_workload/capture.rs`) also reads.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, BufWriter, Read, Write},
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::ResumeResult;

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

/// Manifest schema; later additions are optional fields.
const SCHEMA: u32 = 1;
/// Stamp hash of new manifests.
const HASH: StampHash = StampHash::MulRotate64;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
/// Odd multiplier of the word-wise stamp (2^64 divided by the golden ratio).
const WORD_MULTIPLIER: u64 = 0x9e37_79b9_7f4a_7c15;
/// Read size while stamping a payload.
const STAMP_BUFFER: usize = 1 << 20;

/// Accidental-corruption detection, not a cryptographic authenticity claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StampHash {
    /// FNV-1a 64 over every byte: every manifest written before word-wise stamps existed.
    Fnv1a,
    /// One multiplication per eight bytes: `state = ((state ^ word) * WORD_MULTIPLIER)
    /// .rotate_left(29)` over the little-endian 64-bit words, the zero-padded tail word and
    /// finally the byte count. Every step is a bijection of the state, so a difference
    /// confined to one word always changes the stamp.
    MulRotate64,
}

/// Size and digest of one payload. The digest's field names its [`StampHash`]: a reader that
/// only knows `fnv1a` refuses a word-wise stamp instead of misreading it as corruption.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct FileStamp {
    bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fnv1a: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mul_rotate64: Option<u64>,
}

impl FileStamp {
    /// The hash that verifies this stamp; `None` for a malformed stamp.
    const fn hash(&self) -> Option<StampHash> {
        match (self.fnv1a, self.mul_rotate64) {
            (Some(_), None) => Some(StampHash::Fnv1a),
            (None, Some(_)) => Some(StampHash::MulRotate64),
            _ => None,
        }
    }
}

/// One committed generation with what verifies it.
#[derive(Clone, Serialize, Deserialize)]
struct Bundle {
    generation: String,
    iteration: usize,
    files: BTreeMap<String, FileStamp>,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    schema: u32,
    iteration: usize,
    generation: String,
    previous: Option<String>,
    files: BTreeMap<String, FileStamp>,
    /// Stamps of `previous`, so a damaged latest generation can fall back to it; absent in
    /// manifests written before the fallback existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_bundle: Option<Bundle>,
}

impl Manifest {
    fn bundle(&self) -> Bundle {
        Bundle {
            generation: self.generation.clone(),
            iteration: self.iteration,
            files: self.files.clone(),
        }
    }
}

/// A payload stamped while it was written and synced, so publication need not read it back.
pub(super) struct StampedPayload {
    name: String,
    stamp: FileStamp,
}

/// Streaming [`StampHash`] digest.
struct Stamper {
    hash: StampHash,
    state: u64,
    bytes: u64,
    tail: [u8; 8],
    tail_len: usize,
}

impl Stamper {
    const fn new(hash: StampHash) -> Self {
        Self {
            hash,
            state: FNV_OFFSET,
            bytes: 0,
            tail: [0; 8],
            tail_len: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.bytes += data.len() as u64;
        if self.hash == StampHash::Fnv1a {
            for byte in data {
                self.state = (self.state ^ u64::from(*byte)).wrapping_mul(FNV_PRIME);
            }
            return;
        }
        if self.tail_len > 0 {
            let take = (8 - self.tail_len).min(data.len());
            self.tail[self.tail_len..self.tail_len + take].copy_from_slice(&data[..take]);
            self.tail_len += take;
            data = &data[take..];
            if self.tail_len < 8 {
                return;
            }
            self.word(u64::from_le_bytes(self.tail));
            self.tail_len = 0;
        }
        let mut words = data.chunks_exact(8);
        for chunk in &mut words {
            let mut word = [0; 8];
            word.copy_from_slice(chunk);
            self.word(u64::from_le_bytes(word));
        }
        let rest = words.remainder();
        self.tail[..rest.len()].copy_from_slice(rest);
        self.tail_len = rest.len();
    }

    const fn word(&mut self, word: u64) {
        self.state = (self.state ^ word)
            .wrapping_mul(WORD_MULTIPLIER)
            .rotate_left(29);
    }

    fn finish(mut self) -> FileStamp {
        match self.hash {
            StampHash::Fnv1a => FileStamp {
                bytes: self.bytes,
                fnv1a: Some(self.state),
                mul_rotate64: None,
            },
            StampHash::MulRotate64 => {
                if self.tail_len > 0 {
                    self.tail[self.tail_len..].fill(0);
                    self.word(u64::from_le_bytes(self.tail));
                }
                self.word(self.bytes);
                FileStamp {
                    bytes: self.bytes,
                    fnv1a: None,
                    mul_rotate64: Some(self.state),
                }
            }
        }
    }
}

/// Stamps exactly the bytes the file accepted.
struct StampingWriter {
    file: File,
    stamper: Stamper,
}

impl Write for StampingWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let written = self.file.write(data)?;
        self.stamper.update(&data[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

pub(super) fn read_json<T: DeserializeOwned>(path: &Path) -> ResumeResult<T> {
    Ok(serde_json::from_reader(BufReader::new(File::open(path)?))?)
}

/// Writes and syncs a JSON payload, stamping the bytes on their way to the file.
pub(super) fn write_json_stamped(
    path: &Path,
    value: &impl Serialize,
) -> ResumeResult<StampedPayload> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid recovery payload name")?
        .to_owned();
    let mut writer = BufWriter::new(StampingWriter {
        file: File::create(path)?,
        stamper: Stamper::new(HASH),
    });
    serde_json::to_writer(&mut writer, value)?;
    let stamping = writer
        .into_inner()
        .map_err(io::IntoInnerError::into_error)?;
    stamping.file.sync_all()?;
    Ok(StampedPayload {
        name,
        stamp: stamping.stamper.finish(),
    })
}

/// Publishes `pending` as the latest generation: every payload is synced and stamped, then the
/// manifest is replaced. `stamped` was stamped and synced by its writer.
///
/// A payload byte-identical to the one of the previous generation at the same iteration (a
/// second commit without a training step in between, such as the commit after a validation)
/// becomes a hard link to that already durable file instead of a second synced copy; when
/// linking fails, the written copy stays. Afterwards pending generations a crash left behind
/// are removed.
pub(super) fn publish(
    pending: tempfile::TempDir,
    root: &Path,
    manifest_path: &Path,
    iteration: usize,
    stamped: Option<&StampedPayload>,
) -> ResumeResult<()> {
    let previous = if manifest_path.is_file() {
        Some(read_manifest(manifest_path)?)
    } else {
        None
    };
    // Stamps of different hashes never compare equal, so an older manifest links nothing.
    let twin = previous
        .as_ref()
        .filter(|old| old.iteration == iteration)
        .map(|old| (root.join(&old.generation), &old.files));
    // Collected first: linking adds and renames entries of this directory.
    let items = fs::read_dir(pending.path())?.collect::<Result<Vec<_>, _>>()?;
    let mut files = BTreeMap::new();
    for item in items {
        if !item.file_type()?.is_file() {
            return Err("recovery bundle may contain only regular payload files".into());
        }
        let name = item
            .file_name()
            .into_string()
            .map_err(|_| "non-UTF-8 recovery payload name")?;
        let path = item.path();
        let stamp = match stamped {
            Some(known) if known.name == name => known.stamp,
            _ => {
                let stamp = stamp_file(&path, HASH)?;
                let linked = twin.as_ref().is_some_and(|(directory, old)| {
                    old.get(&name) == Some(&stamp)
                        && link_identical(&directory.join(&name), &path, stamp.bytes)
                });
                if !linked {
                    sync_file(&path)?;
                }
                stamp
            }
        };
        files.insert(name, stamp);
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
        schema: SCHEMA,
        iteration,
        generation,
        previous: previous.as_ref().map(|old| old.generation.clone()),
        files,
        previous_bundle: previous.as_ref().map(Manifest::bundle),
    };
    // Keeping the directory before publication can leave an orphan after a crash (removed by
    // the next sweep), but never a manifest pointing at a directory removed by TempDir's
    // destructor.
    let _committed = pending.keep();
    #[cfg(unix)]
    sync_directory(root)?;
    atomic_json(manifest_path, &manifest)?;
    if let Some(old) = previous.and_then(|old| old.previous)
        && let Err(error) = remove_old_generation(root, &old, &manifest)
    {
        eprintln!("warning: latest recovery is committed; old snapshot cleanup failed: {error}");
    }
    for warning in sweep(root, &manifest) {
        eprintln!("warning: latest recovery is committed; {warning}");
    }
    Ok(())
}

/// The directory of the latest committed generation, verified against its stamps.
///
/// When the latest generation fails verification and the manifest carries the stamps of the
/// previous one, that one is verified instead, with a loud warning. It then becomes the latest
/// generation, and the damaged one is renamed `damaged-<name>` (kept for inspection, never
/// swept), so the next commit cannot delete the generation this run continues from.
pub(super) fn read_committed(root: &Path, manifest_path: &Path) -> ResumeResult<PathBuf> {
    let manifest = read_manifest(manifest_path)?;
    let failure = match verify(root, &manifest.bundle()) {
        Ok(directory) => return Ok(directory),
        Err(error) => error,
    };
    let Some(previous) = manifest
        .previous_bundle
        .as_ref()
        .filter(|bundle| manifest.previous.as_ref() == Some(&bundle.generation))
    else {
        return Err(format!(
            "latest recovery generation {} failed verification ({failure}); no verifiable previous generation is recorded",
            manifest.generation
        )
        .into());
    };
    eprintln!(
        "\nWARNING: latest recovery generation {} (iteration {}) failed verification: {failure}\n\
         WARNING: resuming from the previous generation {} (iteration {}); the work after it is repeated.\n\
         WARNING: the damaged generation is kept as damaged-{} in {}.\n",
        manifest.generation,
        manifest.iteration,
        previous.generation,
        previous.iteration,
        manifest.generation,
        root.display()
    );
    let directory = verify(root, previous).map_err(|error| {
        format!(
            "latest recovery generation {} failed verification ({failure}); the previous generation {} failed too ({error})",
            manifest.generation, previous.generation
        )
    })?;
    promote(root, manifest_path, &manifest.generation, previous)?;
    Ok(directory)
}

/// Removes pending generations of this run that no manifest references, for example the
/// half-written directory of a killed save. Returns warnings; failures never stop the run.
pub(super) fn sweep_stale_generations(root: &Path, manifest_path: &Path) -> Vec<String> {
    if !manifest_path.is_file() {
        return remove_matching(root, is_pending_generation, "stale recovery generation");
    }
    read_manifest(manifest_path).map_or_else(
        |error| vec![format!("stale recovery generations were kept: {error}")],
        |manifest| sweep(root, &manifest),
    )
}

/// Removes the validation journals of iterations before `finished`, the latest validation whose
/// result is committed. Its own journal (for any backend route) and later ones stay, so a
/// fallback to the previous generation can still complete a pending validation. Returns
/// warnings; failures never stop the run.
pub(super) fn prune_validation_journals(root: &Path, finished: usize) -> Vec<String> {
    remove_matching(
        root,
        |name| validation_iteration(name).is_some_and(|iteration| iteration < finished),
        "finished validation journal",
    )
}

fn read_manifest(path: &Path) -> ResumeResult<Manifest> {
    let manifest: Manifest = read_json(path)?;
    if manifest.schema != SCHEMA {
        return Err(format!("unsupported latest-checkpoint schema {}", manifest.schema).into());
    }
    Ok(manifest)
}

fn verify(root: &Path, bundle: &Bundle) -> ResumeResult<PathBuf> {
    validate_generation(&bundle.generation)?;
    let directory = root.join(&bundle.generation);
    if !fs::symlink_metadata(&directory)?.is_dir() {
        return Err("recovery generation is not a regular directory".into());
    }
    for required in REQUIRED_PAYLOADS {
        if !bundle.files.contains_key(required) {
            return Err(format!("recovery generation does not include {required}").into());
        }
    }
    for (name, expected) in &bundle.files {
        validate_leaf(name)?;
        let hash = expected
            .hash()
            .ok_or("recovery manifest contains a malformed payload stamp")?;
        let path = directory.join(name);
        if !fs::symlink_metadata(&path)?.is_file() || stamp_file(&path, hash)? != *expected {
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
    if counter.iteration != bundle.iteration {
        return Err("recovery manifest and resume state disagree on iteration".into());
    }
    Ok(directory)
}

/// Makes the verified `previous` bundle the latest generation after `damaged` failed.
fn promote(
    root: &Path,
    manifest_path: &Path,
    damaged: &str,
    previous: &Bundle,
) -> ResumeResult<()> {
    // Renamed before the manifest changes: an interruption in between leaves a manifest whose
    // latest generation is missing, which falls back again.
    let path = root.join(damaged);
    if fs::symlink_metadata(&path).is_ok()
        && let Err(error) = fs::rename(&path, root.join(format!("damaged-{damaged}")))
    {
        eprintln!(
            "warning: could not rename the damaged recovery generation {}: {error}",
            path.display()
        );
    }
    let manifest = Manifest {
        schema: SCHEMA,
        iteration: previous.iteration,
        generation: previous.generation.clone(),
        previous: None,
        files: previous.files.clone(),
        previous_bundle: None,
    };
    atomic_json(manifest_path, &manifest)
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

fn stamp_file(path: &Path, hash: StampHash) -> io::Result<FileStamp> {
    let mut file = File::open(path)?;
    let mut stamper = Stamper::new(hash);
    let mut buffer = vec![0u8; STAMP_BUFFER];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => return Ok(stamper.finish()),
            Ok(count) => stamper.update(&buffer[..count]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

fn sync_file(path: &Path) -> io::Result<()> {
    // Windows FlushFileBuffers requires a writable handle. Opening read-only can make every
    // otherwise valid checkpoint publication fail with access denied.
    OpenOptions::new().write(true).open(path)?.sync_all()
}

/// Replaces the freshly written `target` by a hard link to `source`, the identical payload an
/// earlier generation already made durable. Returns whether it did; on any failure the written
/// copy stays (the copy fallback), so publication never depends on link support.
fn link_identical(source: &Path, target: &Path, bytes: u64) -> bool {
    if !fs::symlink_metadata(source).is_ok_and(|meta| meta.is_file() && meta.len() == bytes) {
        return false;
    }
    let Some(name) = target.file_name() else {
        return false;
    };
    let mut staged_name = OsString::from(".");
    staged_name.push(name);
    staged_name.push(".link");
    let staged = target.with_file_name(staged_name);
    if fs::hard_link(source, &staged).is_err() {
        return false;
    }
    if fs::rename(&staged, target).is_ok() {
        return true;
    }
    let _ = fs::remove_file(&staged);
    false
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

/// The names `tempfile` gives pending generations: `generation-<iteration>-<alphanumerics>`,
/// the iteration with at least eight digits.
fn is_pending_generation(name: &str) -> bool {
    name.strip_prefix("generation-")
        .and_then(|rest| rest.split_once('-'))
        .is_some_and(|(iteration, suffix)| {
            iteration.len() >= 8
                && iteration.bytes().all(|byte| byte.is_ascii_digit())
                && !suffix.is_empty()
                && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
}

/// The iteration of a `validation-v3-<route>-<iteration>` journal directory name.
fn validation_iteration(name: &str) -> Option<usize> {
    let (route, iteration) = name.strip_prefix("validation-v3-")?.rsplit_once('-')?;
    if route.is_empty()
        || iteration.len() < 8
        || !iteration.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    iteration.parse().ok()
}

/// Removes the pending generations no longer referenced by `live`.
fn sweep(root: &Path, live: &Manifest) -> Vec<String> {
    let kept = [Some(live.generation.as_str()), live.previous.as_deref()];
    remove_matching(
        root,
        |name| {
            is_pending_generation(name) && !kept.iter().flatten().any(|&retained| retained == name)
        },
        "stale recovery generation",
    )
}

fn remove_matching(root: &Path, matches: impl Fn(&str) -> bool, what: &str) -> Vec<String> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => return vec![format!("could not list {}: {error}", root.display())],
    };
    let mut warnings = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                warnings.push(format!("could not list {}: {error}", root.display()));
                continue;
            }
        };
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if matches(&name)
            && let Err(error) = remove_flat_directory(root, &entry.path())
        {
            warnings.push(format!(
                "could not remove {what} {}: {error}",
                entry.path().display()
            ));
        }
    }
    warnings
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
    remove_flat_directory(root, &path)
}

/// Removes a direct child directory of `root` that holds only regular files (hard links to a
/// retained generation's payloads included); never recurses or follows a link.
fn remove_flat_directory(root: &Path, path: &Path) -> ResumeResult<()> {
    if !fs::symlink_metadata(path)?.is_dir()
        || fs::canonicalize(path)?.parent() != Some(fs::canonicalize(root)?.as_path())
    {
        return Err("not a direct child directory of the run directory".into());
    }
    let entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
    if entries
        .iter()
        .any(|entry| entry.file_type().map_or(true, |kind| !kind.is_file()))
    {
        return Err("directory contains unexpected non-file entries".into());
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
