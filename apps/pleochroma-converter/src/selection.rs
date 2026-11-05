//! Select one usable attachment per catalogue entry, without merging designs.

use crate::{
    convert::{self, Converted},
    database::{Entry, Source},
    report::{Record, Status},
};

pub struct Selection {
    pub selected: Option<(usize, Converted)>,
    pub records: Vec<Record>,
}

pub fn select(entry: &Entry, sources: &[Source], adopt_meets: bool) -> Selection {
    let mut records: Vec<_> = sources
        .iter()
        .map(|source| {
            let mut record = Record::new(entry, Some(source));
            record.status = Status::Skipped;
            record.notes.push(
                "Alternative attachment not selected; only one native file is exported per entry."
                    .into(),
            );
            record
        })
        .collect();
    let mut order: Vec<_> = (0..sources.len()).collect();
    order.sort_unstable_by_key(|&index| (source_priority(&sources[index]), sources[index].id));
    let mut selected = None;
    for index in order {
        match convert::convert(entry, &sources[index], adopt_meets) {
            Ok(converted) => {
                if !converted.draft {
                    selected = Some((index, converted));
                    break;
                }
                records[index]
                    .notes
                    .extend(converted.warnings.iter().cloned());
                records[index].notes.push(
                    "This source converts to a draft; checking alternatives for a complete design."
                        .into(),
                );
                if selected.is_none() {
                    selected = Some((index, converted));
                }
            }
            Err(error) => {
                records[index].status = Status::Failed;
                records[index].notes = vec![format!("{error:#}")];
                eprintln!("  {}: {error:#}", sources[index].name);
            }
        }
    }
    Selection { selected, records }
}

fn source_priority(source: &Source) -> u8 {
    // Match the library's primary-attachment preference, then use attachment ID
    // for a stable choice among copies of the same format.
    match source
        .name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "psdd" => 0,
        "asc" => 1,
        "gem" => 2,
        "gcs" => 3,
        _ => 4,
    }
}
