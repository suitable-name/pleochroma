//! The parser of `materials`: an optional material name and the options that shape the table or
//! the one material's report.

use super::{
    types::{
        Command, MarkdownTable, MaterialColumn, MaterialFilter, MaterialScale, MaterialsArgs,
        MaterialsFormat,
    },
    values::Cursor,
};

/// The column a `--sort` word names.
fn parse_column(word: &str) -> Result<MaterialColumn, String> {
    match word.to_ascii_lowercase().as_str() {
        "name" | "material" => Ok(MaterialColumn::Name),
        "system" | "crystal-system" => Ok(MaterialColumn::CrystalSystem),
        "optic" | "optic-character" => Ok(MaterialColumn::OpticCharacter),
        "ri" | "n" => Ok(MaterialColumn::Ri),
        "birefringence" | "biref" => Ok(MaterialColumn::Birefringence),
        "dispersion" | "disp" => Ok(MaterialColumn::Dispersion),
        "sg" | "specific-gravity" => Ok(MaterialColumn::SpecificGravity),
        "hardness" | "mohs" => Ok(MaterialColumn::Hardness),
        "toughness" => Ok(MaterialColumn::Toughness),
        "cleavage" => Ok(MaterialColumn::Cleavage),
        other => Err(format!(
            "unknown column {other:?} (expected name, system, optic, ri, birefringence, \
             dispersion, sg, hardness, toughness or cleavage)"
        )),
    }
}

/// The filter a `--filter` word names.
fn parse_filter(word: &str) -> Result<MaterialFilter, String> {
    match word.to_ascii_lowercase().as_str() {
        "all" => Ok(MaterialFilter::All),
        "natural" => Ok(MaterialFilter::Natural),
        "synthetic" => Ok(MaterialFilter::Synthetic),
        "glass" => Ok(MaterialFilter::Glass),
        "custom" => Ok(MaterialFilter::Custom),
        other => Err(format!(
            "unknown filter {other:?} (expected all, natural, synthetic, glass or custom)"
        )),
    }
}

/// The dispersion figure a `--dispersion` word names.
fn parse_scale(word: &str) -> Result<MaterialScale, String> {
    match word.to_ascii_lowercase().as_str() {
        "bg" | "b-g" => Ok(MaterialScale::Bg),
        "fc" | "f-c" => Ok(MaterialScale::Fc),
        other => Err(format!(
            "unknown dispersion {other:?} (expected \"bg\" or \"fc\")"
        )),
    }
}

/// The table a `--markdown` word names.
fn parse_markdown(word: &str) -> Result<MarkdownTable, String> {
    match word.to_ascii_lowercase().as_str() {
        "optics" => Ok(MarkdownTable::Optics),
        "gem-data" => Ok(MarkdownTable::GemData),
        "built-in" => Ok(MarkdownTable::BuiltIn),
        "sources" => Ok(MarkdownTable::Sources),
        other => Err(format!(
            "unknown table {other:?} (expected optics, gem-data, built-in or sources)"
        )),
    }
}

/// Records the output format, refusing a second one.
fn set_format(slot: &mut Option<MaterialsFormat>, format: MaterialsFormat) -> Result<(), String> {
    if slot.is_some() {
        return Err("give one of --json, --csv and --markdown TABLE, and give it once".to_string());
    }
    *slot = Some(format);
    Ok(())
}

/// Parses the words after `materials`.
///
/// The words that are not flags name the material, joined with single spaces, so `materials
/// Cubic Zirconia` needs no quotes.
pub(super) fn parse_materials(args: &[String]) -> Result<Command, String> {
    let mut cursor = Cursor::new("materials", args);
    let mut words: Vec<&str> = Vec::new();
    let mut format = None;
    let (mut sort, mut filter, mut scale) = (None, None, None);
    let (mut descending, mut bands) = (false, false);
    let (mut curves, mut out, mut db) = (None, None, None);
    while let Some(word) = cursor.token() {
        match word {
            "--json" => set_format(&mut format, MaterialsFormat::Json)?,
            "--csv" => set_format(&mut format, MaterialsFormat::Csv)?,
            "--markdown" => {
                let table = parse_markdown(cursor.value("--markdown")?)?;
                set_format(&mut format, MaterialsFormat::Markdown(table))?;
            }
            "--sort" => sort = Some(parse_column(cursor.value("--sort")?)?),
            "--desc" => descending = true,
            "--filter" => filter = Some(parse_filter(cursor.value("--filter")?)?),
            "--dispersion" => scale = Some(parse_scale(cursor.value("--dispersion")?)?),
            "--bands" => bands = true,
            "--curves" => curves = Some(cursor.path("--curves")?),
            "--out" => out = Some(cursor.path("--out")?),
            "--db" => db = Some(cursor.path("--db")?),
            flag if flag.starts_with('-') => {
                return Err(format!(
                    "unknown flag {flag:?} for \"materials\" (see --help)"
                ));
            }
            name => words.push(name),
        }
    }
    let name = (!words.is_empty()).then(|| words.join(" "));
    let format = format.unwrap_or(MaterialsFormat::Text);
    if name.is_some() {
        if matches!(format, MaterialsFormat::Csv | MaterialsFormat::Markdown(_)) {
            return Err(
                "--csv and --markdown write the table of every material: leave out the name"
                    .to_string(),
            );
        }
        if sort.is_some() || descending || filter.is_some() {
            return Err(
                "--sort, --desc and --filter order and select the table: leave out the name"
                    .to_string(),
            );
        }
    } else if bands || curves.is_some() {
        return Err("--bands and --curves describe one material: give its name".to_string());
    }
    Ok(Command::Materials(MaterialsArgs {
        name,
        format,
        sort,
        descending,
        filter: filter.unwrap_or(MaterialFilter::All),
        scale: scale.unwrap_or(MaterialScale::Bg),
        bands,
        curves,
        out,
        db,
    }))
}
