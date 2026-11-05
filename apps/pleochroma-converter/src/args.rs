//! Command-line options for the offline converter.

use anyhow::{Result, bail};
use std::{ffi::OsString, path::PathBuf};

pub const HELP: &str = "pleochroma-converter --output FOLDER [--db FILE] [--no-adopt-meets]

Export one native PSDD per library entry from its best usable design attachment.
Complete designs are preferred to drafts; format order is PSDD, ASC, GEM, GCS.
The database is opened read-only. FOLDER must be new or empty.

  --db FILE          Database (default: DIAGRAM_DB_PATH, then facet_diagrams.sqlite)
  --output FOLDER    Separate destination folder, required
  --no-adopt-meets   Preserve all imported pins instead of adopting verified meets
  --help, -h        Show this help
  --version, -V     Show the converter version

Original geometry is retained. Meet adoption must pass the original-design gate.
Unsolved designs go in drafts/; missing sources and errors appear in report.json.
Exit codes: 0 complete; 2 partial conversion; 1 setup or output failure.
";

pub struct Args {
    pub db: PathBuf,
    pub output: PathBuf,
    pub adopt_meets: bool,
}

pub enum Command {
    Run(Args),
    Help,
    Version,
}

pub fn parse(
    arguments: impl IntoIterator<Item = OsString>,
    database_env: Option<OsString>,
) -> Result<Command> {
    let mut arguments = arguments.into_iter();
    let mut db = None;
    let mut output = None;
    let mut adopt_meets = true;
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--help" | "-h") => return Ok(Command::Help),
            Some("--version" | "-V") => return Ok(Command::Version),
            Some("--db") => set_path(&mut db, arguments.next(), "--db")?,
            Some("--output") => set_path(&mut output, arguments.next(), "--output")?,
            Some("--no-adopt-meets") => adopt_meets = false,
            _ => bail!("unknown argument \"{}\"; use --help", argument.display()),
        }
    }
    let output =
        output.ok_or_else(|| anyhow::anyhow!("--output FOLDER is required; use --help"))?;
    let db = db.unwrap_or_else(|| {
        database_env
            .filter(|value| !value.is_empty())
            .map_or_else(|| PathBuf::from("facet_diagrams.sqlite"), PathBuf::from)
    });
    Ok(Command::Run(Args {
        db,
        output,
        adopt_meets,
    }))
}

fn set_path(target: &mut Option<PathBuf>, value: Option<OsString>, option: &str) -> Result<()> {
    if target.is_some() {
        bail!("{option} was supplied more than once");
    }
    let value = value
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("{option} requires a path"))?;
    if value.to_string_lossy().starts_with("--") {
        bail!("{option} requires a path, got \"{}\"", value.display());
    }
    *target = Some(PathBuf::from(value));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_database_overrides_environment_and_adoption_can_be_disabled() {
        let arguments = [
            "--db",
            "explicit.sqlite",
            "--output",
            "converted",
            "--no-adopt-meets",
        ];
        let Command::Run(args) =
            parse(arguments.map(OsString::from), Some("env.sqlite".into())).unwrap()
        else {
            panic!("expected conversion options");
        };
        assert_eq!(args.db, PathBuf::from("explicit.sqlite"));
        assert_eq!(args.output, PathBuf::from("converted"));
        assert!(!args.adopt_meets);
    }

    #[test]
    fn missing_or_duplicate_paths_are_rejected() {
        for arguments in [
            vec!["--db", "file.sqlite"],
            vec!["--output", "--db", "file.sqlite"],
            vec!["--output", "one", "--output", "two"],
        ] {
            assert!(parse(arguments.into_iter().map(OsString::from), None).is_err());
        }
    }
}
