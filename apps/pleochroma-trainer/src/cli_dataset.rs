//! CLI-only directory input; native parsing is shared with the GUI library loader.

use crate::dataset::DbDesignRecord;
use std::path::Path;

/// Load supported regular files directly inside a corpus directory in sorted filename order.
///
/// Directory records use negative, stable stem IDs; native/ASC siblings remain in one split.
/// Files are passed to their own format readers, never converted through ASC. Subdirectories
/// and symbolic links are intentionally not followed.
///
/// # Errors
/// Returns a path-specific I/O error, or a message when no supported files exist.
pub fn load_design_directory(path: &Path) -> Result<Vec<DbDesignRecord>, String> {
    let error = |e| format!("design directory {}: {e}", path.display());
    let mut files = Vec::new();
    for entry in std::fs::read_dir(path).map_err(error)? {
        let entry = entry.map_err(error)?;
        if !entry.file_type().map_err(error)?.is_file() {
            continue;
        }
        let file = entry.path();
        let extension = file
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or_default();
        if ["asc", "psdd"]
            .iter()
            .any(|v| extension.eq_ignore_ascii_case(v))
        {
            files.push(file);
        }
    }
    files.sort();
    let mut records = Vec::with_capacity(files.len());
    for file in files {
        let title = file
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let filename = file
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let bytes =
            std::fs::read(&file).map_err(|e| format!("design file {}: {e}", file.display()))?;
        records.push(DbDesignRecord {
            id: directory_id(&title),
            title,
            filename,
            asc_bytes: bytes,
        });
    }
    if records.is_empty() {
        Err("no .asc or .psdd designs were found".to_owned())
    } else {
        Ok(records)
    }
}

fn directory_id(stem: &str) -> i64 {
    let hash = stem
        .to_lowercase()
        .as_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
        });
    -i64::try_from(hash & (i64::MAX as u64))
        .expect("masked to i64")
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_order_and_sibling_ids_are_stable_without_recursion() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.psdd", "a.asc", "ignored.txt"] {
            std::fs::write(dir.path().join(name), "reader owns format validation").unwrap();
        }
        std::fs::create_dir(dir.path().join("child")).unwrap();
        std::fs::write(dir.path().join("child/hidden.asc"), "ignored").unwrap();
        let records = load_design_directory(dir.path()).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].filename, "a.asc");
        assert_eq!(records[1].filename, "a.psdd");
        assert_eq!(records[0].id, records[1].id);
        assert!(records[0].id < 0);
    }
}
