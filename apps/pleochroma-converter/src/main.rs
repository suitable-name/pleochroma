//! Export a design library as native files without modifying its database.

mod args;
mod convert;
mod database;
mod enrich;
mod output;
mod report;
mod selection;

use anyhow::{Context, Result, ensure};
use args::{Args, Command};
use database::{Entry, Library};
use output::Output;
use report::{Record, Report, Status, Summary};
use std::{env, process::ExitCode};

fn main() -> ExitCode {
    match run() {
        Ok(partial) => ExitCode::from(if partial { 2 } else { 0 }),
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<bool> {
    match args::parse(env::args_os().skip(1), env::var_os("DIAGRAM_DB_PATH"))? {
        Command::Help => {
            print!("{}", args::HELP);
            Ok(false)
        }
        Command::Version => {
            println!("pleochroma-converter {}", env!("CARGO_PKG_VERSION"));
            Ok(false)
        }
        Command::Run(args) => export(&args),
    }
}

fn export(args: &Args) -> Result<bool> {
    let library = Library::open(&args.db)?;
    let entries = library.entries()?;
    ensure!(
        !entries.is_empty(),
        "the database contains no design entries"
    );
    let output = Output::prepare(&args.output)?;
    let mut report = Report {
        schema: 2,
        converter_version: env!("CARGO_PKG_VERSION"),
        database: args.db.canonicalize()?.to_string_lossy().into_owned(),
        adopt_source_meets: args.adopt_meets,
        summary: Summary {
            entries: entries.len(),
            ..Summary::default()
        },
        records: Vec::new(),
    };
    eprintln!(
        "Converting {} entries from {} (read-only)",
        entries.len(),
        args.db.display()
    );
    for (index, entry) in entries.iter().enumerate() {
        eprintln!(
            "[{}/{}] entry {}: {}",
            index + 1,
            entries.len(),
            entry.id,
            entry.title
        );
        if let Err(error) = export_entry(&library, &output, args, entry, &mut report) {
            // Keep the completed accounting if the output device fails midway.
            let _ = save_report(&output, &report);
            return Err(error);
        }
    }
    save_report(&output, &report)?;
    let summary = &report.summary;
    eprintln!(
        "Finished: {} converted, {} drafts, {} failures, {} entries without sources; {} alternative attachments skipped, {} meets adopted.",
        summary.converted,
        summary.drafts,
        summary.failed,
        summary.missing_sources,
        summary.skipped,
        summary.adopted_meets,
    );
    eprintln!("Report: {}", args.output.join("report.json").display());
    Ok(summary.is_partial())
}

fn export_entry(
    library: &Library,
    output: &Output,
    args: &Args,
    entry: &Entry,
    report: &mut Report,
) -> Result<()> {
    let sources = match library.sources(entry.id) {
        Ok(sources) => sources,
        Err(error) => {
            let mut record = Record::new(entry, None);
            record.notes.push(format!("{error:#}"));
            report.summary.failed += 1;
            report.records.push(record);
            eprintln!("  failed to read sources: {error:#}");
            return Ok(());
        }
    };
    if sources.is_empty() {
        let mut record = Record::new(entry, None);
        record.status = Status::MissingSource;
        record.notes.push("No ASC, PSDD, GEM or GCS attachment. An angle table alone does not record mast depths; no geometry was invented.".into());
        report.summary.missing_sources += 1;
        report.records.push(record);
        return Ok(());
    }
    report.summary.sources += sources.len();
    let selection::Selection {
        selected,
        mut records,
    } = selection::select(entry, &sources, args.adopt_meets);
    let result = if let Some((index, converted)) = selected {
        let record = &mut records[index];
        record.notes = converted.warnings;
        let name = output::design_name(entry.id, &entry.title);
        match output.write_design(&name, converted.draft, &converted.text) {
            Ok(path) => {
                eprintln!(
                    "  {} -> {path} ({} meets adopted)",
                    sources[index].name, converted.adopted_meets
                );
                record.output = Some(path);
                record.adopted_meets = converted.adopted_meets;
                record.status = if converted.draft {
                    Status::Draft
                } else {
                    Status::Converted
                };
                Ok(())
            }
            Err(error) => {
                record.status = Status::Failed;
                record.notes.push(format!("output failure: {error:#}"));
                Err(error)
            }
        }
    } else {
        Ok(())
    };
    append_records(report, records);
    result
}

fn append_records(report: &mut Report, records: Vec<Record>) {
    for record in records {
        match record.status {
            Status::Converted => report.summary.converted += 1,
            Status::Draft => report.summary.drafts += 1,
            Status::Failed => report.summary.failed += 1,
            Status::Skipped => report.summary.skipped += 1,
            Status::MissingSource => report.summary.missing_sources += 1,
        }
        report.summary.adopted_meets += record.adopted_meets;
        report.records.push(record);
    }
}

fn save_report(output: &Output, report: &Report) -> Result<()> {
    output
        .write_report(&serde_json::to_vec_pretty(report)?)
        .context("could not save conversion report")
}

#[cfg(test)]
#[path = "export_tests.rs"]
mod tests;
