//! Collision-free output and publication of complete files.

use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub struct Output {
    root: PathBuf,
}

impl Output {
    pub fn prepare(path: &Path) -> Result<Self> {
        fs::create_dir_all(path).with_context(|| format!("create {}", path.display()))?;
        ensure!(
            fs::read_dir(path)?.next().is_none(),
            "output folder {} is not empty; choose a new folder",
            path.display()
        );
        Ok(Self {
            root: path.to_path_buf(),
        })
    }

    pub fn write_design(&self, name: &str, draft: bool, text: &str) -> Result<String> {
        let relative = if draft {
            PathBuf::from("drafts").join(name)
        } else {
            PathBuf::from(name)
        };
        self.write(&relative, text.as_bytes())?;
        Ok(relative.to_string_lossy().replace('\\', "/"))
    }

    pub fn write_report(&self, report: &[u8]) -> Result<()> {
        self.write(Path::new("report.json"), report)
    }

    fn write(&self, relative: &Path, bytes: &[u8]) -> Result<()> {
        let destination = self.root.join(relative);
        let parent = destination
            .parent()
            .context("output has no parent folder")?;
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::Builder::new()
            .prefix(".pleochroma-")
            .tempfile_in(parent)?;
        temporary.write_all(bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist_noclobber(&destination).with_context(|| {
            format!(
                "write {} without replacing existing files",
                destination.display()
            )
        })?;
        Ok(())
    }
}

pub fn design_name(entry_id: i64, title: &str) -> String {
    let mut slug = String::new();
    for character in title.chars() {
        if slug.len() >= 64 {
            break;
        }
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "design" } else { slug };
    format!("entry-{entry_id}-{slug}.psdd")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsafe_and_duplicate_titles_cannot_escape_or_collide() {
        let first = design_name(12, "../../CON\\? Ruby: sample");
        assert!(!first.contains(['/', '\\', ':', '?']));
        assert_ne!(first, design_name(13, "../../CON\\? Ruby: sample"));
        assert_eq!(design_name(1, "💎"), "entry-1-design.psdd");
    }

    #[test]
    fn output_never_replaces_a_previous_file() {
        let directory = tempfile::tempdir().unwrap();
        let output = Output::prepare(directory.path()).unwrap();
        output.write_design("one.psdd", false, "first").unwrap();
        assert!(output.write_design("one.psdd", false, "second").is_err());
        assert_eq!(
            fs::read_to_string(directory.path().join("one.psdd")).unwrap(),
            "first"
        );
        assert!(Output::prepare(directory.path()).is_err());
    }
}
